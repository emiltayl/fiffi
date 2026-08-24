use crate::types::Type;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ValueClass {
    Integer,
    IntegerInteger,
    IntegerSse,
    Sse,
    SseSse,
    SseInteger,
    Memory,
}

impl ValueClass {
    pub(super) fn classify(ty: &Type) -> Self {
        match ty {
            Type::I128 | Type::U128 => Self::IntegerInteger,
            Type::F32 | Type::F64 => Self::Sse,
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

            Type::Struct(fields) => {
                let layout = ty.layout();
                if layout.size > 16 {
                    Self::Memory
                } else {
                    let mut eightbyte_classes = [EightbyteClass::NoClass, EightbyteClass::NoClass];

                    let field_offsets = ty.field_offsets();
                    for (field_ty, field_offset) in
                        fields.as_slice().iter().zip(field_offsets.iter())
                    {
                        Self::classify_into_eightbytes(
                            field_ty,
                            *field_offset,
                            &mut eightbyte_classes,
                        );
                    }

                    Self::from_eightbyte_classes(eightbyte_classes)
                }
            }

            Type::Union(variants) => {
                let layout = ty.layout();
                if layout.size > 16 {
                    Self::Memory
                } else {
                    let mut eightbyte_classes = [EightbyteClass::NoClass, EightbyteClass::NoClass];

                    for variant_ty in variants.as_slice() {
                        Self::classify_into_eightbytes(variant_ty, 0, &mut eightbyte_classes);
                    }

                    Self::from_eightbyte_classes(eightbyte_classes)
                }
            }
        }
    }

    fn classify_into_eightbytes(
        ty: &Type,
        base_offset: usize,
        eightbyte_classes: &mut [EightbyteClass; 2],
    ) {
        // All supported scalar types (except 128-bit integers) will reside fully in either the
        // first or second set of 8 bytes. If its offset is in the first 8 bytes, it will not spill
        // over into the second 8 bytes.
        let eightbyte_index = base_offset / 8;

        match ty {
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
            | Type::Pointer => {
                eightbyte_classes[eightbyte_index].merge_with(EightbyteClass::Integer);
            }
            Type::F32 | Type::F64 => {
                eightbyte_classes[eightbyte_index].merge_with(EightbyteClass::Sse);
            }
            Type::I128 | Type::U128 => {
                debug_assert_eq!(base_offset, 0);
                eightbyte_classes[0] = EightbyteClass::Integer;
                eightbyte_classes[1] = EightbyteClass::Integer;
            }
            Type::Struct(fields) => {
                let field_offsets = ty.field_offsets();
                for (field_ty, field_offset) in fields.as_slice().iter().zip(field_offsets.iter()) {
                    Self::classify_into_eightbytes(
                        field_ty,
                        base_offset + *field_offset,
                        eightbyte_classes,
                    );
                }
            }

            Type::Union(variants) => {
                for variant_ty in variants.as_slice() {
                    Self::classify_into_eightbytes(variant_ty, base_offset, eightbyte_classes);
                }
            }
        }
    }

    fn from_eightbyte_classes(eightbyte_classes: [EightbyteClass; 2]) -> Self {
        match eightbyte_classes {
            [EightbyteClass::Sse, EightbyteClass::Sse] => Self::SseSse,
            [EightbyteClass::Sse, EightbyteClass::Integer] => Self::SseInteger,
            [EightbyteClass::Sse, EightbyteClass::NoClass] => Self::Sse,
            [EightbyteClass::Integer, EightbyteClass::Sse] => Self::IntegerSse,
            [EightbyteClass::Integer, EightbyteClass::Integer] => Self::IntegerInteger,
            [EightbyteClass::Integer, EightbyteClass::NoClass] => Self::Integer,
            // For `EightbyteClass` arrays that are fully initialized, it should not be possible to
            // end up in a situation where the first element is `NoClass` without `unsafe` code.
            // Both structs and unions will have at least one field or variant at offset `0` whose
            // class is not `NoClass`.
            [EightbyteClass::NoClass, _] => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EightbyteClass {
    Integer,
    Sse,
    NoClass,
}

impl EightbyteClass {
    fn merge_with(&mut self, other: EightbyteClass) {
        *self = match (*self, other) {
            (_, EightbyteClass::Integer) | (EightbyteClass::Integer, _) => EightbyteClass::Integer,
            (EightbyteClass::Sse, _) | (_, EightbyteClass::Sse) => EightbyteClass::Sse,
            // No scalar types have `EightbyteClass::NoClass`, so a `NoClass` should never be merged
            // with `NoClass`.
            (EightbyteClass::NoClass, EightbyteClass::NoClass) => unreachable!(),
        }
    }
}
