use core::mem::{MaybeUninit, offset_of};
use core::ptr;

use super::plan::{ArgumentMove, ArgumentMoveKind, MarshalPlan, RegisterBank, ReturnStrategy};
use crate::FnPtr;
use crate::backend::x86_64::Register;
use crate::backend::x86_64::asm::{hidden_return_asm, stack_copy_asm, stack_setup_asm};
use crate::function::{Arg, Ret};

// Dispatch uses bit zero to select a register bank and reads Arg as one pointer. Field
// offsets/strides use offset_of! and size_of! below; the structs need no fixed field order.
const _: () = {
    assert!(ArgumentMoveKind::Gpr as u8 == 0);
    assert!(ArgumentMoveKind::Xmm as u8 == 1);
    assert!(ArgumentMoveKind::Stack as u8 == 2);
};

#[derive(Debug)]
struct CallFrame<'arg> {
    argument_moves: *const ArgumentMove,
    argument_move_len: usize,
    arguments: *const Arg<'arg>,

    /// Hidden return storage address or offset from pre-call `rsp`.
    /// Only `(0, false)` is absent: offset zero denotes the allocation base.
    return_pointer: usize,
    /// Whether `return_pointer` must be added to pre-call `rsp`.
    return_pointer_is_offset: bool,

    /// Saved bytes returned in `rax` and `rdx`.
    return_gpr: [Register; 2],
    /// Saved low eight bytes returned in `xmm0` and `xmm1`.
    return_xmm: [Register; 2],

    /// Stack arguments and alignment padding, plus storage for a discarded hidden return.
    stack_allocation_len: usize,

    fn_ptr: FnPtr,

    /// See [`MarshalPlan::al`].
    al: u8,
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
            return_gpr: <[Register; 2] as Default>::default(),
            return_xmm: <[Register; 2] as Default>::default(),
            stack_allocation_len: marshal_plan.stack_allocation_size,
            fn_ptr,
            al: marshal_plan.al,
        };

        // The hidden return pointer occupies the first GPR.
        if let ReturnStrategy::HiddenPointer {
            size: return_size,
            align_log2: return_align,
        } = marshal_plan.return_strategy
        {
            if let Some(ret) = ret {
                call_frame.return_pointer = ret.as_ptr().expose_provenance();
            } else {
                // Reserve aligned storage after the plan's allocation for a discarded return.
                // `invoke` resolves the offset from pre-call `rsp` and passes its address in rdi.
                let return_align = 1usize
                    .checked_shl(u32::from(return_align))
                    .expect("invalid SysV return alignment");
                // The outgoing allocation base supports alignments up to 16 bytes.
                debug_assert!(return_align <= 16);

                let ret_ptr_offset = call_frame
                    .stack_allocation_len
                    .checked_next_multiple_of(return_align)
                    .expect("SysV discarded-return alignment overflow");

                call_frame.return_pointer = ret_ptr_offset;
                call_frame.return_pointer_is_offset = true;
                call_frame.stack_allocation_len = ret_ptr_offset.strict_add(return_size);
            }
        }

        call_frame
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
    ret: Ret<'_>,
) {
    let ret_ptr = ret.as_ptr();
    let return_register = |bank: RegisterBank, index: usize| match bank {
        RegisterBank::Gpr => &call_frame.return_gpr[index],
        RegisterBank::Xmm => &call_frame.return_xmm[index],
    };

    match return_strategy {
        ReturnStrategy::Void | ReturnStrategy::HiddenPointer { .. } => {}
        ReturnStrategy::SingleRegister { bank, byte_length } => {
            let register = return_register(bank, 0);

            // SAFETY:
            // - `byte_length` is at most eight bytes, so the source register is valid as a source
            //   when reading `byte_length` bytes.
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
    let mut call_frame = CallFrame::new(marshal_plan, fn_ptr, args, ret.as_ref());

    // SAFETY:
    // - The frame, plan, argument array and storage, and return storage remain alive during the
    //   call and match the target signature.
    // - The internal plan supplies valid descriptors, and arguments cannot overlap the fresh
    //   outgoing stack allocation.
    unsafe {
        invoke(&raw mut call_frame);
    }

    if let Some(ret) = ret {
        // SAFETY:
        // - `invoke` saved the return registers in the frame.
        // - The caller supplies valid return storage, disjoint from the frame.
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
/// * The internal plan must supply valid kinds (0, 1, 2), argument indices, and copy bounds. GPR
///   indices must be below six, XMM indices below eight, and register payloads in `1..=8`. Register
///   source offsets must be zero or eight, with offset + size within the argument layout. Stack
///   offsets must have their low three bits clear (zero is valid); offset + size must fit inside
///   the allocation. Stack copies start at source offset zero and use exact byte counts.
/// * Enough stack must be available for the allocation and up to 15 bytes of alignment padding;
///   supported alignments are at most 16.
/// * The plan, arguments, and return storage must match the target's ABI and signature.
/// * Any return storage must remain writable for the signature throughout the call.
#[unsafe(naked)]
unsafe extern "sysv64-unwind" fn invoke(call_frame: *mut CallFrame) {
    core::arch::naked_asm!(
        #[cfg(not(windows))]
        ".cfi_startproc",
        #[cfg(windows)]
        ".seh_proc {__unwind_function}",

        // Preserve nonvolatile registers; rbp anchors unwinding while rsp moves.
        // After push rbp / push r12 / mov rbp, rsp:
        //   rbp + 16       return address to invoke's caller
        //   rbp + 8        saved rbp
        //   rbp            saved r12
        //   below rbp      dynamic outgoing allocation, including alignment padding
        // After stack setup, relative to rsp immediately before call r11:
        //   rsp + 0...     target's stack arguments, with planned alignment gaps
        //   following     optional aligned discarded-return storage
        // The call pushes a return address, so target entry rsp is eight bytes lower;
        // the first stack argument at pre-call rsp + 0 is at target entry rsp + 8.
        // There is no caller shadow space for nonvolatile saves in this ABI.
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
        stack_setup_asm!(),

        // Registers:
        // * r10: Current ArgumentMove pointer
        // * r11: Remaining move count; later the target address
        // * r12: CallFrame, preserved across the target call
        // * rsp: Fixed outgoing allocation base throughout marshalling
        // * xmm8/xmm9/xmm10: Deferred rdi/rsi/rcx argument payloads
        // * rax: Payload or copy scratch
        // * rsi: Argument data pointer
        // * rdi: Packed destination, then register slot or copy destination
        // * rcx: Kind, size, fragment scratch, or copy count
        // Preserve populated rdx/r8/r9 and xmm0..xmm7. SysV makes xmm8..xmm15
        // caller-saved.
        "mov r10, [r12 + {argument_moves_offset}]",
        "mov r11, [r12 + {argument_move_len_offset}]",

        "pxor xmm8, xmm8",
        "pxor xmm9, xmm9",
        "pxor xmm10, xmm10",
        "test r11, r11",
        "jz 3000f",

        "2000:",
        "mov rdi, [r10 + {move_destination_offset}]",
        "mov rcx, rdi",
        "and rcx, {kind_mask}",

        // ArgumentTo(Gpr|Xmm) share code to read the value. This branches if kind is 0 or 1.
        "cmp rcx, {argument_to_xmm}",
        "jbe 2100f",
        // The internal plan guarantees that the remaining kind is ArgumentStack.
        "jmp 2400f",

        // Shared argument lookup and exact-width loads for both register banks.
        "2100:",
        "mov rax, [r10 + {move_source_offset}]",
        "mov rsi, [r12 + {arguments_offset}]",
        "mov rsi, [rsi + rax * {arg_stride}]",
        // Decode the source offset only for register moves, before the loader reuses rcx.
        "mov rcx, rdi",
        "shr rcx, {source_offset_shift}",
        "and rcx, {source_offset_mask}",
        "lea rsi, [rsi + rcx * {source_offset_scale}]",
        "mov rcx, [r10 + {move_size_offset}]",

        // Read exactly rcx bytes (1..=8) from rsi into rax, zero-extending short payloads.
        // Unaligned fragments stay within the source range; padding bits remain in assembly.
        // Clobber only rax, rcx, and flags, preserving pointers and remaining registers.
        "cmp rcx, 8",
        "je 2118f",
        "cmp rcx, 4",
        "je 2114f",
        "jb 2110f",
        "cmp rcx, 6",
        "je 2116f",
        "cmp rcx, 5",
        "je 2115f",
        // The supported-size invariant leaves seven bytes.
        "mov eax, [rsi]",
        "movzx ecx, word ptr [rsi + 4]",
        "shl rcx, 32",
        "or rax, rcx",
        "movzx ecx, byte ptr [rsi + 6]",
        "shl rcx, 48",
        "or rax, rcx",
        "jmp 2120f",
        "2110:",
        "cmp rcx, 2",
        "je 2112f",
        "cmp rcx, 1",
        "je 2111f",
        // The supported-size invariant leaves three bytes.
        "movzx eax, word ptr [rsi]",
        "movzx ecx, byte ptr [rsi + 2]",
        "shl rcx, 16",
        "or rax, rcx",
        "jmp 2120f",
        "2118:",
        "mov rax, [rsi]",
        "jmp 2120f",
        "2114:",
        "mov eax, [rsi]",
        "jmp 2120f",
        "2112:",
        "movzx eax, word ptr [rsi]",
        "jmp 2120f",
        "2111:",
        "movzx eax, byte ptr [rsi]",
        "jmp 2120f",
        "2115:",
        "mov eax, [rsi]",
        "movzx ecx, byte ptr [rsi + 4]",
        "shl rcx, 32",
        "or rax, rcx",
        "jmp 2120f",
        "2116:",
        "mov eax, [rsi]",
        "movzx ecx, word ptr [rsi + 4]",
        "shl rcx, 32",
        "or rax, rcx",

        "2120:",
        "test dil, 1",
        "jz 2700f",

        // Transfer payload bits without floating-point conversion.
        "2200:",
        "shr rdi, {register_index_shift}",
        // Unlike Win64, shifting also includes the source-offset bit.
        "and rdi, {register_index_mask}",
        "cmp rdi, 4",
        "jb 2201f",
        "cmp rdi, 6",
        "jb 2202f",
        "je 2216f",
        "movq xmm7, rax",
        "jmp 2900f",
        "2202:",
        "cmp rdi, 4",
        "je 2214f",
        "movq xmm5, rax",
        "jmp 2900f",
        "2201:",
        "cmp rdi, 2",
        "jb 2203f",
        "je 2212f",
        "movq xmm3, rax",
        "jmp 2900f",
        "2203:",
        "test rdi, rdi",
        "jz 2210f",
        "movq xmm1, rax",
        "jmp 2900f",
        "2210:",
        "movq xmm0, rax",
        "jmp 2900f",
        "2212:",
        "movq xmm2, rax",
        "jmp 2900f",
        "2214:",
        "movq xmm4, rax",
        "jmp 2900f",
        "2216:",
        "movq xmm6, rax",
        "jmp 2900f",

        stack_copy_asm!("2900f"),

        // Shared GPR writer for argument payloads.
        "2700:",
        "shr rdi, {register_index_shift}",
        "and rdi, {register_index_mask}",
        "cmp rdi, 3",
        "jb 2701f",
        "je 2713f",
        "cmp rdi, 4",
        "je 2714f",
        "mov r9, rax",
        "jmp 2900f",
        "2701:",
        "test rdi, rdi",
        "jz 2710f",
        "cmp rdi, 1",
        "je 2711f",
        "mov rdx, rax",
        "jmp 2900f",

        "2710:",
        "movq xmm8, rax",
        "jmp 2900f",
        "2711:",
        "movq xmm9, rax",
        "jmp 2900f",
        "2713:",
        "movq xmm10, rax",
        "jmp 2900f",
        "2714:",
        "mov r8, rax",
        "2900:",
        // Execute descriptors in plan order, including split arguments and whole spills.
        "add r10, {move_stride}",
        "dec r11",
        "jnz 2000b",

        // Marshalling is complete: restore pending rdi/rsi/rcx, then any hidden return pointer.
        "3000:",
        "movq rdi, xmm8",
        "movq rsi, xmm9",
        "movq rcx, xmm10",

        hidden_return_asm!("rdi"),

        // Set the vector count after all payload and hidden-pointer work that uses rax.
        "mov al, [r12 + {al_offset}]",
        "mov r11, [r12 + {fn_ptr_offset}]",
        "call r11",

        // Save registers used for return values.
        "mov [r12 + {return_gpr_offset} + {register_size} * 0], rax",
        "mov [r12 + {return_gpr_offset} + {register_size} * 1], rdx",
        "movq [r12 + {return_xmm_offset} + {register_size} * 0], xmm0",
        "movq [r12 + {return_xmm_offset} + {register_size} * 1], xmm1",

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
        stack_allocation_len_offset = const offset_of!(CallFrame, stack_allocation_len),
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
        register_index_mask = const ArgumentMove::REGISTER_INDEX_MASK,
        source_offset_shift = const ArgumentMove::SOURCE_OFFSET_SHIFT,
        source_offset_mask = const ArgumentMove::SOURCE_OFFSET_MASK,
        source_offset_scale = const ArgumentMove::SOURCE_OFFSET_SCALE,
        argument_to_xmm = const ArgumentMoveKind::Xmm as u8,

        return_gpr_offset = const offset_of!(CallFrame, return_gpr),
        return_xmm_offset = const offset_of!(CallFrame, return_xmm),
        register_size = const size_of::<Register>(),

        fn_ptr_offset = const offset_of!(CallFrame, fn_ptr),

        al_offset = const offset_of!(CallFrame, al),
    );
}
#[cfg(test)]
mod tests {
    use std::panic::catch_unwind;

    use super::*;
    use crate::function::Function;
    use crate::test_utils::GuardedReturn;
    use crate::test_utils::structs::{
        F32X3_ARG, F32x3, F64X2_ARG, F64x2, NESTED_F32_F32_U32_ARG, NESTED_F32_U32_F32_ARG,
        NestedF32F32U32, NestedF32U32F32, U64_F64_ARG, U64F64, U64x3,
    };
    use crate::test_utils::unions::{
        UNION_F32X3_U8_F64_ARG, UNION_U8_F64_F32X3_ARG, UnionF32x3U8F64, UnionU8F64F32x3,
    };
    use crate::types::{FfiType, Type, VariadicType};
    use crate::{VariadicAbi, fn_ptrize};

    // These probes describe concrete SysV entry signatures. Offsets include the return address;
    // SysV has no shadow space. Only volatile GPRs and XMM registers are modified.
    #[test]
    fn variadic_float_aggregate_preserves_partial_second_eightbyte() {
        #[unsafe(naked)]
        unsafe extern "sysv64" fn capture(_output: *mut [u64; 3], _value: F32x3, _tail: f64) {
            core::arch::naked_asm!(
                "movq rax, xmm0",
                "mov [rdi], rax",
                "movd eax, xmm1",
                "mov [rdi + 8], rax",
                "movq rax, xmm2",
                "mov [rdi + 16], rax",
                "ret",
            );
        }
        let function = Function::variadic_with_abi(
            fn_ptrize!(capture),
            &[Type::Pointer],
            &[
                VariadicType::try_from(F32x3::ffi_type()).unwrap(),
                VariadicType::F64,
            ],
            None,
            VariadicAbi::SysV,
        );
        let mut output = [0; 3];
        let pointer = &raw mut output;
        let tail = -73.5f64;
        // SAFETY: The explicit SysV signature matches the aggregate, promoted tail, and separate
        // live output buffer. The probe reads only 4 bytes from the partial second eightbyte.
        unsafe {
            function.call(
                &[Arg::new(&pointer), Arg::new(&F32X3_ARG), Arg::new(&tail)],
                None,
            );
        }
        assert_eq!(
            output,
            [
                u64::from(F32X3_ARG.a.to_bits()) | (u64::from(F32X3_ARG.b.to_bits()) << 32),
                u64::from(F32X3_ARG.c.to_bits()),
                tail.to_bits(),
            ]
        );
    }

    #[test]
    fn variadic_two_vector_aggregate_spills_before_last_vector_scalar() {
        #[unsafe(naked)]
        unsafe extern "sysv64" fn capture(
            _output: *mut [u64; 10],
            _a: f64,
            _b: f64,
            _c: f64,
            _d: f64,
            _e: f64,
            _f: f64,
            _g: f64,
            _pair: F64x2,
            _tail: f64,
        ) {
            core::arch::naked_asm!(
                "movq rax, xmm0",
                "mov [rdi + 0], rax",
                "movq rax, xmm1",
                "mov [rdi + 8], rax",
                "movq rax, xmm2",
                "mov [rdi + 16], rax",
                "movq rax, xmm3",
                "mov [rdi + 24], rax",
                "movq rax, xmm4",
                "mov [rdi + 32], rax",
                "movq rax, xmm5",
                "mov [rdi + 40], rax",
                "movq rax, xmm6",
                "mov [rdi + 48], rax",
                "mov rax, [rsp + 8]",
                "mov [rdi + 56], rax",
                "mov rax, [rsp + 16]",
                "mov [rdi + 64], rax",
                "movq rax, xmm7",
                "mov [rdi + 72], rax",
                "ret",
            );
        }
        let mut fixed = vec![Type::Pointer];
        fixed.extend(vec![Type::F64; 7]);
        let function = Function::variadic_with_abi(
            fn_ptrize!(capture),
            &fixed,
            &[
                VariadicType::try_from(F64x2::ffi_type()).unwrap(),
                VariadicType::F64,
            ],
            None,
            VariadicAbi::SysV,
        );
        let mut output = [0; 10];
        let pointer = &raw mut output;
        let floats = [11.25f64, -22.5, 33.75, -44.25, 55.5, -66.75, 77.25];
        let tail = -89.5f64;
        let mut args = vec![Arg::new(&pointer)];
        args.extend(floats.iter().map(Arg::new));
        args.extend([Arg::new(&F64X2_ARG), Arg::new(&tail)]);
        // SAFETY: The fixed output and seven floats plus the aggregate and promoted tail match
        // capture. The pair starts at entry rsp+8; the scalar uses the remaining xmm7.
        unsafe {
            function.call(&args, None);
        }
        let mut expected = floats.map(f64::to_bits).to_vec();
        expected.extend([F64X2_ARG.a.to_bits(), F64X2_ARG.b.to_bits(), tail.to_bits()]);
        assert_eq!(output.as_slice(), expected);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep entry probes beside their concrete signatures and live buffers."
    )]
    fn variadic_mixed_spill_keeps_vector_register_with_and_without_hidden_return() {
        #[unsafe(naked)]
        unsafe extern "sysv64" fn capture(
            _output: *mut [u64; 8],
            _a: u64,
            _b: u64,
            _c: u64,
            _d: u64,
            _e: u64,
            _pair: U64F64,
            _tail: f64,
        ) {
            core::arch::naked_asm!(
                "mov [rdi], rsi",
                "mov [rdi + 8], rdx",
                "mov [rdi + 16], rcx",
                "mov [rdi + 24], r8",
                "mov [rdi + 32], r9",
                "mov rax, [rsp + 8]",
                "mov [rdi + 40], rax",
                "mov rax, [rsp + 16]",
                "mov [rdi + 48], rax",
                "movq rax, xmm0",
                "mov [rdi + 56], rax",
                "ret",
            );
        }
        #[unsafe(naked)]
        unsafe extern "sysv64" fn capture_hidden(
            _output: *mut [u64; 8],
            _a: u64,
            _b: u64,
            _c: u64,
            _d: u64,
            _pair: U64F64,
            _tail: f64,
        ) -> U64x3 {
            core::arch::naked_asm!(
                // Hidden pointer rdi, output rsi, four scalars exhaust rdx through r9.
                "mov [rsi], rdx",
                "mov [rsi + 8], rcx",
                "mov [rsi + 16], r8",
                "mov [rsi + 24], r9",
                "mov rax, [rsp + 8]",
                "mov [rsi + 32], rax",
                "mov [rdi], rax",
                "mov rax, [rsp + 16]",
                "mov [rsi + 40], rax",
                "mov [rdi + 8], rax",
                "movq rax, xmm0",
                "mov [rsi + 48], rax",
                "mov [rdi + 16], rax",
                "mov rax, rdi",
                "ret",
            );
        }
        let integers = [0x1122u64, 0x3344, 0x5566, 0x7788, 0x99aa];
        let tail = -117.5f64;
        let return_type = U64x3::ffi_type();
        for hidden in [false, true] {
            let count = if hidden { 4 } else { 5 };
            let mut fixed = vec![Type::Pointer];
            fixed.extend(vec![Type::U64; count]);
            let function = Function::variadic_with_abi(
                if hidden {
                    fn_ptrize!(capture_hidden)
                } else {
                    fn_ptrize!(capture)
                },
                &fixed,
                &[
                    VariadicType::try_from(U64F64::ffi_type()).unwrap(),
                    VariadicType::F64,
                ],
                hidden.then_some(&return_type),
                VariadicAbi::SysV,
            );
            let mut output = [0; 8];
            let pointer = &raw mut output;
            let mut args = vec![Arg::new(&pointer)];
            args.extend(integers[..count].iter().map(Arg::new));
            args.extend([Arg::new(&U64_F64_ARG), Arg::new(&tail)]);
            let mut result = GuardedReturn::<U64x3>::new();
            // SAFETY: The selected entry signature includes the output slot, fixed scalars,
            // stack pair, vector tail, and optional hidden return. All storage remains live.
            unsafe {
                function.call(&args, hidden.then(|| result.ret()));
            }
            let mut expected = integers[..count].to_vec();
            expected.extend([U64_F64_ARG.a, U64_F64_ARG.b.to_bits(), tail.to_bits()]);
            assert_eq!(&output[..expected.len()], expected);
            if hidden {
                // SAFETY: capture_hidden initialized each field of the hidden return.
                let result = unsafe { result.get() };
                assert_eq!(
                    result,
                    U64x3 {
                        a: U64_F64_ARG.a,
                        b: U64_F64_ARG.b.to_bits(),
                        c: tail.to_bits()
                    }
                );
                output.fill(0);
                // SAFETY: The same signature and buffers remain live; discarded storage is
                // provided by Function for all fields written by capture_hidden.
                unsafe {
                    function.call(&args, None);
                }
                assert_eq!(&output[..expected.len()], expected);
            }
        }
    }

    #[test]
    fn variadic_nested_and_reversed_union_views_have_independent_return_classes() {
        #[unsafe(naked)]
        unsafe extern "sysv64" fn capture(
            _output: *mut [u64; 4],
            _nested: NestedF32U32F32,
            _union: UnionU8F64F32x3,
        ) -> NestedF32F32U32 {
            core::arch::naked_asm!(
                "mov [rdi], rsi", "movd eax, xmm0", "mov [rdi + 8], rax",
                "movzx eax, dl", "mov [rdi + 16], rax",
                "movq rax, xmm1", "mov [rdi + 24], rax",
                // Return has the opposite ordering: xmm0 holds two f32s, eax one u32.
                "mov rax, {float_bits}", "movq xmm0, rax", "mov eax, {integer}", "ret",
                float_bits = const (NESTED_F32_F32_U32_ARG.head.to_bits() as u64)
                    | ((NESTED_F32_F32_U32_ARG.inner.a.to_bits() as u64) << 32),
                integer = const NESTED_F32_F32_U32_ARG.inner.b,
            );
        }
        // Both concrete union declarations have identical ABI payload layouts.
        for reversed in [false, true] {
            let union_type = if reversed {
                UnionU8F64F32x3::ffi_type()
            } else {
                UnionF32x3U8F64::ffi_type()
            };
            let function = Function::variadic_with_abi(
                fn_ptrize!(capture),
                &[Type::Pointer],
                &[
                    VariadicType::try_from(NestedF32U32F32::ffi_type()).unwrap(),
                    VariadicType::try_from(union_type).unwrap(),
                ],
                Some(&NestedF32F32U32::ffi_type()),
                VariadicAbi::SysV,
            );
            let mut output = [0; 4];
            let pointer = &raw mut output;
            let mut result = GuardedReturn::<NestedF32F32U32>::new();
            let union = if reversed {
                Arg::new(&UNION_U8_F64_F32X3_ARG)
            } else {
                Arg::new(&UNION_F32X3_U8_F64_ARG)
            };
            // SAFETY: Both unions initialize the U8F64 variant; the probe inspects its byte and
            // f64 only, avoiding padding. Fixed, variadic and return descriptions are independent.
            unsafe {
                function.call(
                    &[Arg::new(&pointer), Arg::new(&NESTED_F32_U32_F32_ARG), union],
                    Some(result.ret()),
                );
            }
            let (byte, float) = if reversed {
                // SAFETY: The fixture initializes mixed.
                unsafe {
                    (
                        UNION_U8_F64_F32X3_ARG.mixed.a,
                        UNION_U8_F64_F32X3_ARG.mixed.b,
                    )
                }
            } else {
                // SAFETY: The fixture initializes mixed.
                unsafe {
                    (
                        UNION_F32X3_U8_F64_ARG.mixed.a,
                        UNION_F32X3_U8_F64_ARG.mixed.b,
                    )
                }
            };
            assert_eq!(
                output,
                [
                    u64::from(NESTED_F32_U32_F32_ARG.head.to_bits())
                        | (u64::from(NESTED_F32_U32_F32_ARG.inner.a) << 32),
                    u64::from(NESTED_F32_U32_F32_ARG.inner.b.to_bits()),
                    u64::from(byte),
                    float.to_bits(),
                ]
            );
            // SAFETY: The probe initializes both floating fields and the integer return field.
            assert_eq!(unsafe { result.get() }, NESTED_F32_F32_U32_ARG);
        }
    }

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
                al: 0,
            };
            CallFrame::new(&plan, fn_ptrize!(unused_target), &[], None)
        };
        assert!(catch_unwind(|| frame(usize::MAX - 14, 1, 4)).is_err());
        assert!(catch_unwind(|| frame(usize::MAX - 15, 16, 4)).is_err());
        for (stack, size, align_log2, expected_offset, expected_len) in [
            (0, 24, 3, 0, 24),
            (usize::MAX - 16, 15, 4, usize::MAX - 15, usize::MAX),
            (0, usize::MAX, 0, 0, usize::MAX),
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
        reason = "Keep four entry shapes beside their shared payload assertions."
    )]
    fn variadic_signed_and_unsigned_128_bit_values_obey_atomic_spills_and_alignment() {
        #[unsafe(naked)]
        unsafe extern "sysv64" fn available(_output: *mut [u64; 9], _wide: u128, _trailing: u64) {
            core::arch::naked_asm!(
                "mov rax, rsi",
                "mov [rdi + 0], rax",
                "mov rax, rdx",
                "mov [rdi + 8], rax",
                "mov rax, rcx",
                "mov [rdi + 16], rax",
                "ret"
            );
        }

        #[unsafe(naked)]
        unsafe extern "sysv64" fn one_gpr_left(
            _output: *mut [u64; 9],
            _i0: u64,
            _i1: u64,
            _i2: u64,
            _i3: u64,
            _wide: u128,
            _trailing: u64,
        ) {
            core::arch::naked_asm!(
                "mov [rdi + 0], rsi",
                "mov [rdi + 8], rdx",
                "mov [rdi + 16], rcx",
                "mov [rdi + 24], r8",
                "mov rax, [rsp + 8]",
                "mov [rdi + 32], rax",
                "mov rax, [rsp + 16]",
                "mov [rdi + 40], rax",
                "mov rax, r9",
                "mov [rdi + 48], rax",
                "ret"
            );
        }

        #[unsafe(naked)]
        unsafe extern "sysv64" fn exhausted(
            _output: *mut [u64; 9],
            _i0: u64,
            _i1: u64,
            _i2: u64,
            _i3: u64,
            _i4: u64,
            _wide: u128,
            _trailing: u64,
        ) {
            core::arch::naked_asm!(
                "mov [rdi + 0], rsi",
                "mov [rdi + 8], rdx",
                "mov [rdi + 16], rcx",
                "mov [rdi + 24], r8",
                "mov [rdi + 32], r9",
                "mov rax, [rsp + 8]",
                "mov [rdi + 40], rax",
                "mov rax, [rsp + 16]",
                "mov [rdi + 48], rax",
                "mov rax, [rsp + 24]",
                "mov [rdi + 56], rax",
                "ret"
            );
        }

        #[unsafe(naked)]
        unsafe extern "sysv64" fn after_spill(
            _output: *mut [u64; 9],
            _i0: u64,
            _i1: u64,
            _i2: u64,
            _i3: u64,
            _i4: u64,
            _marker: u64,
            _wide: u128,
            _trailing: u64,
        ) {
            core::arch::naked_asm!(
                "mov [rdi + 0], rsi",
                "mov [rdi + 8], rdx",
                "mov [rdi + 16], rcx",
                "mov [rdi + 24], r8",
                "mov [rdi + 32], r9",
                "mov rax, [rsp + 8]",
                "mov [rdi + 40], rax",
                "mov rax, [rsp + 24]",
                "mov [rdi + 48], rax",
                "mov rax, [rsp + 32]",
                "mov [rdi + 56], rax",
                "mov rax, [rsp + 40]",
                "mov [rdi + 64], rax",
                "ret"
            );
        }

        let integers = [0x1122u64, 0x3344, 0x5566, 0x7788, 0x99aa];
        let wide = 0xfedc_ba98_7654_3210_0123_4567_89ab_cdefu128;
        let signed = wide.cast_signed();
        let marker = 0xabcd_ef01_2345_6789u64;
        let trailing = 0x1357_9bdf_2468_ace0u64;
        for (target, count, prior_spill) in [
            (fn_ptrize!(available), 0, false),
            (fn_ptrize!(one_gpr_left), 4, false),
            (fn_ptrize!(exhausted), 5, false),
            (fn_ptrize!(after_spill), 5, true),
        ] {
            for signed_type in [false, true] {
                let mut fixed = vec![Type::Pointer];
                fixed.extend(vec![Type::U64; count]);
                let mut tail = Vec::new();
                if prior_spill {
                    tail.push(VariadicType::U64);
                }
                tail.extend([
                    if signed_type {
                        VariadicType::I128
                    } else {
                        VariadicType::U128
                    },
                    VariadicType::U64,
                ]);
                let function =
                    Function::variadic_with_abi(target, &fixed, &tail, None, VariadicAbi::SysV);
                let mut output = [0; 9];
                let pointer = &raw mut output;
                let mut args = vec![Arg::new(&pointer)];
                args.extend(integers[..count].iter().map(Arg::new));
                if prior_spill {
                    args.push(Arg::new(&marker));
                }
                args.push(if signed_type {
                    Arg::new(&signed)
                } else {
                    Arg::new(&wide)
                });
                args.push(Arg::new(&trailing));
                // SAFETY: Each target matches the number of fixed GPR slots and stack payloads.
                // Signed and unsigned storage share the 16-byte layout; the naked entry observes
                // their two's-complement bits. With a prior spill, rsp+16 is padding and the wide
                // value begins at rsp+24. Output storage is separate and live.
                unsafe {
                    function.call(&args, None);
                }
                let mut expected = integers[..count].to_vec();
                if prior_spill {
                    expected.push(marker);
                }
                expected.extend([0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210, trailing]);
                assert_eq!(&output[..expected.len()], expected);
            }
        }
    }
}
