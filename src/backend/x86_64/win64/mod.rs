//! Win64 calls use four positional register slots and 32 bytes of shadow space.
//! Other arguments use eight-byte stack slots. Indirect copies and the outgoing stack are aligned
//! to 16 bytes. Integer returns use `rax`; floats and 128-bit integers use `xmm0`.

mod call;
mod classification;
mod plan;

pub(super) use call::call;
pub(super) use plan::MarshalPlan;
