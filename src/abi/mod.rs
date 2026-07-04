//! This module defines ABIs supported by fiffi.

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64::Abi;

// TODO static assert that `Abi` implements required traits?
// #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
