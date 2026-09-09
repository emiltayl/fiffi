extern crate alloc;

#[cfg(not(test))]
use alloc::{boxed::Box, vec::Vec};

use super::classification::ValueClass;
use crate::types::Type;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MarshalPlan {
    /// Argument copies and destinations.
    pub(super) argument_moves: Box<[ArgumentMove]>,

    /// Stack argument buffer size in bytes.
    pub(super) stack_buffer_size: usize,

    /// How the function returns its value.
    pub(super) return_strategy: ReturnStrategy,
}

impl MarshalPlan {
    pub(crate) fn build(argument_types: &[Type], return_type: Option<&Type>) -> Self {
        let mut register_allocator = RegisterAllocator::default();

        let mut argument_moves = Vec::with_capacity(argument_types.len());
        let mut stack_buffer_size: usize = 0;

        let return_strategy = ReturnStrategy::for_return_type(return_type);

        // Reserve the first GPR for the hidden return pointer if needed.
        if return_strategy == ReturnStrategy::HiddenPointer {
            register_allocator.allocate(RegisterRequirements::One(RegisterBank::Gpr));
        }

        for (argument_index, argument) in argument_types.iter().enumerate() {
            let argument_layout = argument.layout();
            let argument_class = ValueClass::classify(argument);

            let allocation = RegisterRequirements::for_value_class(argument_class)
                .and_then(|requirements| register_allocator.allocate(requirements));

            match allocation {
                None => {
                    // Stack arguments appear in argument order, starting at a 16-byte boundary.
                    stack_buffer_size = stack_buffer_size.next_multiple_of(argument_layout.align);
                    argument_moves.push(ArgumentMove {
                        argument_index,
                        destination: ArgumentDestination::Stack {
                            offset: stack_buffer_size,
                            size: argument_layout.size,
                        },
                    });
                    stack_buffer_size =
                        (stack_buffer_size + argument_layout.size).next_multiple_of(8);
                }
                Some(RegisterAllocation::One(destination)) => {
                    argument_moves.push(ArgumentMove {
                        argument_index,
                        destination: destination.into_destination(0, argument_layout.size),
                    });
                }
                Some(RegisterAllocation::Two(first_destination, second_destination)) => {
                    argument_moves.push(ArgumentMove {
                        argument_index,
                        destination: first_destination.into_destination(0, 8),
                    });

                    argument_moves.push(ArgumentMove {
                        argument_index,
                        destination: second_destination
                            .into_destination(8, argument_layout.size - 8),
                    });
                }
            }
        }

        MarshalPlan {
            argument_moves: argument_moves.into_boxed_slice(),
            stack_buffer_size,
            return_strategy,
        }
    }
}

/// Argument destination and copy range.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(
    variant_size_differences,
    reason = "Stack offsets and sizes stay pointer-width while register metadata is compact"
)]
pub(super) enum ArgumentDestination {
    /// Copy part of an argument into a general-purpose register.
    Gpr {
        /// Register index.
        index: u8,
        /// Byte offset in the source argument.
        source_offset: u8,
        /// Source bytes to copy.
        size: u8,
    },

    /// Copy part of an argument into an XMM register.
    Xmm {
        /// Register index.
        index: u8,
        /// Byte offset in the source argument.
        source_offset: u8,
        /// Source bytes to copy.
        size: u8,
    },

    /// Copy the whole argument, starting at source offset zero, into the stack buffer.
    Stack {
        /// Byte offset in the stack argument buffer.
        offset: usize,
        /// Source bytes to copy.
        size: usize,
    },
}

/// Argument copy performed before the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ArgumentMove {
    /// Argument index.
    pub(super) argument_index: usize,

    /// Copy destination and source range.
    pub(super) destination: ArgumentDestination,
}

const ARGUMENT_GPR_COUNT: usize = 6;
const ARGUMENT_XMM_COUNT: usize = 8;

/// Register bank for arguments and returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegisterBank {
    /// General-purpose registers.
    Gpr,

    /// XMM registers.
    Xmm,
}

impl RegisterBank {
    fn gpr_count(self) -> usize {
        usize::from(self == RegisterBank::Gpr)
    }

    fn xmm_count(self) -> usize {
        usize::from(self == RegisterBank::Xmm)
    }
}

/// Return value location.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReturnStrategy {
    /// No return value.
    Void,

    /// Result written through a hidden pointer to caller-provided memory.
    HiddenPointer,

    /// Result in one register.
    SingleRegister {
        /// Return register bank.
        bank: RegisterBank,

        /// Result size in bytes.
        byte_length: u8,
    },

    /// Result in two registers.
    TwoRegisters {
        /// Bank for the first eightbyte.
        first_bank: RegisterBank,

        /// Bank for the second eightbyte.
        second_bank: RegisterBank,

        /// Result bytes in the second register; the first holds eight.
        second_byte_length: u8,
    },
}

impl ReturnStrategy {
    fn for_return_type(return_type: Option<&Type>) -> Self {
        let Some(return_type) = return_type else {
            return Self::Void;
        };

        let Some(register_requirements) =
            RegisterRequirements::for_value_class(ValueClass::classify(return_type))
        else {
            return Self::HiddenPointer;
        };

        let byte_length = u8::try_from(return_type.layout().size)
            .expect("register return types cannot exceed 16 bytes");

        match register_requirements {
            RegisterRequirements::One(bank) => Self::SingleRegister { bank, byte_length },
            RegisterRequirements::Two(first_bank, second_bank) => Self::TwoRegisters {
                first_bank,
                second_bank,
                second_byte_length: byte_length - 8,
            },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RegisterRequirements {
    One(RegisterBank),
    Two(RegisterBank, RegisterBank),
}

impl RegisterRequirements {
    fn for_value_class(value_class: ValueClass) -> Option<Self> {
        use RegisterBank::{Gpr, Xmm};
        use RegisterRequirements::{One, Two};

        match value_class {
            ValueClass::Integer => Some(One(Gpr)),
            ValueClass::IntegerInteger => Some(Two(Gpr, Gpr)),
            ValueClass::IntegerSse => Some(Two(Gpr, Xmm)),
            ValueClass::Sse => Some(One(Xmm)),
            ValueClass::SseSse => Some(Two(Xmm, Xmm)),
            ValueClass::SseInteger => Some(Two(Xmm, Gpr)),
            ValueClass::Memory => None,
        }
    }

    fn counts(self) -> (usize, usize) {
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
    One(AllocatedRegister),
    Two(AllocatedRegister, AllocatedRegister),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AllocatedRegister {
    bank: RegisterBank,
    index: usize,
}

impl AllocatedRegister {
    fn into_destination(self, source_offset: u8, size: usize) -> ArgumentDestination {
        let index =
            u8::try_from(self.index).expect("argument register indices cannot exceed seven");
        let size = u8::try_from(size).expect("register argument copies cannot exceed eight bytes");

        match self.bank {
            RegisterBank::Gpr => ArgumentDestination::Gpr {
                index,
                source_offset,
                size,
            },
            RegisterBank::Xmm => ArgumentDestination::Xmm {
                index,
                source_offset,
                size,
            },
        }
    }
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

        (self.next_gpr_index + gpr_required) <= ARGUMENT_GPR_COUNT
            && (self.next_xmm_index + xmm_required) <= ARGUMENT_XMM_COUNT
    }

    fn take(&mut self, bank: RegisterBank) -> AllocatedRegister {
        let index = match bank {
            RegisterBank::Gpr => {
                let index = self.next_gpr_index;
                self.next_gpr_index += 1;
                index
            }

            RegisterBank::Xmm => {
                let index = self.next_xmm_index;
                self.next_xmm_index += 1;
                index
            }
        };
        AllocatedRegister { bank, index }
    }
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
            .map(|argument_move| {
                let (destination, source_offset, size) = match argument_move.destination {
                    ArgumentDestination::Gpr {
                        index,
                        source_offset,
                        size,
                    } => (
                        ExpectedLocation::Gpr(usize::from(index)),
                        usize::from(source_offset),
                        usize::from(size),
                    ),
                    ArgumentDestination::Xmm {
                        index,
                        source_offset,
                        size,
                    } => (
                        ExpectedLocation::Xmm(usize::from(index)),
                        usize::from(source_offset),
                        usize::from(size),
                    ),
                    ArgumentDestination::Stack { offset, size } => {
                        (ExpectedLocation::Stack(offset), 0, size)
                    }
                };
                ExpectedMove {
                    argument_index: argument_move.argument_index,
                    source_offset,
                    size,
                    destination,
                }
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
    fn return_strategies_follow_return_classes_and_layout_lengths() {
        let integer_sse = struct_type(&[Type::U32, Type::U32, Type::F32]);
        let sse_integer = struct_type(&[Type::F32, Type::F32, Type::U32]);
        let sse_sse = struct_type(&[Type::F32, Type::F32, Type::F32]);
        let memory = struct_type(&[Type::U64, Type::U64, Type::U64]);

        let cases = [
            (None, ReturnStrategy::Void),
            (
                Some(Type::U8),
                ReturnStrategy::SingleRegister {
                    bank: RegisterBank::Gpr,
                    byte_length: 1,
                },
            ),
            (
                Some(Type::Pointer),
                ReturnStrategy::SingleRegister {
                    bank: RegisterBank::Gpr,
                    byte_length: 8,
                },
            ),
            (
                Some(Type::F32),
                ReturnStrategy::SingleRegister {
                    bank: RegisterBank::Xmm,
                    byte_length: 4,
                },
            ),
            (
                Some(Type::F64),
                ReturnStrategy::SingleRegister {
                    bank: RegisterBank::Xmm,
                    byte_length: 8,
                },
            ),
            (
                Some(Type::U128),
                ReturnStrategy::TwoRegisters {
                    first_bank: RegisterBank::Gpr,
                    second_bank: RegisterBank::Gpr,
                    second_byte_length: 8,
                },
            ),
            (
                Some(integer_sse),
                ReturnStrategy::TwoRegisters {
                    first_bank: RegisterBank::Gpr,
                    second_bank: RegisterBank::Xmm,
                    second_byte_length: 4,
                },
            ),
            (
                Some(sse_integer),
                ReturnStrategy::TwoRegisters {
                    first_bank: RegisterBank::Xmm,
                    second_bank: RegisterBank::Gpr,
                    second_byte_length: 4,
                },
            ),
            (
                Some(sse_sse),
                ReturnStrategy::TwoRegisters {
                    first_bank: RegisterBank::Xmm,
                    second_bank: RegisterBank::Xmm,
                    second_byte_length: 4,
                },
            ),
            (Some(memory), ReturnStrategy::HiddenPointer),
        ];

        for (return_type, expected_strategy) in cases {
            let plan = MarshalPlan::build(&[], return_type.as_ref());
            assert_eq!(plan.return_strategy, expected_strategy);
        }
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
