//! System V AMD64 calls use six integer and eight XMM argument registers.
//!
//! Aggregates use up to two eightbytes or pass on the stack. Outgoing stacks are 16-byte aligned.

mod call;
mod classification;
mod plan;

pub(super) use call::call;
pub(super) use plan::MarshalPlan;
