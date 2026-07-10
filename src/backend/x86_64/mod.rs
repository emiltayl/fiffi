//! ABI definitions for 64-bit x86.

mod sysv;

extern crate alloc;

use alloc::vec::Vec;
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
    args: Vec<Type>,
    return_type: Option<Type>,
    marshal_plan: MarshalPlan,
}

#[derive(Clone, Debug)]
enum MarshalPlan {
    SysVPlan(sysv::MarshalPlan),
}

#[repr(align(8))]
struct Register(MaybeUninit<[u8; 8]>);

#[repr(align(16))]
struct FloatRegister(MaybeUninit<[u8; 8]>);

const _: () = {
    assert!(size_of::<usize>() == size_of::<Register>());
    assert!(align_of::<usize>() == align_of::<Register>());
    assert!(size_of::<usize>() <= size_of::<FloatRegister>());
    assert!(align_of::<usize>() <= align_of::<FloatRegister>());
};
