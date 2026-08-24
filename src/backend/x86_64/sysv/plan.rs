extern crate alloc;

use alloc::vec::Vec;

use super::classification::ValueClass;
use crate::types::{FfiTypeLayout, Type};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MarshalPlan {
    /// Where to put arguments to prepare for a function call.
    pub(super) argument_moves: Vec<ArgumentMove>,

    /// The size of the buffer containing arguments passed on the stack.
    pub(super) stack_buffer_size: usize,

    /// How the function returns its value.
    pub(super) return_strategy: ReturnStrategy,
}

impl MarshalPlan {
    pub(super) fn build(argument_types: &[Type], return_type: Option<&Type>) -> Self {
        let mut register_allocator = RegisterAllocator::default();

        let mut argument_moves: Vec<ArgumentMove> = Vec::with_capacity(argument_types.len());

        let mut stack_buffer_size: usize = 0;
        let mut stack_arguments: Vec<(usize, FfiTypeLayout)> = Vec::new();

        let return_strategy = ReturnStrategy::for_return_type(return_type);

        // Reserve the first argument register for the hidden return pointer if the return type's
        // strategy is memory. There is always a register available at the start, so we do not need
        // to check `RegisterAllocator::allocate`'s return value.
        if return_strategy == ReturnStrategy::HiddenPointer {
            register_allocator.allocate(RegisterRequirements::One(RegisterBank::Gpr));
        }

        for (argument_index, argument) in argument_types.iter().enumerate() {
            let argument_layout = argument.layout();

            let argument_class = ValueClass::classify(argument);

            let allocation = RegisterRequirements::for_value_class(argument_class)
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
            return_strategy,
        }
    }
}

/// Where an argument should be placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ArgumentDestination {
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
pub(super) struct ArgumentMove {
    /// The index of the argument to move.
    pub(super) argument_index: usize,

    /// The offset from the source pointer to start moving data from.
    ///
    /// # TODO
    ///
    /// This could potentially be something smaller than an usize? Would it shrink this struct
    /// though?
    pub(super) source_offset: usize,

    /// The number of bytes to move to `destination`.
    pub(super) size: usize,

    /// Where the argument should be moved to.
    pub(super) destination: ArgumentDestination,
}

const GPR_ARGUMENT_REGISTER_COUNT: usize = 6;
const XMM_ARGUMENT_REGISTER_COUNT: usize = 8;

/// A bank of registers used to pass or return values.
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

/// Describes how a function returns its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReturnStrategy {
    /// The function does not return a value.
    Void,

    /// The function writes its result through a hidden pointer to caller-provided memory.
    ///
    /// This is distinct from returning a pointer value, which uses [`Self::SingleRegister`] with
    /// [`RegisterBank::Gpr`].
    HiddenPointer,

    /// The function returns its value in one register.
    SingleRegister {
        /// The bank containing the return register.
        bank: RegisterBank,

        /// The number of bytes in the result type.
        byte_length: u8,
    },

    /// The function returns its value in two registers.
    TwoRegisters {
        /// The bank containing the first eightbyte of the result.
        first_bank: RegisterBank,

        /// The bank containing the second eightbyte of the result.
        second_bank: RegisterBank,

        /// The number of bytes in the result type stored in the second register.
        ///
        /// The first register always provides eight bytes.
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
