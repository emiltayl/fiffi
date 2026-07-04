//! Error types for the `fiffi` crate.

use core::error::Error;
use core::fmt::Display;

use libffi_sys::{
    ffi_status, ffi_status_FFI_BAD_ABI, ffi_status_FFI_BAD_ARGTYPE, ffi_status_FFI_BAD_TYPEDEF,
};

use crate::types::Type;

/// Errors returned by libffi.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum LibffiError {
    /// Returned by libffi if a malformed `ffi_type` is provided.
    ///
    /// This is typically due to a struct without any `elements` or an unrecognized `type` tag. This
    /// error should never be returned when using `fiffi`.
    FfiBadTypeDef = ffi_status_FFI_BAD_TYPEDEF,

    /// Returned by libffi if an unknown or invalid ABI is provided.
    ///
    /// All `Abi`s defined in `fiffi` are tested to ensure they do not return an error.
    FfiBadAbi = ffi_status_FFI_BAD_ABI,

    /// Returned by libffi if an invalid argument type is provided as a variadic argument type.
    ///
    /// This typically happens when attempting to pass a 32-bit float or an 8- or 16-bit integer
    /// as a variadic argument.
    FfiBadArgType = ffi_status_FFI_BAD_ARGTYPE,
}

impl Display for LibffiError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LibffiError::FfiBadTypeDef => write!(
                f,
                "Attempted to use a malformed `ffi_type`. This is most likely caused by an empty struct, which is not supported by libffi."
            ),
            LibffiError::FfiBadAbi => {
                write!(f, "Attempted to use an ABI not recognized by libffi.")
            }
            LibffiError::FfiBadArgType => write!(
                f,
                "Attempted to use an unsupported type for variadic arguments."
            ),
        }
    }
}

impl Error for LibffiError {}

impl LibffiError {
    /// Converts a raw libffi status return value into a [`LibffiError`].
    ///
    /// Returns `None` if `status` is not one of the known libffi error constants.
    #[expect(
        non_upper_case_globals,
        reason = "Constant names from an external crate (libffi_sys)."
    )]
    pub fn from_status(status: ffi_status) -> Option<LibffiError> {
        match status {
            ffi_status_FFI_BAD_TYPEDEF => Some(LibffiError::FfiBadTypeDef),
            ffi_status_FFI_BAD_ABI => Some(LibffiError::FfiBadAbi),
            ffi_status_FFI_BAD_ARGTYPE => Some(LibffiError::FfiBadArgType),
            // `FFI_STATUS` is an C enum with the three errors above and a "OK" variant. If this
            // changes, `LibffiError` and this function will need to be changed to reflect that
            // change and possible status codes.
            _ => None,
        }
    }
}

/// Error returned when trying to create an empty struct type.
///
/// This is not supported by libffi. Trying to use an empty struct with libffi will result in a
/// `FFI_BAD_TYPEDEF` error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EmptyStructError;

impl Display for EmptyStructError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Tried to create a struct `Type` without any members.")
    }
}

impl Error for EmptyStructError {}

/// Error returned when trying to convert a [`Type`] that is not a valid type for variadic arguments
/// to [`VariadicType`](crate::types::VariadicType).
///
/// This typically happens when attempting to pass a 32-bit float or an 8- or 16-bit integer as a
/// variadic argument. Promote those values according to C's default argument promotions before
/// using them as variadic arguments.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InvalidVariadicTypeError(pub Type);

impl Display for InvalidVariadicTypeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Tried to use `{:?}` as a variadic argument type, but it is not valid for variadic arguments.",
            self.0
        )
    }
}

impl Error for InvalidVariadicTypeError {}

/// Error returned if libffi is unable to allocate a new closure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClosureAllocationError;

impl Display for ClosureAllocationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Libffi was unable to allocate memory for the closure.")
    }
}

impl Error for ClosureAllocationError {}

#[cfg(test)]
mod test {
    use core::ptr::null_mut;

    use libffi_sys::{
        ffi_abi, ffi_abi_FFI_DEFAULT_ABI, ffi_cif, ffi_prep_cif, ffi_prep_cif_var, ffi_type,
        ffi_type_enum_STRUCT,
    };

    use super::*;
    use crate::raw::{ffi_type_sint8, ffi_type_void};

    #[test]
    fn no_error_on_ok() {
        let mut cif = ffi_cif::default();
        // SAFETY:
        // * `cif` points to a valid `ffi_cif`.
        // * `rtype` points to a valid `ffi_type`.
        // * `atypes` will not be read from if `nargs` is 0.
        let status = unsafe {
            ffi_prep_cif(
                &raw mut cif,
                ffi_abi_FFI_DEFAULT_ABI,
                0,
                &raw mut ffi_type_void,
                null_mut(),
            )
        };

        assert!(LibffiError::from_status(status).is_none());
    }

    #[test]
    fn bad_abi_status() {
        let mut cif = ffi_cif::default();
        // SAFETY:
        // * `cif` points to a valid `ffi_cif`.
        // * `rtype` points to a valid `ffi_type`.
        // * `atypes` will not be read from if `nargs` is 0.
        let status = unsafe {
            ffi_prep_cif(
                &raw mut cif,
                ffi_abi::MAX,
                0,
                &raw mut ffi_type_void,
                null_mut(),
            )
        };

        assert_eq!(
            LibffiError::from_status(status),
            Some(LibffiError::FfiBadAbi)
        );
    }

    #[test]
    fn bad_typedef_status() {
        // A zero-sized type (a struct with no members) should cause a bad typedef status.
        let mut typedef_members = [null_mut()];
        let mut bad_typedef = ffi_type {
            type_: ffi_type_enum_STRUCT,
            elements: typedef_members.as_mut_ptr(),
            ..Default::default()
        };

        let mut cif = ffi_cif::default();
        // SAFETY:
        // * `cif` points to a valid `ffi_cif`.
        // * `rtype` points to a well-formed `ffi_type`.
        // * `atypes` will not be read from if `nargs` is 0.
        let status = unsafe {
            ffi_prep_cif(
                &raw mut cif,
                ffi_abi_FFI_DEFAULT_ABI,
                0,
                &raw mut bad_typedef,
                null_mut(),
            )
        };

        assert_eq!(
            LibffiError::from_status(status),
            Some(LibffiError::FfiBadTypeDef)
        );
    }

    #[test]
    fn bad_argtype_status() {
        // Arguments smaller than the size of an int are not supported as variadic argument types.
        // Note that two arguments are needed, as at least one fixed argument is required.
        let mut arguments = [&raw mut ffi_type_sint8, &raw mut ffi_type_sint8];

        let mut cif = ffi_cif::default();
        // SAFETY:
        // * `cif` points to a valid `ffi_cif`.
        // * `rtype` points to a valid `ffi_type`.
        // * `atypes` points to an array with `ntotalargs` pointers to valid `ffi_type`s.
        let status = unsafe {
            ffi_prep_cif_var(
                &raw mut cif,
                ffi_abi_FFI_DEFAULT_ABI,
                1,
                2,
                &raw mut ffi_type_void,
                arguments.as_mut_ptr(),
            )
        };

        assert_eq!(
            LibffiError::from_status(status),
            Some(LibffiError::FfiBadArgType)
        );
    }
}
