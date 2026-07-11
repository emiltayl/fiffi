pub mod structs;
pub mod unions;

use core::ffi::{CStr, c_char, c_void};

use crate::types::{FfiType, Type};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct Ptr(*const c_void);

// SAFETY: This struct is only intended for passing a pointers that will not be read or written
// to across FFI boundaries.
unsafe impl Send for Ptr {}

// SAFETY: This struct is only intended for passing a pointers that will not be read or written
// to across FFI boundaries.
unsafe impl Sync for Ptr {}

// SAFETY: `Ptr` is `repr(transparent)` with a single pointer field.
unsafe impl FfiType for Ptr {
    fn ffi_type() -> Type {
        Type::Pointer
    }
}

pub static I8_ARG: i8 = 0x11;
pub static U8_ARG: u8 = 0x22;
pub static I16_ARG: i16 = 0x3333;
pub static U16_ARG: u16 = 0x4444;
pub static I32_ARG: i32 = 0x5555_6666;
pub static U32_ARG: u32 = 0x6666_7777;
pub static I64_ARG: i64 = 0x7777_8888_9999_aaaa;
pub static U64_ARG: u64 = 0xbbbb_cccc_dddd_eeee;
pub static I128_ARG: i128 = 0x7777_6666_5555_4444_3333_2222_1111_ffff;
pub static U128_ARG: u128 = 0xaaaa_bbbb_cccc_dddd_eeee_ffff_0000_1111;
#[expect(
    clippy::cast_possible_truncation,
    reason = "Truncating is not a problem in this instance as we are comparing the argument with `ISIZE_ARG` anyways."
)]
pub static ISIZE_ARG: isize = I64_ARG as isize;
#[expect(
    clippy::cast_possible_truncation,
    reason = "Truncating is not a problem in this instance as we are comparing the argument with `USIZE_ARG` anyways."
)]
pub static USIZE_ARG: usize = U64_ARG as usize;
pub static PTR_ARG: Ptr = Ptr((&raw const U8_ARG).cast());
pub static F32_ARG: f32 = core::f32::consts::PI;
pub static F64_ARG: f64 = core::f64::consts::E;

pub static SNPRINTF_FORMAT: &CStr = c"1: %d, 2: %u, 3: %lld, 4: %llu, 5: \"%s\", 6: %.1f.\n";
pub static SNPRINTF_ARG_1: i32 = 1_234_567;
pub static SNPRINTF_ARG_2: u32 = 9_876_543;
pub static SNPRINTF_ARG_3: i64 = 12_345_678_900;
pub static SNPRINTF_ARG_4: u64 = 98_765_432_100;
pub static SNPRINTF_ARG_5: &CStr = c"This is a &CStr";
pub static SNPRINTF_ARG_6: f64 = 1.0;
pub static SNPRINTF_EXPECTED_OUTPUT: &CStr =
    c"1: 1234567, 2: 9876543, 3: 12345678900, 4: 98765432100, 5: \"This is a &CStr\", 6: 1.0.\n";
pub static SNPRINTF_EXPECTED_RETURN_VALUE: i32 = 86;

#[cfg_attr(target_env = "msvc", link(name = "legacy_stdio_definitions"))]
unsafe extern "C" {
    pub unsafe fn snprintf(s: *mut c_char, n: usize, format: *const c_char, ...) -> i32;
}
