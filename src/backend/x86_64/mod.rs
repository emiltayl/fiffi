//! ABI definitions for 64-bit x86.

mod asm;
mod sysv;

use core::mem::MaybeUninit;

use crate::FnPtr;
use crate::function::{Arg, Ret};
use crate::types::Type;

/// ABI constants for 64-bit x86 targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Abi {
    #[default]
    SysV,
    // TODO implement after SysV
    //Win64,
}

impl Abi {
    #[cfg(test)]
    #[doc(hidden)]
    pub const ABIS: [Self; 1] = [Self::SysV];
}

#[derive(Clone, Debug)]
pub(crate) enum CallInterface {
    SysV(sysv::CallInterface),
}

impl CallInterface {
    pub(crate) fn new(argument_types: &[Type], return_type: Option<&Type>, abi: Abi) -> Self {
        match abi {
            Abi::SysV => Self::SysV(sysv::CallInterface::new(argument_types, return_type)),
        }
    }

    /// Calls a function using this interface.
    ///
    /// # Safety
    ///
    /// The safety contract of [`crate::function::Function::call`] must be upheld. `fn_ptr`, `args`,
    /// and `ret` must match the signature used to create this interface.
    pub(crate) unsafe fn call(&self, fn_ptr: FnPtr, args: &[Arg<'_>], ret: Ret<'_>) {
        match self {
            Self::SysV(call_interface) => {
                // SAFETY: This method has the same safety contract as the ABI-specific call method
                // and forwards the function pointer, arguments, and return storage unchanged.
                unsafe { call_interface.call(fn_ptr, args, ret) };
            }
        }
    }
}

#[derive(Debug)]
#[repr(align(8))]
struct Register([MaybeUninit<u8>; 8]);

impl Register {
    fn update_from_bytes(&mut self, bytes: &[u8]) {
        assert!(bytes.len() <= self.0.len());

        for (dst, src) in self.0.iter_mut().zip(bytes.iter()) {
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
