extern crate alloc;

use alloc::vec::Vec;

use crate::types::Type;

#[derive(Clone, Debug)]
pub(super) struct MarshalPlan {}

enum ArgumentClass {
    Fpr,
    FprFpr,
    FprGpr,
    Gpr,
    GprGpr,
    GprFpr,
    Memory,
}

impl ArgumentClass {
    fn classify(ty: &Type) -> Self {
        match ty {
            Type::I128 | Type::U128 => Self::GprGpr,
            Type::F32 | Type::F64 => Self::Fpr,
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
            | Type::Pointer => Self::Gpr,

            Type::Struct(fields_vec) => {
                let layout = ty.layout();
                if layout.size > 16 {
                    Self::Memory
                } else {
                    todo!()
                }
            }

            Type::Union(non_empty_vec) => todo!(),
        }
    }
}
