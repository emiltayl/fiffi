//! Test structs used to test ffi behavior.
//!
//! Defined structs in declaration order:
//! `U8`, `U8x3`, `U16x3`, `U32x2`, `U32x3`, `U32x4`, `U64`, `U64x2`, `U64x3`, `U64x4`, `U128`,
//! `U128x2`, `F32`, `F32x2`, `F32x3`, `F32x4`, `F64`, `F64x2`, `F64x3`, `F64x4`, `U64F64`,
//! `F64U64`, `U32F32`, `F32x3U32`, `U32F32x3`, `F64F32`, `U8U16`, `U8U64`, `U64U8`, `U8F64`,
//! `U8F64U8`, `U32U64U32`, `U8U128`, `U128U8`, `U8U128U8`, `NestedU8U32x2`, `NestedF32x2x2`,
//! `NestedF64x2x2`, `NestedU8U64x2`, `NestedUnionU32F32`, `NestedUnionU32F32x2`,
//! `NestedU8UnionU64F64`, `NestedUnionU8U128U8`, `NestedU8UnionU128U8`, `UsizePointer`

use core::ffi::c_void;
use core::ptr;

use crate::test_utils::unions::{UnionU128U8, UnionU32F32, UnionU64F64, UnionU8U128};
use crate::types::{FfiType, Type};

macro_rules! ffi_struct_type {
    ($($field:expr),+ $(,)?) => {{
        let fields = vec![$($field),+];

        // SAFETY: Every generated struct type description in this file is non-empty.
        unsafe { Type::create_struct_unchecked(fields) }
    }};
}

macro_rules! impl_ffi_struct {
    ($type:ty, $($field:expr),+ $(,)?) => {
        // SAFETY: The target type is a `#[repr(C)]` `Copy` struct, and the type list matches its
        // C field order.
        unsafe impl FfiType for $type {
            fn ffi_type() -> Type {
                ffi_struct_type!($($field),+)
            }
        }
    };
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U8 {
    pub a: u8,
}

impl_ffi_struct!(U8, Type::U8);

pub static U8_ARG: U8 = U8 { a: 0x11 };

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U8x3 {
    pub a: u8,
    pub b: u8,
    pub c: u8,
}

impl_ffi_struct!(U8x3, Type::U8, Type::U8, Type::U8);

pub static U8X3_ARG: U8x3 = U8x3 {
    a: 0x12,
    b: 0x13,
    c: 0x14,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U16x3 {
    pub a: u16,
    pub b: u16,
    pub c: u16,
}

impl_ffi_struct!(U16x3, Type::U16, Type::U16, Type::U16);

pub static U16X3_ARG: U16x3 = U16x3 {
    a: 0x2001,
    b: 0x2002,
    c: 0x2003,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U32x2 {
    pub a: u32,
    pub b: u32,
}

impl_ffi_struct!(U32x2, Type::U32, Type::U32);

pub static U32X2_ARG: U32x2 = U32x2 {
    a: 0x3000_0001,
    b: 0x3000_0002,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U32x3 {
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

impl_ffi_struct!(U32x3, Type::U32, Type::U32, Type::U32);

pub static U32X3_ARG: U32x3 = U32x3 {
    a: 0x3000_0003,
    b: 0x3000_0004,
    c: 0x3000_0005,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U32x4 {
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub d: u32,
}

impl_ffi_struct!(U32x4, Type::U32, Type::U32, Type::U32, Type::U32);

pub static U32X4_ARG: U32x4 = U32x4 {
    a: 0x3000_0006,
    b: 0x3000_0007,
    c: 0x3000_0008,
    d: 0x3000_0009,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U64 {
    pub a: u64,
}

impl_ffi_struct!(U64, Type::U64);

pub static U64_ARG: U64 = U64 {
    a: 0x4000_0000_0000_0001,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U64x2 {
    pub a: u64,
    pub b: u64,
}

impl_ffi_struct!(U64x2, Type::U64, Type::U64);

pub static U64X2_ARG: U64x2 = U64x2 {
    a: 0x4000_0000_0000_0002,
    b: 0x4000_0000_0000_0003,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U64x3 {
    pub a: u64,
    pub b: u64,
    pub c: u64,
}

impl_ffi_struct!(U64x3, Type::U64, Type::U64, Type::U64);

pub static U64X3_ARG: U64x3 = U64x3 {
    a: 0x4000_0000_0000_0004,
    b: 0x4000_0000_0000_0005,
    c: 0x4000_0000_0000_0006,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U64x4 {
    pub a: u64,
    pub b: u64,
    pub c: u64,
    pub d: u64,
}

impl_ffi_struct!(U64x4, Type::U64, Type::U64, Type::U64, Type::U64);

pub static U64X4_ARG: U64x4 = U64x4 {
    a: 0x4000_0000_0000_0007,
    b: 0x4000_0000_0000_0008,
    c: 0x4000_0000_0000_0009,
    d: 0x4000_0000_0000_000a,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U128 {
    pub a: u128,
}

impl_ffi_struct!(U128, Type::U128);

pub static U128_ARG: U128 = U128 {
    a: 0x6000_0000_0000_0000_0000_0000_0000_0001,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U128x2 {
    pub a: u128,
    pub b: u128,
}

impl_ffi_struct!(U128x2, Type::U128, Type::U128);

pub static U128X2_ARG: U128x2 = U128x2 {
    a: 0x6000_0000_0000_0000_0000_0000_0000_0002,
    b: 0x6000_0000_0000_0000_0000_0000_0000_0003,
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct F32 {
    pub a: f32,
}

impl_ffi_struct!(F32, Type::F32);

pub static F32_ARG: F32 = F32 {
    a: f32::from_bits(0x3f80_0001),
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct F32x2 {
    pub a: f32,
    pub b: f32,
}

impl_ffi_struct!(F32x2, Type::F32, Type::F32);

pub static F32X2_ARG: F32x2 = F32x2 {
    a: f32::from_bits(0x3f80_0002),
    b: f32::from_bits(0x3f80_0003),
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct F32x3 {
    pub a: f32,
    pub b: f32,
    pub c: f32,
}

impl_ffi_struct!(F32x3, Type::F32, Type::F32, Type::F32);

pub static F32X3_ARG: F32x3 = F32x3 {
    a: f32::from_bits(0x3f80_0004),
    b: f32::from_bits(0x3f80_0005),
    c: f32::from_bits(0x3f80_0006),
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct F32x4 {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}

impl_ffi_struct!(F32x4, Type::F32, Type::F32, Type::F32, Type::F32);

pub static F32X4_ARG: F32x4 = F32x4 {
    a: f32::from_bits(0x3f80_0007),
    b: f32::from_bits(0x3f80_0008),
    c: f32::from_bits(0x3f80_0009),
    d: f32::from_bits(0x3f80_000a),
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct F64 {
    pub a: f64,
}

impl_ffi_struct!(F64, Type::F64);

pub static F64_ARG: F64 = F64 {
    a: f64::from_bits(0x3ff0_0000_0000_0001),
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct F64x2 {
    pub a: f64,
    pub b: f64,
}

impl_ffi_struct!(F64x2, Type::F64, Type::F64);

pub static F64X2_ARG: F64x2 = F64x2 {
    a: f64::from_bits(0x3ff0_0000_0000_0002),
    b: f64::from_bits(0x3ff0_0000_0000_0003),
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct F64x3 {
    pub a: f64,
    pub b: f64,
    pub c: f64,
}

impl_ffi_struct!(F64x3, Type::F64, Type::F64, Type::F64);

pub static F64X3_ARG: F64x3 = F64x3 {
    a: f64::from_bits(0x3ff0_0000_0000_0004),
    b: f64::from_bits(0x3ff0_0000_0000_0005),
    c: f64::from_bits(0x3ff0_0000_0000_0006),
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct F64x4 {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
}

impl_ffi_struct!(F64x4, Type::F64, Type::F64, Type::F64, Type::F64);

pub static F64X4_ARG: F64x4 = F64x4 {
    a: f64::from_bits(0x3ff0_0000_0000_0007),
    b: f64::from_bits(0x3ff0_0000_0000_0008),
    c: f64::from_bits(0x3ff0_0000_0000_0009),
    d: f64::from_bits(0x3ff0_0000_0000_000a),
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct U64F64 {
    pub a: u64,
    pub b: f64,
}

impl_ffi_struct!(U64F64, Type::U64, Type::F64);

pub static U64_F64_ARG: U64F64 = U64F64 {
    a: 0x4000_0000_0000_000b,
    b: f64::from_bits(0x3ff0_0000_0000_000b),
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct F64U64 {
    pub a: f64,
    pub b: u64,
}

impl_ffi_struct!(F64U64, Type::F64, Type::U64);

pub static F64_U64_ARG: F64U64 = F64U64 {
    a: f64::from_bits(0x3ff0_0000_0000_000c),
    b: 0x4000_0000_0000_000c,
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct U32F32 {
    pub a: u32,
    pub b: f32,
}

impl_ffi_struct!(U32F32, Type::U32, Type::F32);

pub static U32_F32_ARG: U32F32 = U32F32 {
    a: 0x3000_000a,
    b: f32::from_bits(0x3f80_000b),
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct F32x3U32 {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: u32,
}

impl_ffi_struct!(F32x3U32, Type::F32, Type::F32, Type::F32, Type::U32);

pub static F32X3_U32_ARG: F32x3U32 = F32x3U32 {
    a: f32::from_bits(0x3f80_000c),
    b: f32::from_bits(0x3f80_000d),
    c: f32::from_bits(0x3f80_000e),
    d: 0x3000_000b,
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct U32F32x3 {
    pub a: u32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}

impl_ffi_struct!(U32F32x3, Type::U32, Type::F32, Type::F32, Type::F32);

pub static U32_F32X3_ARG: U32F32x3 = U32F32x3 {
    a: 0x3000_000c,
    b: f32::from_bits(0x3f80_000f),
    c: f32::from_bits(0x3f80_0010),
    d: f32::from_bits(0x3f80_0011),
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct F64F32 {
    pub a: f64,
    pub b: f32,
}

impl_ffi_struct!(F64F32, Type::F64, Type::F32);

pub static F64_F32_ARG: F64F32 = F64F32 {
    a: f64::from_bits(0x3ff0_0000_0000_000d),
    b: f32::from_bits(0x3f80_0012),
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U8U16 {
    pub a: u8,
    pub b: u16,
}

impl_ffi_struct!(U8U16, Type::U8, Type::U16);

pub static U8_U16_ARG: U8U16 = U8U16 { a: 0x15, b: 0x2004 };

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U8U64 {
    pub a: u8,
    pub b: u64,
}

impl_ffi_struct!(U8U64, Type::U8, Type::U64);

pub static U8_U64_ARG: U8U64 = U8U64 {
    a: 0x16,
    b: 0x4000_0000_0000_000d,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U64U8 {
    pub a: u64,
    pub b: u8,
}

impl_ffi_struct!(U64U8, Type::U64, Type::U8);

pub static U64_U8_ARG: U64U8 = U64U8 {
    a: 0x4000_0000_0000_000e,
    b: 0x17,
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct U8F64 {
    pub a: u8,
    pub b: f64,
}

impl_ffi_struct!(U8F64, Type::U8, Type::F64);

pub static U8_F64_ARG: U8F64 = U8F64 {
    a: 0x18,
    b: f64::from_bits(0x3ff0_0000_0000_000e),
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct U8F64U8 {
    pub a: u8,
    pub b: f64,
    pub c: u8,
}

impl_ffi_struct!(U8F64U8, Type::U8, Type::F64, Type::U8);

pub static U8_F64_U8_ARG: U8F64U8 = U8F64U8 {
    a: 0x19,
    b: f64::from_bits(0x3ff0_0000_0000_000f),
    c: 0x1a,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U32U64U32 {
    pub a: u32,
    pub b: u64,
    pub c: u32,
}

impl_ffi_struct!(U32U64U32, Type::U32, Type::U64, Type::U32);

pub static U32_U64_U32_ARG: U32U64U32 = U32U64U32 {
    a: 0x3000_000d,
    b: 0x4000_0000_0000_000f,
    c: 0x3000_000e,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U8U128 {
    pub a: u8,
    pub b: u128,
}

impl_ffi_struct!(U8U128, Type::U8, Type::U128);

pub static U8_U128_ARG: U8U128 = U8U128 {
    a: 0x1b,
    b: 0x6000_0000_0000_0000_0000_0000_0000_0004,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U128U8 {
    pub a: u128,
    pub b: u8,
}

impl_ffi_struct!(U128U8, Type::U128, Type::U8);

pub static U128_U8_ARG: U128U8 = U128U8 {
    a: 0x6000_0000_0000_0000_0000_0000_0000_0005,
    b: 0x1c,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct U8U128U8 {
    pub a: u8,
    pub b: u128,
    pub c: u8,
}

impl_ffi_struct!(U8U128U8, Type::U8, Type::U128, Type::U8);

pub static U8_U128_U8_ARG: U8U128U8 = U8U128U8 {
    a: 0x1d,
    b: 0x6000_0000_0000_0000_0000_0000_0000_0006,
    c: 0x1e,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct NestedU8U32x2 {
    pub tag: u8,
    pub x: U32x2,
}

impl_ffi_struct!(NestedU8U32x2, Type::U8, U32x2::ffi_type());

pub static NESTED_U8_U32X2_ARG: NestedU8U32x2 = NestedU8U32x2 {
    tag: 0x1f,
    x: U32x2 {
        a: 0x3000_000f,
        b: 0x3000_0010,
    },
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct NestedF32x2x2 {
    pub x: F32x2,
    pub y: F32x2,
}

impl_ffi_struct!(NestedF32x2x2, F32x2::ffi_type(), F32x2::ffi_type());

pub static NESTED_F32X2X2_ARG: NestedF32x2x2 = NestedF32x2x2 {
    x: F32x2 {
        a: f32::from_bits(0x3f80_0013),
        b: f32::from_bits(0x3f80_0014),
    },
    y: F32x2 {
        a: f32::from_bits(0x3f80_0015),
        b: f32::from_bits(0x3f80_0016),
    },
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct NestedF64x2x2 {
    pub x: F64x2,
    pub y: F64x2,
}

impl_ffi_struct!(NestedF64x2x2, F64x2::ffi_type(), F64x2::ffi_type());

pub static NESTED_F64X2X2_ARG: NestedF64x2x2 = NestedF64x2x2 {
    x: F64x2 {
        a: f64::from_bits(0x3ff0_0000_0000_0010),
        b: f64::from_bits(0x3ff0_0000_0000_0011),
    },
    y: F64x2 {
        a: f64::from_bits(0x3ff0_0000_0000_0012),
        b: f64::from_bits(0x3ff0_0000_0000_0013),
    },
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct NestedU8U64x2 {
    pub tag: u8,
    pub x: U64x2,
}

impl_ffi_struct!(NestedU8U64x2, Type::U8, U64x2::ffi_type());

pub static NESTED_U8_U64X2_ARG: NestedU8U64x2 = NestedU8U64x2 {
    tag: 0x20,
    x: U64x2 {
        a: 0x4000_0000_0000_0010,
        b: 0x4000_0000_0000_0011,
    },
};

#[derive(Copy, Clone, PartialEq)]
#[repr(C)]
pub struct NestedUnionU32F32 {
    pub x: UnionU32F32,
}

impl_ffi_struct!(NestedUnionU32F32, UnionU32F32::ffi_type());

pub static NESTED_UNION_U32_F32_ARG: NestedUnionU32F32 = NestedUnionU32F32 {
    x: UnionU32F32 { i: 0x3000_0017 },
};

#[derive(Copy, Clone, PartialEq)]
#[repr(C)]
pub struct NestedUnionU32F32x2 {
    pub x: UnionU32F32,
    pub y: UnionU32F32,
}

impl_ffi_struct!(
    NestedUnionU32F32x2,
    UnionU32F32::ffi_type(),
    UnionU32F32::ffi_type()
);

pub static NESTED_UNION_U32_F32X2_ARG: NestedUnionU32F32x2 = NestedUnionU32F32x2 {
    x: UnionU32F32 { i: 0x3000_0018 },
    y: UnionU32F32 { i: 0x3000_0019 },
};

#[derive(Copy, Clone, PartialEq)]
#[repr(C)]
pub struct NestedU8UnionU64F64 {
    pub tag: u8,
    pub x: UnionU64F64,
}

impl_ffi_struct!(NestedU8UnionU64F64, Type::U8, UnionU64F64::ffi_type());

pub static NESTED_U8_UNION_U64_F64_ARG: NestedU8UnionU64F64 = NestedU8UnionU64F64 {
    tag: 0x26,
    x: UnionU64F64 {
        f: f64::from_bits(0x3ff0_0000_0000_001d),
    },
};

#[derive(Copy, Clone, PartialEq)]
#[repr(C)]
pub struct NestedUnionU8U128U8 {
    pub x: UnionU8U128,
    pub tail: u8,
}

impl_ffi_struct!(NestedUnionU8U128U8, UnionU8U128::ffi_type(), Type::U8);

pub static NESTED_UNION_U8_U128_U8_ARG: NestedUnionU8U128U8 = NestedUnionU8U128U8 {
    x: UnionU8U128 { small: 0x27 },
    tail: 0x28,
};

#[derive(Copy, Clone, PartialEq)]
#[repr(C)]
pub struct NestedU8UnionU128U8 {
    pub tag: u8,
    pub x: UnionU128U8,
}

impl_ffi_struct!(NestedU8UnionU128U8, Type::U8, UnionU128U8::ffi_type());

pub static NESTED_U8_UNION_U128_U8_ARG: NestedU8UnionU128U8 = NestedU8UnionU128U8 {
    tag: 0x29,
    x: UnionU128U8 {
        big: 0x6000_0000_0000_0000_0000_0000_0000_0009,
    },
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct UsizePointer {
    pub size: usize,
    pub pointer: *const c_void,
}

// SAFETY: `UsizePointer` is only used for testing, pointers should never be used for reading or
// writing memory.
unsafe impl Send for UsizePointer {}

// SAFETY: `UsizePointer` is only used for testing, pointers should never be used for reading or
// writing memory.
unsafe impl Sync for UsizePointer {}

impl_ffi_struct!(UsizePointer, Type::Usize, Type::Pointer);

pub static USIZE_POINTER_ARG: UsizePointer = UsizePointer {
    size: 0x5000_0001,
    pointer: ptr::without_provenance::<c_void>(0x5000_0002),
};
