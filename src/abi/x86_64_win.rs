//! ABI definitions for Windows on 64-bit x86 based on
//! <https://github.com/libffi/libffi/blob/3276df05a758f8081adb6a910abf8de627ebda46/src/x86/ffitarget.h#L81>

use libffi_sys::{ffi_abi_FFI_GNUW64, ffi_abi_FFI_WIN64};

#[cfg(not(docsrs))]
use super::Abi;

/// ABI constants for Windows 64-bit x86 targets.
#[cfg(docsrs)]
pub struct Abi(libffi_sys::ffi_abi);

impl Abi {
    /// Windows x64 ABI.
    pub const WIN64: Self = Abi(ffi_abi_FFI_WIN64);

    /// GNU Windows x64 ABI.
    pub const GNUW64: Self = Abi(ffi_abi_FFI_GNUW64);

    /// Native Windows system ABI for 64-bit x86.
    pub const SYSTEM: Self = Self::WIN64;

    #[cfg(test)]
    #[doc(hidden)]
    pub const ABIS: [Self; 3] = [Self::WIN64, Self::GNUW64, Self::SYSTEM];
}

impl Default for Abi {
    fn default() -> Self {
        #[cfg(target_env = "msvc")]
        return Self::WIN64;

        #[cfg(target_env = "gnu")]
        return Self::GNUW64;
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::test_create_closure_and_call_with_abi;
    use super::Abi;

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
    fn test_system_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::SYSTEM);
    }
}
