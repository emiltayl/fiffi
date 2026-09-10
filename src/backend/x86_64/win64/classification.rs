use crate::types::{FfiTypeLayout, Type};

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
    pub(super) fn classify(ty: &Type, layout: &FfiTypeLayout) -> Self {
        match ty {
            Type::I128 | Type::U128 => Self::Indirect,
            Type::F32 | Type::F64 => Self::Xmm,
            Type::I8
            | Type::U8
            | Type::I16
            | Type::U16
            | Type::I32
            | Type::U32
            | Type::I64
            | Type::U64
            | Type::Isize
            | Type::Usize
            | Type::Pointer => Self::Integer,
            Type::Struct(_) | Type::Union(_) => {
                if matches!(layout.size, 1 | 2 | 4 | 8) {
                    Self::Integer
                } else {
                    Self::Indirect
                }
            }
        }
    }
}
