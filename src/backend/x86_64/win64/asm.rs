//! Win64 invocation assembly.

use core::mem::offset_of;

use super::call::CallFrame;
use super::plan::{ArgumentMove, ArgumentMoveKind};
use crate::backend::x86_64::asm::{hidden_return_asm, stack_copy_asm, stack_setup_asm};
use crate::function::Arg;

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
pub(super) unsafe extern "win64-unwind" fn invoke(call_frame: *mut CallFrame) {
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
        // After stack setup, relative to rsp immediately before the target call:
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

        // The Windows dispatcher checks RAX and tail-jumps to it, preserving arguments.
        "mov rax, [r12 + {fn_ptr_offset}]",

        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "call qword ptr [rip + {guard_dispatch}]",

        #[cfg(not(all(windows, any(target_env = "msvc", target_abi = "llvm"))))]
        "call rax",

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
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        guard_dispatch = sym crate::backend::cfguard::__guard_dispatch_icall_fptr,
    );
}
