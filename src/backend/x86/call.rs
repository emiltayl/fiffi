use core::mem::MaybeUninit;
use core::ptr;

use super::asm::invoke;
use super::plan::{ArgumentMove, MarshalPlan, ReturnStrategy};
use crate::FnPtr;
use crate::function::{Arg, Ret};

// Integer and x87 returns share storage, aligned for the widest possible x87 store.
#[repr(C, align(8))]
pub(super) struct ReturnStorage([MaybeUninit<u8>; 8]);

impl Default for ReturnStorage {
    fn default() -> Self {
        Self([MaybeUninit::uninit(); 8])
    }
}

const _: () = {
    assert!(size_of::<ReturnStorage>() == 8);
    assert!(align_of::<ReturnStorage>() == 8);
};

pub(super) struct CallFrame<'arg> {
    pub(super) argument_moves: *const ArgumentMove,
    pub(super) stack_moves_end: *const ArgumentMove,
    pub(super) argument_moves_end: *const ArgumentMove,
    pub(super) arguments: *const Arg<'arg>,
    pub(super) return_pointer: *mut MaybeUninit<u8>,
    pub(super) fn_ptr: FnPtr,
    pub(super) stack_size: usize,
    pub(super) return_value: ReturnStorage,
    /// Zero for non-x87 results; four/eight selects the store precision and pops ST(0).
    pub(super) x87_size: usize,
}

impl<'arg> CallFrame<'arg> {
    /// Records metadata without reading argument payloads. Invoking the frame requires
    /// matching signatures and live plan, argument array, payloads, and result storage.
    fn new(plan: &MarshalPlan, fn_ptr: FnPtr, args: &[Arg<'arg>], ret: Option<&Ret<'_>>) -> Self {
        let moves = plan.arguments.as_ptr_range();

        Self {
            argument_moves: moves.start,
            stack_moves_end: plan.arguments[..plan.stack_move_count].as_ptr_range().end,
            argument_moves_end: moves.end,
            arguments: args.as_ptr(),
            return_pointer: ret.map_or(ptr::null_mut(), |ret| ret.as_ptr().cast()),
            fn_ptr,
            stack_size: match plan.return_strategy {
                ReturnStrategy::Indirect {
                    fallback_stack_size,
                } if ret.is_none() => fallback_stack_size,
                _ => plan.stack_size,
            },
            return_value: ReturnStorage::default(),
            x87_size: if let ReturnStrategy::X87(kind) = plan.return_strategy {
                kind.size()
            } else {
                0
            },
        }
    }
}

/// # Safety
/// The plan, target, arguments and result must satisfy `Function::call`'s contract.
pub(super) unsafe fn call(
    plan: &MarshalPlan,
    fn_ptr: FnPtr,
    args: &[Arg<'_>],
    ret: Option<Ret<'_>>,
) {
    let mut frame = CallFrame::new(plan, fn_ptr, args, ret.as_ref());

    // SAFETY: The raw metadata does not read argument payloads or extend any lifetime. The frame,
    // plan's owned descriptors, argument array and payloads, and optional result storage all
    // remain live throughout invoke. The plan reserves disjoint stack destinations and orders
    // every stack write before final register loads; the caller supplies matching storage/target.
    // Fallback descriptor offsets are in bounds only when fallback storage is selected.
    // Assembly consults those offsets only when the return pointer is null, for which the frame
    // reserves fallback storage.
    unsafe {
        invoke(&raw mut frame);
    }

    if let Some(ret) = ret {
        let size = match plan.return_strategy {
            ReturnStrategy::Integer(size) => size,
            ReturnStrategy::X87(kind) => kind.size(),
            ReturnStrategy::Void | ReturnStrategy::Indirect { .. } => return,
        };

        // SAFETY: Assembly captured the declared result width. The caller provides
        // disjoint writable storage. MaybeUninit preserves padding in aggregate results.
        unsafe {
            ptr::copy_nonoverlapping(frame.return_value.0.as_ptr(), ret.as_ptr().cast(), size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CallFrame;
    use crate::Abi;
    use crate::backend::CallSignature;
    use crate::backend::x86::plan::{ArgumentMoveKind, MarshalPlan};
    use crate::function::{Arg, Ret};
    use crate::types::Type;

    #[test]
    fn indirect_return_fallback_allocation_is_conditional() {
        let plan = MarshalPlan::build(
            CallSignature::new(&[Type::U32], Some(&Type::U128)),
            Abi::Cdecl,
        );
        let args = [Arg::new(&42u32)];
        let mut output = core::mem::MaybeUninit::<u128>::uninit();
        let ret = Ret::new(&mut output);
        // Only frame metadata is inspected; the target is never invoked.
        let fn_ptr = crate::fn_ptrize!(indirect_return_fallback_allocation_is_conditional);

        let captured = CallFrame::new(&plan, fn_ptr, &args, Some(&ret));
        assert_eq!(captured.stack_size, 8);
        assert_eq!(captured.return_pointer, ret.as_ptr().cast());

        let discarded = CallFrame::new(&plan, fn_ptr, &args, None);
        assert_eq!(discarded.stack_size, 32);
        assert!(discarded.return_pointer.is_null());

        let captured_again = CallFrame::new(&plan, fn_ptr, &args, Some(&ret));
        assert_eq!(captured_again.stack_size, 8);
        assert_eq!(captured_again.return_pointer, ret.as_ptr().cast());

        let return_pointer = plan
            .arguments
            .iter()
            .find(|argument| matches!(argument.kind, ArgumentMoveKind::ReturnPointer))
            .expect("missing hidden return pointer");
        assert_eq!(return_pointer.source, 16);
        assert_eq!(plan.stack_size, 8);
    }
}
