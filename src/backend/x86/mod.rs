//! Outgoing calls for 32-bit x86 using Rust's native type layouts and ABI rules.

mod asm;
mod call;
mod classification;
mod plan;

use crate::FnPtr;
use crate::backend::CallSignature;
use crate::function::{Arg, Ret};
use crate::types::{Type, VariadicType};

/// ABIs supported on the current i686 target.
///
/// When calling Win32 API functions (`extern "system"`), `Abi::Stdcall` should be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Abi {
    /// System V stack calling convention, the default outside Windows.
    #[cfg_attr(not(windows), default)]
    SysV,
    /// The default ABI on Windows.
    #[cfg_attr(windows, default)]
    Cdecl,
    /// The `stdcall` ABI, used by Win32 API calls.
    Stdcall,
    /// The `fastcall` ABI, which passes up to two arguments in registers.
    Fastcall,
    /// The `thiscall` ABI.
    ///
    /// Note that this ABI differs from the ABI Microsoft uses for C++ method calls.
    Thiscall,
}

/// ABIs supporting variadic calls on the current i686 target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum VariadicAbi {
    /// See [`Abi::SysV`].
    #[cfg_attr(not(windows), default)]
    SysV,
    /// See [`Abi::Cdecl`].
    #[cfg_attr(windows, default)]
    Cdecl,
}

impl From<VariadicAbi> for Abi {
    fn from(abi: VariadicAbi) -> Self {
        match abi {
            VariadicAbi::SysV => Self::SysV,
            VariadicAbi::Cdecl => Self::Cdecl,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CallInterface(plan::MarshalPlan);

impl CallInterface {
    pub(crate) fn new(argument_types: &[Type], return_type: Option<&Type>, abi: Abi) -> Self {
        let signature = CallSignature::new(argument_types, return_type);
        Self(plan::MarshalPlan::build(signature, abi))
    }

    pub(crate) fn variadic(
        argument_types: &[Type],
        variadic_argument_types: &[VariadicType],
        return_type: Option<&Type>,
        abi: VariadicAbi,
    ) -> Self {
        let signature =
            CallSignature::variadic(argument_types, variadic_argument_types, return_type);
        Self(plan::MarshalPlan::build(signature, abi.into()))
    }

    /// # Safety
    /// Uphold `Function::call`'s requirements with a signature matching this interface.
    pub(crate) unsafe fn call(&self, fn_ptr: FnPtr, args: &[Arg<'_>], ret: Option<Ret<'_>>) {
        // SAFETY: The caller supplies matching signatures and live argument/result storage.
        unsafe { call::call(&self.0, fn_ptr, args, ret) }
    }
}
