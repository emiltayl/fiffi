//! Shared assembly scaffolding for x86-64 call trampolines.

/// Maximum distance between stack probe accesses.
pub(super) const STACK_PROBE_INTERVAL: usize = 4096;

/// Generates assembly that allocates and probes a runtime-sized stack area.
///
/// `allocation_length` must be a literal containing an x86-64 source operand for the number of
/// bytes to allocate. It must not depend on `r10`, which is overwritten before the operand is
/// evaluated. It may use `r11`: the generated assembly consumes the operand before reusing `r11`
/// for probing.
///
/// The generated assembly rounds the target stack pointer down to a 16-byte boundary, probes the
/// allocation at intervals no larger than the caller-provided `__stack_probe_interval` assembly
/// operand, and writes the resulting stack pointer to both `rsp` and `r10`. It clobbers `r11`.
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
            "cmp r11, {__stack_probe_interval}\n",
            "jb 14f\n",
            "mov r11, rsp\n",
            "12:\n",
            "sub r11, {__stack_probe_interval}\n",
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
