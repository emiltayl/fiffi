extern crate alloc;

#[cfg(not(test))]
use alloc::{boxed::Box, vec::Vec};

use super::classification::ValueClass;
use crate::backend::CallSignature;
use crate::types::{ScalarType, TypeRef};

pub(super) const SHADOW_SPACE_SIZE: usize = 32;
pub(super) const POINTER_SIZE: usize = size_of::<usize>();
const STACK_SLOT_SIZE: usize = POINTER_SIZE;

// Separate allocation arithmetic so boundary tests need neither huge type trees nor storage.
fn outgoing_stack_arguments_end(argument_count: usize, available_slots: usize) -> usize {
    SHADOW_SPACE_SIZE.strict_add(
        argument_count
            .saturating_sub(available_slots)
            .strict_mul(STACK_SLOT_SIZE),
    )
}

fn reserve_indirect_copy(allocation_size: &mut usize, size: usize) -> usize {
    let offset = allocation_size
        .checked_next_multiple_of(16)
        .expect("Win64 indirect-copy alignment overflow");
    *allocation_size = offset.strict_add(size);
    offset
}

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
        // All stack offsets are relative to pre-call `rsp`, including shadow space.
        let stack_arguments_end = outgoing_stack_arguments_end(
            signature.argument_count(),
            register_allocator.available_slots(),
        );
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
                let argument_copy_offset =
                    reserve_indirect_copy(&mut stack_allocation_size, argument_layout.size);

                debug_assert!(argument_layout.align <= 16);
                debug_assert_eq!(argument_copy_offset % 16, 0);
                debug_assert!(argument_copy_offset >= stack_arguments_end);

                argument_moves.push(ArgumentMove::argument_to_stack(
                    argument_index,
                    argument_copy_offset,
                    argument_layout.size,
                ));

                argument_moves.push(destination.address_move(argument_copy_offset));
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
    use std::panic::catch_unwind;

    use super::*;
    use crate::test_utils::structs::{F32, U8x3, U64x2, U64x3};
    use crate::test_utils::unions::UnionF32U32;
    use crate::types::{FfiType, Type, VariadicType};

    // Private allocation arithmetic cannot be reached with bounded real signatures.
    #[test]
    fn stack_allocation_checks_slot_multiplication_and_shadow_space_addition() {
        assert!(catch_unwind(|| outgoing_stack_arguments_end(usize::MAX / 8 + 1, 0)).is_err());
        assert!(catch_unwind(|| outgoing_stack_arguments_end(usize::MAX / 8, 0)).is_err());
        assert_eq!(outgoing_stack_arguments_end(0, 4), 32);
        assert_eq!(outgoing_stack_arguments_end(5, 4), 40);
        assert_eq!(
            outgoing_stack_arguments_end((usize::MAX - 32) / 8, 0),
            usize::MAX - 7
        );
    }

    #[test]
    fn indirect_copy_allocation_checks_alignment_and_size_addition() {
        let reserve = |end, size| {
            let mut allocation = end;
            let offset = reserve_indirect_copy(&mut allocation, size);
            (offset, allocation)
        };
        assert!(catch_unwind(|| reserve(usize::MAX - 14, 1)).is_err());
        assert!(catch_unwind(|| reserve(usize::MAX - 15, 16)).is_err());
        assert_eq!(reserve(40, 3), (48, 51));
        assert_eq!(reserve(usize::MAX - 15, 15), (usize::MAX - 15, usize::MAX));
        assert_eq!(
            reserve(usize::MAX - 31, 16),
            (usize::MAX - 31, usize::MAX - 15)
        );
    }

    // Calls cannot reliably expose copy alignment or nonoverlap for under-aligned values.
    // Inspect only the allocation and pointer relationships, not the complete marshal plan.
    fn assert_indirect_copies(
        plan: &MarshalPlan,
        stack_arguments_end: usize,
        sources: &[(usize, usize)],
    ) {
        let mut previous_end = stack_arguments_end;
        let mut register_pointer = false;
        let mut stack_pointer = false;
        for &(source, size) in sources {
            let index = plan
                .argument_moves
                .iter()
                .position(|movement| {
                    movement.source == source
                        && movement.destination & ArgumentMove::KIND_MASK
                            == ArgumentMoveKind::ArgumentToStack as usize
                })
                .expect("missing indirect copy");
            let copy = &plan.argument_moves[index];
            let start = copy.destination & !ArgumentMove::KIND_MASK;
            assert_eq!(start % 16, 0);
            assert!(start >= previous_end);
            assert_eq!(copy.size, size);
            previous_end = start + size;
            assert!(previous_end <= plan.stack_allocation_size);

            let pointer = &plan.argument_moves[index + 1];
            assert_eq!(pointer.source, start);
            assert_eq!(pointer.size, POINTER_SIZE);
            let kind = pointer.destination & ArgumentMove::KIND_MASK;
            if kind == ArgumentMoveKind::StackAddressToGpr as usize {
                register_pointer = true;
            } else {
                assert_eq!(kind, ArgumentMoveKind::StackAddressToStack as usize);
                let slot = pointer.destination & !ArgumentMove::KIND_MASK;
                assert!(slot >= SHADOW_SPACE_SIZE && slot + POINTER_SIZE <= stack_arguments_end);
                stack_pointer = true;
            }
        }
        assert!(register_pointer && stack_pointer);
    }

    #[test]
    fn indirect_copies_follow_stack_arguments_and_are_sixteen_byte_aligned() {
        let arguments = [
            U8x3::ffi_type(),
            Type::U64,
            Type::F64,
            Type::U64,
            U64x2::ffi_type(),
        ];
        let return_type = U64x3::ffi_type();
        for hidden in [false, true] {
            let plan = MarshalPlan::build(CallSignature::new(
                &arguments,
                hidden.then_some(&return_type),
            ));
            let stack_arguments_end = SHADOW_SPACE_SIZE + if hidden { 16 } else { 8 };
            assert_indirect_copies(
                &plan,
                stack_arguments_end,
                &[(0, size_of::<U8x3>()), (4, size_of::<U64x2>())],
            );
        }
    }

    #[test]
    fn variadic_indirect_copies_are_aligned_after_stack_slots() {
        let variadic = [
            F32::ffi_type(),
            UnionF32U32::ffi_type(),
            U8x3::ffi_type(),
            Type::U128,
            U64x2::ffi_type(),
        ]
        .into_iter()
        .map(|ty| VariadicType::try_from(ty).unwrap())
        .collect::<Vec<_>>();
        let return_type = U64x3::ffi_type();
        for hidden in [false, true] {
            let plan = MarshalPlan::build(CallSignature::variadic(
                &[],
                &variadic,
                hidden.then_some(&return_type),
            ));
            let stack_arguments_end = SHADOW_SPACE_SIZE + if hidden { 16 } else { 8 };
            assert_indirect_copies(
                &plan,
                stack_arguments_end,
                &[
                    (2, size_of::<U8x3>()),
                    (3, size_of::<u128>()),
                    (4, size_of::<U64x2>()),
                ],
            );
        }
    }
}
