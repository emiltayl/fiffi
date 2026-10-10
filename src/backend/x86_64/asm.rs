//! Shared assembly scaffolding for x86-64 call trampolines.

/// Allocates, aligns, and probes a runtime-sized stack area.
///
/// * `allocation_length`: source operand, must not depend on `r10`; may use `r11`.
/// * Subtraction underflow traps with `ud2` before alignment, probing, or updating `rsp`.
/// * Alignment rounds down by at most 15 bytes without wrapping the address.
/// * Allocations of at least 4096 bytes are probed in steps of at most 4096 bytes.
/// * Sets `rsp` and `r10` to the 16-byte-aligned allocation base; clobbers `r11` and flags.
/// * Reserves local labels 12 through 16.
///
/// The guard checks address arithmetic only; the caller must ensure enough stack is available.
macro_rules! stack_setup_asm {
    ($allocation_length:literal) => {
        concat!(
            "mov r10, rsp\n",
            "sub r10, ",
            $allocation_length,
            "\n",
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
