//! Shared assembly scaffolding for x86-64 call trampolines.

/// Allocates, aligns, and probes a runtime-sized stack area.
///
/// * `allocation_length`: source operand, must not depend on `r10`; may use `r11`.
/// * Allocations of at least 4096 bytes are probed in steps of at most 4096 bytes.
/// * Sets `rsp` and `r10` to the 16-byte-aligned allocation base; clobbers `r11` and flags.
///
/// The caller must ensure the size cannot wrap the stack address.
macro_rules! stack_setup_asm {
    ($allocation_length:literal) => {
        concat!(
            "mov r10, rsp\n",
            "sub r10, ",
            $allocation_length,
            "\n",
            "and r10, -16\n",
            "mov r11, rsp\n",
            "sub r11, r10\n",
            "cmp r11, 4096\n", // Probe if stack allocation crosses a 4096 byte boundary
            "jb 14f\n",
            "mov r11, rsp\n",
            "12:\n",
            "sub r11, 4096\n",
            "cmp r11, r10\n",
            "jae 13f\n",
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
