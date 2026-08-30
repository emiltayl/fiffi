extern crate alloc;

use alloc::vec;
use core::mem::{MaybeUninit, offset_of};
use core::ptr;

use super::plan::{ArgumentDestination, ArgumentMove, ArgumentSource, MarshalPlan, ReturnStrategy};
use crate::FnPtr;
use crate::backend::x86_64::Register;
use crate::backend::x86_64::asm::stack_setup_asm;
use crate::function::{Arg, Ret};

// TODO Fix assembly for Win64. Note that 32 bytes need to be reserved on the stack prior to any
// `call`.

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
    /// Plan-owned list of stack-buffer offsets containing pointers that need the outgoing stack
    /// address added to them.
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
    /// `marshal_plan`, `args`, and `ret` must describe the same function signature. Every argument
    /// referenced by the plan must be readable for its declared layout, and the argument storage
    /// must not overlap `stack_buffer`. The marshal plan, return storage, and stack buffer must
    /// remain alive while the returned frame is used.
    unsafe fn new(
        marshal_plan: &MarshalPlan,
        fn_ptr: FnPtr,
        args: &[Arg<'_>],
        ret: &Ret<'_>,
        stack_buffer: &mut [MaybeUninit<u8>],
    ) -> Self {
        assert_eq!(stack_buffer.len(), marshal_plan.stack_buffer_size);

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
            stack_buffer_ptr: stack_buffer.as_ptr(),
            stack_buffer_len: stack_buffer.len(),
            fn_ptr,
        };

        // If the return value is passed through a "hidden" pointer, we can simply pass along the
        // `ret` pointer as the first argument
        if marshal_plan.return_strategy == ReturnStrategy::HiddenPointer {
            let ret_ptr_bytes = ret.as_ptr().expose_provenance().to_ne_bytes();
            call_frame.gpr_registers[0].update_from_bytes(&ret_ptr_bytes);
        }

        for step in &marshal_plan.argument_moves {
            match &step.source {
                ArgumentSource::Argument { argument_index } => {
                    let arg = args
                        .get(*argument_index)
                        .expect("marshal plan references an argument that was not provided");
                    let destination = copy_destination(&mut call_frame, stack_buffer, step);

                    // SAFETY: The caller guarantees that `arg` is readable for the layout used to
                    // build `marshal_plan`. `destination` is a bounds-checked slice of exactly
                    // `step.size` bytes in fresh call-owned storage, which the safety contract
                    // requires not to overlap the argument storage. Copying as `MaybeUninit<u8>`
                    // permits argument padding bytes to remain uninitialized.
                    unsafe {
                        ptr::copy_nonoverlapping(
                            arg.as_ptr().cast::<MaybeUninit<u8>>(),
                            destination.as_mut_ptr(),
                            destination.len(),
                        );
                    }
                }
                ArgumentSource::StackAddress { offset } => {
                    let destination = copy_destination(&mut call_frame, stack_buffer, step);
                    let offset_bytes = offset.to_ne_bytes();
                    for (destination_byte, offset_byte) in destination.iter_mut().zip(offset_bytes)
                    {
                        destination_byte.write(offset_byte);
                    }
                }
            }
        }

        call_frame
    }
}

fn copy_destination<'frame>(
    call_frame: &'frame mut CallFrame,
    stack_buffer: &'frame mut [MaybeUninit<u8>],
    step: &ArgumentMove,
) -> &'frame mut [MaybeUninit<u8>] {
    match &step.destination {
        ArgumentDestination::Gpr(index) => call_frame
            .gpr_registers
            .get_mut(*index)
            .and_then(|register| register.0.get_mut(..step.size))
            .expect("marshal plan contains an invalid general-purpose register destination"),
        ArgumentDestination::Xmm(index) => call_frame
            .xmm_registers
            .get_mut(*index)
            .and_then(|register| register.0.get_mut(..step.size))
            .expect("marshal plan contains an invalid vector register destination"),
        ArgumentDestination::Stack(offset) => stack_buffer
            .get_mut(*offset..(*offset + step.size))
            .expect("marshal plan contains an invalid stack destination"),
    }
}

/// Writes a register-returned value from a call frame into caller-provided storage.
///
/// The first two entries in each register bank are reused for the corresponding ABI return
/// registers: `rax` and `rdx` for the general-purpose bank and `xmm0` and `xmm1` for the vector
/// bank.
///
/// # Safety
///
/// The registers selected by `return_strategy` must contain the return value described by the
/// strategy. For register returns, `ret` must point to writable storage large enough for the
/// described value, and that storage must not overlap `call_frame`.
unsafe fn write_register_return(
    call_frame: &CallFrame,
    return_strategy: ReturnStrategy,
    ret: Ret<'_>,
) {
    match return_strategy {
        ReturnStrategy::Void | ReturnStrategy::HiddenPointer => {}

        // SAFETY: The caller guarantees that `ret` is valid for writes of `byte_length` bytes and
        // does not overlap `call_frame`. The given `ReturnStrategy` guarantees that the register
        // has been filled with `byte_length` bytes containing the result.
        ReturnStrategy::Rax { byte_length } => unsafe {
            ptr::copy_nonoverlapping(
                call_frame.gpr_registers[0].0.as_ptr(),
                ret.as_ptr().cast(),
                usize::from(byte_length),
            );
        },

        // SAFETY: The caller guarantees that `ret` is valid for writes of `byte_length` bytes and
        // does not overlap `call_frame`. The given `ReturnStrategy` guarantees that the register
        // has been filled with `byte_length` bytes containing the result. Certain return values are
        // returned in the full xmm0 register, which will be written to the two first `Register`s in
        // `call_frame.xmm_registers`. Reading 16 successive bytes is valid as they are adjacent in
        // memory and part of the same allocation.
        ReturnStrategy::Xmm0 { byte_length } => unsafe {
            ptr::copy_nonoverlapping(
                call_frame.xmm_registers[0].0.as_ptr(),
                ret.as_ptr().cast(),
                usize::from(byte_length),
            );
        },
    }
}

/// Calls a function using the Win64 marshal plan.
///
/// # Safety
///
/// The safety contract of [`crate::function::Function::call`] must be upheld. `marshal_plan` must
/// have been built for the exact signature described by `fn_ptr`, `args`, and `ret`.
pub(super) unsafe fn call(marshal_plan: &MarshalPlan, fn_ptr: FnPtr, args: &[Arg], ret: Ret) {
    let mut stack_buffer = vec![MaybeUninit::<u8>::uninit(); marshal_plan.stack_buffer_size];

    // SAFETY: The caller upholds this function's contract. `stack_buffer` is freshly allocated at
    // the size required by `marshal_plan`, so it cannot overlap the live caller-owned arguments.
    let mut call_frame =
        unsafe { CallFrame::new(marshal_plan, fn_ptr, args, &ret, &mut stack_buffer) };

    // SAFETY: `call_frame` and its backing `stack_buffer` remain alive for the duration of the
    // invocation. `CallFrame::new` populated them according to `marshal_plan`, and this function's
    // contract guarantees that the plan matches `fn_ptr`, the arguments, and the return storage.
    // The assembly supplies the platform-specific unwind metadata required by `invoke`.
    unsafe {
        invoke(&raw mut call_frame);
    }

    // SAFETY: The caller guarantees that `ret` is valid for the planned return type. `invoke`
    // writes register-returned values into the corresponding register slots in `call_frame`.
    unsafe {
        write_register_return(&call_frame, marshal_plan.return_strategy, ret);
    }
}

/// Invokes the function described by a call frame.
///
/// # Safety
///
/// `call_frame` must point to a valid [`CallFrame`] that remains alive for the duration of the
/// invocation, along with the stack buffer and marshal-plan metadata referenced by the frame. Its
/// function pointer must be callable with the ABI arguments represented by the frame, and every
/// register or stack byte read by the called function must contain the corresponding argument
/// data. Any return storage must be valid for the signature. The assembly implementation must also
/// provide correct unwind metadata.
#[unsafe(naked)]
unsafe extern "win64-unwind" fn invoke(call_frame: *mut CallFrame) {
    core::arch::naked_asm!(
        #[cfg(not(windows))]
        ".cfi_startproc",
        #[cfg(windows)]
        ".seh_proc {__unwind_function}",

        // Save the caller's frame pointer, then preserve the other nonvolatile registers used by
        // this function in the first three slots of the caller-provided shadow space. After the
        // `rbp` push, those slots are at `rsp + 16`, `rsp + 24`, and `rsp + 32`.
        "push rbp",
        #[cfg(not(windows))]
        ".cfi_adjust_cfa_offset 8",
        #[cfg(not(windows))]
        ".cfi_offset rbp, -16",
        #[cfg(windows)]
        ".seh_pushreg rbp",
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
        // Use `rbp` as a stable reference while the body moves `rsp`. The `call_frame` pointer is
        // stored in `r12`.
        "mov rbp, rsp",
        #[cfg(not(windows))]
        ".cfi_def_cfa_register rbp",
        #[cfg(windows)]
        ".seh_setframe rbp, 0",
        #[cfg(windows)]
        ".seh_endprologue",

        "mov r12, rcx",
        "mov r11, [r12 + {stack_buffer_len_offset}]",
        "add r11, 32",
        stack_setup_asm!("r11"),
        // Expose the destination for the buffered arguments to the ABI-specific body.
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

        // Calculate offsets to indirect arguments for register arguments
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

        // Calculate offsets to indirect arguments for stack arguments
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

        // Restore the stack in one operation so this remains correct after runtime-sized stack
        // allocations. `lea` also gives the Windows unwinder a recognized frame-pointer-based
        // epilogue.
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
        __stack_probe_interval = const crate::backend::x86_64::asm::STACK_PROBE_INTERVAL,
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
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;
    use crate::fn_ptrize;
    use crate::test_utils::structs::{U8X3_ARG, U8x3, U64X2_ARG, U64x2, U64x3};
    use crate::types::{FfiType, Type};

    extern "C" fn unused_target() {}

    fn initialized_bytes<const N: usize>(bytes: &[MaybeUninit<u8>]) -> [u8; N] {
        assert_eq!(bytes.len(), N);

        core::array::from_fn(|index| {
            // SAFETY: Tests only call this helper for zero-initialized registers or stack-buffer
            // regions populated by the argument moves under test.
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

        // SAFETY: Every argument matches the corresponding type used to build the plan and cannot
        // overlap the separately allocated stack buffer. The void return matches the plan.
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

        // SAFETY: Every argument matches the corresponding type used to build the plan and cannot
        // overlap the separately allocated stack buffer. The void return matches the plan.
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

        // SAFETY: Both arguments and the return storage match the types used to build the plan and
        // cannot overlap the separately allocated stack buffer.
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
