//! Shared unwind-aware assembly scaffolding for x86-64 call trampolines.

/// Maximum distance between stack probe accesses.
pub(super) const STACK_PROBE_INTERVAL: usize = 4096;

/// Emits a naked assembly body inside an unwindable frame-pointer frame.
///
/// The generated frame preserves `rbp` and `r12`, then copies `call_frame_register` into `r12` for
/// use by the supplied instructions. `stack_buffer_len_offset` locates the stack-buffer length in
/// that call frame. `stack_argument_area_offset` is the number of bytes between the outgoing `rsp`
/// and the first byte copied from the stack buffer. This is zero for `SysV` and 32 for `Win64` to
/// reserve the required home area before any stack arguments. The home area is not part of the
/// stack buffer, and the return address subsequently pushed by `call` is not part of this offset.
///
/// The macro computes the final stack pointer in `r10`, then uses `r11` to probe the allocation at
/// intervals no larger than [`STACK_PROBE_INTERVAL`] before changing `rsp`. The last probe is
/// clamped to the final stack pointer so it never extends past the allocation. Both registers are
/// fixed because they are volatile under the supported x86-64 ABIs.
///
/// After probing, the macro sets the new, 16-byte-aligned `rsp`. The body starts with `r10`
/// pointing to the destination for the stack buffer and must leave `rbp` unchanged, preserve the
/// alignment of `rsp` before making another call, and fall through to the generated epilogue. Both
/// `r10` and `r11` may be reused after the stack buffer has been copied.
macro_rules! trampoline_asm_with_unwind_frame {
    (
        function = $function:path,
        call_frame_register = $call_frame_register:ident,
        stack_buffer_len_offset = $stack_buffer_len_offset:expr,
        stack_argument_area_offset = $stack_argument_area_offset:expr,
        instructions = [$($instruction:expr),* $(,)?]
        $(, operands = [
            $($operand_name:ident = $operand_kind:ident $operand_value:expr),* $(,)?
        ])?
        $(,)?
    ) => {
        core::arch::naked_asm!(
            #[cfg(not(windows))]
            ".cfi_startproc",
            #[cfg(windows)]
            ".seh_proc {__unwind_function}",

            // Save the caller's frame pointer, then use `rbp` as a stable reference to this
            // frame. This lets the body move `rsp` while still giving the unwinder a fixed way to
            // find the caller's stack pointer. The `call_frame` pointer is stored in `r12`.
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

            concat!("mov r12, ", stringify!($call_frame_register)),
            // Reserve `stack_argument_area_offset` before the buffered stack arguments and round
            // the resulting stack pointer down to preserve the required call-site alignment.
            "mov r10, rsp",
            "sub r10, [r12 + {stack_buffer_len_offset}]",
            "sub r10, {stack_argument_area_offset}",
            "and r10, -16",
            // Skip probing when the allocation cannot cross an entire probe interval.
            "mov r11, rsp",
            "sub r11, r10",
            "cmp r11, {__stack_probe_interval}",
            "jb 4f",
            // Touch each interval in descending address order. Clamp the last access to the
            // requested stack pointer so probing never extends past the allocation.
            "mov r11, rsp",
            "2:",
            "sub r11, {__stack_probe_interval}",
            "cmp r11, r10",
            "jae 3f",
            "mov r11, r10",
            "3:",
            "test qword ptr [r11], r11",
            "cmp r11, r10",
            "jne 2b",
            "4:",
            "mov rsp, r10",
            // Expose the destination for the buffered arguments to the ABI-specific body.
            "add r10, {stack_argument_area_offset}",

            // Run ABI-specific instructions to load arguments and call function.
            $($instruction,)*

            // Restore the stack in one operation so this remains correct after runtime-sized
            // stack allocations. `lea` also gives the Windows unwinder a recognized
            // frame-pointer-based epilogue.
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
            __unwind_function = sym $function,
            __stack_probe_interval = const $crate::backend::x86_64::asm::STACK_PROBE_INTERVAL,
            stack_buffer_len_offset = const $stack_buffer_len_offset,
            stack_argument_area_offset = const $stack_argument_area_offset,
            $($(
                $operand_name = $operand_kind $operand_value,
            )*)?
        );
    };
}

pub(crate) use trampoline_asm_with_unwind_frame;
