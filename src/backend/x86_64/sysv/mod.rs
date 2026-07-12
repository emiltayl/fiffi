//! TODO Top-level documentation with information about how ABI works and assumptions (such as stack
//! should start aligned to 16 bytes).

extern crate alloc;

use alloc::vec::Vec;

use crate::types::{FfiTypeLayout, Type};

const GPR_ARGUMENT_REGISTER_COUNT: usize = 6;
const XMM_ARGUMENT_REGISTER_COUNT: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RegisterBank {
    Gpr,
    Xmm,
}

impl RegisterBank {
    fn gpr_count(&self) -> usize {
        usize::from(*self == RegisterBank::Gpr)
    }

    fn xmm_count(&self) -> usize {
        usize::from(*self == RegisterBank::Xmm)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RegisterRequirements {
    One(RegisterBank),
    Two(RegisterBank, RegisterBank),
}

impl RegisterRequirements {
    fn counts(&self) -> (usize, usize) {
        match self {
            RegisterRequirements::One(bank) => (bank.gpr_count(), bank.xmm_count()),
            RegisterRequirements::Two(bank_a, bank_b) => (
                bank_a.gpr_count() + bank_b.gpr_count(),
                bank_a.xmm_count() + bank_b.xmm_count(),
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RegisterAllocation {
    One(ArgumentDestination),
    Two(ArgumentDestination, ArgumentDestination),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RegisterAllocator {
    next_gpr_index: usize,
    next_xmm_index: usize,
}

impl RegisterAllocator {
    fn allocate(&mut self, requirements: RegisterRequirements) -> Option<RegisterAllocation> {
        if !self.space_available_for(requirements) {
            return None;
        }

        Some(match requirements {
            RegisterRequirements::One(bank) => RegisterAllocation::One(self.take(bank)),
            RegisterRequirements::Two(first_bank, second_bank) => {
                RegisterAllocation::Two(self.take(first_bank), self.take(second_bank))
            }
        })
    }

    fn space_available_for(&self, requirements: RegisterRequirements) -> bool {
        let (gpr_required, xmm_required) = requirements.counts();

        (self.next_gpr_index + gpr_required) <= GPR_ARGUMENT_REGISTER_COUNT
            && (self.next_xmm_index + xmm_required) <= XMM_ARGUMENT_REGISTER_COUNT
    }

    fn take(&mut self, bank: RegisterBank) -> ArgumentDestination {
        match bank {
            RegisterBank::Gpr => {
                let index = self.next_gpr_index;
                self.next_gpr_index += 1;
                ArgumentDestination::Gpr(index)
            }

            RegisterBank::Xmm => {
                let index = self.next_xmm_index;
                self.next_xmm_index += 1;
                ArgumentDestination::Xmm(index)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MarshalPlan {
    /// Where to put arguments to prepare for a function call.
    argument_moves: Vec<ArgumentMove>,

    /// The size of the buffer containing arguments passed on the stack.
    stack_buffer_size: usize,
    // TODO Handle return values as well
}

impl MarshalPlan {
    pub(super) fn build(argument_types: &[Type], return_type: Option<&Type>) -> Self {
        let mut register_allocator = RegisterAllocator::default();

        let mut argument_moves: Vec<ArgumentMove> = Vec::with_capacity(argument_types.len());

        let mut stack_buffer_size: usize = 0;
        let mut stack_arguments: Vec<(usize, FfiTypeLayout)> = Vec::new();

        // TODO handle `ret` first. May use register slots for return pointer?

        for (argument_index, argument) in argument_types.iter().enumerate() {
            let argument_layout = argument.layout();

            let argument_class = ArgumentClass::classify(argument);

            let allocation = argument_class
                .register_requirements()
                .and_then(|requirements| register_allocator.allocate(requirements));

            match allocation {
                None => stack_arguments.push((argument_index, argument_layout)),
                Some(RegisterAllocation::One(destination)) => {
                    argument_moves.push(ArgumentMove {
                        argument_index,
                        source_offset: 0,
                        size: argument_layout.size,
                        destination,
                    });
                }
                Some(RegisterAllocation::Two(first_destination, second_destination)) => {
                    argument_moves.push(ArgumentMove {
                        argument_index,
                        source_offset: 0,
                        size: 8,
                        destination: first_destination,
                    });

                    argument_moves.push(ArgumentMove {
                        argument_index,
                        source_offset: 8,
                        size: argument_layout.size - 8,
                        destination: second_destination,
                    });
                }
            }
        }

        // Arguments are pushed to the stack right to left, which leaves the first argument on the
        // stack at the lowest address as the stack grows "down" towards lower addresses. Fiffi will
        // ensure that the first argument on the stack will be aligned to 16 bytes.
        for (argument_index, argument_layout) in stack_arguments {
            stack_buffer_size = stack_buffer_size.next_multiple_of(argument_layout.align);

            argument_moves.push(ArgumentMove {
                argument_index,
                source_offset: 0,
                size: argument_layout.size,
                destination: ArgumentDestination::Stack(stack_buffer_size),
            });

            stack_buffer_size = (stack_buffer_size + argument_layout.size).next_multiple_of(8);
        }

        MarshalPlan {
            argument_moves,
            stack_buffer_size,
        }
    }
}

/// Where an argument should be placed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ArgumentDestination {
    /// Place argument in a general purpose register.
    ///
    /// The `usize` is the index in the integer register array.
    Gpr(usize),

    /// Place argument in a XMM register.
    ///
    /// The `usize` is the index in the XMM register array.
    Xmm(usize),

    /// Place the argument on the stack.
    ///
    /// The `usize` is the offset from the start of the buffer that will be put on the stack before
    /// the call.
    Stack(usize),
}

/// Instructions for Rust for how to prepare arguments for function calls.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ArgumentMove {
    /// The index of the argument to move.
    argument_index: usize,

    /// The offset from the source pointer to start moving data from.
    ///
    /// # TODO
    ///
    /// This could potentially be something smaller than an usize? Would it shrink this struct
    /// though?
    source_offset: usize,

    /// The number of bytes to move to `destination`.
    size: usize,

    /// Where the argument should be moved to.
    destination: ArgumentDestination,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArgumentClass {
    Integer,
    IntegerInteger,
    IntegerSse,
    Sse,
    SseSse,
    SseInteger,
    Memory,
}

impl ArgumentClass {
    fn classify(ty: &Type) -> Self {
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

    fn register_requirements(self) -> Option<RegisterRequirements> {
        use RegisterBank::{Gpr, Xmm};
        use RegisterRequirements::{One, Two};

        match self {
            Self::Integer => Some(One(Gpr)),
            Self::IntegerInteger => Some(Two(Gpr, Gpr)),
            Self::IntegerSse => Some(Two(Gpr, Xmm)),
            Self::Sse => Some(One(Xmm)),
            Self::SseSse => Some(Two(Xmm, Xmm)),
            Self::SseInteger => Some(Two(Xmm, Gpr)),
            Self::Memory => None,
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

#[cfg(test)]
mod tests {
    use super::*;
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
    use crate::types::FfiType;

    fn assert_ffi_class<T: FfiType>(expected: ArgumentClass) {
        assert_eq!(ArgumentClass::classify(&T::ffi_type()), expected);
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum ExpectedLocation {
        Gpr(usize),
        Xmm(usize),
        Stack(usize),
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct ExpectedMove {
        argument_index: usize,
        source_offset: usize,
        size: usize,
        destination: ExpectedLocation,
    }

    impl ExpectedMove {
        fn whole_argument(
            argument_types: &[Type],
            argument_index: usize,
            destination: ExpectedLocation,
        ) -> Self {
            Self {
                argument_index,
                source_offset: 0,
                size: argument_types[argument_index].layout().size,
                destination,
            }
        }

        fn eightbyte(
            argument_index: usize,
            source_offset: usize,
            size: usize,
            destination: ExpectedLocation,
        ) -> Self {
            Self {
                argument_index,
                source_offset,
                size,
                destination,
            }
        }
    }

    fn assert_marshal_plan(
        argument_types: &[Type],
        return_type: Option<&Type>,
        expected_moves: &[ExpectedMove],
        expected_stack_buffer_size: usize,
    ) {
        let plan = MarshalPlan::build(argument_types, return_type);

        let mut actual_moves = plan
            .argument_moves
            .iter()
            .map(|argument_move| ExpectedMove {
                argument_index: argument_move.argument_index,
                source_offset: argument_move.source_offset,
                size: argument_move.size,
                destination: match argument_move.destination {
                    ArgumentDestination::Gpr(index) => ExpectedLocation::Gpr(index),
                    ArgumentDestination::Xmm(index) => ExpectedLocation::Xmm(index),
                    ArgumentDestination::Stack(offset) => ExpectedLocation::Stack(offset),
                },
            })
            .collect::<Vec<_>>();

        actual_moves.sort_by_key(|argument_move| {
            (argument_move.argument_index, argument_move.source_offset)
        });

        let mut expected_moves = expected_moves.to_vec();
        expected_moves.sort_by_key(|argument_move| {
            (argument_move.argument_index, argument_move.source_offset)
        });

        assert_eq!(actual_moves, expected_moves);
        assert_eq!(plan.stack_buffer_size, expected_stack_buffer_size);
    }

    fn struct_type(fields: &[Type]) -> Type {
        Type::create_struct_from_slice(fields).expect("Test struct must contain at least one field")
    }

    #[test]
    fn empty_signature_requires_no_argument_storage() {
        assert_marshal_plan(&[], None, &[], 0);
    }

    #[test]
    fn integer_arguments_fill_six_registers_before_using_the_stack() {
        let argument_types = [
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
        ];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Gpr(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Gpr(1)),
            ExpectedMove::whole_argument(&argument_types, 2, ExpectedLocation::Gpr(2)),
            ExpectedMove::whole_argument(&argument_types, 3, ExpectedLocation::Gpr(3)),
            ExpectedMove::whole_argument(&argument_types, 4, ExpectedLocation::Gpr(4)),
            ExpectedMove::whole_argument(&argument_types, 5, ExpectedLocation::Gpr(5)),
            ExpectedMove::whole_argument(&argument_types, 6, ExpectedLocation::Stack(0)),
        ];

        assert_marshal_plan(&argument_types, None, &expected_moves, 8);
    }

    #[test]
    fn floating_arguments_fill_eight_registers_before_using_the_stack() {
        let argument_types = [
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
        ];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Xmm(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Xmm(1)),
            ExpectedMove::whole_argument(&argument_types, 2, ExpectedLocation::Xmm(2)),
            ExpectedMove::whole_argument(&argument_types, 3, ExpectedLocation::Xmm(3)),
            ExpectedMove::whole_argument(&argument_types, 4, ExpectedLocation::Xmm(4)),
            ExpectedMove::whole_argument(&argument_types, 5, ExpectedLocation::Xmm(5)),
            ExpectedMove::whole_argument(&argument_types, 6, ExpectedLocation::Xmm(6)),
            ExpectedMove::whole_argument(&argument_types, 7, ExpectedLocation::Xmm(7)),
            ExpectedMove::whole_argument(&argument_types, 8, ExpectedLocation::Stack(0)),
        ];

        assert_marshal_plan(&argument_types, None, &expected_moves, 8);
    }

    #[test]
    fn integer_and_vector_register_banks_are_allocated_independently() {
        let argument_types = [Type::U64, Type::F64, Type::Pointer, Type::F32];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Gpr(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Xmm(0)),
            ExpectedMove::whole_argument(&argument_types, 2, ExpectedLocation::Gpr(1)),
            ExpectedMove::whole_argument(&argument_types, 3, ExpectedLocation::Xmm(1)),
        ];

        assert_marshal_plan(&argument_types, None, &expected_moves, 0);
    }

    #[test]
    fn fields_in_one_eightbyte_are_merged_before_register_assignment() {
        let integer_dominates_sse = struct_type(&[Type::U32, Type::F32]);
        let sse_fields_share_one_eightbyte = struct_type(&[Type::F32, Type::F32]);
        let argument_types = [integer_dominates_sse, sse_fields_share_one_eightbyte];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Gpr(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Xmm(0)),
        ];

        assert_marshal_plan(&argument_types, None, &expected_moves, 0);
    }

    #[test]
    fn two_eightbyte_aggregates_use_registers_in_eightbyte_order() {
        let cases = [
            (
                struct_type(&[Type::U64, Type::U64]),
                ExpectedLocation::Gpr(0),
                ExpectedLocation::Gpr(1),
            ),
            (
                struct_type(&[Type::F64, Type::F64]),
                ExpectedLocation::Xmm(0),
                ExpectedLocation::Xmm(1),
            ),
            (
                struct_type(&[Type::U64, Type::F64]),
                ExpectedLocation::Gpr(0),
                ExpectedLocation::Xmm(0),
            ),
            (
                struct_type(&[Type::F64, Type::U64]),
                ExpectedLocation::Xmm(0),
                ExpectedLocation::Gpr(0),
            ),
        ];

        for (argument_type, first_destination, second_destination) in cases {
            let argument_types = [argument_type];
            let expected_moves = [
                ExpectedMove::eightbyte(0, 0, 8, first_destination),
                ExpectedMove::eightbyte(0, 8, 8, second_destination),
            ];

            assert_marshal_plan(&argument_types, None, &expected_moves, 0);
        }
    }

    #[test]
    fn final_aggregate_eightbyte_only_copies_bytes_in_the_value() {
        let argument_types = [struct_type(&[Type::F32, Type::F32, Type::F32])];
        let expected_moves = [
            ExpectedMove::eightbyte(0, 0, 8, ExpectedLocation::Xmm(0)),
            ExpectedMove::eightbyte(0, 8, 4, ExpectedLocation::Xmm(1)),
        ];

        assert_marshal_plan(&argument_types, None, &expected_moves, 0);
    }

    #[test]
    fn two_integer_eightbytes_spill_atomically_when_one_register_remains() {
        let argument_types = [
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U128,
            Type::U64,
        ];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Gpr(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Gpr(1)),
            ExpectedMove::whole_argument(&argument_types, 2, ExpectedLocation::Gpr(2)),
            ExpectedMove::whole_argument(&argument_types, 3, ExpectedLocation::Gpr(3)),
            ExpectedMove::whole_argument(&argument_types, 4, ExpectedLocation::Gpr(4)),
            ExpectedMove::whole_argument(&argument_types, 5, ExpectedLocation::Stack(0)),
            ExpectedMove::whole_argument(&argument_types, 6, ExpectedLocation::Gpr(5)),
        ];

        assert_marshal_plan(&argument_types, None, &expected_moves, 16);
    }

    #[test]
    fn two_sse_eightbytes_spill_atomically_when_one_register_remains() {
        let sse_pair = struct_type(&[Type::F64, Type::F64]);
        let argument_types = [
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            sse_pair,
            Type::F64,
        ];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Xmm(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Xmm(1)),
            ExpectedMove::whole_argument(&argument_types, 2, ExpectedLocation::Xmm(2)),
            ExpectedMove::whole_argument(&argument_types, 3, ExpectedLocation::Xmm(3)),
            ExpectedMove::whole_argument(&argument_types, 4, ExpectedLocation::Xmm(4)),
            ExpectedMove::whole_argument(&argument_types, 5, ExpectedLocation::Xmm(5)),
            ExpectedMove::whole_argument(&argument_types, 6, ExpectedLocation::Xmm(6)),
            ExpectedMove::whole_argument(&argument_types, 7, ExpectedLocation::Stack(0)),
            ExpectedMove::whole_argument(&argument_types, 8, ExpectedLocation::Xmm(7)),
        ];

        assert_marshal_plan(&argument_types, None, &expected_moves, 16);
    }

    #[test]
    fn mixed_aggregate_spill_does_not_consume_available_vector_register() {
        let mixed_aggregate = struct_type(&[Type::U64, Type::F64]);
        let argument_types = [
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            mixed_aggregate,
            Type::F64,
        ];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Gpr(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Gpr(1)),
            ExpectedMove::whole_argument(&argument_types, 2, ExpectedLocation::Gpr(2)),
            ExpectedMove::whole_argument(&argument_types, 3, ExpectedLocation::Gpr(3)),
            ExpectedMove::whole_argument(&argument_types, 4, ExpectedLocation::Gpr(4)),
            ExpectedMove::whole_argument(&argument_types, 5, ExpectedLocation::Gpr(5)),
            ExpectedMove::whole_argument(&argument_types, 6, ExpectedLocation::Stack(0)),
            ExpectedMove::whole_argument(&argument_types, 7, ExpectedLocation::Xmm(0)),
        ];

        assert_marshal_plan(&argument_types, None, &expected_moves, 16);
    }

    #[test]
    fn mixed_aggregate_spill_does_not_consume_available_integer_register() {
        let mixed_aggregate = struct_type(&[Type::F64, Type::U64]);
        let argument_types = [
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            Type::F64,
            mixed_aggregate,
            Type::U64,
        ];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Xmm(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Xmm(1)),
            ExpectedMove::whole_argument(&argument_types, 2, ExpectedLocation::Xmm(2)),
            ExpectedMove::whole_argument(&argument_types, 3, ExpectedLocation::Xmm(3)),
            ExpectedMove::whole_argument(&argument_types, 4, ExpectedLocation::Xmm(4)),
            ExpectedMove::whole_argument(&argument_types, 5, ExpectedLocation::Xmm(5)),
            ExpectedMove::whole_argument(&argument_types, 6, ExpectedLocation::Xmm(6)),
            ExpectedMove::whole_argument(&argument_types, 7, ExpectedLocation::Xmm(7)),
            ExpectedMove::whole_argument(&argument_types, 8, ExpectedLocation::Stack(0)),
            ExpectedMove::whole_argument(&argument_types, 9, ExpectedLocation::Gpr(0)),
        ];

        assert_marshal_plan(&argument_types, None, &expected_moves, 16);
    }

    #[test]
    fn memory_argument_uses_the_stack_without_consuming_registers() {
        let memory_argument = struct_type(&[Type::U64, Type::U64, Type::U64]);
        let argument_types = [memory_argument, Type::U64, Type::F64];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Stack(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Gpr(0)),
            ExpectedMove::whole_argument(&argument_types, 2, ExpectedLocation::Xmm(0)),
        ];

        assert_marshal_plan(&argument_types, None, &expected_moves, 24);
    }

    #[test]
    fn stack_arguments_follow_argument_order_and_alignment_requirements() {
        let memory_argument = struct_type(&[Type::U64, Type::U64, Type::U64]);
        let argument_types = [
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U8,
            Type::U128,
            Type::U32,
            memory_argument,
        ];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Gpr(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Gpr(1)),
            ExpectedMove::whole_argument(&argument_types, 2, ExpectedLocation::Gpr(2)),
            ExpectedMove::whole_argument(&argument_types, 3, ExpectedLocation::Gpr(3)),
            ExpectedMove::whole_argument(&argument_types, 4, ExpectedLocation::Gpr(4)),
            ExpectedMove::whole_argument(&argument_types, 5, ExpectedLocation::Gpr(5)),
            ExpectedMove::whole_argument(&argument_types, 6, ExpectedLocation::Stack(0)),
            ExpectedMove::whole_argument(&argument_types, 7, ExpectedLocation::Stack(16)),
            ExpectedMove::whole_argument(&argument_types, 8, ExpectedLocation::Stack(32)),
            ExpectedMove::whole_argument(&argument_types, 9, ExpectedLocation::Stack(40)),
        ];

        assert_marshal_plan(&argument_types, None, &expected_moves, 64);
    }

    #[test]
    fn memory_return_reserves_first_integer_argument_register() {
        let return_type = struct_type(&[Type::U64, Type::U64, Type::U64]);
        let argument_types = [
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
        ];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Gpr(1)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Gpr(2)),
            ExpectedMove::whole_argument(&argument_types, 2, ExpectedLocation::Gpr(3)),
            ExpectedMove::whole_argument(&argument_types, 3, ExpectedLocation::Gpr(4)),
            ExpectedMove::whole_argument(&argument_types, 4, ExpectedLocation::Gpr(5)),
            ExpectedMove::whole_argument(&argument_types, 5, ExpectedLocation::Stack(0)),
        ];

        assert_marshal_plan(&argument_types, Some(&return_type), &expected_moves, 8);
    }

    #[test]
    fn memory_return_does_not_consume_vector_argument_registers() {
        let return_type = struct_type(&[Type::U64, Type::U64, Type::U64]);
        let argument_types = [Type::F64, Type::F32];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Xmm(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Xmm(1)),
        ];

        assert_marshal_plan(&argument_types, Some(&return_type), &expected_moves, 0);
    }

    #[test]
    fn memory_return_participates_in_atomic_argument_register_allocation() {
        let return_type = struct_type(&[Type::U64, Type::U64, Type::U64]);
        let argument_types = [
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U64,
            Type::U128,
            Type::U64,
        ];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Gpr(1)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Gpr(2)),
            ExpectedMove::whole_argument(&argument_types, 2, ExpectedLocation::Gpr(3)),
            ExpectedMove::whole_argument(&argument_types, 3, ExpectedLocation::Gpr(4)),
            ExpectedMove::whole_argument(&argument_types, 4, ExpectedLocation::Stack(0)),
            ExpectedMove::whole_argument(&argument_types, 5, ExpectedLocation::Gpr(5)),
        ];

        assert_marshal_plan(&argument_types, Some(&return_type), &expected_moves, 16);
    }

    #[test]
    fn register_returns_do_not_consume_argument_registers() {
        let argument_types = [Type::U64, Type::F64];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Gpr(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Xmm(0)),
        ];
        let return_types = [
            Type::U64,
            Type::F64,
            struct_type(&[Type::U64, Type::U64]),
            struct_type(&[Type::F64, Type::F64]),
        ];

        for return_type in return_types {
            assert_marshal_plan(&argument_types, Some(&return_type), &expected_moves, 0);
        }
    }

    #[test]
    fn void_return_does_not_consume_argument_registers() {
        let argument_types = [Type::U64, Type::F64];
        let expected_moves = [
            ExpectedMove::whole_argument(&argument_types, 0, ExpectedLocation::Gpr(0)),
            ExpectedMove::whole_argument(&argument_types, 1, ExpectedLocation::Xmm(0)),
        ];

        assert_marshal_plan(&argument_types, None, &expected_moves, 0);
    }

    #[test]
    fn scalar_classification() {
        let cases = [
            (Type::I8, ArgumentClass::Integer),
            (Type::U8, ArgumentClass::Integer),
            (Type::I16, ArgumentClass::Integer),
            (Type::U16, ArgumentClass::Integer),
            (Type::I32, ArgumentClass::Integer),
            (Type::U32, ArgumentClass::Integer),
            (Type::I64, ArgumentClass::Integer),
            (Type::U64, ArgumentClass::Integer),
            (Type::Isize, ArgumentClass::Integer),
            (Type::Usize, ArgumentClass::Integer),
            (Type::Pointer, ArgumentClass::Integer),
            (Type::F32, ArgumentClass::Sse),
            (Type::F64, ArgumentClass::Sse),
            (Type::I128, ArgumentClass::IntegerInteger),
            (Type::U128, ArgumentClass::IntegerInteger),
        ];

        for (ty, expected) in cases {
            assert_eq!(ArgumentClass::classify(&ty), expected);
        }
    }

    #[test]
    fn struct_classification() {
        assert_ffi_class::<F32x2>(ArgumentClass::Sse);
        assert_ffi_class::<U32F32>(ArgumentClass::Integer);
        assert_ffi_class::<F64x2>(ArgumentClass::SseSse);
        assert_ffi_class::<F64U64>(ArgumentClass::SseInteger);
        assert_ffi_class::<U64F64>(ArgumentClass::IntegerSse);
        assert_ffi_class::<U64x2>(ArgumentClass::IntegerInteger);
        assert_ffi_class::<F32x3U32>(ArgumentClass::SseInteger);
        assert_ffi_class::<U32F32x3>(ArgumentClass::IntegerSse);
        assert_ffi_class::<U64x3>(ArgumentClass::Memory);
        assert_ffi_class::<U128>(ArgumentClass::IntegerInteger);
        assert_ffi_class::<U128x2>(ArgumentClass::Memory);
    }

    #[test]
    fn recursive_struct_classification() {
        assert_ffi_class::<NestedF32x2x2>(ArgumentClass::SseSse);
        assert_ffi_class::<NestedU8U32x2>(ArgumentClass::IntegerInteger);
        assert_ffi_class::<NestedU8U64x2>(ArgumentClass::Memory);
        assert_ffi_class::<NestedF64x2x2>(ArgumentClass::Memory);
    }

    #[test]
    fn basic_union_classification() {
        assert_ffi_class::<UnionI32U32>(ArgumentClass::Integer);
        assert_ffi_class::<UnionU32F32>(ArgumentClass::Integer);
        assert_ffi_class::<UnionU64F64>(ArgumentClass::Integer);
        assert_ffi_class::<UnionU128>(ArgumentClass::IntegerInteger);
        assert_ffi_class::<UnionU8U128>(ArgumentClass::IntegerInteger);
        assert_ffi_class::<UnionU128U8>(ArgumentClass::IntegerInteger);
        assert_ffi_class::<UnionNestedF64x2>(ArgumentClass::SseSse);
        assert_ffi_class::<UnionNestedU64x2>(ArgumentClass::IntegerInteger);
    }

    #[test]
    fn mixed_aggregate_union_classification() {
        assert_ffi_class::<UnionNestedU8x3F32x2>(ArgumentClass::Integer);
        assert_ffi_class::<UnionNestedF32x2U64>(ArgumentClass::Integer);
        assert_ffi_class::<UnionNestedU16x3F64x2>(ArgumentClass::IntegerSse);
        assert_ffi_class::<UnionNestedF32x4U32x4>(ArgumentClass::IntegerInteger);
        assert_ffi_class::<UnionNestedF64x2U64x2>(ArgumentClass::IntegerInteger);
    }

    #[test]
    fn large_union_classification() {
        assert_ffi_class::<UnionNestedF64x4U64x4>(ArgumentClass::Memory);
        assert_ffi_class::<UnionNestedU64x4F64x4>(ArgumentClass::Memory);
    }

    #[test]
    fn nested_union_struct_classification() {
        assert_ffi_class::<NestedUnionU32F32>(ArgumentClass::Integer);
        assert_ffi_class::<NestedUnionU32F32x2>(ArgumentClass::Integer);
        assert_ffi_class::<NestedU8UnionU64F64>(ArgumentClass::IntegerInteger);
        assert_ffi_class::<NestedUnionU8U128U8>(ArgumentClass::Memory);
        assert_ffi_class::<NestedU8UnionU128U8>(ArgumentClass::Memory);
    }

    #[test]
    fn synthetic_union_classification() {
        let one_floating_eightbyte =
            Type::create_union_from_slice(&[Type::F32, Type::F64]).unwrap();
        assert_eq!(
            ArgumentClass::classify(&one_floating_eightbyte),
            ArgumentClass::Sse,
        );

        let first_sse_second_integer =
            Type::create_union_from_slice(&[F64U64::ffi_type(), F64x2::ffi_type()]).unwrap();
        assert_eq!(
            ArgumentClass::classify(&first_sse_second_integer),
            ArgumentClass::SseInteger,
        );

        for variants in [[Type::F32, Type::U32], [Type::U32, Type::F32]] {
            let integer_dominates = Type::create_union_from_slice(&variants).unwrap();
            assert_eq!(
                ArgumentClass::classify(&integer_dominates),
                ArgumentClass::Integer,
            );
        }

        let floating_union = Type::create_union_from_slice(&[Type::F64]).unwrap();
        let union_at_nonzero_offset =
            Type::create_struct_from_slice(&[Type::U64, floating_union]).unwrap();
        assert_eq!(
            ArgumentClass::classify(&union_at_nonzero_offset),
            ArgumentClass::IntegerSse,
        );

        let floating_union = Type::create_union_from_slice(&[Type::F64]).unwrap();
        let union_before_integer =
            Type::create_struct_from_slice(&[floating_union, Type::U64]).unwrap();
        assert_eq!(
            ArgumentClass::classify(&union_before_integer),
            ArgumentClass::SseInteger,
        );
    }
}
