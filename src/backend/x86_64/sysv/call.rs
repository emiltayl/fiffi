use core::mem::{MaybeUninit, offset_of};
use core::ptr;

use super::plan::{ArgumentMove, ArgumentMoveKind, MarshalPlan, RegisterBank, ReturnStrategy};
use crate::FnPtr;
use crate::backend::x86_64::Register;
use crate::backend::x86_64::asm::stack_setup_asm;
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
        stack_setup_asm!("[r12 + {stack_allocation_len_offset}]"),

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

        // Copy exact byte counts, including odd and zero lengths, leaving slot padding untouched.
        // The ABI requires a clear direction flag; source storage cannot overlap this allocation.
        "2400:",
        "mov rax, [r10 + {move_source_offset}]",
        "mov rsi, [r12 + {arguments_offset}]",
        "mov rsi, [rsi + rax * {arg_stride}]",
        "and rdi, {stack_offset_mask}",
        "lea rdi, [rsp + rdi]",
        "mov rcx, [r10 + {move_size_offset}]",
        // If we are moving 8, 4, 2, or 1 bytes, do it with mov instructions instead of `rep movsb`.
        "cmp rcx, 8",
        "je 2418f",
        "cmp rcx, 4",
        "je 2414f",
        "cmp rcx, 2",
        "je 2412f",
        "cmp rcx, 1",
        "je 2411f",
        // All other lengths, including zero.
        "rep movsb",
        "jmp 2900f",

        "2418:",
        "mov rax, [rsi]",
        "mov [rdi], rax",
        "jmp 2900f",
        "2414:",
        "mov eax, [rsi]",
        "mov [rdi], eax",
        "jmp 2900f",
        "2412:",
        "movzx eax, word ptr [rsi]",
        "mov [rdi], ax",
        "jmp 2900f",
        "2411:",
        "movzx eax, byte ptr [rsi]",
        "mov [rdi], al",
        "jmp 2900f",

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

        // Deliver hidden return storage after argument marshalling. Resolve offset mode first:
        // a discarded return can start at rsp + 0, whereas (0, false) denotes no hidden return.
        "mov rax, [r12 + {return_pointer_offset}]",
        "cmp byte ptr [r12 + {return_pointer_is_offset_offset}], 0",
        "je 3010f",
        "add rax, rsp",
        "3010:",
        "test rax, rax",
        "cmovnz rdi, rax",

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
    use super::*;
    use crate::fn_ptrize;

    extern "C" fn unused_target() {}

    fn initialized_bytes<const N: usize>(bytes: &[MaybeUninit<u8>]) -> [u8; N] {
        assert_eq!(bytes.len(), N);

        core::array::from_fn(|index| {
            // SAFETY: Tests supply initialized bytes without padding.
            unsafe { *bytes[index].assume_init_ref() }
        })
    }

    const GPR_RETURN_0: [u8; 8] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17];
    const GPR_RETURN_1: [u8; 8] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27];
    const XMM_RETURN_0: [u8; 8] = [0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37];
    const XMM_RETURN_1: [u8; 8] = [0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47];
    const RETURN_SENTINEL: u8 = 0xa5;

    fn synthetic_return_frame<'arg>() -> CallFrame<'arg> {
        let mut call_frame = CallFrame {
            return_gpr: <[Register; 2] as Default>::default(),
            return_xmm: <[Register; 2] as Default>::default(),
            stack_allocation_len: 0,
            argument_moves: ptr::null(),
            argument_move_len: 0,
            arguments: ptr::null(),
            return_pointer: 0,
            return_pointer_is_offset: false,
            fn_ptr: fn_ptrize!(unused_target),
            al: 0,
        };

        call_frame.return_gpr[0].update_from_bytes(&GPR_RETURN_0);
        call_frame.return_gpr[1].update_from_bytes(&GPR_RETURN_1);
        call_frame.return_xmm[0].update_from_bytes(&XMM_RETURN_0);
        call_frame.return_xmm[1].update_from_bytes(&XMM_RETURN_1);

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

        let mut return_buffer = [MaybeUninit::new(RETURN_SENTINEL); 16];

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

        assert_eq!(
            initialized_bytes::<16>(&return_buffer),
            [RETURN_SENTINEL; 16]
        );
    }
}
