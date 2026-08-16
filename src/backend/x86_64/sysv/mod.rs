//! TODO Top-level documentation with information about how ABI works and assumptions (such as stack
//! should start aligned to 16 bytes).

mod call;
mod classification;
mod plan;

pub(super) use call::CallFrame;
pub(super) use plan::MarshalPlan;
