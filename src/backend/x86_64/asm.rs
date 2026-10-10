//! Shared assembly scaffolding for x86-64 call trampolines.

/// Allocates, aligns, and probes a runtime-sized stack area.
///
/// * `r12` must point to the call frame; the enclosing assembly supplies
///   `stack_allocation_len_offset` for its allocation-length field.
/// * Subtraction underflow traps with `ud2` before alignment, probing, or updating `rsp`.
/// * Alignment rounds down by at most 15 bytes without wrapping the address.
/// * Allocations of at least 4096 bytes are probed in steps of at most 4096 bytes.
/// * Sets `rsp` and `r10` to the 16-byte-aligned allocation base; clobbers `r11` and flags.
/// * Reserves local labels 12 through 16.
///
/// The guard checks address arithmetic only; the caller must ensure enough stack is available.
macro_rules! stack_setup_asm {
    () => {
        concat!(
            "mov r10, rsp\n",
            "sub r10, [r12 + {stack_allocation_len_offset}]\n",
            "jnc 15f\n", // Check subtraction before alignment overwrites the carry flag.
            "ud2\n",
            "15:\n",
            // Clearing the low bits rounds downward without underflow, keeping r10 <= rsp.
            "and r10, -16\n",
            "mov r11, rsp\n",
            "sub r11, r10\n",
            "cmp r11, 4096\n", // Probe if stack allocation crosses a 4096 byte boundary
            "jb 14f\n",
            "mov r11, rsp\n",
            "12:\n",
            "sub r11, 4096\n",
            "jb 16f\n", // Clamp a final probe step that would wrap near address zero.
            "cmp r11, r10\n",
            "jae 13f\n",
            "16:\n",
            "mov r11, r10\n",
            "13:\n",
            "test qword ptr [r11], r11\n",
            "cmp r11, r10\n",
            "jne 12b\n",
            "14:\n",
            "mov rsp, r10\n",
        )
    };
}

pub(crate) use stack_setup_asm;

/// Copies an argument into the outgoing stack allocation with exact-width accesses.
///
/// * `continuation` is a string literal naming a caller-owned jump target, including any
///   numeric-label direction suffix. Both current callers pass `"2900f"`.
/// * Inputs: `r10` points to the current `ArgumentMove`, `r12` points to its ABI's `CallFrame`,
///   `rdi` contains the packed stack destination, and `rsp` is the fixed allocation base.
/// * The enclosing assembly supplies `move_source_offset`, `arguments_offset`, `arg_stride`,
///   `stack_offset_mask`, and `move_size_offset` using its own layouts and destination mask.
/// * Clobbers `rax`, `rsi`, `rdi`, `rcx`, flags, and exactly the requested destination bytes.
/// * Preserves `r10`, `r11`, `r12`, `rsp`, populated `rdx`/`r8`/`r9`, and all XMM registers.
/// * Defines entry label 2400 and local branches 2411, 2412, 2414, and 2418. Every path jumps to
///   `continuation`, which the caller must define in the enclosing assembly. Expand once per
///   trampoline.
/// * Requires valid descriptor and argument-array entries, readable source and writable destination
///   for the copy length, nonoverlapping storage, and a clear direction flag.
///
/// Uses 1/2/4/8-byte `mov` paths and `rep movsb` for all other lengths, including zero,
/// leaving slot padding untouched.
macro_rules! stack_copy_asm {
    ($continuation:literal) => {
        concat!(
            "2400:\n",
            "mov rax, [r10 + {move_source_offset}]\n",
            "mov rsi, [r12 + {arguments_offset}]\n",
            "mov rsi, [rsi + rax * {arg_stride}]\n",
            "and rdi, {stack_offset_mask}\n",
            "lea rdi, [rsp + rdi]\n",
            "mov rcx, [r10 + {move_size_offset}]\n",
            // Copy 8, 4, 2, or 1 bytes with mov instructions instead of rep movsb.
            "cmp rcx, 8\n",
            "je 2418f\n",
            "cmp rcx, 4\n",
            "je 2414f\n",
            "cmp rcx, 2\n",
            "je 2412f\n",
            "cmp rcx, 1\n",
            "je 2411f\n",
            // All other lengths, including zero.
            "rep movsb\n",
            "jmp ",
            $continuation,
            "\n",
            "2418:\n",
            "mov rax, [rsi]\n",
            "mov [rdi], rax\n",
            "jmp ",
            $continuation,
            "\n",
            "2414:\n",
            "mov eax, [rsi]\n",
            "mov [rdi], eax\n",
            "jmp ",
            $continuation,
            "\n",
            "2412:\n",
            "movzx eax, word ptr [rsi]\n",
            "mov [rdi], ax\n",
            "jmp ",
            $continuation,
            "\n",
            "2411:\n",
            "movzx eax, byte ptr [rsi]\n",
            "mov [rdi], al\n",
            "jmp ",
            $continuation,
            "\n",
        )
    };
}

pub(crate) use stack_copy_asm;

/// Resolves hidden-return storage after deferred argument registers have been restored.
///
/// * `destination` is a string literal naming the ABI's first integer argument register: `"rdi"`
///   for SysV or `"rcx"` for Win64.
/// * Inputs: `r12` points to the call frame and `rsp` is the pre-call allocation base.
/// * The enclosing assembly supplies `return_pointer_offset` and `return_pointer_is_offset_offset`
///   using its own call-frame layout.
/// * Clobbers `rax`, flags, and the selected destination; preserves all other registers, including
///   `r11`.
/// * Reserves local label 3010 and falls through to the caller's next instruction.
///
/// Resolves offsets before testing the address: `(0, true)` passes `rsp`, while `(0, false)`
/// leaves the restored first argument unchanged. Nonzero addresses pass through; nonzero
/// offsets add `rsp`, including Win64's shadow-space allowance.
macro_rules! hidden_return_asm {
    ($destination:literal) => {
        concat!(
            "mov rax, [r12 + {return_pointer_offset}]\n",
            "cmp byte ptr [r12 + {return_pointer_is_offset_offset}], 0\n",
            "je 3010f\n",
            "add rax, rsp\n",
            "3010:\n",
            "test rax, rax\n",
            "cmovnz ",
            $destination,
            ", rax\n",
        )
    };
}

pub(crate) use hidden_return_asm;
