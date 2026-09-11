//! Target-specific calling conventions.

use crate::types::{Type, TypeRef, VariadicType};

cfg_select! {
    target_arch = "x86_64" => {
        mod x86_64;
        use x86_64 as native;
    }

    _ => {
        compile_error!("`fiffi` is not supported on this platform");
    }
}

pub(crate) use native::CallInterface;
pub use native::{Abi, VariadicAbi};

/// Borrowed inputs shared by ABI-specific marshalling planners.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CallSignature<'ty> {
    fixed: &'ty [Type],
    /// `Some(&[])` still describes a variadic call and may affect fixed arguments.
    variadic: Option<&'ty [VariadicType]>,
    return_type: Option<&'ty Type>,
}

impl<'ty> CallSignature<'ty> {
    pub(crate) fn new(fixed: &'ty [Type], return_type: Option<&'ty Type>) -> Self {
        Self {
            fixed,
            variadic: None,
            return_type,
        }
    }

    pub(crate) fn variadic(
        fixed: &'ty [Type],
        variadic: &'ty [VariadicType],
        return_type: Option<&'ty Type>,
    ) -> Self {
        Self {
            fixed,
            variadic: Some(variadic),
            return_type,
        }
    }

    pub(crate) fn arguments(self) -> impl Iterator<Item = TypeRef<'ty>> {
        self.fixed
            .iter()
            .map(TypeRef::from)
            .chain(self.variadic.unwrap_or(&[]).iter().map(TypeRef::from))
    }

    pub(crate) fn argument_count(self) -> usize {
        self.fixed
            .len()
            .strict_add(self.variadic.unwrap_or(&[]).len())
    }

    pub(crate) fn is_variadic(self) -> bool {
        self.variadic.is_some()
    }

    pub(crate) fn return_type(self) -> Option<TypeRef<'ty>> {
        self.return_type.map(TypeRef::from)
    }
}
