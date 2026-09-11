use crate::types::{FfiTypeLayout, ScalarType, TypeRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ValueClass {
    /// Passed in a general-purpose register or a stack slot.
    Integer,
    /// Passed in an XMM register or a stack slot.
    Xmm,
    /// Passed by a pointer to a caller-owned copy.
    Indirect,
}

impl ValueClass {
    pub(super) fn classify(ty: TypeRef<'_>, layout: &FfiTypeLayout) -> Self {
        match ty {
            TypeRef::Scalar(scalar) => match scalar {
                ScalarType::I128 | ScalarType::U128 => Self::Indirect,
                ScalarType::F32 | ScalarType::F64 => Self::Xmm,
                ScalarType::I8
                | ScalarType::U8
                | ScalarType::I16
                | ScalarType::U16
                | ScalarType::I32
                | ScalarType::U32
                | ScalarType::I64
                | ScalarType::U64
                | ScalarType::Isize
                | ScalarType::Usize
                | ScalarType::Pointer => Self::Integer,
            },
            TypeRef::Struct(_) | TypeRef::Union(_) => {
                if matches!(layout.size, 1 | 2 | 4 | 8) {
                    Self::Integer
                } else {
                    Self::Indirect
                }
            }
        }
    }
}
