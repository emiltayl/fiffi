extern crate alloc;

use alloc::vec::Vec;
use core::mem::size_of;

use super::classification::ValueClass;
use crate::types::Type;

const STACK_ARGUMENT_SLOT_SIZE: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MarshalPlan {
    /// Where to put arguments to prepare for a function call.
    pub(super) argument_moves: Vec<ArgumentMove>,

    /// Bit mask identifying GPR slots containing offsets from the outgoing stack-buffer base.
    pub(super) gpr_indirect_regs_mask: u8,

    /// Stack-buffer offsets containing pointers that must be based on the outgoing stack address.
    pub(super) stack_indirect_arguments_offsets: Vec<usize>,

    /// The size of the buffer containing arguments passed on the stack.
    pub(super) stack_buffer_size: usize,

    /// How the function returns its value.
    pub(super) return_strategy: ReturnStrategy,
}

impl MarshalPlan {
    pub(super) fn build(argument_types: &[Type], return_type: Option<&Type>) -> Self {
        let return_strategy = ReturnStrategy::for_return_type(return_type);
        let mut register_allocator = RegisterSlotAllocator::default();

        // Reserve the first argument register slot for the hidden return pointer if the return
        // type's strategy is memory. There is always a register available at the start, so we do
        // not need to check `RegisterAllocator::allocate`'s return value.
        if return_strategy == ReturnStrategy::HiddenPointer {
            register_allocator.allocate();
        }

        // Every Win64 argument consumes one positional slot. Calculate the stack space required for
        // argument slots so space required for indirect arguments can be calculated with only one
        // loop.
        let stack_argument_buffer_size = argument_types
            .len()
            .saturating_sub(register_allocator.available_slots())
            .checked_mul(STACK_ARGUMENT_SLOT_SIZE)
            .expect("stack argument buffer size overflow");

        let mut next_stack_argument_offset = 0;
        let mut stack_buffer_size = stack_argument_buffer_size;

        let mut argument_moves = Vec::with_capacity(argument_types.len());
        // Bit flag with one bit for each argument register, set to 1 if the register contains an
        // offset to rsp that must be calculated to correctly pass indirect arguments.
        let mut gpr_indirect_regs_mask = 0;
        let mut stack_indirect_arguments_offsets = Vec::new();

        for (argument_index, argument) in argument_types.iter().enumerate() {
            let argument_size = argument.layout().size;
            let argument_class = ValueClass::classify(argument);

            let destination = match register_allocator.allocate() {
                Some(slot_index) if argument_class == ValueClass::Xmm => {
                    ArgumentDestination::Xmm(slot_index)
                }
                Some(slot_index) => ArgumentDestination::Gpr(slot_index),
                None => {
                    let destination = ArgumentDestination::Stack(next_stack_argument_offset);
                    next_stack_argument_offset += STACK_ARGUMENT_SLOT_SIZE;
                    destination
                }
            };

            let argument_source = ArgumentSource::Argument { argument_index };

            if argument_class == ValueClass::Indirect {
                // Win64 requires caller-owned copies of indirect arguments to be 16-byte aligned.
                // The stack buffer itself starts at a 16-byte-aligned address in the call
                // trampoline. Note that the following line will need to be changed if types that
                // are aligned to more than 16 bytes are added to fiffi.
                stack_buffer_size = stack_buffer_size.next_multiple_of(16);
                let argument_copy_offset = stack_buffer_size;

                argument_moves.push(ArgumentMove {
                    source: argument_source,
                    size: argument_size,
                    destination: ArgumentDestination::Stack(argument_copy_offset),
                });

                match &destination {
                    ArgumentDestination::Gpr(index) => {
                        gpr_indirect_regs_mask |= 1 << *index;
                    }
                    ArgumentDestination::Stack(offset) => {
                        stack_indirect_arguments_offsets.push(*offset);
                    }
                    ArgumentDestination::Xmm(_) => {
                        unreachable!("indirect arguments are not passed in vector registers");
                    }
                }

                argument_moves.push(ArgumentMove {
                    source: ArgumentSource::StackAddress {
                        offset: argument_copy_offset,
                    },
                    size: size_of::<usize>(),
                    destination,
                });

                stack_buffer_size += argument_size;
            } else {
                argument_moves.push(ArgumentMove {
                    source: argument_source,
                    size: argument_size,
                    destination,
                });
            }
        }

        debug_assert_eq!(next_stack_argument_offset, stack_argument_buffer_size);

        Self {
            argument_moves,
            gpr_indirect_regs_mask,
            stack_indirect_arguments_offsets,
            stack_buffer_size,
            return_strategy,
        }
    }
}

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
    /// The `usize` is the offset from the start of the buffer that will be copied to the stack.
    Stack(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ArgumentSource {
    /// Copy bytes from one of the caller-provided arguments.
    Argument {
        /// The index of the argument to copy.
        argument_index: usize,
    },

    /// Materialize the address at `offset` from the final outgoing stack-buffer base.
    ///
    /// These moves must be completed after the assembly trampoline has established the outgoing
    /// stack address.
    StackAddress {
        /// Offset of the pointee from the start of the outgoing stack buffer.
        offset: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ArgumentMove {
    /// Where the bytes or address for this move come from.
    pub(super) source: ArgumentSource,

    /// The number of bytes written to `destination`.
    pub(super) size: usize,

    /// Where the bytes or address are written.
    pub(super) destination: ArgumentDestination,
}

/// Describes how a function returns its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReturnStrategy {
    /// The function does not return a value.
    Void,

    /// The function writes its result through a hidden pointer to caller-provided memory.
    ///
    /// This is distinct from returning a pointer value, which uses [`ReturnStrategy::Rax`].
    HiddenPointer,

    /// The function provides its return value in the `byte_length` first bytes of rax.
    Rax { byte_length: u8 },

    /// The function provides its return value in the `byte_length` first bytes of xmm0.
    Xmm0 { byte_length: u8 },
}

impl ReturnStrategy {
    fn for_return_type(return_type: Option<&Type>) -> Self {
        let Some(return_type) = return_type else {
            return Self::Void;
        };

        if matches!(return_type, Type::I128 | Type::U128) {
            return Self::Xmm0 { byte_length: 16 };
        }

        match ValueClass::classify(return_type) {
            ValueClass::Indirect => Self::HiddenPointer,
            ValueClass::Integer => {
                let byte_length = u8::try_from(return_type.layout().size)
                    .expect("values returned in rax cannot exceed eight bytes");
                Self::Rax { byte_length }
            }
            ValueClass::Xmm => {
                let byte_length = u8::try_from(return_type.layout().size)
                    .expect("scalar values returned in xmm0 cannot exceed eight bytes");
                Self::Xmm0 { byte_length }
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RegisterSlotAllocator {
    next_slot: usize,
}

impl RegisterSlotAllocator {
    const REGISTER_SLOTS: usize = 4;

    fn available_slots(&self) -> usize {
        Self::REGISTER_SLOTS - self.next_slot
    }

    fn allocate(&mut self) -> Option<usize> {
        if !self.is_slot_available() {
            return None;
        }

        Some(self.take_slot())
    }

    fn is_slot_available(&self) -> bool {
        self.next_slot < Self::REGISTER_SLOTS
    }

    fn take_slot(&mut self) -> usize {
        let slot = self.next_slot;
        self.next_slot += 1;

        slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::structs::{U8x3, U64x2, U64x3};
    use crate::types::FfiType;

    fn argument(argument_index: usize) -> ArgumentSource {
        ArgumentSource::Argument { argument_index }
    }

    fn argument_move(
        argument_index: usize,
        size: usize,
        destination: ArgumentDestination,
    ) -> ArgumentMove {
        ArgumentMove {
            source: argument(argument_index),
            size,
            destination,
        }
    }

    fn address_move(offset: usize, destination: ArgumentDestination) -> ArgumentMove {
        ArgumentMove {
            source: ArgumentSource::StackAddress { offset },
            size: 8,
            destination,
        }
    }

    #[test]
    fn mixed_arguments_use_shared_positional_register_slots() {
        let plan = MarshalPlan::build(
            &[Type::U64, Type::F64, Type::U64, Type::F32, Type::F64],
            None,
        );

        assert_eq!(
            plan,
            MarshalPlan {
                argument_moves: alloc::vec![
                    argument_move(0, 8, ArgumentDestination::Gpr(0)),
                    argument_move(1, 8, ArgumentDestination::Xmm(1)),
                    argument_move(2, 8, ArgumentDestination::Gpr(2)),
                    argument_move(3, 4, ArgumentDestination::Xmm(3)),
                    argument_move(4, 8, ArgumentDestination::Stack(0)),
                ],
                gpr_indirect_regs_mask: 0,
                stack_indirect_arguments_offsets: alloc::vec![],
                stack_buffer_size: 8,
                return_strategy: ReturnStrategy::Void,
            }
        );
    }

    #[test]
    fn hidden_return_pointer_shifts_every_argument_position() {
        let return_type = U64x3::ffi_type();
        let plan = MarshalPlan::build(
            &[Type::U64, Type::F64, Type::U64, Type::F32],
            Some(&return_type),
        );

        assert_eq!(
            plan,
            MarshalPlan {
                argument_moves: alloc::vec![
                    argument_move(0, 8, ArgumentDestination::Gpr(1)),
                    argument_move(1, 8, ArgumentDestination::Xmm(2)),
                    argument_move(2, 8, ArgumentDestination::Gpr(3)),
                    argument_move(3, 4, ArgumentDestination::Stack(0)),
                ],
                gpr_indirect_regs_mask: 0,
                stack_indirect_arguments_offsets: alloc::vec![],
                stack_buffer_size: 8,
                return_strategy: ReturnStrategy::HiddenPointer,
            }
        );
    }

    #[test]
    fn indirect_copies_follow_stack_arguments_and_are_sixteen_byte_aligned() {
        let plan = MarshalPlan::build(
            &[
                U8x3::ffi_type(),
                Type::U64,
                Type::F64,
                Type::U64,
                U64x2::ffi_type(),
            ],
            None,
        );

        assert_eq!(
            plan,
            MarshalPlan {
                argument_moves: alloc::vec![
                    argument_move(0, 3, ArgumentDestination::Stack(16)),
                    address_move(16, ArgumentDestination::Gpr(0)),
                    argument_move(1, 8, ArgumentDestination::Gpr(1)),
                    argument_move(2, 8, ArgumentDestination::Xmm(2)),
                    argument_move(3, 8, ArgumentDestination::Gpr(3)),
                    argument_move(4, 16, ArgumentDestination::Stack(32)),
                    address_move(32, ArgumentDestination::Stack(0)),
                ],
                gpr_indirect_regs_mask: 0b0001,
                stack_indirect_arguments_offsets: alloc::vec![0],
                stack_buffer_size: 48,
                return_strategy: ReturnStrategy::Void,
            }
        );
    }

    #[test]
    fn primitive_u128_is_an_indirect_argument_but_an_xmm0_return() {
        let plan = MarshalPlan::build(&[Type::U128], Some(&Type::U128));

        assert_eq!(
            plan,
            MarshalPlan {
                argument_moves: alloc::vec![
                    argument_move(0, 16, ArgumentDestination::Stack(0)),
                    address_move(0, ArgumentDestination::Gpr(0)),
                ],
                gpr_indirect_regs_mask: 0b0001,
                stack_indirect_arguments_offsets: alloc::vec![],
                stack_buffer_size: 16,
                return_strategy: ReturnStrategy::Xmm0 { byte_length: 16 },
            }
        );
    }
}
