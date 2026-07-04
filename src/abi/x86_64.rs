//! ABI definitions for 64-bit x86 based on
//! <https://github.com/libffi/libffi/blob/3276df05a758f8081adb6a910abf8de627ebda46/src/x86/ffitarget.h#L92>

use libffi_sys::{ffi_abi_FFI_GNUW64, ffi_abi_FFI_UNIX64, ffi_abi_FFI_WIN64};

#[cfg(not(docsrs))]
use super::Abi;

/// ABI constants for non-Windows 64-bit x86 targets.
#[cfg(docsrs)]
pub struct Abi(libffi_sys::ffi_abi);

impl Abi {
    /// System V ABI
    pub const UNIX64: Self = Self(ffi_abi_FFI_UNIX64);

    /// Windows ABI
    pub const WIN64: Self = Self(ffi_abi_FFI_WIN64);

    /// GNU Windows ABI
    pub const GNUW64: Self = Self(ffi_abi_FFI_GNUW64);

    /// UEFI ABI
    pub const EFI64: Self = Self::WIN64;

    #[cfg(test)]
    #[doc(hidden)]
    pub const ABIS: [Self; 4] = [Self::UNIX64, Self::WIN64, Self::GNUW64, Self::EFI64];
}

impl Default for Abi {
    fn default() -> Self {
        Self::UNIX64
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::test_create_closure_and_call_with_abi;
    use super::Abi;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_unix64_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::UNIX64);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_win64_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::WIN64);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_gnuw64_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::GNUW64);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_efi64_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::EFI64);
    }
}
