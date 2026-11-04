//! System V AMD64 invocation assembly.

use core::mem::offset_of;

use super::call::CallFrame;
use super::plan::{ArgumentMove, ArgumentMoveKind};
use crate::backend::x86_64::Register;
use crate::backend::x86_64::asm::{hidden_return_asm, stack_copy_asm, stack_setup_asm};
use crate::function::Arg;

// Dispatch uses bit zero to select a register bank and reads Arg as one pointer. Field
// offsets/strides use offset_of! and size_of! below; the structs need no fixed field order.
const _: () = {
    assert!(ArgumentMoveKind::Gpr as u8 == 0);
    assert!(ArgumentMoveKind::Xmm as u8 == 1);
    assert!(ArgumentMoveKind::Stack as u8 == 2);
};

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
pub(super) unsafe extern "sysv64-unwind" fn invoke(call_frame: *mut CallFrame) {
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
        // With Windows CFG hooks, an additional push r13 precedes mov rbp, rsp:
        // saved r13 is at rbp, and each slot above it moves up by eight bytes.
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
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "push r13",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        ".seh_pushreg r13",
        "mov rbp, rsp",
        #[cfg(not(windows))]
        ".cfi_def_cfa_register rbp",
        #[cfg(windows)]
        ".seh_setframe rbp, 0",
        #[cfg(windows)]
        ".seh_endprologue",

        "mov r12, rdi",
        // SysV needs AL for its variadic vector count, so the RAX-based Windows dispatcher
        // cannot be used. Check before marshalling, with aligned Win64 shadow space, and
        // retain the checked target in nonvolatile R13 until the call. Do not reload it
        // from the frame after checking. The checker may clobber Win64 volatile registers.
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "mov r13, [r12 + {fn_ptr_offset}]",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "mov rcx, r13",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "sub rsp, 32",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "call qword ptr [rip + {guard_check}]",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "add rsp, 32",

        stack_setup_asm!(),

        // Registers:
        // * r10: Current ArgumentMove pointer
        // * r11: Remaining move count; later the target address
        // * r12: CallFrame, preserved across the target call
        // * r13: Checked target on Windows with CFG hooks, otherwise untouched
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

        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "mov r11, r13",

        #[cfg(not(all(windows, any(target_env = "msvc", target_abi = "llvm"))))]
        "mov r11, [r12 + {fn_ptr_offset}]",
        "call r11",

        // Save registers used for return values.
        "mov [r12 + {return_gpr_offset} + {register_size} * 0], rax",
        "mov [r12 + {return_gpr_offset} + {register_size} * 1], rdx",
        "movq [r12 + {return_xmm_offset} + {register_size} * 0], xmm0",
        "movq [r12 + {return_xmm_offset} + {register_size} * 1], xmm1",

        // Restore the stack with a Windows-recognized frame-pointer epilogue.
        "lea rsp, [rbp]",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "pop r13",
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
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        guard_check = sym crate::backend::cfguard::__guard_check_icall_fptr,

        al_offset = const offset_of!(CallFrame, al),
    );
}
