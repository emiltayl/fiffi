use super::ArgumentDestination;
use super::classification::ArgumentClass;

const GPR_ARGUMENT_REGISTER_COUNT: usize = 6;
const XMM_ARGUMENT_REGISTER_COUNT: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RegisterBank {
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
pub(super) enum RegisterRequirements {
    One(RegisterBank),
    Two(RegisterBank, RegisterBank),
}

impl RegisterRequirements {
    pub(super) fn for_argument_class(argument_class: ArgumentClass) -> Option<Self> {
        use RegisterBank::{Gpr, Xmm};
        use RegisterRequirements::{One, Two};

        match argument_class {
            ArgumentClass::Integer => Some(One(Gpr)),
            ArgumentClass::IntegerInteger => Some(Two(Gpr, Gpr)),
            ArgumentClass::IntegerSse => Some(Two(Gpr, Xmm)),
            ArgumentClass::Sse => Some(One(Xmm)),
            ArgumentClass::SseSse => Some(Two(Xmm, Xmm)),
            ArgumentClass::SseInteger => Some(Two(Xmm, Gpr)),
            ArgumentClass::Memory => None,
        }
    }

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
pub(super) enum RegisterAllocation {
    One(ArgumentDestination),
    Two(ArgumentDestination, ArgumentDestination),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct RegisterAllocator {
    next_gpr_index: usize,
    next_xmm_index: usize,
}

impl RegisterAllocator {
    pub(super) fn allocate(
        &mut self,
        requirements: RegisterRequirements,
    ) -> Option<RegisterAllocation> {
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
