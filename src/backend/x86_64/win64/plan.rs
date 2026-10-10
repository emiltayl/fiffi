extern crate alloc;

#[cfg(not(test))]
use alloc::{boxed::Box, vec::Vec};

use super::classification::ValueClass;
use crate::backend::CallSignature;
use crate::types::{ScalarType, TypeRef};

pub(super) const SHADOW_SPACE_SIZE: usize = 32;
pub(super) const POINTER_SIZE: usize = size_of::<usize>();
const STACK_SLOT_SIZE: usize = POINTER_SIZE;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MarshalPlan {
    /// Argument copies and destinations.
    pub(super) argument_moves: Box<[ArgumentMove]>,

    /// Shadow space, stack argument slots, and aligned indirect copies in bytes, excluding
    /// discarded hidden-return storage.
    pub(super) stack_allocation_size: usize,

    /// How the function returns its value.
    pub(super) return_strategy: ReturnStrategy,
}

impl MarshalPlan {
    pub(crate) fn build(signature: CallSignature<'_>) -> Self {
        let return_strategy = ReturnStrategy::for_return_type(signature.return_type());
        let mut register_allocator = RegisterSlotAllocator::default();

        // Reserve the first slot for a hidden return pointer if needed.
        if matches!(return_strategy, ReturnStrategy::HiddenPointer { .. }) {
            register_allocator.allocate();
        }

        // Reserve all stack slots before placing indirect copies after them.
        let stack_arguments_size = signature
            .argument_count()
            .saturating_sub(register_allocator.available_slots())
            .strict_mul(STACK_SLOT_SIZE);

        // All stack offsets are relative to pre-call `rsp`, including shadow space.
        let stack_arguments_end = SHADOW_SPACE_SIZE.strict_add(stack_arguments_size);
        let mut next_stack_offset = SHADOW_SPACE_SIZE;
        let mut stack_allocation_size = stack_arguments_end;

        let mut argument_moves = Vec::with_capacity(signature.argument_count());

        for (argument_index, argument) in signature.arguments().enumerate() {
            let argument_layout = argument.layout();
            let argument_class = ValueClass::classify(argument, &argument_layout);

            let destination = match register_allocator.allocate() {
                Some(slot_index) if argument_class == ValueClass::Xmm => {
                    // Every register-passed scalar float in a variadic call also occupies
                    // its corresponding GPR register.
                    if signature.is_variadic() {
                        let destination = ArgumentDestination::Gpr(slot_index);
                        argument_moves
                            .push(destination.argument_move(argument_index, argument_layout.size));
                    }

                    ArgumentDestination::Xmm(slot_index)
                }
                Some(slot_index) => ArgumentDestination::Gpr(slot_index),
                None => {
                    let destination = ArgumentDestination::Stack(next_stack_offset);
                    next_stack_offset = next_stack_offset.strict_add(STACK_SLOT_SIZE);
                    destination
                }
            };

            if argument_class == ValueClass::Indirect {
                // Copies and the outgoing stack allocation base are 16-byte aligned.
                // Revisit this when adding types with greater alignment.
                stack_allocation_size = stack_allocation_size
                    .checked_next_multiple_of(16)
                    .expect("Win64 indirect-copy alignment overflow");
                let argument_copy_offset = stack_allocation_size;

                debug_assert!(argument_layout.align <= 16);
                debug_assert_eq!(argument_copy_offset % 16, 0);
                debug_assert!(argument_copy_offset >= stack_arguments_end);

                argument_moves.push(ArgumentMove::argument_to_stack(
                    argument_index,
                    argument_copy_offset,
                    argument_layout.size,
                ));

                argument_moves.push(destination.address_move(argument_copy_offset));

                stack_allocation_size = stack_allocation_size.strict_add(argument_layout.size);
            } else {
                argument_moves
                    .push(destination.argument_move(argument_index, argument_layout.size));
            }
        }

        debug_assert_eq!(next_stack_offset, stack_arguments_end);

        Self {
            argument_moves: argument_moves.into_boxed_slice(),
            stack_allocation_size,
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

    /// Byte offset from pre-call `rsp`, including shadow space.
    Stack(usize),
}

impl ArgumentDestination {
    fn argument_move(self, argument_index: usize, size: usize) -> ArgumentMove {
        match self {
            Self::Gpr(index) => ArgumentMove::register_move(
                argument_index,
                index,
                size,
                ArgumentMoveKind::ArgumentToGpr,
            ),
            Self::Xmm(index) => ArgumentMove::register_move(
                argument_index,
                index,
                size,
                ArgumentMoveKind::ArgumentToXmm,
            ),
            Self::Stack(stack_offset) => {
                ArgumentMove::argument_to_stack(argument_index, stack_offset, size)
            }
        }
    }

    fn address_move(self, source: usize) -> ArgumentMove {
        match self {
            Self::Gpr(index) => ArgumentMove::register_move(
                source,
                index,
                POINTER_SIZE,
                ArgumentMoveKind::StackAddressToGpr,
            ),
            Self::Stack(stack_offset) => ArgumentMove::stack_move(
                source,
                stack_offset,
                POINTER_SIZE,
                ArgumentMoveKind::StackAddressToStack,
            ),
            Self::Xmm(_) => unreachable!("indirect arguments are not passed in vector registers"),
        }
    }
}

/// Kind of argument bytes or stack address written before the call.
///
/// Kinds 0 and 1 are direct register payloads; bit zero selects GPR/XMM. Kinds 4 and 5
/// are stack addresses; bit zero selects GPR/stack. Bit zero selects a destination only
/// after dispatch establishes which pair applies.
///
/// Register destinations contain only kind bits and a two-bit slot index. Shifting away
/// the kind therefore yields the entire index, without a further mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum ArgumentMoveKind {
    /// Copy argument bytes into a general-purpose register.
    ArgumentToGpr = 0,

    /// Copy argument bytes into an XMM register.
    ArgumentToXmm = 1,

    /// Copy argument bytes into a stack slot or indirect copy buffer.
    ArgumentToStack = 2,

    /// Store an address into the outgoing allocation in a GPR.
    StackAddressToGpr = 4,

    /// Store an address into the outgoing allocation in a stack slot.
    StackAddressToStack = 5,
}

/// Argument bytes or a stack address written before the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ArgumentMove {
    /// Argument index for byte copies; pointee offset from pre-call `rsp` for address moves.
    pub(super) source: usize,

    /// Bytes copied or written. Address moves always use pointer width.
    pub(super) size: usize,

    /// Bits 0..=2: `ArgumentMoveKind`.
    /// Stack: remaining bits hold the byte offset from pre-call `rsp`, including shadow space.
    /// Registers: bits 3..=4 hold the index; all higher bits are zero.
    pub(super) destination: usize,
}

impl ArgumentMove {
    pub(super) const KIND_MASK: usize = 0b111;
    pub(super) const REGISTER_INDEX_SHIFT: usize = 3;
    pub(super) const REGISTER_INDEX_MASK: usize = 0b11;

    fn argument_to_stack(source: usize, stack_offset: usize, size: usize) -> Self {
        Self::stack_move(
            source,
            stack_offset,
            size,
            ArgumentMoveKind::ArgumentToStack,
        )
    }

    fn stack_move(source: usize, stack_offset: usize, size: usize, kind: ArgumentMoveKind) -> Self {
        // Stack offsets already include shadow space and have their low three bits clear.
        debug_assert!(stack_offset >= SHADOW_SPACE_SIZE);
        debug_assert_eq!(stack_offset & Self::KIND_MASK, 0);
        debug_assert!(matches!(
            kind,
            ArgumentMoveKind::ArgumentToStack | ArgumentMoveKind::StackAddressToStack
        ));
        if kind == ArgumentMoveKind::StackAddressToStack {
            debug_assert_eq!(size, POINTER_SIZE);
            debug_assert!(source >= SHADOW_SPACE_SIZE);
            debug_assert_eq!(source % 16, 0);
        }
        // ArgumentToStack accepts any byte count, including zero and odd indirect copies.
        Self {
            source,
            size,
            destination: stack_offset | kind as usize,
        }
    }

    fn register_move(source: usize, index: usize, size: usize, kind: ArgumentMoveKind) -> Self {
        // Slot allocation bounds indices to 0..4, so shifting them cannot overflow.
        debug_assert!(index <= Self::REGISTER_INDEX_MASK);
        debug_assert!(matches!(
            kind,
            ArgumentMoveKind::ArgumentToGpr
                | ArgumentMoveKind::ArgumentToXmm
                | ArgumentMoveKind::StackAddressToGpr
        ));
        match kind {
            ArgumentMoveKind::ArgumentToGpr => debug_assert!(matches!(size, 1 | 2 | 4 | 8)),
            ArgumentMoveKind::ArgumentToXmm => debug_assert!(matches!(size, 4 | 8)),
            ArgumentMoveKind::StackAddressToGpr => {
                debug_assert_eq!(size, POINTER_SIZE);
                debug_assert!(source >= SHADOW_SPACE_SIZE);
                debug_assert_eq!(source % 16, 0);
            }
            _ => {}
        }
        Self {
            source,
            size,
            destination: (index << Self::REGISTER_INDEX_SHIFT) | kind as usize,
        }
    }
}

/// Describes how a function returns its value.
#[expect(
    variant_size_differences,
    reason = "Return type layout must be stored to support discarding return values."
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReturnStrategy {
    /// The function does not return a value.
    Void,

    /// Writes the result through a hidden first argument.
    HiddenPointer { size: usize, align_log2: u8 },

    /// Returns `byte_length` low bytes in `rax`.
    Rax { byte_length: u8 },

    /// Returns `byte_length` low bytes in `xmm0`.
    Xmm0 { byte_length: u8 },
}

impl ReturnStrategy {
    fn for_return_type(return_type: Option<TypeRef<'_>>) -> Self {
        let Some(return_type) = return_type else {
            return Self::Void;
        };

        if matches!(
            return_type,
            TypeRef::Scalar(ScalarType::I128 | ScalarType::U128)
        ) {
            return Self::Xmm0 { byte_length: 16 };
        }

        let return_layout = return_type.layout();
        match ValueClass::classify(return_type, &return_layout) {
            ValueClass::Indirect => Self::HiddenPointer {
                size: return_layout.size,
                align_log2: u8::try_from(return_layout.align.trailing_zeros())
                    .expect("`usize::trailing_zeros` will always fit inside an `u8`."),
            },
            ValueClass::Integer => {
                let byte_length = u8::try_from(return_layout.size)
                    .expect("values returned in rax cannot exceed eight bytes");
                Self::Rax { byte_length }
            }
            ValueClass::Xmm => {
                let byte_length = u8::try_from(return_layout.size)
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
        // `allocate` checks availability, keeping the index below four and the counter at most
        // four.
        let slot = self.next_slot;
        self.next_slot += 1;

        slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::structs::{U8x3, U64x2, U64x3};
    use crate::types::{FfiType, Type, VariadicType};

    // Build expected encodings independently of the production constructors and accessors.
    fn argument_move(
        argument_index: usize,
        size: usize,
        destination: ArgumentDestination,
    ) -> ArgumentMove {
        match destination {
            ArgumentDestination::Gpr(index) => ArgumentMove {
                source: argument_index,
                size,
                destination: index * 8,
            },
            ArgumentDestination::Xmm(index) => ArgumentMove {
                source: argument_index,
                size,
                destination: index * 8 + 1,
            },
            ArgumentDestination::Stack(stack_offset) => ArgumentMove {
                source: argument_index,
                size,
                destination: stack_offset + 2,
            },
        }
    }

    fn address_move(source: usize, destination: ArgumentDestination) -> ArgumentMove {
        match destination {
            ArgumentDestination::Gpr(index) => ArgumentMove {
                source,
                size: size_of::<usize>(),
                destination: index * 8 + 4,
            },
            ArgumentDestination::Stack(stack_offset) => ArgumentMove {
                source,
                size: size_of::<usize>(),
                destination: stack_offset + 5,
            },
            ArgumentDestination::Xmm(_) => {
                panic!("stack addresses cannot be passed in XMM registers")
            }
        }
    }

    #[test]
    fn mixed_arguments_use_shared_positional_register_slots() {
        let plan = MarshalPlan::build(CallSignature::new(
            &[Type::U64, Type::F64, Type::U64, Type::F32, Type::F64],
            None,
        ));

        assert_eq!(
            plan,
            MarshalPlan {
                argument_moves: alloc::vec![
                    argument_move(0, 8, ArgumentDestination::Gpr(0)),
                    argument_move(1, 8, ArgumentDestination::Xmm(1)),
                    argument_move(2, 8, ArgumentDestination::Gpr(2)),
                    argument_move(3, 4, ArgumentDestination::Xmm(3)),
                    argument_move(4, 8, ArgumentDestination::Stack(32)),
                ]
                .into_boxed_slice(),
                stack_allocation_size: 40,
                return_strategy: ReturnStrategy::Void,
            }
        );
    }

    #[test]
    fn hidden_return_pointer_shifts_every_argument_position() {
        let return_type = U64x3::ffi_type();
        let return_layout = return_type.layout();
        let plan = MarshalPlan::build(CallSignature::new(
            &[Type::U64, Type::F64, Type::U64, Type::F32],
            Some(&return_type),
        ));

        assert_eq!(
            plan,
            MarshalPlan {
                argument_moves: alloc::vec![
                    argument_move(0, 8, ArgumentDestination::Gpr(1)),
                    argument_move(1, 8, ArgumentDestination::Xmm(2)),
                    argument_move(2, 8, ArgumentDestination::Gpr(3)),
                    argument_move(3, 4, ArgumentDestination::Stack(32)),
                ]
                .into_boxed_slice(),
                stack_allocation_size: 40,
                return_strategy: ReturnStrategy::HiddenPointer {
                    size: return_layout.size,
                    align_log2: u8::try_from(return_layout.align.trailing_zeros())
                        .expect("`usize::trailing_zeros` will always fit inside an `u8`."),
                },
            }
        );
    }

    #[test]
    fn indirect_copies_follow_stack_arguments_and_are_sixteen_byte_aligned() {
        let plan = MarshalPlan::build(CallSignature::new(
            &[
                U8x3::ffi_type(),
                Type::U64,
                Type::F64,
                Type::U64,
                U64x2::ffi_type(),
            ],
            None,
        ));

        assert_eq!(
            plan,
            MarshalPlan {
                argument_moves: alloc::vec![
                    argument_move(0, 3, ArgumentDestination::Stack(48)),
                    address_move(48, ArgumentDestination::Gpr(0)),
                    argument_move(1, 8, ArgumentDestination::Gpr(1)),
                    argument_move(2, 8, ArgumentDestination::Xmm(2)),
                    argument_move(3, 8, ArgumentDestination::Gpr(3)),
                    argument_move(4, 16, ArgumentDestination::Stack(64)),
                    address_move(64, ArgumentDestination::Stack(32)),
                ]
                .into_boxed_slice(),
                stack_allocation_size: 80,
                return_strategy: ReturnStrategy::Void,
            }
        );
    }

    #[test]
    fn primitive_u128_is_an_indirect_argument_but_an_xmm0_return() {
        let plan = MarshalPlan::build(CallSignature::new(&[Type::U128], Some(&Type::U128)));

        assert_eq!(
            plan,
            MarshalPlan {
                argument_moves: alloc::vec![
                    argument_move(0, 16, ArgumentDestination::Stack(32)),
                    address_move(32, ArgumentDestination::Gpr(0)),
                ]
                .into_boxed_slice(),
                stack_allocation_size: 48,
                return_strategy: ReturnStrategy::Xmm0 { byte_length: 16 },
            }
        );
    }

    #[test]
    fn variadic_empty_tail_duplicates_fixed_scalar_floats() {
        let fixed = [Type::F32, Type::F64];
        let ordinary = MarshalPlan::build(CallSignature::new(&fixed, None));
        assert_eq!(
            &*ordinary.argument_moves,
            &[
                argument_move(0, 4, ArgumentDestination::Xmm(0)),
                argument_move(1, 8, ArgumentDestination::Xmm(1)),
            ]
        );

        let variadic = MarshalPlan::build(CallSignature::variadic(&fixed, &[], None));
        assert_eq!(
            &*variadic.argument_moves,
            &[
                argument_move(0, 4, ArgumentDestination::Gpr(0)),
                argument_move(0, 4, ArgumentDestination::Xmm(0)),
                argument_move(1, 8, ArgumentDestination::Gpr(1)),
                argument_move(1, 8, ArgumentDestination::Xmm(1)),
            ]
        );
        assert_eq!(variadic.stack_allocation_size, 32);
    }

    #[test]
    fn variadic_floats_duplicate_register_slots_and_spill_once() {
        let plan = MarshalPlan::build(CallSignature::variadic(
            &[Type::F32],
            &[const { VariadicType::F64 }; 4],
            None,
        ));
        assert_eq!(
            &*plan.argument_moves,
            &[
                argument_move(0, 4, ArgumentDestination::Gpr(0)),
                argument_move(0, 4, ArgumentDestination::Xmm(0)),
                argument_move(1, 8, ArgumentDestination::Gpr(1)),
                argument_move(1, 8, ArgumentDestination::Xmm(1)),
                argument_move(2, 8, ArgumentDestination::Gpr(2)),
                argument_move(2, 8, ArgumentDestination::Xmm(2)),
                argument_move(3, 8, ArgumentDestination::Gpr(3)),
                argument_move(3, 8, ArgumentDestination::Xmm(3)),
                argument_move(4, 8, ArgumentDestination::Stack(32)),
            ]
        );
        assert_eq!(plan.stack_allocation_size, 40);
    }

    #[test]
    fn variadic_hidden_return_shifts_float_duplicates_and_stack_boundary() {
        let return_type = U64x3::ffi_type();
        let plan = MarshalPlan::build(CallSignature::variadic(
            &[Type::F32],
            &[const { VariadicType::F64 }; 4],
            Some(&return_type),
        ));
        assert_eq!(
            &*plan.argument_moves,
            &[
                argument_move(0, 4, ArgumentDestination::Gpr(1)),
                argument_move(0, 4, ArgumentDestination::Xmm(1)),
                argument_move(1, 8, ArgumentDestination::Gpr(2)),
                argument_move(1, 8, ArgumentDestination::Xmm(2)),
                argument_move(2, 8, ArgumentDestination::Gpr(3)),
                argument_move(2, 8, ArgumentDestination::Xmm(3)),
                argument_move(3, 8, ArgumentDestination::Stack(32)),
                argument_move(4, 8, ArgumentDestination::Stack(40)),
            ]
        );
        assert_eq!(plan.stack_allocation_size, 48);
        assert_eq!(
            plan.return_strategy,
            ReturnStrategy::HiddenPointer {
                size: 24,
                align_log2: 3
            }
        );
    }

    #[test]
    fn variadic_aggregates_use_integer_slots_and_aligned_indirect_copies() {
        let variadic = [
            VariadicType::create_struct(vec![Type::F32]).unwrap(),
            VariadicType::create_union(vec![Type::F32, Type::U8]).unwrap(),
            VariadicType::create_struct(vec![Type::U8; 3]).unwrap(),
            VariadicType::U128,
            VariadicType::create_struct(vec![Type::U64; 2]).unwrap(),
        ];
        let plan = MarshalPlan::build(CallSignature::variadic(&[], &variadic, Some(&Type::U128)));
        assert_eq!(
            plan,
            MarshalPlan {
                argument_moves: vec![
                    argument_move(0, 4, ArgumentDestination::Gpr(0)),
                    argument_move(1, 4, ArgumentDestination::Gpr(1)),
                    argument_move(2, 3, ArgumentDestination::Stack(48)),
                    address_move(48, ArgumentDestination::Gpr(2)),
                    argument_move(3, 16, ArgumentDestination::Stack(64)),
                    address_move(64, ArgumentDestination::Gpr(3)),
                    argument_move(4, 16, ArgumentDestination::Stack(80)),
                    address_move(80, ArgumentDestination::Stack(32)),
                ]
                .into_boxed_slice(),
                stack_allocation_size: 96,
                return_strategy: ReturnStrategy::Xmm0 { byte_length: 16 },
            }
        );
    }
}
