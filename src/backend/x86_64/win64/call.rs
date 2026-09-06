extern crate alloc;

use alloc::vec;
use core::mem::{MaybeUninit, offset_of};
use core::ptr;

use super::plan::{ArgumentDestination, ArgumentSource, MarshalPlan, ReturnStrategy};
use crate::FnPtr;
use crate::backend::x86_64::Register;
use crate::backend::x86_64::asm::stack_setup_asm;
use crate::function::{Arg, Ret};

#[derive(Debug)]
struct CallFrame {
    /// Arguments passed in integer registers. The first element is also used for return values
    /// passed in rax.
    gpr_registers: [Register; 4],
    /// Arguments passed in xmm registers. The two first elements are also used for return values
    /// values passed in xmm0.
    xmm_registers: [Register; 4],

    /// Bit mask identifying GPR slots containing offsets from the outgoing stack-buffer base.
    gpr_indirect_regs_mask: u8,
    /// Plan-owned offsets of stack slots rebased onto the outgoing stack buffer.
    stack_indirect_arguments_offsets_ptr: *const usize,
    stack_indirect_arguments_offsets_len: usize,

    stack_buffer_ptr: *const MaybeUninit<u8>,
    stack_buffer_len: usize,
    fn_ptr: FnPtr,
}

impl CallFrame {
    /// Creates a call frame and marshals the arguments into it.
    ///
    /// # Safety
    ///
    /// * `marshal_plan`, `args`, and `ret` must describe the same signature.
    /// * Arguments must be readable for their layouts and must not overlap `stack_buffer`.
    /// * `stack_buffer` must have the planned size.
    /// * The plan, return storage, and stack buffer must outlive use of the frame.
    unsafe fn new(
        marshal_plan: &MarshalPlan,
        fn_ptr: FnPtr,
        args: &[Arg<'_>],
        ret: &Ret<'_>,
        stack_buffer: &mut [MaybeUninit<u8>],
    ) -> Self {
        let mut call_frame = Self {
            gpr_registers: <[Register; 4] as Default>::default(),
            xmm_registers: <[Register; 4] as Default>::default(),
            gpr_indirect_regs_mask: marshal_plan.gpr_indirect_regs_mask,
            stack_indirect_arguments_offsets_ptr: marshal_plan
                .stack_indirect_arguments_offsets
                .as_ptr(),
            stack_indirect_arguments_offsets_len: marshal_plan
                .stack_indirect_arguments_offsets
                .len(),
            stack_buffer_ptr: ptr::null(),
            stack_buffer_len: stack_buffer.len(),
            fn_ptr,
        };

        // A hidden return pointer occupies the first argument slot.
        if marshal_plan.return_strategy == ReturnStrategy::HiddenPointer {
            let ret_ptr_bytes = ret.as_ptr().expose_provenance().to_ne_bytes();
            call_frame.gpr_registers[0].update_from_bytes(&ret_ptr_bytes);
        }

        for step in &marshal_plan.argument_moves {
            let destination = match &step.destination {
                ArgumentDestination::Gpr(index) => {
                    &mut call_frame.gpr_registers[*index].0[..step.size]
                }
                ArgumentDestination::Xmm(index) => {
                    &mut call_frame.xmm_registers[*index].0[..step.size]
                }
                ArgumentDestination::Stack(offset) => {
                    &mut stack_buffer[*offset..(*offset + step.size)]
                }
            };
            match &step.source {
                ArgumentSource::Argument { argument_index } => {
                    let arg = &args[*argument_index];

                    // SAFETY:
                    // * The caller provides readable argument storage of the planned size.
                    // * The destination is in bounds and does not overlap the argument.
                    // * `MaybeUninit<u8>` permits uninitialized padding.
                    unsafe {
                        ptr::copy_nonoverlapping(
                            arg.as_ptr().cast::<MaybeUninit<u8>>(),
                            destination.as_mut_ptr(),
                            destination.len(),
                        );
                    }
                }
                ArgumentSource::StackAddress { offset } => {
                    let offset_bytes = offset.to_ne_bytes();
                    for (destination_byte, offset_byte) in destination.iter_mut().zip(offset_bytes)
                    {
                        destination_byte.write(offset_byte);
                    }
                }
            }
        }

        // Derive the shared pointer after the final mutable borrow of the buffer.
        call_frame.stack_buffer_ptr = stack_buffer.as_ptr();
        call_frame
    }
}

/// Writes a register-returned value from a call frame into caller-provided storage.
///
/// # Safety
///
/// * The selected bytes must be in bounds and contain the described return value.
/// * For register returns, `ret` must be writable for that value and not overlap the frame.
unsafe fn write_register_return(
    call_frame: &CallFrame,
    return_strategy: ReturnStrategy,
    ret: Ret<'_>,
) {
    let (source, byte_length) = match return_strategy {
        ReturnStrategy::Void | ReturnStrategy::HiddenPointer => return,
        ReturnStrategy::Rax { byte_length } => {
            (call_frame.gpr_registers[0].0.as_ptr(), byte_length)
        }
        ReturnStrategy::Xmm0 { byte_length } => {
            // Borrow the whole bank: a full `xmm0` return spans two eight-byte slots.
            (call_frame.xmm_registers.as_ptr().cast(), byte_length)
        }
    };

    // SAFETY:
    // * The selected bytes contain the return value.
    // * The caller provides enough writable, nonoverlapping return storage.
    // * `MaybeUninit<u8>` permits uninitialized padding.
    unsafe {
        ptr::copy_nonoverlapping(source, ret.as_ptr().cast(), usize::from(byte_length));
    }
}

/// Calls a function using the Win64 marshal plan.
///
/// # Safety
///
/// * Uphold [`crate::function::Function::call`]'s safety contract.
/// * `marshal_plan` must describe `fn_ptr`, `args`, and `ret`.
pub(crate) unsafe fn call(marshal_plan: &MarshalPlan, fn_ptr: FnPtr, args: &[Arg], ret: Ret) {
    let mut stack_buffer = vec![MaybeUninit::<u8>::uninit(); marshal_plan.stack_buffer_size];

    // SAFETY:
    // * The caller provides arguments and return storage matching the plan.
    // * The fresh stack buffer has the planned size and cannot overlap the arguments.
    let mut call_frame =
        unsafe { CallFrame::new(marshal_plan, fn_ptr, args, &ret, &mut stack_buffer) };

    // SAFETY:
    // * The frame, buffer, plan, and return storage remain alive during the call.
    // * The frame contains arguments matching the target signature.
    // * The caller provides valid return storage.
    unsafe {
        invoke(&raw mut call_frame);
    }

    // SAFETY:
    // * `invoke` stored the returned registers in the frame.
    // * The caller provides valid return storage, separate from the frame.
    unsafe {
        write_register_return(&call_frame, marshal_plan.return_strategy, ret);
    }
}

/// Invokes the function described by a call frame.
///
/// # Safety
///
/// * The frame must remain writable for the call.
/// * Its stack buffer and plan metadata must remain readable for the call.
/// * The target must accept the ABI arguments stored in the frame.
/// * Any return storage must be valid for the signature.
#[unsafe(naked)]
unsafe extern "win64-unwind" fn invoke(call_frame: *mut CallFrame) {
    core::arch::naked_asm!(
        #[cfg(not(windows))]
        ".cfi_startproc",
        #[cfg(windows)]
        ".seh_proc {__unwind_function}",

        // Establish the frame pointer before recording frame-relative register saves.
        "push rbp",
        #[cfg(not(windows))]
        ".cfi_adjust_cfa_offset 8",
        #[cfg(not(windows))]
        ".cfi_offset rbp, -16",
        #[cfg(windows)]
        ".seh_pushreg rbp",
        "mov rbp, rsp",
        #[cfg(not(windows))]
        ".cfi_def_cfa_register rbp",
        #[cfg(windows)]
        ".seh_setframe rbp, 0",

        // Save nonvolatile registers in the caller's shadow space.
        "mov [rsp + 16], r12",
        #[cfg(not(windows))]
        ".cfi_offset r12, 0",
        #[cfg(windows)]
        ".seh_savereg r12, 16",
        "mov [rsp + 24], rdi",
        #[cfg(not(windows))]
        ".cfi_offset rdi, 8",
        #[cfg(windows)]
        ".seh_savereg rdi, 24",
        "mov [rsp + 32], rsi",
        #[cfg(not(windows))]
        ".cfi_offset rsi, 16",
        #[cfg(windows)]
        ".seh_savereg rsi, 32",
        #[cfg(windows)]
        ".seh_endprologue",

        // Keep the `CallFrame` pointer in a nonvolatile register across the target call.
        "mov r12, rcx",
        "mov r11, [r12 + {stack_buffer_len_offset}]",
        "add r11, 32",
        stack_setup_asm!("r11"),
        // Skip the outgoing shadow space.
        "add r10, 32",

        // Copy the stack arguments into the probed allocation.
        "mov rsi, [r12 + {stack_buffer_ptr_offset}]",
        "mov rcx, [r12 + {stack_buffer_len_offset}]",
        "mov rdi, r10",
        "rep movsb",

        // Set up register arguments.
        "mov rcx, [r12 + {gpr_registers_offset} + {register_size} * 0]",
        "mov rdx, [r12 + {gpr_registers_offset} + {register_size} * 1]",
        "mov r8, [r12 + {gpr_registers_offset} + {register_size} * 2]",
        "mov r9, [r12 + {gpr_registers_offset} + {register_size} * 3]",

        "movq xmm0, [r12 + {xmm_registers_offset} + {register_size} * 0]",
        "movq xmm1, [r12 + {xmm_registers_offset} + {register_size} * 1]",
        "movq xmm2, [r12 + {xmm_registers_offset} + {register_size} * 2]",
        "movq xmm3, [r12 + {xmm_registers_offset} + {register_size} * 3]",

        // Rebase indirect register arguments onto the outgoing stack buffer.
        "mov al, [r12 + {gpr_indirect_regs_mask_offset}]",
        "test al, 1 << 0",
        "jz 20f",
        "add rcx, r10",
        "20:",
        "test al, 1 << 1",
        "jz 21f",
        "add rdx, r10",
        "21:",
        "test al, 1 << 2",
        "jz 22f",
        "add r8, r10",
        "22:",
        "test al, 1 << 3",
        "jz 23f",
        "add r9, r10",
        "23:",

        // Rebase indirect stack arguments onto the outgoing stack buffer.
        "mov rax, [r12 + {stack_indirect_arguments_offset}]",
        "mov r11, [r12 + {stack_indirect_arguments_len}]",

        "test r11, r11",
        "jz 25f",
        "24:",
        "mov rsi, [rax + r11 * 8 - 8]",
        "add [r10 + rsi], r10",
        "dec r11",
        "jnz 24b",
        "25:",

        "mov r11, [r12 + {fn_ptr_offset}]",
        "call r11",

        // Save registers used for return values.
        "mov [r12 + {gpr_registers_offset}], rax",
        "movups [r12 + {xmm_registers_offset}], xmm0",

        // Restore the nonvolatile registers before entering the Windows-recognized epilogue.
        "mov r12, [rbp + 16]",
        #[cfg(not(windows))]
        ".cfi_restore r12",
        "mov rdi, [rbp + 24]",
        #[cfg(not(windows))]
        ".cfi_restore rdi",
        "mov rsi, [rbp + 32]",
        #[cfg(not(windows))]
        ".cfi_restore rsi",

        // Restore the dynamic stack allocation with a Windows-recognized epilogue.
        "lea rsp, [rbp]",
        "pop rbp",
        #[cfg(not(windows))]
        ".cfi_def_cfa rsp, 8",
        "ret",

        #[cfg(not(windows))]
        ".cfi_endproc",
        #[cfg(windows)]
        ".seh_endproc",
        #[cfg(windows)]
        __unwind_function = sym invoke,
        stack_buffer_len_offset = const offset_of!(CallFrame, stack_buffer_len),
        stack_buffer_ptr_offset = const offset_of!(CallFrame, stack_buffer_ptr),

        gpr_registers_offset = const offset_of!(CallFrame, gpr_registers),
        xmm_registers_offset = const offset_of!(CallFrame, xmm_registers),
        register_size = const size_of::<Register>(),

        fn_ptr_offset = const offset_of!(CallFrame, fn_ptr),

        gpr_indirect_regs_mask_offset = const offset_of!(CallFrame, gpr_indirect_regs_mask),
        stack_indirect_arguments_offset = const offset_of!(CallFrame, stack_indirect_arguments_offsets_ptr),
        stack_indirect_arguments_len = const offset_of!(CallFrame, stack_indirect_arguments_offsets_len),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fn_ptrize;
    use crate::test_utils::structs::{U8X3_ARG, U8x3, U64X2_ARG, U64x2, U64x3};
    use crate::types::{FfiType, Type};

    extern "C" fn unused_target() {}

    fn initialized_bytes<const N: usize>(bytes: &[MaybeUninit<u8>]) -> [u8; N] {
        assert_eq!(bytes.len(), N);

        core::array::from_fn(|index| {
            // SAFETY: These tests pass only initialized argument or register bytes.
            unsafe { *bytes[index].assume_init_ref() }
        })
    }

    fn register_usize(register: &Register) -> usize {
        usize::from_ne_bytes(initialized_bytes(&register.0))
    }

    fn register_u64(register: &Register) -> u64 {
        u64::from_ne_bytes(initialized_bytes(&register.0))
    }

    #[test]
    fn mixed_direct_arguments_are_copied_to_their_planned_destinations() {
        let marshal_plan = MarshalPlan::build(
            &[Type::U64, Type::F64, Type::U64, Type::F32, Type::F64],
            None,
        );
        let first_gpr = 0x1020_3040_5060_7080u64;
        let first_xmm = core::f64::consts::PI;
        let second_gpr = 0x90a0_b0c0_d0e0_f000u64;
        let second_xmm = core::f32::consts::E;
        let stack_argument = -core::f64::consts::SQRT_2;
        let args = [
            Arg::new(&first_gpr),
            Arg::new(&first_xmm),
            Arg::new(&second_gpr),
            Arg::new(&second_xmm),
            Arg::new(&stack_argument),
        ];
        let ret = Ret::void();
        let mut stack_buffer = vec![MaybeUninit::uninit(); marshal_plan.stack_buffer_size];

        // SAFETY:
        // * Arguments and the void return match the plan.
        // * The separate stack buffer has the planned size.
        // * The plan and buffer outlive the frame.
        let call_frame = unsafe {
            CallFrame::new(
                &marshal_plan,
                fn_ptrize!(unused_target),
                &args,
                &ret,
                &mut stack_buffer,
            )
        };

        assert_eq!(
            initialized_bytes::<8>(&call_frame.gpr_registers[0].0),
            first_gpr.to_ne_bytes()
        );
        assert_eq!(
            initialized_bytes::<8>(&call_frame.xmm_registers[1].0),
            first_xmm.to_ne_bytes()
        );
        assert_eq!(
            initialized_bytes::<8>(&call_frame.gpr_registers[2].0),
            second_gpr.to_ne_bytes()
        );
        assert_eq!(
            initialized_bytes::<4>(&call_frame.xmm_registers[3].0[..4]),
            second_xmm.to_ne_bytes()
        );
        assert_eq!(
            initialized_bytes::<8>(&stack_buffer),
            stack_argument.to_ne_bytes()
        );
        assert_eq!(call_frame.gpr_indirect_regs_mask, 0);
        assert_eq!(call_frame.stack_indirect_arguments_offsets_len, 0);
    }

    #[test]
    fn indirect_arguments_store_copies_and_deferred_stack_addresses() {
        let marshal_plan = MarshalPlan::build(
            &[
                U8x3::ffi_type(),
                Type::U64,
                Type::F64,
                Type::U64,
                U64x2::ffi_type(),
            ],
            None,
        );
        let first_direct = 0x1020_3040_5060_7080u64;
        let float_direct = core::f64::consts::PI;
        let second_direct = 0x90a0_b0c0_d0e0_f000u64;
        let args = [
            Arg::new(&U8X3_ARG),
            Arg::new(&first_direct),
            Arg::new(&float_direct),
            Arg::new(&second_direct),
            Arg::new(&U64X2_ARG),
        ];
        let ret = Ret::void();
        let mut stack_buffer = vec![MaybeUninit::new(0xa5); marshal_plan.stack_buffer_size];

        // SAFETY:
        // * Arguments and the void return match the plan.
        // * The separate stack buffer has the planned size.
        // * The plan and buffer outlive the frame.
        let call_frame = unsafe {
            CallFrame::new(
                &marshal_plan,
                fn_ptrize!(unused_target),
                &args,
                &ret,
                &mut stack_buffer,
            )
        };

        assert_eq!(call_frame.gpr_indirect_regs_mask, 0b0001);
        assert_eq!(register_usize(&call_frame.gpr_registers[0]), 16);
        assert_eq!(register_u64(&call_frame.gpr_registers[1]), first_direct);
        assert_eq!(
            initialized_bytes::<8>(&call_frame.xmm_registers[2].0),
            float_direct.to_ne_bytes()
        );
        assert_eq!(register_u64(&call_frame.gpr_registers[3]), second_direct);
        assert_eq!(
            usize::from_ne_bytes(initialized_bytes(&stack_buffer[..8])),
            32
        );
        assert_eq!(
            initialized_bytes::<3>(&stack_buffer[16..19]),
            [U8X3_ARG.a, U8X3_ARG.b, U8X3_ARG.c]
        );
        assert_eq!(
            initialized_bytes::<8>(&stack_buffer[32..40]),
            U64X2_ARG.a.to_ne_bytes()
        );
        assert_eq!(
            initialized_bytes::<8>(&stack_buffer[40..48]),
            U64X2_ARG.b.to_ne_bytes()
        );
        assert_eq!(
            call_frame.stack_indirect_arguments_offsets_ptr,
            marshal_plan.stack_indirect_arguments_offsets.as_ptr()
        );
        assert_eq!(call_frame.stack_indirect_arguments_offsets_len, 1);
        assert_eq!(marshal_plan.stack_indirect_arguments_offsets, [0]);
    }

    #[test]
    fn hidden_return_and_direct_pointer_are_not_marked_as_stack_addresses() {
        let return_type = U64x3::ffi_type();
        let marshal_plan = MarshalPlan::build(&[Type::Pointer, Type::U128], Some(&return_type));
        let direct_pointer = ptr::without_provenance::<core::ffi::c_void>(0xfedc_ba98_7654_3210);
        let indirect_argument = 0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00u128;
        let args = [Arg::new(&direct_pointer), Arg::new(&indirect_argument)];
        let mut return_value = MaybeUninit::<U64x3>::uninit();
        let ret = Ret::new(&mut return_value);
        let return_address = ret.as_ptr().expose_provenance();
        let mut stack_buffer = vec![MaybeUninit::uninit(); marshal_plan.stack_buffer_size];

        // SAFETY:
        // * Arguments and return storage match the plan.
        // * The separate stack buffer has the planned size.
        // * The plan, return storage, and buffer outlive the frame.
        let call_frame = unsafe {
            CallFrame::new(
                &marshal_plan,
                fn_ptrize!(unused_target),
                &args,
                &ret,
                &mut stack_buffer,
            )
        };

        assert_eq!(call_frame.gpr_indirect_regs_mask, 0b0100);
        assert_eq!(register_usize(&call_frame.gpr_registers[0]), return_address);
        assert_eq!(
            register_usize(&call_frame.gpr_registers[1]),
            direct_pointer.expose_provenance()
        );
        assert_eq!(register_usize(&call_frame.gpr_registers[2]), 0);
        assert_eq!(
            initialized_bytes::<16>(&stack_buffer),
            indirect_argument.to_ne_bytes()
        );
        assert_eq!(call_frame.stack_indirect_arguments_offsets_len, 0);
    }
}
