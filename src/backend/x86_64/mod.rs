//! ABI definitions for 64-bit x86.

mod asm;
mod sysv;
mod win64;

use core::mem::MaybeUninit;

use crate::FnPtr;
use crate::function::{Arg, Ret};
use crate::types::Type;

/// ABI constants for 64-bit x86 targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Abi {
    /// System V calling convention for `x86_64`.
    #[cfg_attr(not(any(windows, target_os = "uefi")), default)]
    SysV,
    /// Microsoft Windows calling convention for `x86_64`.
    // TODO note typically no floats for UEFI
    #[cfg_attr(any(windows, target_os = "uefi"), default)]
    Win64,
}

impl Abi {
    #[cfg(test)]
    #[doc(hidden)]
    pub const ABIS: [Self; 2] = [Self::SysV, Self::Win64];
}

#[derive(Clone, Debug)]
pub(crate) enum CallInterface {
    SysV(sysv::MarshalPlan),
    Win64(win64::MarshalPlan),
}

impl CallInterface {
    pub(crate) fn new(argument_types: &[Type], return_type: Option<&Type>, abi: Abi) -> Self {
        match abi {
            Abi::SysV => Self::SysV(sysv::MarshalPlan::build(argument_types, return_type)),
            Abi::Win64 => Self::Win64(win64::MarshalPlan::build(argument_types, return_type)),
        }
    }

    /// Calls a function using this interface.
    ///
    /// # Safety
    ///
    /// * Uphold [`crate::function::Function::call`]'s safety requirements.
    /// * `fn_ptr`, `args`, and `ret` must match this interface's signature.
    pub(crate) unsafe fn call(&self, fn_ptr: FnPtr, args: &[Arg<'_>], ret: Option<Ret<'_>>) {
        // SAFETY:
        // * The caller upholds the ABI-specific call contract.
        // * Each plan was built for this interface's signature.
        unsafe {
            match self {
                Self::SysV(plan) => sysv::call(plan, fn_ptr, args, ret),
                Self::Win64(plan) => win64::call(plan, fn_ptr, args, ret),
            }
        }
    }
}

#[derive(Debug)]
#[repr(align(8))]
struct Register([MaybeUninit<u8>; 8]);

impl Register {
    fn update_from_bytes(&mut self, bytes: &[u8]) {
        for (dst, src) in self.0[..bytes.len()].iter_mut().zip(bytes) {
            dst.write(*src);
        }
    }
}

impl Default for Register {
    fn default() -> Self {
        Self([MaybeUninit::new(0u8); 8])
    }
}

const _: () = {
    assert!(size_of::<usize>() == size_of::<Register>());
    assert!(align_of::<usize>() == align_of::<Register>());
};
