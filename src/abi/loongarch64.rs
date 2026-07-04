//! ABI definitions for Loongarch64 based on
//! <https://github.com/libffi/libffi/blob/3276df05a758f8081adb6a910abf8de627ebda46/src/loongarch64/ffitarget.h#L47>

#[cfg(not(docsrs))]
use libffi_sys::{ffi_abi_FFI_LP64D, ffi_abi_FFI_LP64F, ffi_abi_FFI_LP64S};

#[cfg(docsrs)]
const ffi_abi_FFI_LP64D: libffi_sys::ffi_abi = 3;
#[cfg(docsrs)]
const ffi_abi_FFI_LP64F: libffi_sys::ffi_abi = 2;
#[cfg(docsrs)]
const ffi_abi_FFI_LP64S: libffi_sys::ffi_abi = 1;

#[cfg(not(docsrs))]
use super::Abi;

/// ABI constants for LoongArch64 targets.
#[cfg(docsrs)]
pub struct Abi(libffi_sys::ffi_abi);

impl Abi {
    /// LP64 soft-float ABI.
    pub const LP64S: Self = Abi(ffi_abi_FFI_LP64S);

    /// LP64 single-precision floating-point ABI.
    pub const LP64F: Self = Abi(ffi_abi_FFI_LP64F);

    /// LP64 double-precision floating-point ABI.
    pub const LP64D: Self = Abi(ffi_abi_FFI_LP64D);

    #[cfg(test)]
    #[doc(hidden)]
    pub const ABIS: [Self; 3] = [Self::LP64S, Self::LP64F, Self::LP64D];
}

/// Default value based on
/// <https://github.com/libffi-rs/libffi-rs/blob/5eb8bfd9eee19e5cfcd41a8b1d8239292f6ecde9/libffi-sys-rs/src/arch.rs#L370>
impl Default for Abi {
    fn default() -> Self {
        Self::LP64D
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::test_create_closure_and_call_with_abi;
    use super::Abi;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_lp64s_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::LP64S);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_lp64f_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::LP64F);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_lp64d_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::LP64D);
    }
}
