extern crate alloc;

use alloc::vec;
use core::mem::{MaybeUninit, offset_of};
use core::ptr;

use super::plan::{ArgumentMove, MarshalPlan, ReturnStrategy};
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
    indirect_register_mask: u8,
    /// Plan-owned offsets of stack slots rebased onto the outgoing stack buffer.
    indirect_stack_offsets_pointer: *const usize,
    indirect_stack_offsets_count: usize,

    /// Space required to reserve on the stack. This includes memory required to provide storage
    /// space for hidden pointer return if we simply want to discard the return value.
    stack_allocation_len: usize,

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
        ret: Option<&Ret<'_>>,
        stack_buffer: &mut [MaybeUninit<u8>],
    ) -> Self {
        let mut call_frame = Self {
            gpr_registers: <[Register; 4] as Default>::default(),
            xmm_registers: <[Register; 4] as Default>::default(),
            indirect_register_mask: marshal_plan.indirect_register_mask,
            indirect_stack_offsets_pointer: marshal_plan.indirect_stack_offsets.as_ptr(),
            indirect_stack_offsets_count: marshal_plan.indirect_stack_offsets.len(),
            stack_allocation_len: stack_buffer.len(),
            stack_buffer_ptr: ptr::null(),
            stack_buffer_len: stack_buffer.len(),
            fn_ptr,
        };

        // A hidden return pointer occupies the first argument slot.
        if let ReturnStrategy::HiddenPointer {
            size: return_size,
            align_log2: return_align,
        } = marshal_plan.return_strategy
        {
            if let Some(ret) = ret {
                let ret_ptr_bytes = ret.as_ptr().expose_provenance().to_ne_bytes();
                call_frame.gpr_registers[0].update_from_bytes(&ret_ptr_bytes);
            } else {
                // Reserve extra space for the return value if the caller simply wants to discard
                // the return value. Add an offset to the reserved space in the first argument
                // register. `invoke` must calculate the exact address.
                let return_align = 1usize << return_align;

                let ret_ptr_offset = call_frame
                    .stack_allocation_len
                    .next_multiple_of(return_align);

                call_frame.indirect_register_mask |= 1;
                call_frame.gpr_registers[0].update_from_bytes(&ret_ptr_offset.to_ne_bytes());
                call_frame.stack_allocation_len = ret_ptr_offset + return_size;
            }
        }

        for step in &marshal_plan.argument_moves {
            let (argument_index, destination) = match *step {
                ArgumentMove::ArgumentToGpr {
                    argument_index,
                    index,
                    size,
                } => (
                    argument_index,
                    &mut call_frame.gpr_registers[usize::from(index)].0[..usize::from(size)],
                ),
                ArgumentMove::ArgumentToXmm {
                    argument_index,
                    index,
                    size,
                } => (
                    argument_index,
                    &mut call_frame.xmm_registers[usize::from(index)].0[..usize::from(size)],
                ),
                ArgumentMove::ArgumentToStack {
                    argument_index,
                    offset,
                    size,
                } => (argument_index, &mut stack_buffer[offset..offset + size]),
                ArgumentMove::StackAddressToGpr { offset, index } => {
                    call_frame.gpr_registers[usize::from(index)]
                        .update_from_bytes(&offset.to_ne_bytes());
                    continue;
                }
                ArgumentMove::StackAddressToStack {
                    source_offset,
                    destination_offset,
                } => {
                    let destination = &mut stack_buffer
                        [destination_offset..destination_offset + size_of::<usize>()];
                    for (destination_byte, offset_byte) in
                        destination.iter_mut().zip(source_offset.to_ne_bytes())
                    {
                        destination_byte.write(offset_byte);
                    }
                    continue;
                }
            };
            let arg = &args[argument_index];

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
    let ret_ptr = ret.as_ptr();
    let (source, byte_length) = match return_strategy {
        ReturnStrategy::Void | ReturnStrategy::HiddenPointer { .. } => return,
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
        ptr::copy_nonoverlapping(source, ret_ptr.cast(), usize::from(byte_length));
    }
}

/// Calls a function using the Win64 marshal plan.
///
/// # Safety
///
/// * Uphold [`crate::function::Function::call`]'s safety contract.
/// * `marshal_plan` must describe `fn_ptr`, `args`, and `ret`.
pub(crate) unsafe fn call(
    marshal_plan: &MarshalPlan,
    fn_ptr: FnPtr,
    args: &[Arg],
    ret: Option<Ret<'_>>,
) {
    let mut stack_buffer = vec![MaybeUninit::<u8>::uninit(); marshal_plan.stack_buffer_size];

    // SAFETY:
    // * The caller provides arguments and return storage matching the plan.
    // * The fresh stack buffer has the planned size and cannot overlap the arguments.
    let mut call_frame =
        unsafe { CallFrame::new(marshal_plan, fn_ptr, args, ret.as_ref(), &mut stack_buffer) };

    // SAFETY:
    // * The frame, buffer, plan, and return storage remain alive during the call.
    // * The frame contains arguments matching the target signature.
    // * The caller provides valid return storage.
    unsafe {
        invoke(&raw mut call_frame);
    }

    if let Some(ret) = ret {
        // SAFETY:
        // * `invoke` stored the returned registers in the frame.
        // * The caller provides valid return storage, separate from the frame.
        unsafe {
            write_register_return(&call_frame, marshal_plan.return_strategy, ret);
        }
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
        "mov r11, [r12 + {stack_allocation_len_offset}]",
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
        "mov al, [r12 + {indirect_register_mask_offset}]",
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
        "mov rax, [r12 + {indirect_stack_pointer_offset}]",
        "mov r11, [r12 + {indirect_stack_count_offset}]",

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
        stack_allocation_len_offset = const offset_of!(CallFrame, stack_allocation_len),
        stack_buffer_len_offset = const offset_of!(CallFrame, stack_buffer_len),
        stack_buffer_ptr_offset = const offset_of!(CallFrame, stack_buffer_ptr),

        gpr_registers_offset = const offset_of!(CallFrame, gpr_registers),
        xmm_registers_offset = const offset_of!(CallFrame, xmm_registers),
        register_size = const size_of::<Register>(),

        fn_ptr_offset = const offset_of!(CallFrame, fn_ptr),

        indirect_register_mask_offset = const offset_of!(CallFrame, indirect_register_mask),
        indirect_stack_pointer_offset = const offset_of!(CallFrame, indirect_stack_offsets_pointer),
        indirect_stack_count_offset = const offset_of!(CallFrame, indirect_stack_offsets_count),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::CallSignature;
    use crate::function::Function;
    use crate::test_utils::structs::{
        F32, F32_ARG, F32X2_ARG, F32x2, U8X3_ARG, U8x3, U64X2_ARG, U64x2, U64x3,
    };
    use crate::types::{FfiType, Type, VariadicType};
    use crate::{VariadicAbi, fn_ptrize};

    extern "C" fn unused_target() {}

    #[test]
    fn variadic_float_copies_preserve_bits_and_hidden_return_storage() {
        let fixed = [Type::F32, Type::F64];
        let variadic = [VariadicType::F64, VariadicType::F64];
        let first = f32::from_bits(0xffc0_1234);
        let second = -0.0f64;
        let third = f64::from_bits(0x7ff8_1234_5678_9abc);
        let fourth = -123.5f64;
        let args = [
            Arg::new(&first),
            Arg::new(&second),
            Arg::new(&third),
            Arg::new(&fourth),
        ];
        let return_type = U64x3::ffi_type();

        for hidden_return in [false, true] {
            let plan = MarshalPlan::build(CallSignature::variadic(
                &fixed,
                &variadic,
                hidden_return.then_some(&return_type),
            ));
            let mut return_value = MaybeUninit::<U64x3>::uninit();
            let ret = hidden_return.then(|| Ret::new(&mut return_value));
            let return_address = ret.as_ref().map(|ret| ret.as_ptr().expose_provenance());
            let mut stack_buffer = vec![MaybeUninit::uninit(); plan.stack_buffer_size];
            // SAFETY: Arguments and optional return storage match the plan, remain live,
            // and are disjoint from the correctly sized stack buffer. The frame is not invoked.
            let frame = unsafe {
                CallFrame::new(
                    &plan,
                    fn_ptrize!(unused_target),
                    &args,
                    ret.as_ref(),
                    &mut stack_buffer,
                )
            };
            let first_slot = usize::from(hidden_return);
            for registers in [&frame.gpr_registers, &frame.xmm_registers] {
                assert_eq!(
                    initialized_bytes::<4>(&registers[first_slot].0[..4]),
                    first.to_ne_bytes()
                );
                assert_eq!(register_u64(&registers[first_slot + 1]), second.to_bits());
                assert_eq!(register_u64(&registers[first_slot + 2]), third.to_bits());
                if !hidden_return {
                    assert_eq!(register_u64(&registers[3]), fourth.to_bits());
                }
            }
            if let Some(address) = return_address {
                assert_eq!(register_usize(&frame.gpr_registers[0]), address);
                assert_eq!(initialized_bytes::<8>(&stack_buffer), fourth.to_ne_bytes());
            } else {
                assert!(stack_buffer.is_empty());
            }
            assert_eq!(frame.indirect_register_mask, 0);
            assert_eq!(frame.indirect_stack_offsets_count, 0);
        }
    }

    #[test]
    fn variadic_empty_tail_delivers_float_duplicates_to_machine_registers() {
        // The fixed signature describes every payload. The naked body additionally observes
        // the GPR copies supplied by a variadic caller, without requiring a host-native VaList.
        #[unsafe(naked)]
        unsafe extern "win64" fn capture(_first: f32, _second: f64, _output: *mut [u64; 4]) {
            core::arch::naked_asm!(
                "movd eax, xmm0",
                "mov [r8], rax",
                "mov eax, ecx",
                "mov [r8 + 8], rax",
                "movq rax, xmm1",
                "mov [r8 + 16], rax",
                "mov [r8 + 24], rdx",
                "ret",
            );
        }

        let function = Function::variadic_with_abi(
            fn_ptrize!(capture),
            &[Type::F32, Type::F64, Type::Pointer],
            &[],
            None,
            VariadicAbi::Win64,
        );
        let first = f32::from_bits(0xffc0_1234);
        let second = f64::from_bits(0x7ff8_1234_5678_9abc);
        let mut output = [0u64; 4];
        let output_pointer = &raw mut output;
        // SAFETY: The explicit Win64 ABI and all three payload types match capture.
        // The output pointer names a separate live writable array of four u64s.
        unsafe {
            function.call(
                &[
                    Arg::new(&first),
                    Arg::new(&second),
                    Arg::new(&output_pointer),
                ],
                None,
            );
        }
        assert_eq!(
            output,
            [
                u64::from(first.to_bits()),
                u64::from(first.to_bits()),
                second.to_bits(),
                second.to_bits()
            ]
        );
    }

    fn initialized_bytes<const N: usize>(bytes: &[MaybeUninit<u8>]) -> [u8; N] {
        assert_eq!(bytes.len(), N);

        core::array::from_fn(|index| {
            // SAFETY: These tests pass only initialized payload, register, or sentinel bytes.
            unsafe { *bytes[index].assume_init_ref() }
        })
    }

    fn register_usize(register: &Register) -> usize {
        usize::from_ne_bytes(initialized_bytes(&register.0))
    }

    fn register_u64(register: &Register) -> u64 {
        u64::from_ne_bytes(initialized_bytes(&register.0))
    }

    const GPR_RETURN_0: [u8; 8] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17];
    const XMM_RETURN_LOW: [u8; 8] = [0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57];
    const XMM_RETURN_HIGH: [u8; 8] = [0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67];
    const SENTINEL: u8 = 0xa5;

    fn synthetic_return_frame() -> CallFrame {
        let mut call_frame = CallFrame {
            gpr_registers: <[Register; 4] as Default>::default(),
            xmm_registers: <[Register; 4] as Default>::default(),
            indirect_register_mask: 0,
            indirect_stack_offsets_pointer: ptr::null(),
            indirect_stack_offsets_count: 0,
            stack_allocation_len: 0,
            stack_buffer_ptr: ptr::null(),
            stack_buffer_len: 0,
            fn_ptr: fn_ptrize!(unused_target),
        };

        let registers = call_frame
            .gpr_registers
            .iter_mut()
            .chain(&mut call_frame.xmm_registers);
        for (register, first_byte) in registers.zip((0x10u8..=0x80).step_by(16)) {
            let bytes = [
                first_byte,
                first_byte + 1,
                first_byte + 2,
                first_byte + 3,
                first_byte + 4,
                first_byte + 5,
                first_byte + 6,
                first_byte + 7,
            ];
            register.update_from_bytes(&bytes);
        }

        call_frame
    }

    #[test]
    fn scalar_register_returns_copy_only_the_declared_length() {
        let call_frame = synthetic_return_frame();
        let cases = [
            (ReturnStrategy::Rax { byte_length: 1 }, 1, GPR_RETURN_0),
            (ReturnStrategy::Rax { byte_length: 2 }, 2, GPR_RETURN_0),
            (ReturnStrategy::Rax { byte_length: 4 }, 4, GPR_RETURN_0),
            (ReturnStrategy::Rax { byte_length: 8 }, 8, GPR_RETURN_0),
            (ReturnStrategy::Xmm0 { byte_length: 4 }, 4, XMM_RETURN_LOW),
            (ReturnStrategy::Xmm0 { byte_length: 8 }, 8, XMM_RETURN_LOW),
        ];

        for (strategy, byte_length, expected_register) in cases {
            let mut return_buffer = [MaybeUninit::new(SENTINEL); 24];

            // SAFETY:
            // * The selected register bytes are initialized.
            // * The return slice is large enough and disjoint from the frame.
            unsafe {
                write_register_return(&call_frame, strategy, Ret::new(&mut return_buffer[8..16]));
            }

            let actual = initialized_bytes::<24>(&return_buffer);
            assert_eq!(&actual[..8], &[SENTINEL; 8], "{strategy:?}");
            assert_eq!(
                &actual[8..8 + byte_length],
                &expected_register[..byte_length],
                "{strategy:?}"
            );
            assert_eq!(
                &actual[8 + byte_length..],
                &[SENTINEL; 24][8 + byte_length..],
                "{strategy:?}"
            );
        }
    }

    #[test]
    fn full_xmm0_return_copies_both_saved_halves() {
        let call_frame = synthetic_return_frame();
        let mut return_buffer = [MaybeUninit::new(SENTINEL); 32];

        // SAFETY:
        // * Both eight-byte slots holding the saved xmm0 value are initialized.
        // * The return slice holds 16 bytes and is disjoint from the frame.
        unsafe {
            write_register_return(
                &call_frame,
                ReturnStrategy::Xmm0 { byte_length: 16 },
                Ret::new(&mut return_buffer[8..24]),
            );
        }

        let actual = initialized_bytes::<32>(&return_buffer);
        assert_eq!(&actual[..8], &[SENTINEL; 8]);
        assert_eq!(&actual[8..16], &XMM_RETURN_LOW);
        assert_eq!(&actual[16..24], &XMM_RETURN_HIGH);
        assert_eq!(&actual[24..], &[SENTINEL; 8]);
    }

    #[test]
    fn non_register_returns_do_not_write_return_storage() {
        let call_frame = synthetic_return_frame();

        let mut return_buffer = [MaybeUninit::new(SENTINEL); 24];

        // SAFETY: A hidden-pointer strategy does not access return storage or any register slot.
        unsafe {
            write_register_return(
                &call_frame,
                ReturnStrategy::HiddenPointer {
                    size: 0,
                    align_log2: 0,
                },
                Ret::new(&mut return_buffer),
            );
        }

        assert_eq!(initialized_bytes::<24>(&return_buffer), [SENTINEL; 24]);
    }

    #[test]
    fn mixed_direct_arguments_are_copied_to_their_planned_destinations() {
        let marshal_plan = MarshalPlan::build(CallSignature::new(
            &[Type::U64, Type::F64, Type::U64, Type::F32, Type::F64],
            None,
        ));
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
        let ret = None;
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
                ret.as_ref(),
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
        assert_eq!(call_frame.indirect_register_mask, 0);
        assert_eq!(call_frame.indirect_stack_offsets_count, 0);
    }

    #[test]
    fn short_arguments_preserve_register_tails_and_stack_slot_padding() {
        let marshal_plan = MarshalPlan::build(CallSignature::new(
            &[
                Type::U8,
                Type::U16,
                Type::U32,
                Type::F32,
                Type::U8,
                Type::U16,
                Type::U32,
                Type::F32,
            ],
            None,
        ));
        let first = 0x81u8;
        let second = 0x9234u16;
        let third = 0xa345_6789u32;
        let fourth = -core::f32::consts::PI;
        let stack_u8 = 0xb2u8;
        let stack_u16 = 0xc456u16;
        let stack_u32 = 0xd567_89abu32;
        let stack_f32 = core::f32::consts::E;
        let args = [
            Arg::new(&first),
            Arg::new(&second),
            Arg::new(&third),
            Arg::new(&fourth),
            Arg::new(&stack_u8),
            Arg::new(&stack_u16),
            Arg::new(&stack_u32),
            Arg::new(&stack_f32),
        ];
        let ret = None;
        let mut stack_buffer = vec![MaybeUninit::new(SENTINEL); marshal_plan.stack_buffer_size];

        // SAFETY:
        // * Arguments and the void return match the plan.
        // * The separate stack buffer has the planned size.
        // * The plan and buffer outlive the frame.
        let call_frame = unsafe {
            CallFrame::new(
                &marshal_plan,
                fn_ptrize!(unused_target),
                &args,
                ret.as_ref(),
                &mut stack_buffer,
            )
        };

        // Reading the whole register also checks that its unused high bytes remain zero.
        assert_eq!(register_u64(&call_frame.gpr_registers[0]), u64::from(first));
        assert_eq!(
            register_u64(&call_frame.gpr_registers[1]),
            u64::from(second)
        );
        assert_eq!(register_u64(&call_frame.gpr_registers[2]), u64::from(third));
        assert_eq!(
            register_u64(&call_frame.xmm_registers[3]),
            u64::from(fourth.to_bits())
        );
        assert_eq!(register_u64(&call_frame.gpr_registers[3]), 0);
        for register in &call_frame.xmm_registers[..3] {
            assert_eq!(register_u64(register), 0);
        }

        let actual = initialized_bytes::<32>(&stack_buffer);
        assert_eq!(&actual[..1], &stack_u8.to_ne_bytes());
        assert_eq!(&actual[1..8], &[SENTINEL; 7]);
        assert_eq!(&actual[8..10], &stack_u16.to_ne_bytes());
        assert_eq!(&actual[10..16], &[SENTINEL; 6]);
        assert_eq!(&actual[16..20], &stack_u32.to_ne_bytes());
        assert_eq!(&actual[20..24], &[SENTINEL; 4]);
        assert_eq!(&actual[24..28], &stack_f32.to_ne_bytes());
        assert_eq!(&actual[28..], &[SENTINEL; 4]);
    }

    #[test]
    fn small_float_aggregates_use_gprs_alongside_positional_scalar_floats() {
        let marshal_plan = MarshalPlan::build(CallSignature::new(
            &[F32::ffi_type(), Type::F32, F32x2::ffi_type(), Type::F64],
            None,
        ));
        let scalar_f32 = -core::f32::consts::PI;
        let scalar_f64 = core::f64::consts::E;
        let args = [
            Arg::new(&F32_ARG),
            Arg::new(&scalar_f32),
            Arg::new(&F32X2_ARG),
            Arg::new(&scalar_f64),
        ];
        let ret = None;
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
                ret.as_ref(),
                &mut stack_buffer,
            )
        };

        assert_eq!(
            register_u64(&call_frame.gpr_registers[0]),
            u64::from(F32_ARG.a.to_bits())
        );
        assert_eq!(
            initialized_bytes::<4>(&call_frame.gpr_registers[2].0[..4]),
            F32X2_ARG.a.to_ne_bytes()
        );
        assert_eq!(
            initialized_bytes::<4>(&call_frame.gpr_registers[2].0[4..]),
            F32X2_ARG.b.to_ne_bytes()
        );
        assert_eq!(
            register_u64(&call_frame.xmm_registers[1]),
            u64::from(scalar_f32.to_bits())
        );
        assert_eq!(
            register_u64(&call_frame.xmm_registers[3]),
            scalar_f64.to_bits()
        );
        for index in [0, 2] {
            assert_eq!(register_u64(&call_frame.xmm_registers[index]), 0);
        }
        for index in [1, 3] {
            assert_eq!(register_u64(&call_frame.gpr_registers[index]), 0);
        }
        assert!(stack_buffer.is_empty());
        assert_eq!(call_frame.indirect_register_mask, 0);
        assert_eq!(call_frame.indirect_stack_offsets_count, 0);
    }

    #[test]
    fn indirect_arguments_store_copies_and_deferred_stack_addresses() {
        let marshal_plan = MarshalPlan::build(CallSignature::new(
            &[
                U8x3::ffi_type(),
                Type::U64,
                Type::F64,
                Type::U64,
                U64x2::ffi_type(),
            ],
            None,
        ));
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
        let ret = None;
        let mut stack_buffer = vec![MaybeUninit::new(SENTINEL); marshal_plan.stack_buffer_size];

        // SAFETY:
        // * Arguments and the void return match the plan.
        // * The separate stack buffer has the planned size.
        // * The plan and buffer outlive the frame.
        let call_frame = unsafe {
            CallFrame::new(
                &marshal_plan,
                fn_ptrize!(unused_target),
                &args,
                ret.as_ref(),
                &mut stack_buffer,
            )
        };

        assert_eq!(call_frame.indirect_register_mask, 0b0001);
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
        assert_eq!(initialized_bytes::<8>(&stack_buffer[8..16]), [SENTINEL; 8]);
        assert_eq!(
            initialized_bytes::<3>(&stack_buffer[16..19]),
            [U8X3_ARG.a, U8X3_ARG.b, U8X3_ARG.c]
        );
        assert_eq!(
            initialized_bytes::<13>(&stack_buffer[19..32]),
            [SENTINEL; 13]
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
            call_frame.indirect_stack_offsets_pointer,
            marshal_plan.indirect_stack_offsets.as_ptr()
        );
        assert_eq!(call_frame.indirect_stack_offsets_count, 1);
        assert_eq!(marshal_plan.indirect_stack_offsets.as_ref(), [0]);
        assert_eq!(call_frame.stack_buffer_ptr, stack_buffer.as_ptr());
        assert_eq!(call_frame.stack_buffer_len, stack_buffer.len());
    }

    #[test]
    fn indirect_arguments_fill_all_gprs_and_multiple_stack_pointer_slots() {
        let argument_types = core::array::from_fn::<_, 6, _>(|_| U8x3::ffi_type());
        let marshal_plan = MarshalPlan::build(CallSignature::new(&argument_types, None));
        let values = [0x10u8, 0x20, 0x30, 0x40, 0x50, 0x60].map(|first| U8x3 {
            a: first,
            b: first + 1,
            c: first + 2,
        });
        let args = values.each_ref().map(Arg::new);
        let ret = None;
        let mut stack_buffer = vec![MaybeUninit::new(SENTINEL); marshal_plan.stack_buffer_size];

        // SAFETY:
        // * The six arguments and void return match the plan.
        // * The separate stack buffer has the planned size.
        // * The plan and buffer outlive the frame.
        let call_frame = unsafe {
            CallFrame::new(
                &marshal_plan,
                fn_ptrize!(unused_target),
                &args,
                ret.as_ref(),
                &mut stack_buffer,
            )
        };

        assert_eq!(call_frame.indirect_register_mask, 0b1111);
        for (register, expected_offset) in call_frame.gpr_registers.iter().zip([16, 32, 48, 64]) {
            assert_eq!(register_usize(register), expected_offset);
        }
        assert_eq!(
            usize::from_ne_bytes(initialized_bytes(&stack_buffer[..8])),
            80
        );
        assert_eq!(
            usize::from_ne_bytes(initialized_bytes(&stack_buffer[8..16])),
            96
        );
        assert_eq!(marshal_plan.indirect_stack_offsets.as_ref(), [0, 8]);
        assert_eq!(
            call_frame.indirect_stack_offsets_pointer,
            marshal_plan.indirect_stack_offsets.as_ptr()
        );
        assert_eq!(call_frame.indirect_stack_offsets_count, 2);

        for (value, offset) in values.iter().zip([16, 32, 48, 64, 80, 96]) {
            assert_eq!(
                initialized_bytes::<3>(&stack_buffer[offset..offset + 3]),
                [value.a, value.b, value.c],
                "copy at offset {offset}"
            );
            if offset < 96 {
                assert_eq!(
                    initialized_bytes::<13>(&stack_buffer[offset + 3..offset + 16]),
                    [SENTINEL; 13],
                    "padding after copy at offset {offset}"
                );
            }
        }
        assert_eq!(stack_buffer.len(), 99);
        assert_eq!(call_frame.stack_buffer_ptr, stack_buffer.as_ptr());
        assert_eq!(call_frame.stack_buffer_len, stack_buffer.len());
    }

    #[test]
    fn hidden_return_pointer_shifts_mixed_arguments_into_registers_and_stack() {
        let return_type = U64x3::ffi_type();
        let marshal_plan = MarshalPlan::build(CallSignature::new(
            &[Type::U64, Type::F64, Type::U64, Type::F32],
            Some(&return_type),
        ));
        let first = 0x1020_3040_5060_7080u64;
        let second = core::f64::consts::PI;
        let third = 0x90a0_b0c0_d0e0_f000u64;
        let fourth = -core::f32::consts::E;
        let args = [
            Arg::new(&first),
            Arg::new(&second),
            Arg::new(&third),
            Arg::new(&fourth),
        ];
        let mut return_value = MaybeUninit::<U64x3>::uninit();
        let ret = Ret::new(&mut return_value);
        let return_address = ret.as_ptr().expose_provenance();
        let ret = Some(ret);
        let mut stack_buffer = vec![MaybeUninit::new(SENTINEL); marshal_plan.stack_buffer_size];

        // SAFETY:
        // * Arguments and return storage match the plan.
        // * The separate stack buffer has the planned size.
        // * The plan, return storage, and buffer outlive the frame.
        let call_frame = unsafe {
            CallFrame::new(
                &marshal_plan,
                fn_ptrize!(unused_target),
                &args,
                ret.as_ref(),
                &mut stack_buffer,
            )
        };

        assert_eq!(register_usize(&call_frame.gpr_registers[0]), return_address);
        assert_eq!(register_u64(&call_frame.gpr_registers[1]), first);
        assert_eq!(register_u64(&call_frame.xmm_registers[2]), second.to_bits());
        assert_eq!(register_u64(&call_frame.gpr_registers[3]), third);
        assert_eq!(register_u64(&call_frame.gpr_registers[2]), 0);
        for index in [0, 1, 3] {
            assert_eq!(register_u64(&call_frame.xmm_registers[index]), 0);
        }
        let actual = initialized_bytes::<8>(&stack_buffer);
        assert_eq!(&actual[..4], &fourth.to_ne_bytes());
        assert_eq!(&actual[4..], &[SENTINEL; 4]);
        assert_eq!(call_frame.indirect_register_mask, 0);
        assert_eq!(call_frame.indirect_stack_offsets_count, 0);
    }

    #[test]
    fn hidden_return_and_direct_pointer_are_not_marked_as_stack_addresses() {
        let return_type = U64x3::ffi_type();
        let marshal_plan = MarshalPlan::build(CallSignature::new(
            &[Type::Pointer, Type::U128],
            Some(&return_type),
        ));
        let direct_pointer = ptr::without_provenance::<core::ffi::c_void>(0xfedc_ba98_7654_3210);
        let indirect_argument = 0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00u128;
        let args = [Arg::new(&direct_pointer), Arg::new(&indirect_argument)];
        let mut return_value = MaybeUninit::<U64x3>::uninit();
        let ret = Ret::new(&mut return_value);
        let return_address = ret.as_ptr().expose_provenance();
        let ret = Some(ret);
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
                ret.as_ref(),
                &mut stack_buffer,
            )
        };

        assert_eq!(call_frame.indirect_register_mask, 0b0100);
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
        assert_eq!(call_frame.indirect_stack_offsets_count, 0);
    }
}
