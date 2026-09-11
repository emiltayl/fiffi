extern crate alloc;

#[cfg(not(test))]
use alloc::vec::Vec;

use crate::types::{FfiTypeLayout, LayoutNode, ScalarType, TypeRef};

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
    pub(super) fn classify<'ty>(
        ty: TypeRef<'ty>,
        layout: &FfiTypeLayout,
        scratch: &mut Vec<LayoutNode<'ty>>,
    ) -> Self {
        if layout.size > 16 {
            scratch.clear();
            return Self::Memory;
        }

        let mut eightbyte_classes = [EightbyteClass::NoClass; 2];
        match ty {
            TypeRef::Struct(_) | TypeRef::Union(_) => {
                ty.layout_nodes_into(scratch);
                debug_assert_eq!(scratch[0].layout, *layout);
                Self::classify_into_eightbytes(scratch, 0, 0, &mut eightbyte_classes);
            }
            TypeRef::Scalar(scalar) => {
                scratch.clear();
                Self::classify_scalar_into_eightbytes(scalar, 0, &mut eightbyte_classes);
            }
        }
        Self::from_eightbyte_classes(eightbyte_classes)
    }

    fn classify_into_eightbytes(
        nodes: &[LayoutNode<'_>],
        node_index: usize,
        base_offset: usize,
        eightbyte_classes: &mut [EightbyteClass; 2],
    ) {
        let node = &nodes[node_index];
        match node.ty {
            TypeRef::Struct(_) | TypeRef::Union(_) => {
                let mut child_index = node_index + 1;
                while child_index < node.subtree_end {
                    let child = &nodes[child_index];
                    Self::classify_into_eightbytes(
                        nodes,
                        child_index,
                        base_offset + child.offset_in_parent,
                        eightbyte_classes,
                    );
                    // Skip the already-classified subtree directly to the next sibling.
                    child_index = child.subtree_end;
                }
            }
            TypeRef::Scalar(scalar) => {
                Self::classify_scalar_into_eightbytes(scalar, base_offset, eightbyte_classes);
            }
        }
    }

    fn classify_scalar_into_eightbytes(
        ty: ScalarType,
        base_offset: usize,
        eightbyte_classes: &mut [EightbyteClass; 2],
    ) {
        // Natural alignment keeps scalars within one eightbyte, except 128-bit integers.
        let eightbyte_index = base_offset / 8;

        match ty {
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
            | ScalarType::Pointer => {
                eightbyte_classes[eightbyte_index].merge_with(EightbyteClass::Integer);
            }
            ScalarType::F32 | ScalarType::F64 => {
                eightbyte_classes[eightbyte_index].merge_with(EightbyteClass::Sse);
            }
            ScalarType::I128 | ScalarType::U128 => {
                debug_assert_eq!(base_offset, 0);
                eightbyte_classes[0] = EightbyteClass::Integer;
                eightbyte_classes[1] = EightbyteClass::Integer;
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

impl EightbyteClass {
    fn merge_with(&mut self, other: EightbyteClass) {
        *self = match (*self, other) {
            (_, EightbyteClass::Integer) | (EightbyteClass::Integer, _) => EightbyteClass::Integer,
            (EightbyteClass::Sse, _) | (_, EightbyteClass::Sse) => EightbyteClass::Sse,
            // Only scalar classes are merged into the accumulator.
            (EightbyteClass::NoClass, EightbyteClass::NoClass) => unreachable!(),
        }
    }
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
        ValueClass::classify(TypeRef::from(ty), &ty.layout(), &mut Vec::new())
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

        let mut scratch = Vec::new();
        for (ty, expected) in &cases {
            assert_eq!(
                ValueClass::classify(TypeRef::from(ty), &ty.layout(), &mut scratch),
                *expected
            );
            assert_eq!(scratch.capacity(), 0);
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

        let floating_union = Type::create_union_from_slice(&[Type::F64]).unwrap();
        let union_at_nonzero_offset =
            Type::create_struct_from_slice(&[Type::U64, floating_union]).unwrap();
        assert_eq!(classify(&union_at_nonzero_offset), ValueClass::IntegerSse);

        let floating_union = Type::create_union_from_slice(&[Type::F64]).unwrap();
        let union_before_integer =
            Type::create_struct_from_slice(&[floating_union, Type::U64]).unwrap();
        assert_eq!(classify(&union_before_integer), ValueClass::SseInteger);
    }

    #[test]
    fn nested_fields_crossing_an_eightbyte_keep_their_scalar_classes() {
        let inner = Type::create_struct(vec![Type::U32, Type::F32]).unwrap();
        assert_eq!(classify(&inner), ValueClass::Integer);
        let outer = Type::create_struct(vec![Type::F32, inner]).unwrap();
        assert_eq!(classify(&outer), ValueClass::IntegerSse);
    }

    #[test]
    fn aggregates_at_the_size_cutoff_only_prepare_register_values() {
        let sixteen_bytes = Type::create_struct(vec![Type::U8; 16]).unwrap();
        let seventeen_bytes = Type::create_struct(vec![Type::U8; 17]).unwrap();
        let large_union = Type::create_union(vec![seventeen_bytes.clone(), Type::U128]).unwrap();
        let cases = [
            (&sixteen_bytes, 16, ValueClass::IntegerInteger),
            (&seventeen_bytes, 17, ValueClass::Memory),
            (&large_union, 32, ValueClass::Memory),
        ];

        for (ty, size, expected) in cases {
            let layout = ty.layout();
            assert_eq!(layout.size, size);
            let mut scratch = Vec::new();
            assert_eq!(
                ValueClass::classify(TypeRef::from(ty), &layout, &mut scratch),
                expected
            );
            assert_eq!(scratch.capacity() > 0, size <= 16);
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
            let mut scratch = Vec::new();
            assert_eq!(
                ValueClass::classify(TypeRef::from(&ty), &layout, &mut scratch),
                ValueClass::Sse,
            );
            assert_eq!(scratch.len(), depth + 1);
            for node in &scratch {
                assert_eq!(node.layout, layout);
                assert_eq!(node.offset_in_parent, 0);
                assert_eq!(node.subtree_end, scratch.len());
            }
        }
    }

    #[test]
    fn scratch_is_cleared_and_reused_across_different_value_shapes() {
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
        let mut scratch = Vec::new();
        let return_view = signature.return_type().unwrap();
        assert_eq!(
            ValueClass::classify(return_view, &return_view.layout(), &mut scratch),
            ValueClass::IntegerSse,
        );
        let capacity = scratch.capacity();
        let pointer = scratch.as_ptr();
        let expected = [
            (ValueClass::Integer, 0),
            (ValueClass::Memory, 0),
            (ValueClass::Sse, 3),
            (ValueClass::Integer, 2),
            (ValueClass::Sse, 0),
        ];

        for (ty, (class, node_count)) in signature.arguments().zip(expected) {
            assert_eq!(ValueClass::classify(ty, &ty.layout(), &mut scratch), class);
            assert_eq!(scratch.capacity(), capacity);
            assert_eq!(scratch.as_ptr(), pointer);
            assert_eq!(scratch.len(), node_count);
            if node_count != 0 {
                assert_eq!(scratch[0].ty, ty);
                assert_eq!(scratch[0].subtree_end, node_count);
            }
        }
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
