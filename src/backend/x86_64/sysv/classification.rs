use crate::types::{FfiTypeLayout, ScalarType, TypeRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RegisterSummary {
    layout: FfiTypeLayout,
    integer_bytes: u16,
    sse_bytes: u16,
}

impl RegisterSummary {
    fn for_type(ty: TypeRef<'_>) -> Self {
        match ty {
            TypeRef::Scalar(scalar) => Self::for_scalar(scalar),
            TypeRef::Struct(fields) => {
                let mut summary = Self::empty();

                for field in fields {
                    let child = Self::for_type(TypeRef::from(field));
                    let offset = summary.layout.append_field(child.layout);
                    summary.include_at(child, offset);
                }

                summary.layout.pad_to_alignment();
                summary
            }
            TypeRef::Union(variants) => {
                let mut summary = Self::empty();

                for variant in variants {
                    let child = Self::for_type(TypeRef::from(variant));
                    summary.layout.include_variant(child.layout);
                    summary.integer_bytes |= child.integer_bytes;
                    summary.sse_bytes |= child.sse_bytes;
                }

                summary.layout.pad_to_alignment();
                summary
            }
        }
    }

    fn empty() -> Self {
        Self {
            layout: FfiTypeLayout { align: 1, size: 0 },
            integer_bytes: 0,
            sse_bytes: 0,
        }
    }

    fn for_scalar(scalar: ScalarType) -> Self {
        let layout = TypeRef::Scalar(scalar).layout();
        let occupied_bytes = u16::MAX >> (16 - layout.size);

        match scalar {
            ScalarType::I8
            | ScalarType::U8
            | ScalarType::I16
            | ScalarType::U16
            | ScalarType::I32
            | ScalarType::U32
            | ScalarType::I64
            | ScalarType::U64
            | ScalarType::I128
            | ScalarType::U128
            | ScalarType::Isize
            | ScalarType::Usize
            | ScalarType::Pointer => Self {
                layout,
                integer_bytes: occupied_bytes,
                sse_bytes: 0,
            },
            ScalarType::F32 | ScalarType::F64 => Self {
                layout,
                integer_bytes: 0,
                sse_bytes: occupied_bytes,
            },
        }
    }

    fn include_at(&mut self, child: Self, offset: usize) {
        self.integer_bytes |= child.integer_bytes << offset;
        self.sse_bytes |= child.sse_bytes << offset;
    }

    fn eightbyte_class(self, byte_mask: u16) -> EightbyteClass {
        if self.integer_bytes & byte_mask != 0 {
            EightbyteClass::Integer
        } else if self.sse_bytes & byte_mask != 0 {
            EightbyteClass::Sse
        } else {
            EightbyteClass::NoClass
        }
    }
}

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
    pub(super) fn classify(ty: TypeRef<'_>, layout: &FfiTypeLayout) -> Self {
        if layout.size > 16 {
            return Self::Memory;
        }

        let summary = RegisterSummary::for_type(ty);
        Self::from_eightbyte_classes([
            summary.eightbyte_class(0x00ff),
            summary.eightbyte_class(0xff00),
        ])
    }

    fn from_eightbyte_classes(eightbyte_classes: [EightbyteClass; 2]) -> Self {
        match eightbyte_classes {
            [EightbyteClass::Sse, EightbyteClass::Sse] => Self::SseSse,
            [EightbyteClass::Sse, EightbyteClass::Integer] => Self::SseInteger,
            [EightbyteClass::Sse, EightbyteClass::NoClass] => Self::Sse,
            [EightbyteClass::Integer, EightbyteClass::Sse] => Self::IntegerSse,
            [EightbyteClass::Integer, EightbyteClass::Integer] => Self::IntegerInteger,
            [EightbyteClass::Integer, EightbyteClass::NoClass] => Self::Integer,
            // Nonempty aggregates have a scalar at offset zero.
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
#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::CallSignature;
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
    use crate::types::{FfiType, Type, VariadicType};

    fn classify(ty: &Type) -> ValueClass {
        ValueClass::classify(TypeRef::from(ty), &ty.layout())
    }

    fn assert_ffi_class<T: FfiType>(expected: ValueClass) {
        assert_eq!(classify(&T::ffi_type()), expected);
    }

    #[test]
    fn scalar_classification() {
        let cases = [
            (Type::I8, ValueClass::Integer),
            (Type::U8, ValueClass::Integer),
            (Type::I16, ValueClass::Integer),
            (Type::U16, ValueClass::Integer),
            (Type::I32, ValueClass::Integer),
            (Type::U32, ValueClass::Integer),
            (Type::I64, ValueClass::Integer),
            (Type::U64, ValueClass::Integer),
            (Type::Isize, ValueClass::Integer),
            (Type::Usize, ValueClass::Integer),
            (Type::Pointer, ValueClass::Integer),
            (Type::F32, ValueClass::Sse),
            (Type::F64, ValueClass::Sse),
            (Type::I128, ValueClass::IntegerInteger),
            (Type::U128, ValueClass::IntegerInteger),
        ];

        for (ty, expected) in &cases {
            assert_eq!(
                ValueClass::classify(TypeRef::from(ty), &ty.layout()),
                *expected
            );
        }
    }

    #[test]
    fn struct_classification() {
        assert_ffi_class::<F32x2>(ValueClass::Sse);
        assert_ffi_class::<U32F32>(ValueClass::Integer);
        assert_ffi_class::<F64x2>(ValueClass::SseSse);
        assert_ffi_class::<F64U64>(ValueClass::SseInteger);
        assert_ffi_class::<U64F64>(ValueClass::IntegerSse);
        assert_ffi_class::<U64x2>(ValueClass::IntegerInteger);
        assert_ffi_class::<F32x3U32>(ValueClass::SseInteger);
        assert_ffi_class::<U32F32x3>(ValueClass::IntegerSse);
        assert_ffi_class::<U64x3>(ValueClass::Memory);
        assert_ffi_class::<U128>(ValueClass::IntegerInteger);
        assert_ffi_class::<U128x2>(ValueClass::Memory);
    }

    #[test]
    fn recursive_struct_classification() {
        assert_ffi_class::<NestedF32x2x2>(ValueClass::SseSse);
        assert_ffi_class::<NestedU8U32x2>(ValueClass::IntegerInteger);
        assert_ffi_class::<NestedU8U64x2>(ValueClass::Memory);
        assert_ffi_class::<NestedF64x2x2>(ValueClass::Memory);
    }

    #[test]
    fn basic_union_classification() {
        assert_ffi_class::<UnionI32U32>(ValueClass::Integer);
        assert_ffi_class::<UnionU32F32>(ValueClass::Integer);
        assert_ffi_class::<UnionU64F64>(ValueClass::Integer);
        assert_ffi_class::<UnionU128>(ValueClass::IntegerInteger);
        assert_ffi_class::<UnionU8U128>(ValueClass::IntegerInteger);
        assert_ffi_class::<UnionU128U8>(ValueClass::IntegerInteger);
        assert_ffi_class::<UnionNestedF64x2>(ValueClass::SseSse);
        assert_ffi_class::<UnionNestedU64x2>(ValueClass::IntegerInteger);
    }

    #[test]
    fn mixed_aggregate_union_classification() {
        assert_ffi_class::<UnionNestedU8x3F32x2>(ValueClass::Integer);
        assert_ffi_class::<UnionNestedF32x2U64>(ValueClass::Integer);
        assert_ffi_class::<UnionNestedU16x3F64x2>(ValueClass::IntegerSse);
        assert_ffi_class::<UnionNestedF32x4U32x4>(ValueClass::IntegerInteger);
        assert_ffi_class::<UnionNestedF64x2U64x2>(ValueClass::IntegerInteger);
    }

    #[test]
    fn large_union_classification() {
        assert_ffi_class::<UnionNestedF64x4U64x4>(ValueClass::Memory);
        assert_ffi_class::<UnionNestedU64x4F64x4>(ValueClass::Memory);
    }

    #[test]
    fn nested_union_struct_classification() {
        assert_ffi_class::<NestedUnionU32F32>(ValueClass::Integer);
        assert_ffi_class::<NestedUnionU32F32x2>(ValueClass::Integer);
        assert_ffi_class::<NestedU8UnionU64F64>(ValueClass::IntegerInteger);
        assert_ffi_class::<NestedUnionU8U128U8>(ValueClass::Memory);
        assert_ffi_class::<NestedU8UnionU128U8>(ValueClass::Memory);
    }

    #[test]
    fn synthetic_union_classification() {
        let one_floating_eightbyte =
            Type::create_union_from_slice(&[Type::F32, Type::F64]).unwrap();
        assert_eq!(classify(&one_floating_eightbyte), ValueClass::Sse);

        let first_sse_second_integer =
            Type::create_union_from_slice(&[F64U64::ffi_type(), F64x2::ffi_type()]).unwrap();
        assert_eq!(classify(&first_sse_second_integer), ValueClass::SseInteger);

        for variants in [[Type::F32, Type::U32], [Type::U32, Type::F32]] {
            let integer_dominates = Type::create_union_from_slice(&variants).unwrap();
            assert_eq!(classify(&integer_dominates), ValueClass::Integer);
        }

        let float_fields = Type::create_struct(vec![Type::F32; 3]).unwrap();
        let integer_and_float = Type::create_struct(vec![Type::U8, Type::F64]).unwrap();
        for variants in [
            vec![float_fields.clone(), integer_and_float.clone()],
            vec![integer_and_float, float_fields],
        ] {
            let union = Type::create_union(variants).unwrap();
            assert_eq!(classify(&union), ValueClass::IntegerSse);
        }

        for variants in [vec![Type::F64, Type::U64], vec![Type::U64, Type::F64]] {
            let union = Type::create_union(variants).unwrap();
            let nested = Type::create_struct(vec![Type::F64, union]).unwrap();
            assert_eq!(classify(&nested), ValueClass::SseInteger);
        }

        let two_f64 = Type::create_struct(vec![Type::F64; 2]).unwrap();
        let small_integer_alternative = Type::create_union(vec![Type::U8, two_f64]).unwrap();
        assert_eq!(classify(&small_integer_alternative), ValueClass::IntegerSse);

        let many_alternatives = Type::create_union(vec![Type::F64; 512]).unwrap();
        assert_eq!(many_alternatives.layout(), Type::F64.layout());
        assert_eq!(classify(&many_alternatives), ValueClass::Sse);
    }

    #[test]
    fn nested_fields_crossing_an_eightbyte_keep_their_scalar_classes() {
        let inner = Type::create_struct(vec![Type::U32, Type::F32]).unwrap();
        assert_eq!(classify(&inner), ValueClass::Integer);
        let outer = Type::create_struct(vec![Type::F32, inner]).unwrap();
        assert_eq!(classify(&outer), ValueClass::IntegerSse);

        let reversed_inner = Type::create_struct(vec![Type::F32, Type::U32]).unwrap();
        assert_eq!(classify(&reversed_inner), ValueClass::Integer);
        let reversed_outer = Type::create_struct(vec![Type::F32, reversed_inner]).unwrap();
        assert_eq!(classify(&reversed_outer), ValueClass::SseInteger);
    }

    #[test]
    fn full_width_masks_and_aggregate_size_cutoff() {
        for scalar in [Type::I128, Type::U128] {
            assert_eq!(classify(&scalar), ValueClass::IntegerInteger);
            let wrapped = Type::create_struct(vec![scalar]).unwrap();
            assert_eq!(classify(&wrapped), ValueClass::IntegerInteger);
        }

        let sixteen_bytes = Type::create_struct(vec![Type::U8; 16]).unwrap();
        let seventeen_bytes = Type::create_struct(vec![Type::U8; 17]).unwrap();
        let seventeen_byte_union = Type::create_union(vec![seventeen_bytes.clone()]).unwrap();
        let alignment_enlarged_union =
            Type::create_union(vec![seventeen_bytes.clone(), Type::U128]).unwrap();
        let cases = [
            (&sixteen_bytes, 16, ValueClass::IntegerInteger),
            (&seventeen_bytes, 17, ValueClass::Memory),
            (&seventeen_byte_union, 17, ValueClass::Memory),
            (&alignment_enlarged_union, 32, ValueClass::Memory),
        ];

        for (ty, size, expected) in cases {
            let layout = ty.layout();
            assert_eq!(layout.size, size);
            assert_eq!(ValueClass::classify(TypeRef::from(ty), &layout), expected);
        }
    }

    #[test]
    fn deeply_nested_wrappers_preserve_layout_and_classification() {
        for depth in [32, 64, 128] {
            let mut ty = Type::F64;
            for _ in 0..depth {
                ty = Type::create_struct(vec![ty]).unwrap();
            }

            let layout = ty.layout();
            assert_eq!(layout, Type::F64.layout());
            assert_eq!(
                ValueClass::classify(TypeRef::from(&ty), &layout),
                ValueClass::Sse,
            );
        }
    }

    #[test]
    fn signature_type_views_classify_independently() {
        let return_type = Type::create_struct(vec![
            Type::F32,
            Type::create_struct(vec![Type::U32, Type::F32]).unwrap(),
        ])
        .unwrap();
        let argument_types = [Type::U64, Type::create_struct(vec![Type::U8; 17]).unwrap()];
        let variadic_types = [
            VariadicType::create_union(vec![Type::F32, Type::F64]).unwrap(),
            VariadicType::create_struct(vec![Type::U8]).unwrap(),
            VariadicType::F64,
        ];
        let signature =
            CallSignature::variadic(&argument_types, &variadic_types, Some(&return_type));
        let return_view = signature.return_type().unwrap();
        assert_eq!(
            ValueClass::classify(return_view, &return_view.layout()),
            ValueClass::IntegerSse,
        );
        let expected = [
            ValueClass::Integer,
            ValueClass::Memory,
            ValueClass::Sse,
            ValueClass::Integer,
            ValueClass::Sse,
        ];

        for (ty, class) in signature.arguments().zip(expected) {
            assert_eq!(ValueClass::classify(ty, &ty.layout()), class);
        }
    }
}
