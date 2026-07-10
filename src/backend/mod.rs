//! This module specifies everything that is unique to a given target.

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64::Abi;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::CallInterface;

// TODO static assert that `Abi` implements required traits?
// #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
