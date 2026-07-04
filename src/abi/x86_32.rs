//! ABI definitions for 32-bit X86 based on
//! <https://github.com/libffi/libffi/blob/3276df05a758f8081adb6a910abf8de627ebda46/src/x86/ffitarget.h#L112>

use libffi_sys::{
    ffi_abi_FFI_FASTCALL, ffi_abi_FFI_MS_CDECL, ffi_abi_FFI_PASCAL, ffi_abi_FFI_REGISTER,
    ffi_abi_FFI_STDCALL, ffi_abi_FFI_SYSV, ffi_abi_FFI_THISCALL,
};

#[cfg(not(docsrs))]
use super::Abi;

/// ABI constants for non-Windows 32-bit x86 targets.
#[cfg(docsrs)]
pub struct Abi(libffi_sys::ffi_abi);

impl Abi {
    /// System V i386 ABI.
    pub const SYSV: Self = Self(ffi_abi_FFI_SYSV);

    /// Microsoft `thiscall` ABI.
    pub const THISCALL: Self = Self(ffi_abi_FFI_THISCALL);

    /// Microsoft `fastcall` ABI.
    pub const FASTCALL: Self = Self(ffi_abi_FFI_FASTCALL);

    /// Microsoft `stdcall` ABI.
    pub const STDCALL: Self = Self(ffi_abi_FFI_STDCALL);

    /// Pascal calling convention ABI.
    ///
    /// **Caution!** The pascal ABI may have problems with structs as arguments and return values.
    /// This ABI does not pass fiffi's full test suite at the moment.
    pub const PASCAL: Self = Self(ffi_abi_FFI_PASCAL);

    /// Borland/Delphi register calling convention ABI.
    ///
    /// **Caution!** The register ABI may have problems with structs as arguments and return values.
    /// This ABI does not pass fiffi's full test suite at the moment.
    pub const REGISTER: Self = Self(ffi_abi_FFI_REGISTER);

    /// Microsoft `cdecl` ABI.
    pub const MS_CDECL: Self = Self(ffi_abi_FFI_MS_CDECL);

    #[cfg(test)]
    #[doc(hidden)]
    pub const ABIS: [Self; 5] = [
        Self::SYSV,
        Self::THISCALL,
        Self::FASTCALL,
        Self::STDCALL,
        //Self::PASCAL,
        //Self::REGISTER,
        Self::MS_CDECL,
    ];
}

impl Default for Abi {
    fn default() -> Self {
        Self::SYSV
    }
}

#[cfg(test)]
mod tests {
    use core::ffi::c_void;

    use libffi_sys::{ffi_call, ffi_cif, ffi_prep_cif};

    use super::*;
    use crate::abi::test_utils::{
        generate_call_test_for_abi, test_create_closure_and_call_with_abi,
    };
    use crate::raw::ffi_type_uint32;

    generate_call_test_for_abi!("stdcall", Abi::STDCALL, test_ffi_call_stdcall);
    generate_call_test_for_abi!("fastcall", Abi::FASTCALL, test_ffi_call_fastcall);
    generate_call_test_for_abi!("thiscall", Abi::THISCALL, test_ffi_call_thiscall);

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_sysv_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::SYSV);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_thiscall_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::THISCALL);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_fastcall_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::FASTCALL);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_stdcall_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::STDCALL);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_pascal_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::PASCAL);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_register_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::REGISTER);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_ms_cdecl_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::MS_CDECL);
    }
}
