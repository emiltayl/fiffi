//! ABI definitions for 64-bit x86.

mod sysv;

use core::mem::MaybeUninit;

use crate::types::Type;

/// ABI constants for 64-bit x86 targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Abi {
    #[default]
    SysV,
    // TODO implement after SysV
    //Win64,
}

impl Abi {
    #[cfg(test)]
    #[doc(hidden)]
    pub const ABIS: [Self; 1] = [Self::SysV];
}

#[derive(Clone, Debug)]
pub(crate) struct CallInterface {
    marshal_plan: AbiPlan,
}

impl CallInterface {
    pub(crate) fn new(argument_types: &[Type], return_type: Option<&Type>, abi: Abi) -> Self {
        let marshal_plan = AbiPlan::new(argument_types, return_type, abi);

        CallInterface { marshal_plan }
    }
}

#[derive(Clone, Debug)]
enum AbiPlan {
    SysV(sysv::MarshalPlan),
}

impl AbiPlan {
    pub(crate) fn new(argument_types: &[Type], return_type: Option<&Type>, abi: Abi) -> Self {
        match abi {
            Abi::SysV => Self::SysV(sysv::MarshalPlan::build(argument_types, return_type)),
        }
    }
}

#[derive(Debug)]
#[repr(align(8))]
struct Register([MaybeUninit<u8>; 8]);

// TODO default all 0
impl Default for Register {
    fn default() -> Self {
        Self([MaybeUninit::new(0u8); 8])
    }
}

const _: () = {
    assert!(size_of::<usize>() == size_of::<Register>());
    assert!(align_of::<usize>() == align_of::<Register>());
};
