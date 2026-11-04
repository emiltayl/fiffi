use core::mem::MaybeUninit;
use core::ptr;

use super::asm::invoke;
use super::plan::{ArgumentMove, MarshalPlan, ReturnStrategy};
use crate::FnPtr;
use crate::backend::x86_64::Register;
use crate::function::{Arg, Ret};

#[derive(Debug)]
pub(super) struct CallFrame<'arg> {
    pub(super) argument_moves: *const ArgumentMove,
    pub(super) argument_move_len: usize,
    pub(super) arguments: *const Arg<'arg>,

    /// Hidden return storage address or offset from pre-call `rsp`.
    /// Only `(0, false)` is absent: offset zero denotes the allocation base.
    pub(super) return_pointer: usize,
    /// Whether `return_pointer` includes shadow space and must be added to pre-call `rsp`.
    pub(super) return_pointer_is_offset: bool,

    /// Saved bytes returned in `rax`.
    pub(super) return_rax: Register,
    /// All 16 saved bytes returned in `xmm0`.
    pub(super) return_xmm0: [Register; 2],

    /// Shadow space, stack argument slots, and aligned indirect copies, plus any padding and
    /// storage for a discarded hidden return.
    pub(super) stack_allocation_len: usize,
    pub(super) fn_ptr: FnPtr,
}

impl<'arg> CallFrame<'arg> {
    /// Records metadata for direct argument reads and the outgoing stack allocation.
    ///
    /// Argument storage is not read here. Invoking the frame requires matching signatures,
    /// valid descriptor bounds, and live plan, argument array, argument storage, and return
    /// storage throughout the call.
    fn new(
        marshal_plan: &MarshalPlan,
        fn_ptr: FnPtr,
        args: &[Arg<'arg>],
        ret: Option<&Ret<'_>>,
    ) -> Self {
        let mut call_frame = Self {
            argument_moves: marshal_plan.argument_moves.as_ptr(),
            argument_move_len: marshal_plan.argument_moves.len(),
            arguments: args.as_ptr(),
            return_pointer: 0,
            return_pointer_is_offset: false,
            return_rax: Register::default(),
            return_xmm0: <[Register; 2] as Default>::default(),
            stack_allocation_len: marshal_plan.stack_allocation_size,
            fn_ptr,
        };

        // A hidden return pointer occupies the first argument slot.
        if let ReturnStrategy::HiddenPointer {
            size: return_size,
            align_log2: return_align,
        } = marshal_plan.return_strategy
        {
            if let Some(ret) = ret {
                call_frame.return_pointer = ret.as_ptr().expose_provenance();
            } else {
                // Reserve aligned storage after the plan's allocation for a discarded return.
                // `invoke` resolves the offset from pre-call `rsp` and passes its address in rcx.
                let return_align = 1usize
                    .checked_shl(u32::from(return_align))
                    .expect("invalid Win64 return alignment");
                // The outgoing allocation base supports alignments up to 16 bytes.
                debug_assert!(return_align <= 16);
                let ret_ptr_offset = call_frame
                    .stack_allocation_len
                    .checked_next_multiple_of(return_align)
                    .expect("Win64 discarded-return alignment overflow");

                call_frame.return_pointer = ret_ptr_offset;
                call_frame.return_pointer_is_offset = true;
                call_frame.stack_allocation_len = ret_ptr_offset.strict_add(return_size);
            }
        }

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
        ReturnStrategy::Rax { byte_length } => (call_frame.return_rax.0.as_ptr(), byte_length),
        ReturnStrategy::Xmm0 { byte_length } => {
            // Borrow the whole array: a full `xmm0` return spans both eight-byte slots.
            (
                call_frame.return_xmm0.as_ptr().cast::<MaybeUninit<u8>>(),
                byte_length,
            )
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

/// Calls a function by reading arguments directly according to the Win64 marshal plan.
/// Assembly copies stack arguments and indirect values into the outgoing stack allocation.
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
    let mut call_frame = CallFrame::new(marshal_plan, fn_ptr, args, ret.as_ref());

    // SAFETY:
    // * The frame, plan, argument array and storage, and return storage remain alive during the
    //   call.
    // * The frame references arguments and valid descriptors matching the target signature.
    // * Argument storage cannot overlap the fresh outgoing stack allocation.
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

#[cfg(test)]
mod tests {
    use std::panic::catch_unwind;

    use super::*;
    use crate::function::Function;
    use crate::test_utils::GuardedReturn;
    use crate::test_utils::structs::{Bytes, F32, U64X2_ARG, U64x2, U64x3};
    use crate::test_utils::unions::{UNION_F32_U32_ARG, UnionF32U32};
    use crate::types::{FfiType, Type, VariadicType};
    use crate::{VariadicAbi, fn_ptrize};

    extern "C" fn unused_target() {}

    // Synthetic frames exercise reservation arithmetic only. They are never invoked, and no
    // synthetic storage is allocated or dereferenced.
    #[test]
    fn discarded_hidden_return_reservation_checks_alignment_and_size_overflow() {
        let frame = |stack_size, size, align_log2| {
            let plan = MarshalPlan {
                argument_moves: Box::new([]),
                stack_allocation_size: stack_size,
                return_strategy: ReturnStrategy::HiddenPointer { size, align_log2 },
            };
            CallFrame::new(&plan, fn_ptrize!(unused_target), &[], None)
        };
        assert!(catch_unwind(|| frame(usize::MAX - 14, 1, 4)).is_err());
        assert!(catch_unwind(|| frame(usize::MAX - 15, 16, 4)).is_err());
        for (stack, size, align_log2, expected_offset, expected_len) in [
            (32, 24, 3, 32, 56),
            (usize::MAX - 16, 15, 4, usize::MAX - 15, usize::MAX),
            (32, usize::MAX - 32, 0, 32, usize::MAX),
        ] {
            let result = frame(stack, size, align_log2);
            assert!(result.return_pointer_is_offset);
            assert_eq!(result.return_pointer, expected_offset);
            assert_eq!(result.stack_allocation_len, expected_len);
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep shifted entry probes beside their concrete signatures and live buffers."
    )]
    fn variadic_aggregate_integer_slots_and_indirect_payloads_survive_hidden_shifts() {
        #[unsafe(naked)]
        unsafe extern "win64" fn capture(
            _float: F32,
            _union: UnionF32U32,
            _odd: Bytes<3>,
            _wide: u128,
            _pair: U64x2,
            _output: *mut [u64; 9],
        ) -> u128 {
            core::arch::naked_asm!(
                // Four slots: ecx, edx, r8 and r9. Stack pointer to pair at rsp+40,
                // output at rsp+48, including the return address and 32-byte shadow space.
                "mov r10, [rsp + 48]",
                "mov eax, ecx",
                "mov [r10], rax",
                "mov eax, edx",
                "mov [r10 + 8], rax",
                "movzx eax, byte ptr [r8]",
                "mov [r10 + 16], rax",
                "movzx eax, byte ptr [r8 + 1]",
                "mov [r10 + 24], rax",
                "movzx eax, byte ptr [r8 + 2]",
                "mov [r10 + 32], rax",
                "mov rax, [r9]",
                "mov [r10 + 40], rax",
                "mov rax, [r9 + 8]",
                "mov [r10 + 48], rax",
                "mov r11, [rsp + 40]",
                "mov rax, [r11]",
                "mov [r10 + 56], rax",
                "mov rax, [r11 + 8]",
                "mov [r10 + 64], rax",
                "movdqu xmm0, [r9]",
                "ret",
            );
        }
        #[unsafe(naked)]
        unsafe extern "win64" fn capture_hidden(
            _float: F32,
            _union: UnionF32U32,
            _odd: Bytes<3>,
            _wide: u128,
            _pair: U64x2,
            _output: *mut [u64; 9],
        ) -> U64x3 {
            core::arch::naked_asm!(
                // rcx is hidden return, edx/r8d are small aggregates, r9 is odd copy.
                // Wide/pair/output now occupy rsp+40/48/56 after shadow space.
                "mov r10, [rsp + 56]",
                "mov eax, edx",
                "mov [r10], rax",
                "mov eax, r8d",
                "mov [r10 + 8], rax",
                "movzx eax, byte ptr [r9]",
                "mov [r10 + 16], rax",
                "movzx eax, byte ptr [r9 + 1]",
                "mov [r10 + 24], rax",
                "movzx eax, byte ptr [r9 + 2]",
                "mov [r10 + 32], rax",
                "mov r11, [rsp + 40]",
                "mov rax, [r11]",
                "mov [r10 + 40], rax",
                "mov [rcx], rax",
                "mov rax, [r11 + 8]",
                "mov [r10 + 48], rax",
                "mov [rcx + 8], rax",
                "mov r11, [rsp + 48]",
                "mov rax, [r11]",
                "mov [r10 + 56], rax",
                "mov [rcx + 16], rax",
                "mov rax, [r11 + 8]",
                "mov [r10 + 64], rax",
                "mov rax, rcx",
                "ret",
            );
        }
        let float = F32 {
            a: f32::from_bits(0xffc1_2345),
        };
        let odd = Bytes::<3>::VALUE;
        let wide = 0xfedc_ba98_7654_3210_0123_4567_89ab_cdefu128;
        // SAFETY: The fixture initializes the integer alternative.
        let integer = unsafe { UNION_F32_U32_ARG.integer };
        let expected = [
            u64::from(float.a.to_bits()),
            u64::from(integer),
            u64::from(odd.bytes[0]),
            u64::from(odd.bytes[1]),
            u64::from(odd.bytes[2]),
            0x0123_4567_89ab_cdef,
            0xfedc_ba98_7654_3210,
            U64X2_ARG.a,
            U64X2_ARG.b,
        ];
        for hidden in [false, true] {
            let return_type = if hidden {
                U64x3::ffi_type()
            } else {
                Type::U128
            };
            let function = Function::variadic_with_abi(
                if hidden {
                    fn_ptrize!(capture_hidden)
                } else {
                    fn_ptrize!(capture)
                },
                &[],
                &[
                    F32::ffi_type(),
                    UnionF32U32::ffi_type(),
                    Bytes::<3>::ffi_type(),
                    Type::U128,
                    U64x2::ffi_type(),
                    Type::Pointer,
                ]
                .into_iter()
                .map(|ty| VariadicType::try_from(ty).unwrap())
                .collect::<Vec<_>>(),
                Some(&return_type),
                VariadicAbi::Win64,
            );
            let mut output = [0; 9];
            let pointer = &raw mut output;
            let args = [
                Arg::new(&float),
                Arg::new(&UNION_F32_U32_ARG),
                Arg::new(&odd),
                Arg::new(&wide),
                Arg::new(&U64X2_ARG),
                Arg::new(&pointer),
            ];
            if hidden {
                let mut result = GuardedReturn::<U64x3>::new();
                // SAFETY: The explicit signature matches capture_hidden, including shifted
                // indirect pointers. Output and guarded return storage are separate live buffers.
                unsafe {
                    function.call(&args, Some(result.ret()));
                }
                assert_eq!(output, expected);
                // SAFETY: capture_hidden initialized all three fields.
                let result = unsafe { result.get() };
                assert_eq!(
                    result,
                    U64x3 {
                        a: 0x0123_4567_89ab_cdef,
                        b: 0xfedc_ba98_7654_3210,
                        c: U64X2_ARG.a
                    }
                );
                output.fill(0);
                // SAFETY: The same signature and arguments remain live; Function reserves the
                // discarded hidden result storage that capture_hidden writes.
                unsafe {
                    function.call(&args, None);
                }
                assert_eq!(output, expected);
            } else {
                let mut result = GuardedReturn::<u128>::new();
                // SAFETY: The explicit signature matches capture, which reads live copies and
                // returns all 16 payload bytes in xmm0.
                unsafe {
                    function.call(&args, Some(result.ret()));
                }
                assert_eq!(output, expected);
                // SAFETY: capture initialized the complete u128 result.
                assert_eq!(unsafe { result.get() }, wide);
            }
        }
    }

    #[test]
    fn variadic_indirect_aggregates_and_128_bit_tails_after_slot_exhaustion() {
        #[unsafe(naked)]
        unsafe extern "win64" fn capture(
            _output: *mut [u64; 13],
            _a: u64,
            _b: u64,
            _c: u64,
            _odd: Bytes<3>,
            _signed: i128,
            _unsigned: u128,
            _pair: U64x2,
            _tail: u64,
        ) -> u128 {
            core::arch::naked_asm!(
                "mov [rcx], rdx",
                "mov [rcx + 8], r8",
                "mov [rcx + 16], r9",
                "mov r10, [rsp + 40]",
                "movzx eax, byte ptr [r10]",
                "mov [rcx + 24], rax",
                "movzx eax, byte ptr [r10 + 1]",
                "mov [rcx + 32], rax",
                "movzx eax, byte ptr [r10 + 2]",
                "mov [rcx + 40], rax",
                "mov r10, [rsp + 48]",
                "mov rax, [r10]",
                "mov [rcx + 48], rax",
                "mov rax, [r10 + 8]",
                "mov [rcx + 56], rax",
                "mov r10, [rsp + 56]",
                "mov rax, [r10]",
                "mov [rcx + 64], rax",
                "mov rax, [r10 + 8]",
                "mov [rcx + 72], rax",
                "movdqu xmm0, [r10]",
                "mov r10, [rsp + 64]",
                "mov rax, [r10]",
                "mov [rcx + 80], rax",
                "mov rax, [r10 + 8]",
                "mov [rcx + 88], rax",
                "mov rax, [rsp + 72]",
                "mov [rcx + 96], rax",
                "ret",
            );
        }
        let odd = Bytes::<3>::VALUE;
        let signed = -0x1234_5678_9abc_def0_1122_3344_5566_7788i128;
        let unsigned = 0xfedc_ba98_7654_3210_0123_4567_89ab_cdefu128;
        let tail = 0x1357_9bdf_2468_ace0u64;
        let function = Function::variadic_with_abi(
            fn_ptrize!(capture),
            &[Type::Pointer, Type::U64, Type::U64, Type::U64],
            &[
                VariadicType::try_from(Bytes::<3>::ffi_type()).unwrap(),
                VariadicType::I128,
                VariadicType::U128,
                VariadicType::try_from(U64x2::ffi_type()).unwrap(),
                VariadicType::U64,
            ],
            Some(&Type::U128),
            VariadicAbi::Win64,
        );
        let mut output = [0; 13];
        let pointer = &raw mut output;
        let mut result = GuardedReturn::<u128>::new();
        // SAFETY: Output plus three markers occupy the four fixed slots. Each variadic indirect
        // pointer is read after the return address and shadow space, then exactly its initialized
        // payload width is inspected. The final scalar and 16-byte return match capture.
        unsafe {
            function.call(
                &[
                    Arg::new(&pointer),
                    Arg::new(&11u64),
                    Arg::new(&22u64),
                    Arg::new(&33u64),
                    Arg::new(&odd),
                    Arg::new(&signed),
                    Arg::new(&unsigned),
                    Arg::new(&U64X2_ARG),
                    Arg::new(&tail),
                ],
                Some(result.ret()),
            );
        }
        assert_eq!(
            output,
            [
                11,
                22,
                33,
                u64::from(odd.bytes[0]),
                u64::from(odd.bytes[1]),
                u64::from(odd.bytes[2]),
                0xeedd_ccbb_aa99_8878,
                0xedcb_a987_6543_210f,
                0x0123_4567_89ab_cdef,
                0xfedc_ba98_7654_3210,
                U64X2_ARG.a,
                U64X2_ARG.b,
                tail
            ]
        );
        // SAFETY: capture returned every byte of unsigned in xmm0.
        assert_eq!(unsafe { result.get() }, unsigned);
    }

    #[test]
    fn variadic_float_bits_reach_registers_and_stack_with_hidden_returns() {
        // These fixed signatures describe the payloads. The naked bodies also capture the
        // GPR duplicates supplied by a variadic caller, independently of the host's VaList.
        #[unsafe(naked)]
        unsafe extern "win64" fn capture(
            _first: f32,
            _second: f64,
            _third: f64,
            _fourth: f64,
            _output: *mut [u64; 8],
        ) {
            core::arch::naked_asm!(
                "mov r10, [rsp + 40]",
                "movd eax, xmm0",
                "mov [r10], rax",
                "mov eax, ecx",
                "mov [r10 + 8], rax",
                "movq rax, xmm1",
                "mov [r10 + 16], rax",
                "mov [r10 + 24], rdx",
                "movq rax, xmm2",
                "mov [r10 + 32], rax",
                "mov [r10 + 40], r8",
                "movq rax, xmm3",
                "mov [r10 + 48], rax",
                "mov [r10 + 56], r9",
                "ret",
            );
        }

        #[unsafe(naked)]
        unsafe extern "win64" fn capture_hidden(
            _first: f32,
            _second: f64,
            _third: f64,
            _fourth: f64,
            _output: *mut [u64; 8],
        ) -> U64x3 {
            core::arch::naked_asm!(
                // The hidden pointer shifts every float by one position. The fourth float
                // and output pointer occupy successive stack slots after shadow space.
                "mov r10, [rsp + 48]",
                "movd eax, xmm1",
                "mov [r10], rax",
                "mov eax, edx",
                "mov [r10 + 8], rax",
                "movq rax, xmm2",
                "mov [r10 + 16], rax",
                "mov [r10 + 24], r8",
                "movq rax, xmm3",
                "mov [r10 + 32], rax",
                "mov [r10 + 40], r9",
                "mov rax, [rsp + 40]",
                "mov [r10 + 48], rax",
                "mov [r10 + 56], rcx",
                // Initialize the hidden return buffer and return its address as Win64 requires.
                "mov eax, edx",
                "mov [rcx], rax",
                "mov [rcx + 8], r8",
                "mov [rcx + 16], r9",
                "mov rax, rcx",
                "ret",
            );
        }

        let first = f32::from_bits(0xffc0_1234);
        let second = -0.0f64;
        let third = f64::from_bits(0x7ff8_1234_5678_9abc);
        let fourth = -123.5f64;
        let expected_registers = [
            u64::from(first.to_bits()),
            u64::from(first.to_bits()),
            second.to_bits(),
            second.to_bits(),
            third.to_bits(),
            third.to_bits(),
        ];
        let return_type = U64x3::ffi_type();

        for hidden_return in [false, true] {
            let function = Function::variadic_with_abi(
                if hidden_return {
                    fn_ptrize!(capture_hidden)
                } else {
                    fn_ptrize!(capture)
                },
                &[Type::F32, Type::F64],
                &[VariadicType::F64, VariadicType::F64, VariadicType::Pointer],
                hidden_return.then_some(&return_type),
                VariadicAbi::Win64,
            );
            let mut output = [0u64; 8];
            let output_pointer = &raw mut output;
            let args = [
                Arg::new(&first),
                Arg::new(&second),
                Arg::new(&third),
                Arg::new(&fourth),
                Arg::new(&output_pointer),
            ];
            let mut return_value = MaybeUninit::<U64x3>::uninit();
            let return_address = (&raw mut return_value).expose_provenance();
            let ret = hidden_return.then(|| Ret::new(&mut return_value));
            // SAFETY: The explicit Win64 ABI, payloads, and optional hidden return match the
            // chosen capture function. Output and return storage are separate live buffers.
            unsafe {
                function.call(&args, ret);
            }

            assert_eq!(&output[..6], &expected_registers);
            assert_eq!(output[6], fourth.to_bits());
            if hidden_return {
                assert_eq!(output[7], u64::try_from(return_address).unwrap());
                // SAFETY: `capture_hidden` initialized all three fields through the hidden pointer.
                assert_eq!(
                    unsafe { return_value.assume_init() },
                    U64x3 {
                        a: u64::from(first.to_bits()),
                        b: second.to_bits(),
                        c: third.to_bits(),
                    }
                );

                output.fill(0);
                // SAFETY: The ABI and live argument/output storage still match `capture_hidden`.
                // Discarding the return requests writable temporary hidden-return storage.
                unsafe {
                    function.call(&args, None);
                }
                assert_eq!(&output[..6], &expected_registers);
                assert_eq!(output[6], fourth.to_bits());
                assert_ne!(output[7], 0);
                assert_eq!(output[7] % align_of::<U64x3>() as u64, 0);
            } else {
                assert_eq!(output[7], fourth.to_bits());
            }
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
}
