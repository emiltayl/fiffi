//! ABI definitions for ARMv7 based on
//! <https://github.com/libffi/libffi/blob/3276df05a758f8081adb6a910abf8de627ebda46/src/arm/ffitarget.h#L41>

use libffi_sys::{ffi_abi_FFI_SYSV, ffi_abi_FFI_VFP};

#[cfg(not(docsrs))]
use super::Abi;

/// ABI constants for ARMv7 targets.
#[cfg(docsrs)]
pub struct Abi(libffi_sys::ffi_abi);

impl Abi {
    /// Default ARM ABI on systems without hardware floats.
    pub const SYSV: Self = Abi(ffi_abi_FFI_SYSV);

    /// ARM VFP ABI, used as the default on ARM targets with hard(ware) floats.
    pub const VFP: Self = Abi(ffi_abi_FFI_VFP);

    #[cfg(test)]
    #[doc(hidden)]
    pub const ABIS: [Self; 2] = [Self::SYSV, Self::VFP];
}

impl Default for Abi {
    fn default() -> Self {
        #[cfg(not(any(target_abi = "eabihf", windows)))]
        return Self::SYSV;

        #[cfg(any(target_abi = "eabihf", windows))]
        return Self::VFP;
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
    fn test_vfp_abi_closure_call() {
        test_create_closure_and_call_with_abi(Abi::VFP);
    }
}
