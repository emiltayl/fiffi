//! TODO Top-level documentation with information about how ABI works and assumptions (such as stack
//! should start aligned to 16 bytes).

// mod call; TODO remove comment when implemented
mod classification;
mod plan;

use plan::MarshalPlan;

use crate::FnPtr;
use crate::function::{Arg, Ret};
use crate::types::Type;

#[derive(Clone, Debug)]
pub(crate) struct CallInterface {
    marshal_plan: MarshalPlan,
}

impl CallInterface {
    pub(super) fn new(argument_types: &[Type], return_type: Option<&Type>) -> Self {
        Self {
            marshal_plan: MarshalPlan::build(argument_types, return_type),
        }
    }

    /// Calls a function using this interface.
    ///
    /// # Safety
    ///
    /// The safety contract of [`crate::function::Function::call`] must be upheld. `fn_ptr`, `args`,
    /// and `ret` must match the signature used to create this interface.
    pub(super) unsafe fn call(&self, fn_ptr: FnPtr, args: &[Arg<'_>], ret: Ret<'_>) {
        // SAFETY: This method has the same safety contract as `call::call` and forwards the marshal
        // plan that was created for this interface's signature.
        //unsafe { call::call(&self.marshal_plan, fn_ptr, args, ret) }; TODO remove comment when implemented
    }
}
