//! ABI definitions for s390x based on
//! <https://github.com/libffi/libffi/blob/3276df05a758f8081adb6a910abf8de627ebda46/src/s390/ffitarget.h#L47>

use libffi_sys::ffi_abi_FFI_SYSV;

#[cfg(not(docsrs))]
use super::Abi;

/// ABI constants for s390x targets.
#[cfg(docsrs)]
pub struct Abi(libffi_sys::ffi_abi);

impl Abi {
    /// Default ABI for this target.
    pub const SYSV: Self = Abi(ffi_abi_FFI_SYSV);

    #[cfg(test)]
    #[doc(hidden)]
    pub const ABIS: [Self; 1] = [Self::SYSV];
}

impl Default for Abi {
    fn default() -> Self {
        Self::SYSV
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
}
