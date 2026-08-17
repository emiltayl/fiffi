//! TODO Top-level documentation with information about how ABI works and assumptions (such as stack
//! should start aligned to 16 bytes).

mod call;
mod classification;
mod plan;

use plan::MarshalPlan;

use crate::types::Type;

#[derive(Clone, Debug)]
pub(super) struct CallInterface {
    marshal_plan: MarshalPlan,
}

impl CallInterface {
    pub(super) fn new(argument_types: &[Type], return_type: Option<&Type>) -> Self {
        Self {
            marshal_plan: MarshalPlan::build(argument_types, return_type),
        }
    }
}
