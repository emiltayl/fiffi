use core::mem::{MaybeUninit, offset_of};
use core::ptr;

use super::plan::{ArgumentMoveKind, MarshalPlan, ReturnStrategy};
use crate::FnPtr;
use crate::backend::x86_64::Register;
use crate::backend::x86_64::asm::{hidden_return_asm, stack_copy_asm, stack_setup_asm};
use crate::backend::x86_64::win64::plan::ArgumentMove;
use crate::function::{Arg, Ret};

// The dispatch uses ordered kind pairs and bit zero, and reads Arg as one pointer. Keep these
// assumptions checked even when unit tests are not built. Field offsets/strides use offset_of!
// and size_of! below, so the Rust structs themselves do not need a fixed field order.
const _: () = {
    assert!(size_of::<Arg<'_>>() == size_of::<*const ()>());
    assert!(ArgumentMove::KIND_MASK == 7);
    assert!(ArgumentMove::REGISTER_INDEX_SHIFT == 3);
    assert!(ArgumentMoveKind::ArgumentToGpr as u8 == 0);
    assert!(ArgumentMoveKind::ArgumentToXmm as u8 == 1);
    assert!(ArgumentMoveKind::ArgumentToStack as u8 == 2);
    assert!(ArgumentMoveKind::StackAddressToGpr as u8 == 4);
    assert!(ArgumentMoveKind::StackAddressToStack as u8 == 5);
};

#[derive(Debug)]
struct CallFrame<'arg> {
    argument_moves: *const ArgumentMove,
    argument_move_len: usize,
    arguments: *const Arg<'arg>,

    /// Hidden return storage address or offset from pre-call `rsp`.
    /// Only `(0, false)` is absent: offset zero denotes the allocation base.
    return_pointer: usize,
    /// Whether `return_pointer` includes shadow space and must be added to pre-call `rsp`.
    return_pointer_is_offset: bool,

    /// Saved bytes returned in `rax`.
    return_rax: Register,
    /// All 16 saved bytes returned in `xmm0`.
    return_xmm0: [Register; 2],

    /// Shadow space, stack argument slots, and aligned indirect copies, plus any padding and
    /// storage for a discarded hidden return.
    stack_allocation_len: usize,
    fn_ptr: FnPtr,
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

/// Invokes a function, reading arguments directly and copying values into its outgoing stack
/// allocation as directed by the plan.
///
/// Stack setup traps on allocation-address subtraction underflow before alignment or probing.
/// This guard does not guarantee that enough mapped stack exists.
///
/// # Safety
///
/// * The frame must remain writable for the call.
/// * Its argument array and storage, and plan metadata must remain readable for the call. Argument
///   storage must not overlap the outgoing stack allocation.
/// * The internal plan must supply valid kinds, argument indices, register slots, and bounds.
///   Register payloads must be 1, 2, 4, or 8 bytes. Stack offsets must have their low three bits
///   clear and include shadow space; indirect copies must be sixteen-byte aligned.
/// * Enough stack must be available for the allocation and up to 15 bytes of alignment padding;
///   supported alignments are at most 16.
/// * The plan, arguments, and return storage must match the target's ABI and signature.
/// * Any return storage must remain writable for the signature throughout the call.
#[unsafe(naked)]
unsafe extern "win64-unwind" fn invoke(call_frame: *mut CallFrame) {
    core::arch::naked_asm!(
        #[cfg(not(windows))]
        ".cfi_startproc",
        #[cfg(windows)]
        ".seh_proc {__unwind_function}",

        // Establish the frame pointer before recording frame-relative register saves.
        // After push rbp / mov rbp, rsp:
        //   rbp + 40       unused final slot of caller-provided shadow space
        //   rbp + 32       saved rsi
        //   rbp + 24       saved rdi
        //   rbp + 16       saved r12
        //   rbp + 8        return address to invoke's caller
        //   rbp            saved rbp
        //   below rbp      dynamic outgoing allocation, including alignment padding
        // After stack setup, relative to rsp immediately before call r11:
        //   rsp + 0..31    target's shadow space
        //   rsp + 32...    target's stack argument slots
        //   following     16-byte-aligned indirect copies
        //   following     optional aligned discarded-return storage
        // The call pushes a return address, so target entry rsp is eight bytes lower;
        // the fifth argument at pre-call rsp + 32 is at target entry rsp + 40.
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
        stack_setup_asm!(),

        // arguments passed in: rcx, rdx, r8, r9
        // Registers:
        // * `r10`: Current `ArgumentMove` pointer
        // * `r11`: Remaining move count
        // * `r12`: `CallFrame`
        // * `rsp`: Fixed outgoing allocation base throughout marshalling
        // * `xmm4`: Used to store argument that goes into `rcx`
        // * `rax`: Payload or computed stack address
        // * `rsi`: Argument data pointer
        // * `rdi`: Packed destination, then register slot or copy destination
        // * `rcx`: Kind, size, or copy count
        // Preserve populated rdx/r8/r9 and xmm0..xmm3.
        "mov r10, [r12 + {argument_moves_offset}]",
        "mov r11, [r12 + {argument_move_len_offset}]",

        "test r11, r11",
        "jz 3000f",

        "2000:",
        "mov rdi, [r10 + {move_destination_offset}]",
        "mov rcx, rdi",
        "and rcx, {kind_mask}",

        // ArgumentTo(Gpr|Xmm) share code to read the value. This branches if kind is 0 or 1.
        "cmp rcx, {argument_to_xmm}",
        "jbe 2100f",

        "cmp rcx, {argument_to_stack}",
        "je 2400f",

        // The internal plan guarantees that remaining kinds are StackAddressTo*.
        "jmp 2600f",

        // Shared argument lookup and exact-width loads for both register banks.
        "2100:",
        "mov rax, [r10 + {move_source_offset}]",
        "mov rsi, [r12 + {arguments_offset}]",
        "mov rsi, [rsi + rax * {arg_stride}]",
        "mov rcx, [r10 + {move_size_offset}]",

        "cmp rcx, 8",
        "je 2118f",
        "cmp rcx, 4",
        "je 2114f",
        "cmp rcx, 2",
        "je 2112f",
        // The supported-size invariant leaves only one byte; all narrow loads zero-extend.
        "movzx eax, byte ptr [rsi]",
        "jmp 2120f",

        "2118:",
        "mov rax, [rsi]",
        "jmp 2120f",
        "2114:",
        "mov eax, [rsi]",
        "jmp 2120f",
        "2112:",
        "movzx eax, word ptr [rsi]",

        "2120:",
        "test dil, 1",
        "jz 2700f",

        // Transfer payload bits without floating-point conversion.
        "2200:",
        "shr rdi, {register_index_shift}",
        "test rdi, rdi",
        "jz 2210f",
        "cmp rdi, 1",
        "je 2211f",
        "cmp rdi, 2",
        "je 2212f",
        "movq xmm3, rax",
        "jmp 2900f",

        "2210:",
        "movq xmm0, rax",
        "jmp 2900f",
        "2211:",
        "movq xmm1, rax",
        "jmp 2900f",
        "2212:",
        "movq xmm2, rax",
        "jmp 2900f",

        stack_copy_asm!("2900f"),

        // StackAddressTo(Gpr|Stack).
        // Both stored stack offsets are already relative to pre-call rsp, including shadow space.
        "2600:",
        "mov rax, [r10 + {move_source_offset}]",
        "lea rax, [rsp + rax]",
        "test dil, 1",
        "jz 2700f",
        // The only remaining valid kind is StackAddressToStack; rax holds one pointer.
        "and rdi, {stack_offset_mask}",
        "mov [rsp + rdi], rax",
        "jmp 2900f",

        // Shared GPR writer for argument payloads and indirect addresses.
        "2700:",
        "shr rdi, {register_index_shift}",
        "test rdi, rdi",
        "jz 2710f",
        "cmp rdi, 1",
        "je 2711f",
        "cmp rdi, 2",
        "je 2712f",
        "mov r9, rax",
        "jmp 2900f",

        "2710:",
        // The `rcx` value is stored in `xmm4` as `rcx` is used for argument marshalling.
        "movq xmm4, rax",
        "jmp 2900f",
        "2711:",
        "mov rdx, rax",
        "jmp 2900f",
        "2712:",
        "mov r8, rax",
        "2900:",
        // Execute descriptors in plan order, including variadic duplicates and address moves.
        "add r10, {move_stride}",
        "dec r11",
        "jnz 2000b",

        // Marshalling is complete: restore pending rcx, then resolve any hidden return pointer.
        "3000:",
        "movq rcx, xmm4",

        hidden_return_asm!("rcx"),

        "mov r11, [r12 + {fn_ptr_offset}]",
        "call r11",

        // Save registers used for return values.
        "mov [r12 + {return_rax_offset}], rax",
        "movups [r12 + {return_xmm0_offset}], xmm0",

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
        argument_moves_offset = const offset_of!(CallFrame, argument_moves),
        argument_move_len_offset = const offset_of!(CallFrame, argument_move_len),
        arguments_offset = const offset_of!(CallFrame, arguments),
        return_pointer_offset = const offset_of!(CallFrame, return_pointer),
        return_pointer_is_offset_offset = const offset_of!(CallFrame, return_pointer_is_offset),
        move_source_offset = const offset_of!(ArgumentMove, source),
        move_size_offset = const offset_of!(ArgumentMove, size),
        move_destination_offset = const offset_of!(ArgumentMove, destination),
        move_stride = const size_of::<ArgumentMove>(),
        arg_stride = const size_of::<Arg<'_>>(),
        kind_mask = const ArgumentMove::KIND_MASK,
        stack_offset_mask = const !ArgumentMove::KIND_MASK.cast_signed(),
        register_index_shift = const ArgumentMove::REGISTER_INDEX_SHIFT,
        argument_to_xmm = const ArgumentMoveKind::ArgumentToXmm as u8,
        argument_to_stack = const ArgumentMoveKind::ArgumentToStack as u8,

        stack_allocation_len_offset = const offset_of!(CallFrame, stack_allocation_len),

        return_rax_offset = const offset_of!(CallFrame, return_rax),
        return_xmm0_offset = const offset_of!(CallFrame, return_xmm0),

        fn_ptr_offset = const offset_of!(CallFrame, fn_ptr),
    );
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
