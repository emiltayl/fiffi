//! TODO Top-level documentation with information about how ABI works and assumptions (such as stack
//! should start aligned to 16 bytes).

mod classification;
mod register_allocator;

extern crate alloc;

use alloc::vec::Vec;

use classification::ArgumentClass;
use register_allocator::{RegisterAllocation, RegisterAllocator, RegisterRequirements};

use crate::types::{FfiTypeLayout, Type};

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

            let allocation = RegisterRequirements::for_argument_class(argument_class)
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
