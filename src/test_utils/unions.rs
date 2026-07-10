//! Test unions used to test ffi behavior.
//!
//! These unions implement `PartialEq` for test purposes. The `PartialEq` implementations must only
//! be used to compare unions created from the static union ARGs defined in this module to ensure
//! that the correct variant is compared. Comparing with unions constructed in other manners may
//! cause undefined behavior.
//!
//! Defined unions in declaration order:
//! `UnionI32U32`, `UnionI64U64`, `UnionU128`, `UnionU8U128`, `UnionU128U8`, `UnionU32F32`,
//! `UnionU64F64`, `UnionNestedU8x3U64`, `UnionNestedU8x3F32x2`,
//! `UnionNestedU16x3F64x2`, `UnionNestedU64x2`, `UnionNestedF64x2`, `UnionNestedU8U16U64`,
//! `UnionNestedU64F64`, `UnionNestedF32x4U32x4`, `UnionNestedF64x2U64x2`,
//! `UnionNestedF32x2U64`, `UnionNestedF64x4U64x4`, `UnionNestedU64x4F64x4`

use crate::test_utils::structs::{
    F32x2, F32x4, F64, F64x2, F64x4, U8U16, U8x3, U16x3, U32x4, U64, U64x2, U64x4,
};
use crate::types::{FfiType, Type};

macro_rules! ffi_union_type {
    ($($variant:expr),+ $(,)?) => {{
        let variants = vec![$($variant),+];

        // SAFETY: Every generated union type description in this file is non-empty.
        unsafe { Type::create_union_unchecked(variants) }
    }};
}

macro_rules! impl_ffi_union {
    ($type:ty, $($variant:expr),+ $(,)?) => {
        // SAFETY: The target type is a `#[repr(C)]` `Copy` union, and the type list matches its C
        // variants with array variants represented as aggregate storage.
        unsafe impl FfiType for $type {
            fn ffi_type() -> Type {
                ffi_union_type!($($variant),+)
            }
        }
    };
}

// These equality implementations are fixture-specific. Each union is compared through the same
// variant used by its `_ARG` constant.
macro_rules! impl_union_partial_eq {
    ($type:ty, $field:ident) => {
        impl PartialEq for $type {
            fn eq(&self, other: &Self) -> bool {
                // SAFETY: The test fixtures for this union are initialized through this variant.
                unsafe { (*self).$field == (*other).$field }
            }
        }
    };
}

macro_rules! impl_union_partial_eq_float_bits {
    ($type:ty, $field:ident) => {
        impl PartialEq for $type {
            fn eq(&self, other: &Self) -> bool {
                // SAFETY: The test fixtures for this union are initialized through this variant.
                unsafe { (*self).$field.to_bits() == (*other).$field.to_bits() }
            }
        }
    };
}

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionI32U32 {
    pub i: i32,
    pub u: u32,
}

impl_ffi_union!(UnionI32U32, Type::I32, Type::U32);
impl_union_partial_eq!(UnionI32U32, u);

pub static UNION_I32_U32_ARG: UnionI32U32 = UnionI32U32 { u: 0x3000_0011 };

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionI64U64 {
    pub i: i64,
    pub u: u64,
}

impl_ffi_union!(UnionI64U64, Type::I64, Type::U64);
impl_union_partial_eq!(UnionI64U64, u);

pub static UNION_I64_U64_ARG: UnionI64U64 = UnionI64U64 {
    u: 0x4000_0000_0000_0012,
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionU128 {
    pub u: u128,
}

impl_ffi_union!(UnionU128, Type::U128);
impl_union_partial_eq!(UnionU128, u);

pub static UNION_U128_ARG: UnionU128 = UnionU128 {
    u: 0x6000_0000_0000_0000_0000_0000_0000_0007,
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionU8U128 {
    pub small: u8,
    pub big: u128,
}

impl_ffi_union!(UnionU8U128, Type::U8, Type::U128);
impl_union_partial_eq!(UnionU8U128, small);

pub static UNION_U8_U128_ARG: UnionU8U128 = UnionU8U128 { small: 0x21 };

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionU128U8 {
    pub big: u128,
    pub small: u8,
}

impl_ffi_union!(UnionU128U8, Type::U128, Type::U8);
impl_union_partial_eq!(UnionU128U8, big);

pub static UNION_U128_U8_ARG: UnionU128U8 = UnionU128U8 {
    big: 0x6000_0000_0000_0000_0000_0000_0000_0008,
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionU32F32 {
    pub i: u32,
    pub f: f32,
}

impl_ffi_union!(UnionU32F32, Type::U32, Type::F32);
impl_union_partial_eq!(UnionU32F32, i);

pub static UNION_U32_F32_ARG: UnionU32F32 = UnionU32F32 { i: 0x3000_0012 };

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionU64F64 {
    pub i: u64,
    pub f: f64,
}

impl_ffi_union!(UnionU64F64, Type::U64, Type::F64);
impl_union_partial_eq_float_bits!(UnionU64F64, f);

pub static UNION_U64_F64_ARG: UnionU64F64 = UnionU64F64 {
    f: f64::from_bits(0x3ff0_0000_0000_0014),
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionNestedU8x3U64 {
    pub small: U8x3,
    pub big: u64,
}

impl_ffi_union!(UnionNestedU8x3U64, U8x3::ffi_type(), Type::U64);
impl_union_partial_eq!(UnionNestedU8x3U64, small);

pub static UNION_NESTED_U8X3_U64_ARG: UnionNestedU8x3U64 = UnionNestedU8x3U64 {
    small: U8x3 {
        a: 0x22,
        b: 0x23,
        c: 0x24,
    },
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionNestedU8x3F32x2 {
    pub small: U8x3,
    pub f: F32x2,
}

impl_ffi_union!(UnionNestedU8x3F32x2, U8x3::ffi_type(), F32x2::ffi_type());
impl_union_partial_eq!(UnionNestedU8x3F32x2, f);

pub static UNION_NESTED_U8X3_F32X2_ARG: UnionNestedU8x3F32x2 = UnionNestedU8x3F32x2 {
    f: F32x2 {
        a: f32::from_bits(0x3f80_0019),
        b: f32::from_bits(0x3f80_001a),
    },
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionNestedU16x3F64x2 {
    pub small: U16x3,
    pub f: F64x2,
}

impl_ffi_union!(UnionNestedU16x3F64x2, U16x3::ffi_type(), F64x2::ffi_type());
impl_union_partial_eq!(UnionNestedU16x3F64x2, f);

pub static UNION_NESTED_U16X3_F64X2_ARG: UnionNestedU16x3F64x2 = UnionNestedU16x3F64x2 {
    f: F64x2 {
        a: f64::from_bits(0x3ff0_0000_0000_001e),
        b: f64::from_bits(0x3ff0_0000_0000_001f),
    },
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionNestedU64x2 {
    pub u: U64x2,
}

impl_ffi_union!(UnionNestedU64x2, U64x2::ffi_type());
impl_union_partial_eq!(UnionNestedU64x2, u);

pub static UNION_NESTED_U64X2_ARG: UnionNestedU64x2 = UnionNestedU64x2 {
    u: U64x2 {
        a: 0x4000_0000_0000_0013,
        b: 0x4000_0000_0000_0014,
    },
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionNestedF64x2 {
    pub f: F64x2,
}

impl_ffi_union!(UnionNestedF64x2, F64x2::ffi_type());
impl_union_partial_eq!(UnionNestedF64x2, f);

pub static UNION_NESTED_F64X2_ARG: UnionNestedF64x2 = UnionNestedF64x2 {
    f: F64x2 {
        a: f64::from_bits(0x3ff0_0000_0000_0015),
        b: f64::from_bits(0x3ff0_0000_0000_0016),
    },
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionNestedU8U16U64 {
    pub s: U8U16,
    pub u: u64,
}

impl_ffi_union!(UnionNestedU8U16U64, U8U16::ffi_type(), Type::U64);
impl_union_partial_eq!(UnionNestedU8U16U64, s);

pub static UNION_NESTED_U8_U16_U64_ARG: UnionNestedU8U16U64 = UnionNestedU8U16U64 {
    s: U8U16 { a: 0x25, b: 0x2005 },
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionNestedU64F64 {
    pub i: U64,
    pub f: F64,
}

impl_ffi_union!(UnionNestedU64F64, U64::ffi_type(), F64::ffi_type());
impl_union_partial_eq!(UnionNestedU64F64, i);

pub static UNION_NESTED_U64_F64_ARG: UnionNestedU64F64 = UnionNestedU64F64 {
    i: U64 {
        a: 0x4000_0000_0000_0015,
    },
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionNestedF32x4U32x4 {
    pub f: F32x4,
    pub i: U32x4,
}

impl_ffi_union!(UnionNestedF32x4U32x4, F32x4::ffi_type(), U32x4::ffi_type());
impl_union_partial_eq!(UnionNestedF32x4U32x4, i);

pub static UNION_NESTED_F32X4_U32X4_ARG: UnionNestedF32x4U32x4 = UnionNestedF32x4U32x4 {
    i: U32x4 {
        a: 0x3000_0013,
        b: 0x3000_0014,
        c: 0x3000_0015,
        d: 0x3000_0016,
    },
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionNestedF64x2U64x2 {
    pub f: F64x2,
    pub i: U64x2,
}

impl_ffi_union!(UnionNestedF64x2U64x2, F64x2::ffi_type(), U64x2::ffi_type());
impl_union_partial_eq!(UnionNestedF64x2U64x2, f);

pub static UNION_NESTED_F64X2_U64X2_ARG: UnionNestedF64x2U64x2 = UnionNestedF64x2U64x2 {
    f: F64x2 {
        a: f64::from_bits(0x3ff0_0000_0000_0017),
        b: f64::from_bits(0x3ff0_0000_0000_0018),
    },
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionNestedF32x2U64 {
    pub f: F32x2,
    pub i: U64,
}

impl_ffi_union!(UnionNestedF32x2U64, F32x2::ffi_type(), U64::ffi_type());
impl_union_partial_eq!(UnionNestedF32x2U64, f);

pub static UNION_NESTED_F32X2_U64_ARG: UnionNestedF32x2U64 = UnionNestedF32x2U64 {
    f: F32x2 {
        a: f32::from_bits(0x3f80_0017),
        b: f32::from_bits(0x3f80_0018),
    },
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionNestedF64x4U64x4 {
    pub f: F64x4,
    pub i: U64x4,
}

impl_ffi_union!(UnionNestedF64x4U64x4, F64x4::ffi_type(), U64x4::ffi_type());
impl_union_partial_eq!(UnionNestedF64x4U64x4, f);

pub static UNION_NESTED_F64X4_U64X4_ARG: UnionNestedF64x4U64x4 = UnionNestedF64x4U64x4 {
    f: F64x4 {
        a: f64::from_bits(0x3ff0_0000_0000_0019),
        b: f64::from_bits(0x3ff0_0000_0000_001a),
        c: f64::from_bits(0x3ff0_0000_0000_001b),
        d: f64::from_bits(0x3ff0_0000_0000_001c),
    },
};

#[derive(Copy, Clone)]
#[repr(C)]
pub union UnionNestedU64x4F64x4 {
    pub i: U64x4,
    pub f: F64x4,
}

impl_ffi_union!(UnionNestedU64x4F64x4, U64x4::ffi_type(), F64x4::ffi_type());
impl_union_partial_eq!(UnionNestedU64x4F64x4, i);

pub static UNION_NESTED_U64X4_F64X4_ARG: UnionNestedU64x4F64x4 = UnionNestedU64x4F64x4 {
    i: U64x4 {
        a: 0x4000_0000_0000_0016,
        b: 0x4000_0000_0000_0017,
        c: 0x4000_0000_0000_0018,
        d: 0x4000_0000_0000_0019,
    },
};
