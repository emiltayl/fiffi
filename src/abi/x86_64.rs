//! ABI definitions for 64-bit x86.

/// ABI constants for 64-bit x86 targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Abi {
    SysV,
    // TODO implement after SysV
    //Win64,
}

impl Abi {
    #[cfg(test)]
    #[doc(hidden)]
    pub const ABIS: [Self; 1] = [Self::SysV];
}

impl Default for Abi {
    fn default() -> Self {
        Self::SysV
    }
}
