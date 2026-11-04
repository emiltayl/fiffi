//! i686 frame, page probing, argument marshalling and return capture.
//!
//! DWARF CFI stays anchored to EBP while ESP moves. No SEH directives are required for unwind
//! support when compiling using MSVC or for an UEFI target.

use core::mem::offset_of;

use super::call::CallFrame;
use super::plan::{ArgumentMove, ArgumentMoveKind};
use crate::function::Arg;

// Assembly reads Arg's transparent pointer representation and byte-sized move tags.
// All field offsets and descriptor/argument strides are derived below, not hardcoded.
const _: () = {
    assert!(size_of::<*const ()>() == 4);
    assert!(size_of::<Arg<'_>>() == size_of::<*const ()>());
    assert!(align_of::<Arg<'_>>() == align_of::<*const ()>());
    assert!(size_of::<ArgumentMoveKind>() == 1);
};

/// The internal `fastcall-unwind` contract takes one frame pointer and does not return anything. If
/// this signature changes, re-audit register assignment, hidden parameters, stack cleanup, and
/// platform-specific lowering.
///
/// # Safety
/// The frame and its borrowed plan, argument and result storage must remain live and match the
/// target signature. Enough stack must exist for the allocation and target call. The address checks
/// detect unsigned underflow, but the page probes do not guarantee stack capacity. Sources and
/// outgoing destinations are disjoint, the ABI requires a clear direction flag. Copies preserve
/// arbitrary padding bytes without typed reads and access exactly their size.
#[unsafe(naked)]
pub(super) unsafe extern "fastcall-unwind" fn invoke(frame: *mut CallFrame<'_>) {
    core::arch::naked_asm!(
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_startproc",

        "push ebp",
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_def_cfa_offset 8",
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_offset ebp, -8",

        "mov ebp, esp",
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_def_cfa_register ebp",

        "push ebx",
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_offset ebx, -12",

        "push esi",
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_offset esi, -16",

        "push edi",
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_offset edi, -20",

        // ECX: incoming frame pointer. Copied into saved EBX before ECX is reused.
        // EBP+4: return address. EBP: saved EBP.
        // EBP-4/-8/-12: saved EBX/ESI/EDI. EBX holds the frame through the target call.
        // No incoming stack arguments, return with plain ret. The unwind CFA remains EBP+8.
        "mov ebx, ecx",
        "mov eax, esp",
        "sub eax, [ebx + {stack_size}]",

        // Check whether calling this function would cause the stack to wrap around (underflow).
        "jc 90f",
        "and eax, -16",

        // The target call needs a non-wrapping four-byte return-address slot below this base.
        // Probe through the actual allocation base, including dynamic alignment/guard pages.
        "cmp eax, 4",
        "jb 90f",

        "mov ecx, esp",
        "sub ecx, eax",
        "cmp ecx, 4096",
        "jbe 21f",
        "20:",
        "sub esp, 4096",
        "test byte ptr [esp], 0",
        "sub ecx, 4096",
        "cmp ecx, 4096",
        "ja 20b",
        "21:",
        "mov esp, eax",
        "test byte ptr [esp], 0",

        // Stack phase: ESP is the fixed outgoing base, EBP is the unwind anchor, EBX is the frame.
        // EDX walks descriptors, ESI/EDI are copy pointers, ECX is the size/count, and EAX is
        // scalar/dispatch scratch.
        "mov edx, [ebx + {argument_moves}]",
        "cmp edx, [ebx + {stack_moves_end}]",
        "je 60f",
        "40:",
        "mov edi, [edx + {move_destination}]",
        "add edi, esp",
        "movzx eax, byte ptr [edx + {move_kind}]",
        // Copy takes one kind check, pointer/extension handling stays off that path.
        "cmp eax, {kind_copy}",
        "jne 47f",
        "mov esi, [ebx + {arguments}]",
        "mov ecx, [edx + {move_source}]",
        "mov esi, [esi + ecx * {argument_stride}]",
        "movzx ecx, byte ptr [edx + {move_source_offset}]",
        "add esi, ecx",
        "mov ecx, [edx + {move_size}]",
        "cmp ecx, 4",
        "je 52f",
        "cmp ecx, 1",
        "je 50f",
        "cmp ecx, 2",
        "je 51f",
        "cmp ecx, 8",
        "je 53f",
        // A count of zero does not read any data, other odd/large copies preserve padding.
        "rep movsb",
        "jmp 54f",
        "47:",
        // ReturnPointer's source is an offset, not an argument-array index.
        "cmp eax, {kind_return_pointer}",
        "je 46f",
        // Only SignExtend and ZeroExtend remain in the stack prefix. Duplicating source
        // setup keeps both their dispatch checks off the Copy path.
        "mov esi, [ebx + {arguments}]",
        "mov ecx, [edx + {move_source}]",
        "mov esi, [esi + ecx * {argument_stride}]",
        "movzx ecx, byte ptr [edx + {move_source_offset}]",
        "add esi, ecx",
        "mov ecx, [edx + {move_size}]",
        "cmp eax, {kind_sign_extend}",
        "jne 42f",
        "cmp ecx, 1",
        "jne 43f",
        "movsx eax, byte ptr [esi]",
        "jmp 44f",
        "43:",
        "movsx eax, word ptr [esi]",
        "jmp 44f",
        "42:",
        "cmp ecx, 1",
        "jne 45f",
        "movzx eax, byte ptr [esi]",
        "jmp 44f",
        "45:",
        "movzx eax, word ptr [esi]",
        "jmp 44f",
        "46:",
        "mov eax, [ebx + {return_pointer}]",
        "test eax, eax",
        "jnz 44f",
        "mov eax, [edx + {move_source}]",
        "add eax, esp",
        "44:",
        "mov [edi], eax",
        "jmp 54f",
        "50:",
        "mov al, [esi]",
        "mov [edi], al",
        "jmp 54f",
        "51:",
        "mov ax, [esi]",
        "mov [edi], ax",
        "jmp 54f",
        "53:",
        // Eight-byte copies write their high word first, then share the four-byte tail.
        "mov eax, [esi + 4]",
        "mov [edi + 4], eax",
        "52:",
        "mov eax, [esi]",
        "mov [edi], eax",
        "54:",
        "add edx, {move_stride}",
        "cmp edx, [ebx + {stack_moves_end}]",
        "jne 40b",

        // Phase boundary: every stack operation is complete. ESI now walks the register
        // suffix, EDI is source/address scratch and EAX holds the payload. ECX/EDX are
        // initialized once, then preserved after each final load. The CFG check below saves
        // both argument registers before using ECX. EBX/EBP/ESP retain their fixed roles.
        "60:",
        "mov esi, edx",
        "xor ecx, ecx",
        "xor edx, edx",
        "cmp esi, [ebx + {argument_moves_end}]",
        "je 80f",
        "61:",
        "movzx eax, byte ptr [esi + {move_kind}]",
        "cmp eax, {kind_copy}",
        "jne 71f",
        "mov edi, [esi + {move_source}]",
        "mov eax, [ebx + {arguments}]",
        "mov edi, [eax + edi * {argument_stride}]",
        "movzx eax, byte ptr [esi + {move_source_offset}]",
        "add edi, eax",
        // Register copies are always four bytes.
        "mov eax, [edi]",
        "jmp 68f",
        "71:",
        "cmp eax, {kind_stack_address}",
        "je 66f",
        "cmp eax, {kind_return_pointer}",
        "je 67f",
        "mov edi, [esi + {move_source}]",
        "mov eax, [ebx + {arguments}]",
        "mov edi, [eax + edi * {argument_stride}]",
        "movzx eax, byte ptr [esi + {move_source_offset}]",
        "add edi, eax",
        "movzx eax, byte ptr [esi + {move_kind}]",
        "cmp eax, {kind_sign_extend}",
        "jne 64f",
        "cmp dword ptr [esi + {move_size}], 1",
        "jne 63f",
        "movsx eax, byte ptr [edi]",
        "jmp 68f",
        "63:",
        "movsx eax, word ptr [edi]",
        "jmp 68f",
        "64:",
        "cmp dword ptr [esi + {move_size}], 1",
        "jne 65f",
        "movzx eax, byte ptr [edi]",
        "jmp 68f",
        "65:",
        "movzx eax, word ptr [edi]",
        "jmp 68f",
        "67:",
        "mov eax, [ebx + {return_pointer}]",
        "test eax, eax",
        "jnz 68f",
        // Offset zero is valid for a discarded thiscall result in ECX.
        "66:",
        "mov eax, [esi + {move_source}]",
        "add eax, esp",
        "68:",
        "cmp dword ptr [esi + {move_destination}], 0",
        "jne 69f",
        "mov ecx, eax",
        "jmp 70f",
        "69:",
        "mov edx, eax",
        "70:",
        "add esi, {move_stride}",
        "cmp esi, [ebx + {argument_moves_end}]",
        "jne 61b",
        "80:",

        // The x86 checker receives and preserves the target in ECX. Save both argument
        // registers in nonvolatile scratch, then call the exact address that was checked.
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "mov edi, ecx",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "mov esi, edx",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "mov ecx, [ebx + {fn_ptr}]",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "call dword ptr [{guard_check}]",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "mov eax, ecx",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "mov ecx, edi",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "mov edx, esi",
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        "call eax",

        #[cfg(not(all(windows, any(target_env = "msvc", target_abi = "llvm"))))]
        "call dword ptr [ebx + {fn_ptr}]",
        // Cdecl leaves ordinary arguments to the caller; SysV callees may pop sret;
        // stdcall/fastcall/thiscall pop their stack arguments. All reach this point:
        // capture through EBX, then restore ESP from EBP without a cleanup branch.
        "mov [ebx + {return_value}], eax",
        "mov [ebx + {return_value} + 4], edx",
        // Capture x87 after the GPR stores so they cannot overwrite the shared result.
        // Pop x87 results even when the caller discards them.
        "cmp dword ptr [ebx + {x87_size}], 4",
        "je 30f",
        "cmp dword ptr [ebx + {x87_size}], 8",
        "jne 31f",
        "fstp qword ptr [ebx + {return_value}]",
        "jmp 31f",
        "30:",
        "fstp dword ptr [ebx + {return_value}]",

        "31:",
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_remember_state",
        "lea esp, [ebp - 12]",

        "pop edi",
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_restore edi",

        "pop esi",
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_restore esi",

        "pop ebx",
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_restore ebx",

        "pop ebp",
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_restore ebp",

        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_def_cfa esp, 4",

        "ret",

        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_restore_state",

        // Issue a fault if calling this function would cause the stack to wrap around.
        "90:",
        "ud2",
        #[cfg(not(any(target_env = "msvc", target_os = "uefi")))]
        ".cfi_endproc",

        argument_moves = const offset_of!(CallFrame, argument_moves),
        stack_moves_end = const offset_of!(CallFrame, stack_moves_end),
        argument_moves_end = const offset_of!(CallFrame, argument_moves_end),
        arguments = const offset_of!(CallFrame, arguments),
        return_pointer = const offset_of!(CallFrame, return_pointer),
        stack_size = const offset_of!(CallFrame, stack_size),
        return_value = const offset_of!(CallFrame, return_value),
        x87_size = const offset_of!(CallFrame, x87_size),
        fn_ptr = const offset_of!(CallFrame, fn_ptr),
        #[cfg(all(windows, any(target_env = "msvc", target_abi = "llvm")))]
        guard_check = sym crate::backend::cfguard::__guard_check_icall_fptr,
        move_source = const offset_of!(ArgumentMove, source),
        move_destination = const offset_of!(ArgumentMove, destination),
        move_size = const offset_of!(ArgumentMove, size),
        move_source_offset = const offset_of!(ArgumentMove, source_offset),
        move_kind = const offset_of!(ArgumentMove, kind),
        move_stride = const size_of::<ArgumentMove>(),
        argument_stride = const size_of::<Arg<'_>>(),
        kind_copy = const ArgumentMoveKind::Copy as u8,
        kind_sign_extend = const ArgumentMoveKind::SignExtend as u8,
        kind_stack_address = const ArgumentMoveKind::StackAddress as u8,
        kind_return_pointer = const ArgumentMoveKind::ReturnPointer as u8,
    );
}
