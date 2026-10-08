use core::mem::{MaybeUninit, offset_of};
use core::ptr;

use super::plan::{ArgumentMoveKind, MarshalPlan, ReturnStrategy};
use crate::FnPtr;
use crate::backend::x86_64::Register;
use crate::backend::x86_64::asm::stack_setup_asm;
use crate::backend::x86_64::win64::plan::ArgumentMove;
use crate::function::{Arg, Ret};

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

#[derive(Debug)]
struct CallFrame<'arg> {
    argument_moves: *const ArgumentMove,
    argument_move_len: usize,
    arguments: *const Arg<'arg>,

    /// Hidden return storage address or offset from pre-call `rsp`; zero means absent.
    return_pointer: usize,
    /// Whether `return_pointer` includes shadow space and must be added to pre-call `rsp`.
    return_pointer_is_offset: bool,

    /// Saved bytes returned in `rax`.
    return_rax: Register,
    /// All 16 saved bytes returned in `xmm0`.
    return_xmm0: [Register; 2],

    /// Shadow space, stack argument slots, and aligned indirect copies, plus any padding and
    /// storage for a discarded hidden return.
    stack_allocation_len: usize,
    fn_ptr: FnPtr,
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
            return_rax: Register::default(),
            return_xmm0: <[Register; 2] as Default>::default(),
            stack_allocation_len: marshal_plan.stack_allocation_size,
            fn_ptr,
        };

        // A hidden return pointer occupies the first argument slot.
        if let ReturnStrategy::HiddenPointer {
            size: return_size,
            align_log2: return_align,
        } = marshal_plan.return_strategy
        {
            if let Some(ret) = ret {
                call_frame.return_pointer = ret.as_ptr().expose_provenance();
            } else {
                // Reserve aligned storage after the plan's allocation for a discarded return.
                // `invoke` resolves the offset from pre-call `rsp` and passes its address in rcx.
                let return_align = 1usize
                    .checked_shl(u32::from(return_align))
                    .expect("invalid Win64 return alignment");
                let ret_ptr_offset = call_frame
                    .stack_allocation_len
                    .next_multiple_of(return_align);

                call_frame.return_pointer = ret_ptr_offset;
                call_frame.return_pointer_is_offset = true;
                call_frame.stack_allocation_len = ret_ptr_offset + return_size;
            }
        }

        call_frame
    }
}

/// Writes a register-returned value from a call frame into caller-provided storage.
///
/// # Safety
///
/// * The selected bytes must be in bounds and contain the described return value.
/// * For register returns, `ret` must be writable for that value and not overlap the frame.
unsafe fn write_register_return(
    call_frame: &CallFrame,
    return_strategy: ReturnStrategy,
    ret: Ret<'_>,
) {
    let ret_ptr = ret.as_ptr();
    let (source, byte_length) = match return_strategy {
        ReturnStrategy::Void | ReturnStrategy::HiddenPointer { .. } => return,
        ReturnStrategy::Rax { byte_length } => (call_frame.return_rax.0.as_ptr(), byte_length),
        ReturnStrategy::Xmm0 { byte_length } => {
            // Borrow the whole array: a full `xmm0` return spans both eight-byte slots.
            (
                call_frame.return_xmm0.as_ptr().cast::<MaybeUninit<u8>>(),
                byte_length,
            )
        }
    };

    // SAFETY:
    // * The selected bytes contain the return value.
    // * The caller provides enough writable, nonoverlapping return storage.
    // * `MaybeUninit<u8>` permits uninitialized padding.
    unsafe {
        ptr::copy_nonoverlapping(source, ret_ptr.cast(), usize::from(byte_length));
    }
}

/// Calls a function by reading arguments directly according to the Win64 marshal plan.
/// Assembly copies stack arguments and indirect values into the outgoing stack allocation.
///
/// # Safety
///
/// * Uphold [`crate::function::Function::call`]'s safety contract.
/// * `marshal_plan` must describe `fn_ptr`, `args`, and `ret`.
pub(crate) unsafe fn call(
    marshal_plan: &MarshalPlan,
    fn_ptr: FnPtr,
    args: &[Arg],
    ret: Option<Ret<'_>>,
) {
    let mut call_frame = CallFrame::new(marshal_plan, fn_ptr, args, ret.as_ref());

    // SAFETY:
    // * The frame, plan, argument array and storage, and return storage remain alive during the
    //   call.
    // * The frame references arguments and valid descriptors matching the target signature.
    // * Argument storage cannot overlap the fresh outgoing stack allocation.
    // * The caller provides valid return storage.
    unsafe {
        invoke(&raw mut call_frame);
    }

    if let Some(ret) = ret {
        // SAFETY:
        // * `invoke` stored the returned registers in the frame.
        // * The caller provides valid return storage, separate from the frame.
        unsafe {
            write_register_return(&call_frame, marshal_plan.return_strategy, ret);
        }
    }
}

/// Invokes a function, reading arguments directly and copying values into its outgoing stack
/// allocation as directed by the plan.
///
/// # Safety
///
/// * The frame must remain writable for the call.
/// * Its argument array and storage, and plan metadata must remain readable for the call. Argument
///   storage must not overlap the outgoing stack allocation.
/// * The internal plan must supply valid kinds, argument indices, register slots, and bounds.
///   Register payloads must be 1, 2, 4, or 8 bytes. Stack offsets must have their low three bits
///   clear and include shadow space; indirect copies must be sixteen-byte aligned.
/// * The plan, arguments, and return storage must match the target's ABI and signature.
/// * Any return storage must remain writable for the signature throughout the call.
#[unsafe(naked)]
unsafe extern "win64-unwind" fn invoke(call_frame: *mut CallFrame) {
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
        // After stack setup, relative to rsp immediately before call r11:
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
        stack_setup_asm!("[r12 + {stack_allocation_len_offset}]"),

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

        // Deliver hidden return storage after argument marshalling.
        "mov rax, [r12 + {return_pointer_offset}]",
        "test rax, rax",
        "jz 3020f",
        "lea r11, [rax + rsp]",
        "cmp byte ptr [r12 + {return_pointer_is_offset_offset}], 0",
        "cmovne rax, r11",
        "mov rcx, rax",
        "3020:",

        "mov r11, [r12 + {fn_ptr_offset}]",
        "call r11",

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
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::function::Function;
    use crate::test_utils::structs::U64x3;
    use crate::types::{FfiType, Type, VariadicType};
    use crate::{VariadicAbi, fn_ptrize};

    extern "C" fn unused_target() {}

    #[test]
    fn variadic_float_bits_reach_registers_and_stack_with_hidden_returns() {
        // These fixed signatures describe the payloads. The naked bodies also capture the
        // GPR duplicates supplied by a variadic caller, independently of the host's VaList.
        #[unsafe(naked)]
        unsafe extern "win64" fn capture(
            _first: f32,
            _second: f64,
            _third: f64,
            _fourth: f64,
            _output: *mut [u64; 8],
        ) {
            core::arch::naked_asm!(
                "mov r10, [rsp + 40]",
                "movd eax, xmm0",
                "mov [r10], rax",
                "mov eax, ecx",
                "mov [r10 + 8], rax",
                "movq rax, xmm1",
                "mov [r10 + 16], rax",
                "mov [r10 + 24], rdx",
                "movq rax, xmm2",
                "mov [r10 + 32], rax",
                "mov [r10 + 40], r8",
                "movq rax, xmm3",
                "mov [r10 + 48], rax",
                "mov [r10 + 56], r9",
                "ret",
            );
        }

        #[unsafe(naked)]
        unsafe extern "win64" fn capture_hidden(
            _first: f32,
            _second: f64,
            _third: f64,
            _fourth: f64,
            _output: *mut [u64; 8],
        ) -> U64x3 {
            core::arch::naked_asm!(
                // The hidden pointer shifts every float by one position. The fourth float
                // and output pointer occupy successive stack slots after shadow space.
                "mov r10, [rsp + 48]",
                "movd eax, xmm1",
                "mov [r10], rax",
                "mov eax, edx",
                "mov [r10 + 8], rax",
                "movq rax, xmm2",
                "mov [r10 + 16], rax",
                "mov [r10 + 24], r8",
                "movq rax, xmm3",
                "mov [r10 + 32], rax",
                "mov [r10 + 40], r9",
                "mov rax, [rsp + 40]",
                "mov [r10 + 48], rax",
                "mov [r10 + 56], rcx",
                // Initialize the hidden return buffer and return its address as Win64 requires.
                "mov eax, edx",
                "mov [rcx], rax",
                "mov [rcx + 8], r8",
                "mov [rcx + 16], r9",
                "mov rax, rcx",
                "ret",
            );
        }

        let first = f32::from_bits(0xffc0_1234);
        let second = -0.0f64;
        let third = f64::from_bits(0x7ff8_1234_5678_9abc);
        let fourth = -123.5f64;
        let expected_registers = [
            u64::from(first.to_bits()),
            u64::from(first.to_bits()),
            second.to_bits(),
            second.to_bits(),
            third.to_bits(),
            third.to_bits(),
        ];
        let return_type = U64x3::ffi_type();

        for hidden_return in [false, true] {
            let function = Function::variadic_with_abi(
                if hidden_return {
                    fn_ptrize!(capture_hidden)
                } else {
                    fn_ptrize!(capture)
                },
                &[Type::F32, Type::F64],
                &[VariadicType::F64, VariadicType::F64, VariadicType::Pointer],
                hidden_return.then_some(&return_type),
                VariadicAbi::Win64,
            );
            let mut output = [0u64; 8];
            let output_pointer = &raw mut output;
            let args = [
                Arg::new(&first),
                Arg::new(&second),
                Arg::new(&third),
                Arg::new(&fourth),
                Arg::new(&output_pointer),
            ];
            let mut return_value = MaybeUninit::<U64x3>::uninit();
            let return_address = (&raw mut return_value).expose_provenance();
            let ret = hidden_return.then(|| Ret::new(&mut return_value));
            // SAFETY: The explicit Win64 ABI, payloads, and optional hidden return match the
            // chosen capture function. Output and return storage are separate live buffers.
            unsafe {
                function.call(&args, ret);
            }

            assert_eq!(&output[..6], &expected_registers);
            assert_eq!(output[6], fourth.to_bits());
            if hidden_return {
                assert_eq!(output[7], u64::try_from(return_address).unwrap());
                // SAFETY: `capture_hidden` initialized all three fields through the hidden pointer.
                assert_eq!(
                    unsafe { return_value.assume_init() },
                    U64x3 {
                        a: u64::from(first.to_bits()),
                        b: second.to_bits(),
                        c: third.to_bits(),
                    }
                );

                output.fill(0);
                // SAFETY: The ABI and live argument/output storage still match `capture_hidden`.
                // Discarding the return requests writable temporary hidden-return storage.
                unsafe {
                    function.call(&args, None);
                }
                assert_eq!(&output[..6], &expected_registers);
                assert_eq!(output[6], fourth.to_bits());
                assert_ne!(output[7], 0);
                assert_eq!(output[7] % align_of::<U64x3>() as u64, 0);
            } else {
                assert_eq!(output[7], fourth.to_bits());
            }
        }
    }

    #[test]
    fn variadic_empty_tail_delivers_float_duplicates_to_machine_registers() {
        // The fixed signature describes every payload. The naked body additionally observes
        // the GPR copies supplied by a variadic caller, without requiring a host-native VaList.
        #[unsafe(naked)]
        unsafe extern "win64" fn capture(_first: f32, _second: f64, _output: *mut [u64; 4]) {
            core::arch::naked_asm!(
                "movd eax, xmm0",
                "mov [r8], rax",
                "mov eax, ecx",
                "mov [r8 + 8], rax",
                "movq rax, xmm1",
                "mov [r8 + 16], rax",
                "mov [r8 + 24], rdx",
                "ret",
            );
        }

        let function = Function::variadic_with_abi(
            fn_ptrize!(capture),
            &[Type::F32, Type::F64, Type::Pointer],
            &[],
            None,
            VariadicAbi::Win64,
        );
        let first = f32::from_bits(0xffc0_1234);
        let second = f64::from_bits(0x7ff8_1234_5678_9abc);
        let mut output = [0u64; 4];
        let output_pointer = &raw mut output;
        // SAFETY: The explicit Win64 ABI and all three payload types match capture.
        // The output pointer names a separate live writable array of four u64s.
        unsafe {
            function.call(
                &[
                    Arg::new(&first),
                    Arg::new(&second),
                    Arg::new(&output_pointer),
                ],
                None,
            );
        }
        assert_eq!(
            output,
            [
                u64::from(first.to_bits()),
                u64::from(first.to_bits()),
                second.to_bits(),
                second.to_bits()
            ]
        );
    }

    fn initialized_bytes<const N: usize>(bytes: &[MaybeUninit<u8>]) -> [u8; N] {
        assert_eq!(bytes.len(), N);

        core::array::from_fn(|index| {
            // SAFETY: These tests pass only initialized payload, register, or sentinel bytes.
            unsafe { *bytes[index].assume_init_ref() }
        })
    }

    const GPR_RETURN_0: [u8; 8] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17];
    const XMM_RETURN_LOW: [u8; 8] = [0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57];
    const XMM_RETURN_HIGH: [u8; 8] = [0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67];
    const SENTINEL: u8 = 0xa5;

    fn synthetic_return_frame<'arg>() -> CallFrame<'arg> {
        let mut call_frame = CallFrame {
            argument_moves: ptr::null(),
            argument_move_len: 0,
            arguments: ptr::null(),
            return_pointer: 0,
            return_pointer_is_offset: false,
            return_rax: Register::default(),
            return_xmm0: <[Register; 2] as Default>::default(),
            stack_allocation_len: 0,
            fn_ptr: fn_ptrize!(unused_target),
        };

        call_frame.return_rax.update_from_bytes(&GPR_RETURN_0);
        call_frame.return_xmm0[0].update_from_bytes(&XMM_RETURN_LOW);
        call_frame.return_xmm0[1].update_from_bytes(&XMM_RETURN_HIGH);

        call_frame
    }

    #[test]
    fn scalar_register_returns_copy_only_the_declared_length() {
        let call_frame = synthetic_return_frame();
        let cases = [
            (ReturnStrategy::Rax { byte_length: 1 }, 1, GPR_RETURN_0),
            (ReturnStrategy::Rax { byte_length: 2 }, 2, GPR_RETURN_0),
            (ReturnStrategy::Rax { byte_length: 4 }, 4, GPR_RETURN_0),
            (ReturnStrategy::Rax { byte_length: 8 }, 8, GPR_RETURN_0),
            (ReturnStrategy::Xmm0 { byte_length: 4 }, 4, XMM_RETURN_LOW),
            (ReturnStrategy::Xmm0 { byte_length: 8 }, 8, XMM_RETURN_LOW),
        ];

        for (strategy, byte_length, expected_register) in cases {
            let mut return_buffer = [MaybeUninit::new(SENTINEL); 24];

            // SAFETY:
            // * The selected register bytes are initialized.
            // * The return slice is large enough and disjoint from the frame.
            unsafe {
                write_register_return(&call_frame, strategy, Ret::new(&mut return_buffer[8..16]));
            }

            let actual = initialized_bytes::<24>(&return_buffer);
            assert_eq!(&actual[..8], &[SENTINEL; 8], "{strategy:?}");
            assert_eq!(
                &actual[8..8 + byte_length],
                &expected_register[..byte_length],
                "{strategy:?}"
            );
            assert_eq!(
                &actual[8 + byte_length..],
                &[SENTINEL; 24][8 + byte_length..],
                "{strategy:?}"
            );
        }
    }

    #[test]
    fn full_xmm0_return_copies_both_saved_halves() {
        let call_frame = synthetic_return_frame();
        let mut return_buffer = [MaybeUninit::new(SENTINEL); 32];

        // SAFETY:
        // * Both eight-byte slots holding the saved xmm0 value are initialized.
        // * The return slice holds 16 bytes and is disjoint from the frame.
        unsafe {
            write_register_return(
                &call_frame,
                ReturnStrategy::Xmm0 { byte_length: 16 },
                Ret::new(&mut return_buffer[8..24]),
            );
        }

        let actual = initialized_bytes::<32>(&return_buffer);
        assert_eq!(&actual[..8], &[SENTINEL; 8]);
        assert_eq!(&actual[8..16], &XMM_RETURN_LOW);
        assert_eq!(&actual[16..24], &XMM_RETURN_HIGH);
        assert_eq!(&actual[24..], &[SENTINEL; 8]);
    }

    #[test]
    fn non_register_returns_do_not_write_return_storage() {
        let call_frame = synthetic_return_frame();

        let mut return_buffer = [MaybeUninit::new(SENTINEL); 24];

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

        assert_eq!(initialized_bytes::<24>(&return_buffer), [SENTINEL; 24]);
    }
}
