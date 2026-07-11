extern crate alloc;

use alloc::vec::Vec;

use crate::types::Type;

#[derive(Clone, Debug)]
pub(super) struct MarshalPlan {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArgumentClass {
    Fpr,
    FprFpr,
    FprGpr,
    Gpr,
    GprGpr,
    GprFpr,
    Memory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EightbyteClass {
    Integer,
    Sse,
    NoClass,
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
            [EightbyteClass::Sse, EightbyteClass::Sse] => Self::FprFpr,
            [EightbyteClass::Sse, EightbyteClass::Integer] => Self::FprGpr,
            [EightbyteClass::Sse, EightbyteClass::NoClass] => Self::Fpr,
            [EightbyteClass::Integer, EightbyteClass::Sse] => Self::GprFpr,
            [EightbyteClass::Integer, EightbyteClass::Integer] => Self::GprGpr,
            [EightbyteClass::Integer, EightbyteClass::NoClass] => Self::Gpr,
            // For `EightbyteClass` arrays that are fully initialized, it should not be possible to
            // end up in a situation where the first element is `NoClass` without `unsafe` code.
            // Both structs and unions will have at least one field or variant at offset `0` whose
            // class is not `NoClass`.
            [EightbyteClass::NoClass, _] => unreachable!(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{ArgumentClass, EightbyteClass};
    use crate::test_utils::structs::{
        F32x2, F32x3U32, F64U64, F64x2, NestedF32x2x2, NestedF64x2x2, NestedU8U32x2, NestedU8U64x2,
        NestedU8UnionU64F64, NestedU8UnionU128U8, NestedUnionU8U128U8, NestedUnionU32F32,
        NestedUnionU32F32x2, U32F32, U32F32x3, U64F64, U64x2, U64x3, U128, U128x2,
    };
    use crate::test_utils::unions::{
        UnionI32U32, UnionNestedF32x2U64, UnionNestedF32x4U32x4, UnionNestedF64x2,
        UnionNestedF64x2U64x2, UnionNestedF64x4U64x4, UnionNestedU8x3F32x2, UnionNestedU16x3F64x2,
        UnionNestedU64x2, UnionNestedU64x4F64x4, UnionU8U128, UnionU32F32, UnionU64F64, UnionU128,
        UnionU128U8,
    };
    use crate::types::{FfiType, Type};

    fn assert_ffi_class<T: FfiType>(expected: ArgumentClass) {
        assert_eq!(ArgumentClass::classify(&T::ffi_type()), expected);
    }

    #[test]
    fn scalar_classification() {
        let cases = [
            (Type::I8, ArgumentClass::Gpr),
            (Type::U8, ArgumentClass::Gpr),
            (Type::I16, ArgumentClass::Gpr),
            (Type::U16, ArgumentClass::Gpr),
            (Type::I32, ArgumentClass::Gpr),
            (Type::U32, ArgumentClass::Gpr),
            (Type::I64, ArgumentClass::Gpr),
            (Type::U64, ArgumentClass::Gpr),
            (Type::Isize, ArgumentClass::Gpr),
            (Type::Usize, ArgumentClass::Gpr),
            (Type::Pointer, ArgumentClass::Gpr),
            (Type::F32, ArgumentClass::Fpr),
            (Type::F64, ArgumentClass::Fpr),
            (Type::I128, ArgumentClass::GprGpr),
            (Type::U128, ArgumentClass::GprGpr),
        ];

        for (ty, expected) in cases {
            assert_eq!(ArgumentClass::classify(&ty), expected);
        }
    }

    #[test]
    fn struct_classification() {
        assert_ffi_class::<F32x2>(ArgumentClass::Fpr);
        assert_ffi_class::<U32F32>(ArgumentClass::Gpr);
        assert_ffi_class::<F64x2>(ArgumentClass::FprFpr);
        assert_ffi_class::<F64U64>(ArgumentClass::FprGpr);
        assert_ffi_class::<U64F64>(ArgumentClass::GprFpr);
        assert_ffi_class::<U64x2>(ArgumentClass::GprGpr);
        assert_ffi_class::<F32x3U32>(ArgumentClass::FprGpr);
        assert_ffi_class::<U32F32x3>(ArgumentClass::GprFpr);
        assert_ffi_class::<U64x3>(ArgumentClass::Memory);
        assert_ffi_class::<U128>(ArgumentClass::GprGpr);
        assert_ffi_class::<U128x2>(ArgumentClass::Memory);
    }

    #[test]
    fn recursive_struct_classification() {
        assert_ffi_class::<NestedF32x2x2>(ArgumentClass::FprFpr);
        assert_ffi_class::<NestedU8U32x2>(ArgumentClass::GprGpr);
        assert_ffi_class::<NestedU8U64x2>(ArgumentClass::Memory);
        assert_ffi_class::<NestedF64x2x2>(ArgumentClass::Memory);
    }

    #[test]
    fn basic_union_classification() {
        assert_ffi_class::<UnionI32U32>(ArgumentClass::Gpr);
        assert_ffi_class::<UnionU32F32>(ArgumentClass::Gpr);
        assert_ffi_class::<UnionU64F64>(ArgumentClass::Gpr);
        assert_ffi_class::<UnionU128>(ArgumentClass::GprGpr);
        assert_ffi_class::<UnionU8U128>(ArgumentClass::GprGpr);
        assert_ffi_class::<UnionU128U8>(ArgumentClass::GprGpr);
        assert_ffi_class::<UnionNestedF64x2>(ArgumentClass::FprFpr);
        assert_ffi_class::<UnionNestedU64x2>(ArgumentClass::GprGpr);
    }

    #[test]
    fn mixed_aggregate_union_classification() {
        assert_ffi_class::<UnionNestedU8x3F32x2>(ArgumentClass::Gpr);
        assert_ffi_class::<UnionNestedF32x2U64>(ArgumentClass::Gpr);
        assert_ffi_class::<UnionNestedU16x3F64x2>(ArgumentClass::GprFpr);
        assert_ffi_class::<UnionNestedF32x4U32x4>(ArgumentClass::GprGpr);
        assert_ffi_class::<UnionNestedF64x2U64x2>(ArgumentClass::GprGpr);
    }

    #[test]
    fn large_union_classification() {
        assert_ffi_class::<UnionNestedF64x4U64x4>(ArgumentClass::Memory);
        assert_ffi_class::<UnionNestedU64x4F64x4>(ArgumentClass::Memory);
    }

    #[test]
    fn nested_union_struct_classification() {
        assert_ffi_class::<NestedUnionU32F32>(ArgumentClass::Gpr);
        assert_ffi_class::<NestedUnionU32F32x2>(ArgumentClass::Gpr);
        assert_ffi_class::<NestedU8UnionU64F64>(ArgumentClass::GprGpr);
        assert_ffi_class::<NestedUnionU8U128U8>(ArgumentClass::Memory);
        assert_ffi_class::<NestedU8UnionU128U8>(ArgumentClass::Memory);
    }

    #[test]
    fn synthetic_union_classification() {
        let one_floating_eightbyte =
            Type::create_union_from_slice(&[Type::F32, Type::F64]).unwrap();
        assert_eq!(
            ArgumentClass::classify(&one_floating_eightbyte),
            ArgumentClass::Fpr,
        );

        let first_sse_second_integer =
            Type::create_union_from_slice(&[F64U64::ffi_type(), F64x2::ffi_type()]).unwrap();
        assert_eq!(
            ArgumentClass::classify(&first_sse_second_integer),
            ArgumentClass::FprGpr,
        );

        for variants in [[Type::F32, Type::U32], [Type::U32, Type::F32]] {
            let integer_dominates = Type::create_union_from_slice(&variants).unwrap();
            assert_eq!(
                ArgumentClass::classify(&integer_dominates),
                ArgumentClass::Gpr,
            );
        }

        let floating_union = Type::create_union_from_slice(&[Type::F64]).unwrap();
        let union_at_nonzero_offset =
            Type::create_struct_from_slice(&[Type::U64, floating_union]).unwrap();
        assert_eq!(
            ArgumentClass::classify(&union_at_nonzero_offset),
            ArgumentClass::GprFpr,
        );

        let floating_union = Type::create_union_from_slice(&[Type::F64]).unwrap();
        let union_before_integer =
            Type::create_struct_from_slice(&[floating_union, Type::U64]).unwrap();
        assert_eq!(
            ArgumentClass::classify(&union_before_integer),
            ArgumentClass::FprGpr,
        );
    }

    #[test]
    fn merge_with_mutates_receiver_using_class_precedence() {
        let cases = [
            (
                EightbyteClass::NoClass,
                EightbyteClass::Sse,
                EightbyteClass::Sse,
            ),
            (
                EightbyteClass::NoClass,
                EightbyteClass::Integer,
                EightbyteClass::Integer,
            ),
            (
                EightbyteClass::Sse,
                EightbyteClass::Sse,
                EightbyteClass::Sse,
            ),
            (
                EightbyteClass::Sse,
                EightbyteClass::Integer,
                EightbyteClass::Integer,
            ),
            (
                EightbyteClass::Integer,
                EightbyteClass::Sse,
                EightbyteClass::Integer,
            ),
            (
                EightbyteClass::Integer,
                EightbyteClass::Integer,
                EightbyteClass::Integer,
            ),
        ];

        for (mut receiver, other, expected) in cases {
            receiver.merge_with(other);
            assert_eq!(receiver, expected);
        }
    }
}
