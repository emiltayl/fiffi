extern crate alloc;

use alloc::vec;
use core::mem::{MaybeUninit, offset_of};
use core::ptr;

use super::plan::{ArgumentDestination, ArgumentMove, MarshalPlan, RegisterBank, ReturnStrategy};
use crate::FnPtr;
use crate::backend::x86_64::Register;
use crate::backend::x86_64::asm::stack_setup_asm;
use crate::function::{Arg, Ret};

#[derive(Debug)]
struct CallFrame {
    /// GPR arguments; the first two slots also hold `rax` and `rdx` returns.
    gpr_registers: [Register; 6],
    /// XMM arguments; the first two slots also hold `xmm0` and `xmm1` returns.
    xmm_registers: [Register; 8],

    stack_buffer_ptr: *const MaybeUninit<u8>,
    stack_buffer_len: usize,
    fn_ptr: FnPtr,
}

impl CallFrame {
    /// Creates a call frame and marshals the arguments into it.
    ///
    /// # Safety
    ///
    /// - `marshal_plan`, `args`, and `ret` must match the same signature.
    /// - Arguments must be readable for their layouts and disjoint from `stack_buffer`.
    /// - `stack_buffer` must have the planned size.
    /// - Return storage and `stack_buffer` must outlive use of the frame.
    unsafe fn new(
        marshal_plan: &MarshalPlan,
        fn_ptr: FnPtr,
        args: &[Arg<'_>],
        ret: Option<&Ret<'_>>,
        stack_buffer: &mut [MaybeUninit<u8>],
    ) -> Self {
        let mut call_frame = Self {
            gpr_registers: <[Register; 6] as Default>::default(),
            xmm_registers: <[Register; 8] as Default>::default(),
            stack_buffer_ptr: ptr::null(),
            stack_buffer_len: stack_buffer.len(),
            fn_ptr,
        };

        // The hidden return pointer occupies the first GPR.
        if marshal_plan.return_strategy == ReturnStrategy::HiddenPointer {
            let ret_ptr_bytes = ret
                .map_or(ptr::null_mut(), Ret::as_ptr)
                .expose_provenance()
                .to_ne_bytes();
            call_frame.gpr_registers[0].update_from_bytes(&ret_ptr_bytes);
        }

        for step in &marshal_plan.argument_moves {
            let (dst, source_offset) = copy_destination(&mut call_frame, stack_buffer, step);
            let arg = &args[step.argument_index];

            // SAFETY:
            // - The signature and plan bound each source copy.
            // - `dst` is bounds-checked storage disjoint from the arguments.
            // - `MaybeUninit<u8>` permits uninitialized padding.
            unsafe {
                let src = arg.as_ptr().cast::<MaybeUninit<u8>>().add(source_offset);
                ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), dst.len());
            }
        }

        call_frame.stack_buffer_ptr = stack_buffer.as_ptr();
        call_frame
    }
}

/// Returns the destination range and source byte offset for an argument copy.
fn copy_destination<'frame>(
    call_frame: &'frame mut CallFrame,
    stack_buffer: &'frame mut [MaybeUninit<u8>],
    step: &ArgumentMove,
) -> (&'frame mut [MaybeUninit<u8>], usize) {
    match step.destination {
        ArgumentDestination::Gpr {
            index,
            source_offset,
            size,
        } => (
            &mut call_frame.gpr_registers[usize::from(index)].0[..usize::from(size)],
            usize::from(source_offset),
        ),
        ArgumentDestination::Xmm {
            index,
            source_offset,
            size,
        } => (
            &mut call_frame.xmm_registers[usize::from(index)].0[..usize::from(size)],
            usize::from(source_offset),
        ),
        ArgumentDestination::Stack { offset, size } => {
            (&mut stack_buffer[offset..offset + size], 0)
        }
    }
}

/// Copies a register return into caller-provided storage.
///
/// # Safety
///
/// - The selected registers must contain the planned return value.
/// - For register returns, `ret` must be writable for the return layout and disjoint from the
///   frame.
unsafe fn write_register_return(
    call_frame: &CallFrame,
    return_strategy: ReturnStrategy,
    ret: Option<Ret<'_>>,
) {
    let ret_ptr = ret.as_ref().map_or(ptr::null_mut(), Ret::as_ptr);
    let return_register = |bank: RegisterBank, index: usize| match bank {
        RegisterBank::Gpr => &call_frame.gpr_registers[index],
        RegisterBank::Xmm => &call_frame.xmm_registers[index],
    };

    match return_strategy {
        ReturnStrategy::Void | ReturnStrategy::HiddenPointer => {}
        ReturnStrategy::SingleRegister { bank, byte_length } => {
            let register = return_register(bank, 0);

            // SAFETY:
            // - `byte_length` is at most eight bytes, so the the source register is valid as a
            //   source when reading `byte_length` bytes.
            // - The caller provides `byte_length` writable bytes at `ret`, without overlap.
            unsafe {
                ptr::copy_nonoverlapping(
                    register.0.as_ptr(),
                    ret_ptr.cast::<MaybeUninit<u8>>(),
                    usize::from(byte_length),
                );
            }
        }
        ReturnStrategy::TwoRegisters {
            first_bank,
            second_bank,
            second_byte_length,
        } => {
            let first_register = return_register(first_bank, 0);
            let second_register_index = usize::from(first_bank == second_bank);
            let second_register = return_register(second_bank, second_register_index);
            let ret_ptr = ret_ptr.cast::<MaybeUninit<u8>>();

            // SAFETY:
            // - Each source register holds eight bytes; `second_byte_length` is at most eight.
            // - The caller provides `8 + second_byte_length` writable bytes at `ret`.
            // - Neither source register overlaps the return storage.
            unsafe {
                ptr::copy_nonoverlapping(first_register.0.as_ptr(), ret_ptr, 8);
                ptr::copy_nonoverlapping(
                    second_register.0.as_ptr(),
                    ret_ptr.add(8),
                    usize::from(second_byte_length),
                );
            }
        }
    }
}

/// Calls a function using a `SysV` marshal plan.
///
/// # Safety
///
/// - Uphold [`crate::function::Function::call`]'s safety requirements.
/// - `marshal_plan` must match `fn_ptr`, `args`, and `ret`.
pub(crate) unsafe fn call(
    marshal_plan: &MarshalPlan,
    fn_ptr: FnPtr,
    args: &[Arg<'_>],
    ret: Option<Ret<'_>>,
) {
    let mut stack_buffer = vec![MaybeUninit::<u8>::uninit(); marshal_plan.stack_buffer_size];

    // SAFETY:
    // - The caller supplies matching arguments and return storage.
    // - The fresh buffer has the planned size and outlives the invocation.
    let mut call_frame =
        unsafe { CallFrame::new(marshal_plan, fn_ptr, args, ret.as_ref(), &mut stack_buffer) };

    // SAFETY:
    // - The frame and its buffer remain alive throughout the invocation.
    // - The frame matches the signature and storage supplied by the caller.
    unsafe {
        invoke(&raw mut call_frame);
    }

    // SAFETY:
    // - `invoke` saved the return registers in the frame.
    // - The caller supplies valid return storage, disjoint from the frame.
    unsafe {
        write_register_return(&call_frame, marshal_plan.return_strategy, ret);
    }
}

/// Invokes the function described by a call frame.
///
/// # Safety
///
/// - `call_frame` must be writable and outlive the invocation, together with its buffer.
/// - Register and stack arguments must match the function pointer's `SysV` signature.
/// - Return storage must be valid for that signature.
#[unsafe(naked)]
unsafe extern "sysv64-unwind" fn invoke(call_frame: *mut CallFrame) {
    core::arch::naked_asm!(
        #[cfg(not(windows))]
        ".cfi_startproc",
        #[cfg(windows)]
        ".seh_proc {__unwind_function}",

        // Preserve nonvolatile registers; `rbp` anchors unwinding while `rsp` moves.
        "push rbp",
        #[cfg(not(windows))]
        ".cfi_adjust_cfa_offset 8",
        #[cfg(not(windows))]
        ".cfi_offset rbp, -16",
        #[cfg(windows)]
        ".seh_pushreg rbp",
        "push r12",
        #[cfg(not(windows))]
        ".cfi_adjust_cfa_offset 8",
        #[cfg(not(windows))]
        ".cfi_offset r12, -24",
        #[cfg(windows)]
        ".seh_pushreg r12",
        "mov rbp, rsp",
        #[cfg(not(windows))]
        ".cfi_def_cfa_register rbp",
        #[cfg(windows)]
        ".seh_setframe rbp, 0",
        #[cfg(windows)]
        ".seh_endprologue",

        "mov r12, rdi",
        stack_setup_asm!("[r12 + {stack_buffer_len_offset}]"),

        // Copy the stack arguments into the probed allocation.
        "mov rsi, [r12 + {stack_buffer_ptr_offset}]",
        "mov rcx, [r12 + {stack_buffer_len_offset}]",
        "mov rdi, r10",
        "rep movsb",

        // Set up register arguments.
        "mov rdi, [r12 + {gpr_registers_offset} + {register_size} * 0]",
        "mov rsi, [r12 + {gpr_registers_offset} + {register_size} * 1]",
        "mov rdx, [r12 + {gpr_registers_offset} + {register_size} * 2]",
        "mov rcx, [r12 + {gpr_registers_offset} + {register_size} * 3]",
        "mov r8, [r12 + {gpr_registers_offset} + {register_size} * 4]",
        "mov r9, [r12 + {gpr_registers_offset} + {register_size} * 5]",

        "movq xmm0, [r12 + {xmm_registers_offset} + {register_size} * 0]",
        "movq xmm1, [r12 + {xmm_registers_offset} + {register_size} * 1]",
        "movq xmm2, [r12 + {xmm_registers_offset} + {register_size} * 2]",
        "movq xmm3, [r12 + {xmm_registers_offset} + {register_size} * 3]",
        "movq xmm4, [r12 + {xmm_registers_offset} + {register_size} * 4]",
        "movq xmm5, [r12 + {xmm_registers_offset} + {register_size} * 5]",
        "movq xmm6, [r12 + {xmm_registers_offset} + {register_size} * 6]",
        "movq xmm7, [r12 + {xmm_registers_offset} + {register_size} * 7]",

        "mov r11, [r12 + {fn_ptr_offset}]",
        "call r11",

        // Save registers used for return values.
        "mov [r12 + {gpr_registers_offset} + {register_size} * 0], rax",
        "mov [r12 + {gpr_registers_offset} + {register_size} * 1], rdx",
        "movq [r12 + {xmm_registers_offset} + {register_size} * 0], xmm0",
        "movq [r12 + {xmm_registers_offset} + {register_size} * 1], xmm1",

        // Restore the stack with a Windows-recognized frame-pointer epilogue.
        "lea rsp, [rbp]",
        "pop r12",
        #[cfg(not(windows))]
        ".cfi_restore r12",
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
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fn_ptrize;
    use crate::test_utils::structs::{U64_F64_ARG, U64F64, U64X2_ARG, U64x2, U64x3};
    use crate::types::{FfiType, Type};

    extern "C" fn unused_target() {}

    fn initialized_bytes<const N: usize>(bytes: &[MaybeUninit<u8>]) -> [u8; N] {
        assert_eq!(bytes.len(), N);

        core::array::from_fn(|index| {
            // SAFETY: Tests supply initialized bytes without padding.
            unsafe { *bytes[index].assume_init_ref() }
        })
    }

    fn register_u64(register: &Register) -> u64 {
        u64::from_ne_bytes(initialized_bytes(&register.0))
    }

    const GPR_RETURN_0: [u8; 8] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17];
    const GPR_RETURN_1: [u8; 8] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27];
    const XMM_RETURN_0: [u8; 8] = [0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37];
    const XMM_RETURN_1: [u8; 8] = [0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47];
    const RETURN_SENTINEL: u8 = 0xa5;

    fn synthetic_return_frame() -> CallFrame {
        let mut call_frame = CallFrame {
            gpr_registers: <[Register; 6] as Default>::default(),
            xmm_registers: <[Register; 8] as Default>::default(),
            stack_buffer_ptr: ptr::null(),
            stack_buffer_len: 0,
            fn_ptr: fn_ptrize!(unused_target),
        };

        call_frame.gpr_registers[0].update_from_bytes(&GPR_RETURN_0);
        call_frame.gpr_registers[1].update_from_bytes(&GPR_RETURN_1);
        call_frame.xmm_registers[0].update_from_bytes(&XMM_RETURN_0);
        call_frame.xmm_registers[1].update_from_bytes(&XMM_RETURN_1);

        call_frame
    }

    #[test]
    fn single_register_return_copies_only_the_declared_length() {
        let call_frame = synthetic_return_frame();
        let cases = [
            (RegisterBank::Gpr, 5, GPR_RETURN_0),
            (RegisterBank::Xmm, 3, XMM_RETURN_0),
        ];

        for (bank, byte_length, expected_register) in cases {
            let mut return_buffer = [MaybeUninit::new(RETURN_SENTINEL); 16];

            // SAFETY:
            // - The buffer is large enough and disjoint from the frame.
            // - Selected registers contain initialized bytes.
            unsafe {
                write_register_return(
                    &call_frame,
                    ReturnStrategy::SingleRegister { bank, byte_length },
                    Some(Ret::new(&mut return_buffer)),
                );
            }

            let actual = initialized_bytes::<16>(&return_buffer);
            let byte_length = usize::from(byte_length);
            assert_eq!(&actual[..byte_length], &expected_register[..byte_length]);
            assert_eq!(
                &actual[byte_length..],
                &[RETURN_SENTINEL; 16][byte_length..]
            );
        }
    }

    #[test]
    fn same_bank_two_register_return_uses_slots_zero_and_one() {
        let call_frame = synthetic_return_frame();
        let cases = [
            (RegisterBank::Gpr, GPR_RETURN_0, GPR_RETURN_1),
            (RegisterBank::Xmm, XMM_RETURN_0, XMM_RETURN_1),
        ];

        for (bank, expected_first, expected_second) in cases {
            let mut return_buffer = [MaybeUninit::new(RETURN_SENTINEL); 16];

            // SAFETY:
            // - The buffer is large enough and disjoint from the frame.
            // - Selected registers contain initialized bytes.
            unsafe {
                write_register_return(
                    &call_frame,
                    ReturnStrategy::TwoRegisters {
                        first_bank: bank,
                        second_bank: bank,
                        second_byte_length: 5,
                    },
                    Some(Ret::new(&mut return_buffer)),
                );
            }

            let actual = initialized_bytes::<16>(&return_buffer);
            assert_eq!(&actual[..8], &expected_first);
            assert_eq!(&actual[8..13], &expected_second[..5]);
            assert_eq!(&actual[13..], &[RETURN_SENTINEL; 3]);
        }
    }

    #[test]
    fn mixed_two_register_return_uses_slot_zero_of_each_bank() {
        let call_frame = synthetic_return_frame();
        let cases = [
            (
                RegisterBank::Gpr,
                RegisterBank::Xmm,
                GPR_RETURN_0,
                XMM_RETURN_0,
            ),
            (
                RegisterBank::Xmm,
                RegisterBank::Gpr,
                XMM_RETURN_0,
                GPR_RETURN_0,
            ),
        ];

        for (first_bank, second_bank, expected_first, expected_second) in cases {
            let mut return_buffer = [MaybeUninit::new(RETURN_SENTINEL); 16];

            // SAFETY:
            // - The buffer is large enough and disjoint from the frame.
            // - Selected registers contain initialized bytes.
            unsafe {
                write_register_return(
                    &call_frame,
                    ReturnStrategy::TwoRegisters {
                        first_bank,
                        second_bank,
                        second_byte_length: 6,
                    },
                    Some(Ret::new(&mut return_buffer)),
                );
            }

            let actual = initialized_bytes::<16>(&return_buffer);
            assert_eq!(&actual[..8], &expected_first);
            assert_eq!(&actual[8..14], &expected_second[..6]);
            assert_eq!(&actual[14..], &[RETURN_SENTINEL; 2]);
        }
    }

    #[test]
    fn non_register_returns_do_not_write_return_storage() {
        let call_frame = synthetic_return_frame();

        // SAFETY: A void strategy does not access return storage or any register slot.
        unsafe {
            write_register_return(&call_frame, ReturnStrategy::Void, None);
        }

        let mut return_buffer = [MaybeUninit::new(RETURN_SENTINEL); 16];

        // SAFETY: A hidden-pointer strategy does not access return storage or any register slot.
        unsafe {
            write_register_return(
                &call_frame,
                ReturnStrategy::HiddenPointer,
                Some(Ret::new(&mut return_buffer)),
            );
        }

        assert_eq!(
            initialized_bytes::<16>(&return_buffer),
            [RETURN_SENTINEL; 16]
        );
    }

    #[test]
    fn split_argument_uses_source_offset_for_second_eightbyte() {
        let marshal_plan = MarshalPlan::build(&[U64x2::ffi_type()], None);
        let args = [Arg::new(&U64X2_ARG)];
        let ret = None;
        let mut stack_buffer = vec![MaybeUninit::uninit(); marshal_plan.stack_buffer_size];

        // SAFETY:
        // - The argument and void return match the plan.
        // - The separate buffer has the planned size and outlives the frame.
        let call_frame = unsafe {
            CallFrame::new(
                &marshal_plan,
                fn_ptrize!(unused_target),
                &args,
                ret.as_ref(),
                &mut stack_buffer,
            )
        };

        assert_eq!(register_u64(&call_frame.gpr_registers[0]), U64X2_ARG.a);
        assert_eq!(register_u64(&call_frame.gpr_registers[1]), U64X2_ARG.b);
    }

    #[test]
    fn mixed_aggregate_marshals_each_eightbyte_to_its_register_bank() {
        let marshal_plan = MarshalPlan::build(&[U64F64::ffi_type()], None);
        let args = [Arg::new(&U64_F64_ARG)];
        let ret = None;
        let mut stack_buffer = alloc::vec![
            MaybeUninit::uninit();
            marshal_plan.stack_buffer_size
        ];

        // SAFETY:
        // - The argument and void return match the plan.
        // - The separate buffer has the planned size and outlives the frame.
        let call_frame = unsafe {
            CallFrame::new(
                &marshal_plan,
                fn_ptrize!(unused_target),
                &args,
                ret.as_ref(),
                &mut stack_buffer,
            )
        };

        assert_eq!(register_u64(&call_frame.gpr_registers[0]), U64_F64_ARG.a);
        assert_eq!(
            register_u64(&call_frame.xmm_registers[0]),
            U64_F64_ARG.b.to_bits()
        );
    }

    #[test]
    fn stack_arguments_preserve_alignment_gaps() {
        let argument_types = [
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U128,
        ];
        let marshal_plan = MarshalPlan::build(&argument_types, None);
        let register_arguments = [1u64, 2, 3, 4, 5, 6];
        let stack_u64 = 0x1122_3344_5566_7788u64;
        let stack_u128 = 0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00u128;
        let args = [
            Arg::new(&register_arguments[0]),
            Arg::new(&register_arguments[1]),
            Arg::new(&register_arguments[2]),
            Arg::new(&register_arguments[3]),
            Arg::new(&register_arguments[4]),
            Arg::new(&register_arguments[5]),
            Arg::new(&stack_u64),
            Arg::new(&stack_u128),
        ];
        let ret = None;
        let mut stack_buffer = alloc::vec![MaybeUninit::new(0xa5); marshal_plan.stack_buffer_size];

        // SAFETY:
        // - The arguments and void return match the plan.
        // - The separate buffer has the planned size and outlives the frame.
        let call_frame = unsafe {
            CallFrame::new(
                &marshal_plan,
                fn_ptrize!(unused_target),
                &args,
                ret.as_ref(),
                &mut stack_buffer,
            )
        };

        assert_eq!(call_frame.stack_buffer_ptr, stack_buffer.as_ptr());
        assert_eq!(call_frame.stack_buffer_len, stack_buffer.len());
        assert_eq!(
            initialized_bytes::<8>(&stack_buffer[..8]),
            stack_u64.to_ne_bytes()
        );
        assert_eq!(initialized_bytes::<8>(&stack_buffer[8..16]), [0xa5; 8]);
        assert_eq!(
            initialized_bytes::<16>(&stack_buffer[16..32]),
            stack_u128.to_ne_bytes()
        );
    }

    #[test]
    fn hidden_return_pointer_uses_first_gpr_and_shifts_arguments() {
        let return_type = U64x3::ffi_type();
        let marshal_plan = MarshalPlan::build(&[Type::U64], Some(&return_type));
        let argument = 0x0123_4567_89ab_cdefu64;
        let args = [Arg::new(&argument)];
        let mut return_value = MaybeUninit::<U64x3>::uninit();
        let ret = Ret::new(&mut return_value);
        let return_address = ret.as_ptr().expose_provenance();
        let ret = Some(ret);
        let mut stack_buffer = alloc::vec![
            MaybeUninit::uninit();
            marshal_plan.stack_buffer_size
        ];

        // SAFETY:
        // - The argument and return storage match the plan.
        // - The separate buffer has the planned size and outlives the frame.
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
            usize::from_ne_bytes(initialized_bytes(&call_frame.gpr_registers[0].0)),
            return_address
        );
        assert_eq!(register_u64(&call_frame.gpr_registers[1]), argument);
    }

    #[test]
    #[should_panic = "range end index 9 out of range for slice of length 8"]
    fn malformed_destination_panics_before_copying() {
        let mut call_frame = CallFrame {
            gpr_registers: <[Register; 6] as Default>::default(),
            xmm_registers: <[Register; 8] as Default>::default(),
            stack_buffer_ptr: ptr::null(),
            stack_buffer_len: 0,
            fn_ptr: fn_ptrize!(unused_target),
        };
        let mut stack_buffer = [];
        let invalid_move = ArgumentMove {
            argument_index: 0,
            destination: ArgumentDestination::Gpr {
                index: 0,
                source_offset: 0,
                size: 9,
            },
        };

        let _ = copy_destination(&mut call_frame, &mut stack_buffer, &invalid_move);
    }
}
