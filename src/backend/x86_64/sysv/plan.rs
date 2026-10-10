extern crate alloc;

#[cfg(not(test))]
use alloc::{boxed::Box, vec::Vec};

use super::classification::ValueClass;
use crate::backend::CallSignature;
use crate::types::{FfiTypeLayout, TypeRef};

// Separate allocation arithmetic so boundary tests need neither huge type trees nor storage.
fn reserve_stack_argument(allocation_size: &mut usize, layout: FfiTypeLayout) -> usize {
    let offset = allocation_size
        .checked_next_multiple_of(layout.align)
        .expect("SysV stack allocation alignment overflow");
    *allocation_size = offset
        .strict_add(layout.size)
        .checked_next_multiple_of(8)
        .expect("SysV stack allocation rounding overflow");
    offset
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MarshalPlan {
    /// Argument copies and destinations.
    pub(super) argument_moves: Box<[ArgumentMove]>,

    /// Stack arguments and alignment padding in bytes, excluding discarded hidden-return storage.
    pub(super) stack_allocation_size: usize,

    /// How the function returns its value.
    pub(super) return_strategy: ReturnStrategy,

    /// The contents of the `al` register when calling the function. For variadic functions this
    /// must be the upper bound of how many `xmm` registers are used to pass arguments. It is set
    /// for regular functions as well to keep the code as simple as possible
    pub(super) al: u8,
}

impl MarshalPlan {
    pub(crate) fn build(signature: CallSignature<'_>) -> Self {
        let mut register_allocator = RegisterAllocator::default();

        let mut argument_moves = Vec::with_capacity(signature.argument_count());
        let mut stack_allocation_size: usize = 0;

        let return_strategy = ReturnStrategy::for_return_type(signature.return_type());

        // Reserve the first GPR for the hidden return pointer if needed.
        if matches!(return_strategy, ReturnStrategy::HiddenPointer { .. }) {
            register_allocator.allocate(RegisterRequirements::One(RegisterBank::Gpr));
        }

        for (source, argument) in signature.arguments().enumerate() {
            let argument_layout = argument.layout();
            debug_assert!(argument_layout.align <= 16);
            let argument_class = ValueClass::classify(argument, &argument_layout);

            let allocation = RegisterRequirements::for_value_class(argument_class)
                .and_then(|requirements| register_allocator.allocate(requirements));

            match allocation {
                None => {
                    // Stack arguments appear in argument order, starting at a 16-byte boundary.
                    let stack_offset =
                        reserve_stack_argument(&mut stack_allocation_size, argument_layout);
                    argument_moves.push(ArgumentMove::argument_to_stack(
                        source,
                        stack_offset,
                        argument_layout.size,
                    ));
                }
                Some(RegisterAllocation::One(destination)) => {
                    argument_moves.push(destination.argument_move(
                        source,
                        0,
                        argument_layout.size,
                        argument_layout.size,
                    ));
                }
                Some(RegisterAllocation::Two(first_destination, second_destination)) => {
                    // Two-eightbyte classes have payloads of 9 through 16 bytes.
                    debug_assert!((9..=16).contains(&argument_layout.size));
                    argument_moves.push(first_destination.argument_move(
                        source,
                        0,
                        8,
                        argument_layout.size,
                    ));

                    argument_moves.push(second_destination.argument_move(
                        source,
                        8,
                        argument_layout.size - 8,
                        argument_layout.size,
                    ));
                }
            }
        }

        MarshalPlan {
            argument_moves: argument_moves.into_boxed_slice(),
            stack_allocation_size,
            return_strategy,
            al: u8::try_from(register_allocator.next_xmm_index)
                .expect("Register allocator allocated > 255 xmm registers"),
        }
    }
}

/// Kind of argument bytes written before the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum ArgumentMoveKind {
    /// Copy part of an argument into a general-purpose register.
    Gpr = 0,

    /// Copy part of an argument into an XMM register.
    Xmm = 1,

    /// Copy the whole argument, starting at source offset zero, into the outgoing allocation.
    Stack = 2,
}

/// Argument copy performed before the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ArgumentMove {
    /// Argument index.
    pub(super) source: usize,

    /// Source bytes to copy.
    pub(super) size: usize,

    /// Bits 0..=2: `ArgumentMoveKind`.
    /// Stack: remaining bits hold the byte offset from pre-call `rsp`, without shadow space.
    /// Registers: bits 3..=5 hold the index, bit 6 holds source offset / 8; higher bits are zero.
    pub(super) destination: usize,
}

impl ArgumentMove {
    pub(super) const KIND_MASK: usize = 0b111;
    pub(super) const REGISTER_INDEX_SHIFT: usize = 3;
    pub(super) const REGISTER_INDEX_MASK: usize = 0b111;
    pub(super) const SOURCE_OFFSET_SHIFT: usize = 6;
    pub(super) const SOURCE_OFFSET_MASK: usize = 1;
    pub(super) const SOURCE_OFFSET_SCALE: usize = 8;

    fn argument_to_stack(source: usize, stack_offset: usize, size: usize) -> Self {
        // Offsets are relative to pre-call rsp; zero is valid and there is no shadow space.
        debug_assert_eq!(stack_offset & Self::KIND_MASK, 0);
        Self {
            source,
            size,
            destination: stack_offset | ArgumentMoveKind::Stack as usize,
        }
    }

    fn register_move(
        source: usize,
        index: usize,
        source_offset: usize,
        size: usize,
        kind: ArgumentMoveKind,
    ) -> Self {
        debug_assert!(matches!(
            kind,
            ArgumentMoveKind::Gpr | ArgumentMoveKind::Xmm
        ));
        debug_assert!(if kind == ArgumentMoveKind::Gpr {
            index < ARGUMENT_GPR_COUNT
        } else {
            index < ARGUMENT_XMM_COUNT
        });
        // Allocation bounds indices below eight, so their encoding shifts cannot overflow.
        debug_assert!(matches!(source_offset, 0 | 8));
        debug_assert!((1..=8).contains(&size));

        Self {
            source,
            size,
            destination: kind as usize
                | (index << Self::REGISTER_INDEX_SHIFT)
                | ((source_offset / Self::SOURCE_OFFSET_SCALE) << Self::SOURCE_OFFSET_SHIFT),
        }
    }
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
#[expect(
    variant_size_differences,
    reason = "Return type layout must be stored to support discarding return values."
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReturnStrategy {
    /// No return value.
    Void,

    /// Result written through a hidden pointer to caller-provided memory.
    HiddenPointer { size: usize, align_log2: u8 },

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
    fn for_return_type(return_type: Option<TypeRef<'_>>) -> Self {
        let Some(return_type) = return_type else {
            return Self::Void;
        };

        let return_layout = return_type.layout();

        let Some(register_requirements) = RegisterRequirements::for_value_class(
            ValueClass::classify(return_type, &return_layout),
        ) else {
            return Self::HiddenPointer {
                size: return_layout.size,
                align_log2: u8::try_from(return_layout.align.trailing_zeros())
                    .expect("`usize::trailing_zeros` will always fit inside an `u8`."),
            };
        };

        let byte_length =
            u8::try_from(return_layout.size).expect("register return types cannot exceed 16 bytes");

        match register_requirements {
            RegisterRequirements::One(bank) => Self::SingleRegister { bank, byte_length },
            RegisterRequirements::Two(first_bank, second_bank) => {
                // Two-eightbyte classes have payloads of 9 through 16 bytes.
                debug_assert!((9..=16).contains(&byte_length));
                Self::TwoRegisters {
                    first_bank,
                    second_bank,
                    second_byte_length: byte_length - 8,
                }
            }
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
    fn argument_move(
        self,
        source: usize,
        source_offset: usize,
        size: usize,
        argument_size: usize,
    ) -> ArgumentMove {
        debug_assert!(source_offset.strict_add(size) <= argument_size);

        let kind = match self.bank {
            RegisterBank::Gpr => ArgumentMoveKind::Gpr,
            RegisterBank::Xmm => ArgumentMoveKind::Xmm,
        };
        ArgumentMove::register_move(source, self.index, source_offset, size, kind)
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

        // Counters are at most six/eight and each requirement is at most two registers.
        (self.next_gpr_index + gpr_required) <= ARGUMENT_GPR_COUNT
            && (self.next_xmm_index + xmm_required) <= ARGUMENT_XMM_COUNT
    }

    fn take(&mut self, bank: RegisterBank) -> AllocatedRegister {
        // `allocate` checks both banks first, keeping these increments within their capacities.
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
    use std::panic::catch_unwind;

    use super::*;
    use crate::test_utils::structs::{F64x2, U64x3};
    use crate::types::{FfiType, Type, VariadicType};

    // Private allocation arithmetic cannot be reached with bounded real signatures.
    #[test]
    fn stack_allocation_checks_alignment_addition_and_eightbyte_rounding() {
        let reserve = |end, align, size| {
            let mut allocation = end;
            let offset = reserve_stack_argument(&mut allocation, FfiTypeLayout { align, size });
            (offset, allocation)
        };
        assert!(catch_unwind(|| reserve(usize::MAX - 6, 8, 1)).is_err());
        assert!(catch_unwind(|| reserve(usize::MAX - 15, 16, 16)).is_err());
        assert!(catch_unwind(|| reserve(usize::MAX - 7, 8, 1)).is_err());
        assert_eq!(reserve(1, 16, 9), (16, 32));
        assert_eq!(
            reserve(usize::MAX - 7, 8, 0),
            (usize::MAX - 7, usize::MAX - 7)
        );
        assert_eq!(
            reserve(usize::MAX - 15, 16, 8),
            (usize::MAX - 15, usize::MAX - 7)
        );
        assert_eq!(
            reserve(usize::MAX - 8, 1, 1),
            (usize::MAX - 8, usize::MAX - 7)
        );
    }

    #[test]
    fn variadic_al_counts_only_allocated_argument_vector_registers() {
        // The ABI permits any upper bound from actual vector-register usage through eight.
        // Exact counts deliberately test the current planner's stronger implementation contract.
        let hidden_return = U64x3::ffi_type();
        for (fixed_count, variadic_count, return_type, expected_al) in [
            (0, 0, None, 0),
            (1, 0, None, 1),
            (3, 5, None, 8),
            (3, 6, None, 8),
            (0, 0, Some(&Type::F64), 0),
            (0, 0, Some(&hidden_return), 0),
            (1, 1, Some(&hidden_return), 2),
        ] {
            let fixed = vec![Type::F64; fixed_count];
            let variadic = vec![VariadicType::F64; variadic_count];
            let plan = MarshalPlan::build(CallSignature::variadic(&fixed, &variadic, return_type));
            assert_eq!(
                plan.al, expected_al,
                "{fixed_count} fixed, {variadic_count} variadic, {return_type:?}"
            );
        }
        let fixed = vec![Type::F64; 7];
        let variadic = [
            VariadicType::try_from(F64x2::ffi_type()).unwrap(),
            VariadicType::F64,
        ];
        for return_type in [None, Some(&Type::F64), Some(&hidden_return)] {
            let plan = MarshalPlan::build(CallSignature::variadic(&fixed, &variadic, return_type));
            assert_eq!(
                plan.al, 8,
                "spilled pair must leave the last vector register available"
            );
        }
    }
}
