use crate::types::Type;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ValueClass {
    /// Values that may be passed as arguments in GPR registers.
    Integer,
    /// Values that may be passed as arguments in XMM registers.
    Xmm,
    /// Values that are passed by a pointer to a copy of the argument. The pointer may be passed in
    /// a GPR register provided there are any register slots available.
    Indirect,
}

impl ValueClass {
    pub(super) fn classify(ty: &Type) -> Self {
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
                let layout = ty.layout();
                if Self::is_aggregate_size_passed_in_register(layout.size) {
                    Self::Integer
                } else {
                    Self::Indirect
                }
            }
        }
    }

    fn is_aggregate_size_passed_in_register(size: usize) -> bool {
        matches!(size, 1 | 2 | 4 | 8)
    }
}
