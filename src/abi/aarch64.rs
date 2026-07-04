//! ABI definitions for aarch64 based on
//! <https://github.com/libffi/libffi/blob/3276df05a758f8081adb6a910abf8de627ebda46/src/aarch64/ffitarget.h#L44>

#[cfg(not(docsrs))]
use libffi_sys::{ffi_abi_FFI_SYSV, ffi_abi_FFI_WIN64};

#[cfg(docsrs)]
const ffi_abi_FFI_SYSV: libffi_sys::ffi_abi = 1;
#[cfg(docsrs)]
const ffi_abi_FFI_WIN64: libffi_sys::ffi_abi = 2;

#[cfg(not(docsrs))]
use super::Abi;

/// ABI constants for AArch64 targets.
#[cfg(docsrs)]
pub struct Abi(libffi_sys::ffi_abi);

impl Abi {
    /// Standard AArch64 procedure call ABI.
    pub const SYSV: Self = Self(ffi_abi_FFI_SYSV);

    /// Windows AArch64 ABI.
    pub const WIN64: Self = Self(ffi_abi_FFI_WIN64);

    #[cfg(test)]
    #[doc(hidden)]
    pub const ABIS: [Self; 2] = [Self::SYSV, Self::WIN64];
}

impl Default for Abi {
    fn default() -> Self {
        #[cfg(not(windows))]
        return Self::SYSV;

        #[cfg(windows)]
        return Self::WIN64;
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::test_create_closure_and_call_with_abi;
    use super::Abi;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_sysv_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::SYSV);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_win64_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::WIN64);
    }
}
