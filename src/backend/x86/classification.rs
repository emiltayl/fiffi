use crate::types::{FfiTypeLayout, ScalarType, TypeRef};

/// Identifies whether an x87 return value is an `f32` or an `f64`.
#[derive(Clone, Copy, Debug)]
pub(super) enum FloatKind {
    F32,
    F64,
}

impl FloatKind {
    pub(super) const fn size(self) -> usize {
        match self {
            Self::F32 => 4,
            Self::F64 => 8,
        }
    }
}

/// Describes how the target ABI returns a value.
#[derive(Clone, Copy, Debug)]
pub(super) enum ReturnClass {
    Void,
    /// Returns the integer in register(s). The `usize` is the size of the integer.
    Integer(usize),
    X87(FloatKind),
    Indirect(FfiTypeLayout),
}

impl ReturnClass {
    pub(super) fn classify(ty: Option<TypeRef<'_>>) -> Self {
        let Some(ty) = ty else { return Self::Void };

        let layout = ty.layout();

        match ty {
            // The standard i686 UEFI target uses LLVM's soft-float lowering, so
            // scalar float results have the same transport as integers of that size.
            TypeRef::Scalar(ScalarType::F32 | ScalarType::F64) if cfg!(target_os = "uefi") => {
                Self::Integer(layout.size)
            }
            TypeRef::Scalar(ScalarType::F32) => Self::X87(FloatKind::F32),
            TypeRef::Scalar(ScalarType::F64) => Self::X87(FloatKind::F64),
            TypeRef::Scalar(ScalarType::I128 | ScalarType::U128) => Self::Indirect(layout),
            TypeRef::Scalar(_) => Self::Integer(layout.size),
            TypeRef::Struct(_) | TypeRef::Union(_) => {
                // The built-in targets' `abi_return_struct_as_int` setting. This is not inferred
                // from ABI alone, Linux stdcall still returns structs indirectly.
                let register_return = cfg!(any(
                    target_os = "windows",
                    target_os = "uefi",
                    target_vendor = "apple",
                    target_os = "freebsd",
                    target_os = "openbsd",
                ));
                if !register_return || !matches!(layout.size, 1 | 2 | 4 | 8) {
                    return Self::Indirect(layout);
                }

                // MSVC and UEFI return aggregates containing a single float in integer registers.
                // Other platforms with register aggregate returns use x87 for these aggregates.
                if !cfg!(any(target_env = "msvc", target_os = "uefi"))
                    && let Some(kind) = single_float_element(ty)
                {
                    return Self::X87(kind);
                }

                Self::Integer(layout.size)
            }
        }
    }
}

fn single_float_element(ty: TypeRef<'_>) -> Option<FloatKind> {
    match ty {
        TypeRef::Scalar(ScalarType::F32) => Some(FloatKind::F32),
        TypeRef::Scalar(ScalarType::F64) => Some(FloatKind::F64),
        // Every supported field is nonzero-sized and naturally aligned, so exactly
        // one field also guarantees the outer size equals the scalar size.
        TypeRef::Struct([field]) | TypeRef::Union([field]) => {
            single_float_element(TypeRef::from(field))
        }
        _ => None,
    }
}
