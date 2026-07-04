//! Error types for the `fiffi` crate.

use core::error::Error;
use core::fmt::Display;

use crate::types::Type;

/// Error returned when trying to create an empty struct type, which is not supported by fiffi.
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

/// Error returned if fiffi is unable to allocate a new closure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClosureAllocationError;

impl Display for ClosureAllocationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Libffi was unable to allocate memory for the closure.")
    }
}

impl Error for ClosureAllocationError {}
