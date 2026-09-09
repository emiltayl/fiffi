extern crate alloc;

#[cfg(not(test))]
use alloc::{boxed::Box, vec::Vec};

use super::classification::ValueClass;
use crate::types::Type;

const STACK_ARGUMENT_SLOT_SIZE: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MarshalPlan {
    /// Argument copies and destinations.
    pub(super) argument_moves: Box<[ArgumentMove]>,

    /// Bit mask identifying GPR slots containing offsets from the outgoing stack-buffer base.
    pub(super) gpr_indirect_regs_mask: u8,

    /// Stack-buffer offsets containing pointers that must be based on the outgoing stack address.
    pub(super) stack_indirect_arguments_offsets: Box<[usize]>,

    /// Stack argument and indirect copy buffer size in bytes.
    pub(super) stack_buffer_size: usize,

    /// How the function returns its value.
    pub(super) return_strategy: ReturnStrategy,
}

impl MarshalPlan {
    pub(crate) fn build(argument_types: &[Type], return_type: Option<&Type>) -> Self {
        let return_strategy = ReturnStrategy::for_return_type(return_type);
        let mut register_allocator = RegisterSlotAllocator::default();

        // Reserve the first slot for a hidden return pointer if needed.
        if return_strategy == ReturnStrategy::HiddenPointer {
            register_allocator.allocate();
        }

        // Reserve all stack slots before placing indirect copies after them.
        let stack_argument_buffer_size = argument_types
            .len()
            .saturating_sub(register_allocator.available_slots())
            .checked_mul(STACK_ARGUMENT_SLOT_SIZE)
            .expect("stack argument buffer size overflow");

        let mut next_stack_argument_offset = 0;
        let mut stack_buffer_size = stack_argument_buffer_size;

        let mut argument_moves = Vec::with_capacity(argument_types.len());
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

            if argument_class == ValueClass::Indirect {
                // Copies and the outgoing stack-buffer base are 16-byte aligned.
                // Revisit this when adding types with greater alignment.
                stack_buffer_size = stack_buffer_size.next_multiple_of(16);
                let argument_copy_offset = stack_buffer_size;

                argument_moves.push(ArgumentMove::ArgumentToStack {
                    argument_index,
                    size: argument_size,
                    offset: argument_copy_offset,
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

                argument_moves.push(destination.address_move(argument_copy_offset));

                stack_buffer_size += argument_size;
            } else {
                argument_moves.push(destination.argument_move(argument_index, argument_size));
            }
        }

        debug_assert_eq!(next_stack_argument_offset, stack_argument_buffer_size);

        Self {
            argument_moves: argument_moves.into_boxed_slice(),
            gpr_indirect_regs_mask,
            stack_indirect_arguments_offsets: stack_indirect_arguments_offsets.into_boxed_slice(),
            stack_buffer_size,
            return_strategy,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ArgumentDestination {
    /// General-purpose register slot.
    Gpr(usize),

    /// XMM register slot.
    Xmm(usize),

    /// Byte offset from the outgoing stack-buffer base.
    Stack(usize),
}

impl ArgumentDestination {
    fn argument_move(self, argument_index: usize, size: usize) -> ArgumentMove {
        match self {
            Self::Gpr(index) => ArgumentMove::ArgumentToGpr {
                argument_index,
                index: u8::try_from(index).expect("argument register indices cannot exceed three"),
                size: u8::try_from(size)
                    .expect("register argument copies cannot exceed eight bytes"),
            },
            Self::Xmm(index) => ArgumentMove::ArgumentToXmm {
                argument_index,
                index: u8::try_from(index).expect("argument register indices cannot exceed three"),
                size: u8::try_from(size)
                    .expect("register argument copies cannot exceed eight bytes"),
            },
            Self::Stack(offset) => ArgumentMove::ArgumentToStack {
                argument_index,
                offset,
                size,
            },
        }
    }

    fn address_move(self, offset: usize) -> ArgumentMove {
        match self {
            Self::Gpr(index) => ArgumentMove::StackAddressToGpr {
                offset,
                index: u8::try_from(index).expect("argument register indices cannot exceed three"),
            },
            Self::Stack(destination_offset) => ArgumentMove::StackAddressToStack {
                source_offset: offset,
                destination_offset,
            },
            Self::Xmm(_) => unreachable!("indirect arguments are not passed in vector registers"),
        }
    }
}

/// Argument bytes or a deferred stack address written before the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ArgumentMove {
    /// Copy argument bytes into a general-purpose register.
    ArgumentToGpr {
        /// Argument index.
        argument_index: usize,
        /// Register slot.
        index: u8,
        /// Source bytes to copy.
        size: u8,
    },
    /// Copy argument bytes into an XMM register.
    ArgumentToXmm {
        /// Argument index.
        argument_index: usize,
        /// Register slot.
        index: u8,
        /// Source bytes to copy.
        size: u8,
    },
    /// Copy argument bytes into a stack slot or indirect copy buffer.
    ArgumentToStack {
        /// Argument index.
        argument_index: usize,
        /// Destination byte offset in the outgoing stack buffer.
        offset: usize,
        /// Source bytes to copy.
        size: usize,
    },
    /// Store a pointer-sized offset in a GPR for the trampoline to rebase.
    StackAddressToGpr {
        /// Pointee byte offset in the outgoing stack buffer.
        offset: usize,
        /// Register slot.
        index: u8,
    },
    /// Store a pointer-sized offset in a stack slot for the trampoline to rebase.
    StackAddressToStack {
        /// Pointee byte offset in the outgoing stack buffer.
        source_offset: usize,
        /// Destination byte offset in the outgoing stack buffer.
        destination_offset: usize,
    },
}

/// Describes how a function returns its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReturnStrategy {
    /// The function does not return a value.
    Void,

    /// Writes the result through a hidden first argument.
    HiddenPointer,

    /// Returns `byte_length` low bytes in `rax`.
    Rax { byte_length: u8 },

    /// Returns `byte_length` low bytes in `xmm0`.
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

    fn argument_move(
        argument_index: usize,
        size: usize,
        destination: ArgumentDestination,
    ) -> ArgumentMove {
        match destination {
            ArgumentDestination::Gpr(index) => ArgumentMove::ArgumentToGpr {
                argument_index,
                index: u8::try_from(index).unwrap(),
                size: u8::try_from(size).unwrap(),
            },
            ArgumentDestination::Xmm(index) => ArgumentMove::ArgumentToXmm {
                argument_index,
                index: u8::try_from(index).unwrap(),
                size: u8::try_from(size).unwrap(),
            },
            ArgumentDestination::Stack(offset) => ArgumentMove::ArgumentToStack {
                argument_index,
                offset,
                size,
            },
        }
    }

    fn address_move(offset: usize, destination: ArgumentDestination) -> ArgumentMove {
        match destination {
            ArgumentDestination::Gpr(index) => ArgumentMove::StackAddressToGpr {
                offset,
                index: u8::try_from(index).unwrap(),
            },
            ArgumentDestination::Stack(destination_offset) => ArgumentMove::StackAddressToStack {
                source_offset: offset,
                destination_offset,
            },
            ArgumentDestination::Xmm(_) => {
                panic!("stack addresses cannot be passed in XMM registers")
            }
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
                ]
                .into_boxed_slice(),
                gpr_indirect_regs_mask: 0,
                stack_indirect_arguments_offsets: alloc::vec![].into_boxed_slice(),
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
                ]
                .into_boxed_slice(),
                gpr_indirect_regs_mask: 0,
                stack_indirect_arguments_offsets: alloc::vec![].into_boxed_slice(),
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
                ]
                .into_boxed_slice(),
                gpr_indirect_regs_mask: 0b0001,
                stack_indirect_arguments_offsets: alloc::vec![0].into_boxed_slice(),
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
                ]
                .into_boxed_slice(),
                gpr_indirect_regs_mask: 0b0001,
                stack_indirect_arguments_offsets: alloc::vec![].into_boxed_slice(),
                stack_buffer_size: 16,
                return_strategy: ReturnStrategy::Xmm0 { byte_length: 16 },
            }
        );
    }
}
