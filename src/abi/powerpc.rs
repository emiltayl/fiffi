//! ABI definitions for 32-bit PowerPC based on
//! <https://github.com/libffi/libffi/blob/3276df05a758f8081adb6a910abf8de627ebda46/src/powerpc/ffitarget.h#L108>
//!
//! Currently, only the default ABI is supported for this target. If you need more ABIs, please
//! create a new issue.

use libffi_sys::ffi_abi_FFI_DEFAULT_ABI;

#[cfg(not(docsrs))]
use super::Abi;

/// ABI constants for 32-bit PowerPC targets.
#[cfg(docsrs)]
pub struct Abi(libffi_sys::ffi_abi);

impl Abi {
    /// Default ABI for this target.
    pub const DEFAULT: Self = Self(ffi_abi_FFI_DEFAULT_ABI);

    #[cfg(test)]
    #[doc(hidden)]
    pub const ABIS: [Self; 1] = [Self::DEFAULT];
}

impl Default for Abi {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::test_create_closure_and_call_with_abi;
    use super::Abi;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_default_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::DEFAULT);
    }
}
