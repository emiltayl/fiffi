extern crate alloc;

use alloc::vec;
use core::mem::{MaybeUninit, offset_of, size_of};
use core::ptr;

use super::plan::{ArgumentDestination, ArgumentMove, MarshalPlan, RegisterBank, ReturnStrategy};
use crate::FnPtr;
use crate::backend::x86_64::Register;
use crate::backend::x86_64::asm::trampoline_asm_with_unwind_frame;
use crate::function::{Arg, Ret};

#[derive(Debug)]
struct CallFrame {
    /// Arguments passed in integer registers. The two first elements are also used for return
    /// values passed in integer registers.
    gpr_registers: [Register; 6],
    /// Arguments passed in xmm registers. The two first elements are also used for return values
    /// values passed in xmm registers.
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
    /// `marshal_plan`, `args`, and `ret` must describe the same function signature. Every argument
    /// referenced by the plan must be readable for its declared layout, and the argument storage
    /// must not overlap `stack_buffer`. The arguments, return storage, and stack buffer must remain
    /// alive while the returned frame is used.
    unsafe fn new(
        marshal_plan: &MarshalPlan,
        fn_ptr: FnPtr,
        args: &[Arg<'_>],
        ret: &Ret<'_>,
        stack_buffer: &mut [MaybeUninit<u8>],
    ) -> Self {
        assert_eq!(stack_buffer.len(), marshal_plan.stack_buffer_size);

        let mut call_frame = Self {
            gpr_registers: <[Register; 6] as Default>::default(),
            xmm_registers: <[Register; 8] as Default>::default(),
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
            let dst = move_destination(&mut call_frame, stack_buffer, step);
            let arg = args
                .get(step.argument_index)
                .expect("marshal plan references an argument that was not provided");

            // SAFETY: The caller guarantees that `arg` is readable for the layout used to build
            // `marshal_plan`. The plan guarantees that `source_offset + size` is within that
            // layout. `dst` is a bounds-checked slice of exactly `size` bytes in fresh call-owned
            // storage, which the safety contract requires not to overlap the argument storage.
            // Copying as `MaybeUninit<u8>` permits argument padding bytes to be uninitialized.
            unsafe {
                let src = arg
                    .as_ptr()
                    .cast::<MaybeUninit<u8>>()
                    .add(step.source_offset);
                ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), dst.len());
            }
        }

        call_frame
    }
}

/// Returns the exact destination range for an argument move.
///
/// A malformed internal marshal plan panics here before any unchecked pointer operation occurs.
fn move_destination<'frame>(
    call_frame: &'frame mut CallFrame,
    stack_buffer: &'frame mut [MaybeUninit<u8>],
    step: &ArgumentMove,
) -> &'frame mut [MaybeUninit<u8>] {
    match step.destination {
        ArgumentDestination::Gpr(index) => call_frame
            .gpr_registers
            .get_mut(index)
            .and_then(|register| register.0.get_mut(..step.size))
            .expect("marshal plan contains an invalid general-purpose register destination"),
        ArgumentDestination::Xmm(index) => call_frame
            .xmm_registers
            .get_mut(index)
            .and_then(|register| register.0.get_mut(..step.size))
            .expect("marshal plan contains an invalid vector register destination"),
        ArgumentDestination::Stack(offset) => stack_buffer
            .get_mut(offset..(offset + step.size))
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
    let return_register = |bank: RegisterBank, index: usize| match bank {
        RegisterBank::Gpr => &call_frame.gpr_registers[index],
        RegisterBank::Xmm => &call_frame.xmm_registers[index],
    };

    match return_strategy {
        ReturnStrategy::Void | ReturnStrategy::HiddenPointer => {}
        ReturnStrategy::SingleRegister { bank, byte_length } => {
            let register = return_register(bank, 0);

            // SAFETY: The caller guarantees that `ret` is valid for `byte_length` bytes and does
            // not overlap `call_frame`. `ReturnStrategy` guarantees that a single-register return
            // is no larger than the selected register.
            unsafe {
                ptr::copy_nonoverlapping(
                    register.0.as_ptr(),
                    ret.as_ptr().cast::<MaybeUninit<u8>>(),
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
            let ret_ptr = ret.as_ptr().cast::<MaybeUninit<u8>>();

            // SAFETY: The caller guarantees that `ret` is valid for eight bytes plus
            // `second_byte_length` bytes and does not overlap `call_frame`. `ReturnStrategy`
            // guarantees that both copy lengths fit in their selected registers.
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

/// Calls a function using a SysV marshal plan.
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
    // The trampoline macro supplies the platform-specific unwind metadata required by `invoke`.
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
/// invocation, along with the stack buffer referenced by the frame. Its function pointer must be
/// callable with the ABI arguments represented by the frame, and every register or stack byte read
/// by the called function must contain the corresponding argument data. Any return storage must be
/// valid for the signature. The assembly implementation must also provide correct unwind metadata.
#[unsafe(naked)]
unsafe extern "sysv64-unwind" fn invoke(call_frame: *mut CallFrame) {
    trampoline_asm_with_unwind_frame!(
        function = invoke,
        call_frame_register = rdi,
        stack_buffer_len_offset = offset_of!(CallFrame, stack_buffer_len),
        stack_argument_area_offset = 0,
        instructions = [
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
        ],
        operands = [
            stack_buffer_ptr_offset = const offset_of!(CallFrame, stack_buffer_ptr),

            gpr_registers_offset = const offset_of!(CallFrame, gpr_registers),
            xmm_registers_offset = const offset_of!(CallFrame, xmm_registers),
            register_size = const size_of::<Register>(),

            fn_ptr_offset = const offset_of!(CallFrame, fn_ptr),
        ],
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fn_ptrize;
    use crate::test_utils::structs::{
        F32X3_ARG, F32x3, U64_F64_ARG, U64F64, U64X2_ARG, U64x2, U64x3,
    };
    use crate::types::{FfiType, Type};

    extern "C" fn unused_target() {}

    fn initialized_bytes<const N: usize>(bytes: &[MaybeUninit<u8>]) -> [u8; N] {
        assert_eq!(bytes.len(), N);

        core::array::from_fn(|index| {
            // SAFETY: The tests only call this helper for registers initialized to zero and
            // populated from values without padding, or for stack buffers initialized with a
            // sentinel value before marshalling.
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

            // SAFETY: `return_buffer` is live, non-overlapping storage large enough for each
            // strategy, and the synthetic frame contains initialized bytes in the selected slot.
            unsafe {
                write_register_return(
                    &call_frame,
                    ReturnStrategy::SingleRegister { bank, byte_length },
                    Ret::new(&mut return_buffer),
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

            // SAFETY: `return_buffer` is live, non-overlapping storage large enough for the
            // strategy, and the synthetic frame contains initialized bytes in both selected slots.
            unsafe {
                write_register_return(
                    &call_frame,
                    ReturnStrategy::TwoRegisters {
                        first_bank: bank,
                        second_bank: bank,
                        second_byte_length: 5,
                    },
                    Ret::new(&mut return_buffer),
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

            // SAFETY: `return_buffer` is live, non-overlapping storage large enough for the
            // strategy, and the synthetic frame contains initialized bytes in both selected slots.
            unsafe {
                write_register_return(
                    &call_frame,
                    ReturnStrategy::TwoRegisters {
                        first_bank,
                        second_bank,
                        second_byte_length: 6,
                    },
                    Ret::new(&mut return_buffer),
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
            write_register_return(&call_frame, ReturnStrategy::Void, Ret::void());
        }

        let mut return_buffer = [MaybeUninit::new(RETURN_SENTINEL); 16];

        // SAFETY: A hidden-pointer strategy does not access return storage or any register slot.
        unsafe {
            write_register_return(
                &call_frame,
                ReturnStrategy::HiddenPointer,
                Ret::new(&mut return_buffer),
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
        let ret = Ret::void();
        let mut stack_buffer = vec![MaybeUninit::uninit(); marshal_plan.stack_buffer_size];

        // SAFETY: The argument matches the type used to build the plan and cannot overlap the
        // separately allocated stack buffer. The void return matches the plan.
        let call_frame = unsafe {
            CallFrame::new(
                &marshal_plan,
                fn_ptrize!(unused_target),
                &args,
                &ret,
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
        let ret = Ret::void();
        let mut stack_buffer = alloc::vec![
            MaybeUninit::uninit();
            marshal_plan.stack_buffer_size
        ];

        // SAFETY: The argument matches the type used to build the plan and cannot overlap the
        // separately allocated stack buffer. The void return matches the plan.
        let call_frame = unsafe {
            CallFrame::new(
                &marshal_plan,
                fn_ptrize!(unused_target),
                &args,
                &ret,
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
        let ret = Ret::void();
        let mut stack_buffer = alloc::vec![MaybeUninit::new(0xa5); marshal_plan.stack_buffer_size];

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
        let mut stack_buffer = alloc::vec![
            MaybeUninit::uninit();
            marshal_plan.stack_buffer_size
        ];

        // SAFETY: The argument and return storage match the types used to build the plan and cannot
        // overlap the separately allocated stack buffer.
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
            usize::from_ne_bytes(initialized_bytes(&call_frame.gpr_registers[0].0)),
            return_address
        );
        assert_eq!(register_u64(&call_frame.gpr_registers[1]), argument);
    }

    #[test]
    #[should_panic(
        expected = "marshal plan contains an invalid general-purpose register destination"
    )]
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
            source_offset: 0,
            size: 9,
            destination: ArgumentDestination::Gpr(0),
        };

        let _ = move_destination(&mut call_frame, &mut stack_buffer, &invalid_move);
    }
}
