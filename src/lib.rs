//! Alternative Rust bindings for libffi.
//!
//! This crate is an alternative to the [libffi-rs crate][libffi-rs]. The interface is similar, but
//! `fiffi` aims to be a slimmer version of `libffi-rs` with a smaller public interface and higher
//! test coverage. `fiffi` uses [`libffi-sys-rs`][libffi-sys-rs] for building and linking to the
//! underlying libffi library.
//!
//! There are three main uses for this crate:
//!
//! * Calling foreign functions that are not available at compile time using [`Function`]
//! * Creating function pointers to Rust closures using [`Closure`]
//! * Creating function pointers with arbitrary signatures chosen at run time using
//!   [`DynamicClosure`]
//!
//! # Examples
//!
//! ## Calling an FFI function with [`Function`]:
//!
//! ```
//! use fiffi::function::{Function, arg, ret};
//! use fiffi::types::Type;
//!
//! extern "C" fn add(a: i32, b: i32) -> i32 {
//!     a + b
//! }
//!
//! let function = Function::new(
//!     fiffi::fn_ptrize!(add),
//!     &[Type::I32, Type::I32],
//!     Some(&Type::I32),
//! );
//!
//! let a = 19i32;
//! let b = 23i32;
//! let mut result = 0i32;
//!
//! // SAFETY: `function` was built from `add` with matching argument and return types.
//! unsafe {
//!     function.call([arg(&a), arg(&b)], ret(&mut result));
//! }
//!
//! assert_eq!(result, 42);
//! ```
//!
//! ## Converting a Rust closure to a function pointer with [`Closure`]:
//!
//! ```
//! # #[cfg(feature = "closure")]
//! # {
//! use fiffi::closure::Closure;
//!
//! let offset = 5i32;
//! let add_offset = Closure::new(move |value: i32| value + offset);
//!
//! // `add_offset.as_fn_ptr()` can now be called to get a function pointer to a function that adds
//! // `offset` to the `i32` argument it receives and returns the result.
//!
//! // SAFETY: `add_offset` accepts one `i32` argument, returns `i32`, and remains alive while the
//! // function pointer is used.
//! let add_offset_fn = unsafe {
//!     add_offset
//!         .as_fn_ptr()
//!         .into_fn::<extern "C" fn(i32) -> i32>()
//! };
//!
//! assert_eq!(add_offset_fn(37), 42);
//! # }
//! ```
//!
//! ## Creating a function pointer with a signature defined at run time using [`DynamicClosure`]:
//!
//! ```
//! # #[cfg(feature = "closure")]
//! # {
//! use core::mem::MaybeUninit;
//!
//! use fiffi::closure::DynamicClosure;
//! use fiffi::closure::dynamic::DynamicClosureCall;
//! use fiffi::types::Type;
//!
//! fn add_offset_callback(mut call: DynamicClosureCall<i32>) {
//!     let arg = call
//!         .args()
//!         .get(0)
//!         .expect("This callback must be used with exactly one argument.");
//!
//!     if arg.ty() != &Type::I32 {
//!         // **NOTE** This will abort instead of unwinding regardless of the crate's setting
//!         // because Rust cannot unwind past libffi's `extern "C"` functions.
//!         panic!("This callback can only be used with an `i32` argument.");
//!     }
//!
//!     let mut value = MaybeUninit::<i32>::uninit();
//!
//!     // SAFETY: The argument's type is `i32`. The `copy_to` fully initializes `value`.
//!     let result = unsafe {
//!         arg.copy_to(&mut value);
//!         value.assume_init()
//!     } + call.context();
//!
//!     let ret = call.ret().expect("This callback expects a return value.");
//!
//!     if ret.ty() != &Type::I32 {
//!         // **NOTE** This will abort instead of unwinding regardless of the crate's setting
//!         // because Rust cannot unwind past libffi's `extern "C"` functions.
//!         panic!("This callback can only be used with an `i32` return value.");
//!     }
//!
//!     // SAFETY: The return value's type is `i32`.
//!     unsafe {
//!         ret.write(result);
//!     }
//! }
//!
//! let add_offset = DynamicClosure::new(add_offset_callback, &[Type::I32], Some(&Type::I32), 5);
//!
//! // SAFETY: `add_offset` represents an `extern "C"` function that accepts one `i32` and returns
//! // an `i32`. `add_offset` remains alive while it is called.
//! let add_offset_fn = unsafe {
//!     add_offset
//!         .as_fn_ptr()
//!         .into_fn::<extern "C" fn(i32) -> i32>()
//! };
//!
//! assert_eq!(add_offset_fn(37), 42);
//! # }
//! ```
//!
//! # Features
//!
//! * `closure` - include [`Closure`] and [`DynamicClosure`]. This feature is enabled by default.
//! * `check_only` - Speed up check builds by skipping the actual compilation of libffi.
//! * `system` - enables `libffi-sys`'s `system` flag to dynamically link to libffi instead of
//!   compiling a static library. Note that certain functionality require version 3.5.0 of libffi,
//!   which may cause problems when using the `system` feature.
//!
//!
//! # Panics
//!
//! As a general rule `fiffi` functions do not panic. The exception is when allocating a new
//! [`Closure`] or [`DynamicClosure`], however both types have constructors that return an error
//! instead of panicking if allocation fails.
//!
//! Some safeguards to detect serious bugs in `fiffi` and/or libffi may also panic if a bug is
//! detected or a code path that is only reachable through improper use of `unsafe` is reached.
//!
//! [`Function`]: `crate::function::Function`
//! [libffi-rs]: https://crates.io/crates/libffi/
//! [libffi-sys-rs]: https://crates.io/crates/libffi-sys/

#![cfg_attr(feature = "closure", doc = "[`Closure`]: `crate::closure::Closure`")]
#![cfg_attr(
    feature = "closure",
    doc = "[`DynamicClosure`]: `crate::closure::DynamicClosure`"
)]
#![cfg_attr(not(feature = "closure"), doc = "[`Closure`]: #features")]
#![cfg_attr(not(feature = "closure"), doc = "[`DynamicClosure`]: #features")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(test), no_std)]

pub mod abi;
// TODO enable closures when calls are up and running
#[cfg(all(feature = "closure", false))]
pub mod closure;
pub mod errors;
pub mod function;
pub mod types;

mod fn_ptr;
pub use fn_ptr::FnPtr;

#[cfg(msan)]
unsafe extern "C" {
    /// Expose `__msan_unpoison` if this crate is compiled with custom `msan` cfg.
    pub(crate) unsafe fn __msan_unpoison(addr: *const core::ffi::c_void, size: usize);
    /// Expose `__msan_poison` if this crate is compiled with custom `msan` cfg.
    pub(crate) unsafe fn __msan_poison(addr: *const core::ffi::c_void, size: usize);
    /// Expose `__msan_test_shadow` if this crate is compiled with custom `msan` cfg.
    pub(crate) unsafe fn __msan_test_shadow(addr: *const core::ffi::c_void, size: usize) -> isize;
}

#[cfg(test)]
pub(crate) mod test_utils {
    use core::ffi::{CStr, c_char, c_void};

    use crate::types::{FfiType, Type};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(C)]
    pub struct TestStruct(pub u8, pub u64, pub u64, pub u64, pub u64);

    // TODO new types as needed

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(transparent)]
    pub struct PointerStruct(pub *const c_void);

    // SAFETY: This is only a test struct containing a pointer that should never change.
    unsafe impl Send for PointerStruct {}
    // SAFETY: This is only a test struct containing a pointer that should never change.
    unsafe impl Sync for PointerStruct {}

    // SAFETY: `TestStruct` is `repr(C)`, `Copy`, and its `ffi_type` function lists every field in
    // order.
    unsafe impl FfiType for TestStruct {
        fn ffi_type() -> Type {
            // SAFETY: `Type::create_struct_unchecked` is provided with a non-empty `Vec`.
            unsafe {
                Type::create_struct_unchecked(vec![
                    Type::U8,
                    Type::U64,
                    Type::U64,
                    Type::U64,
                    Type::U64,
                ])
            }
        }
    }

    pub static I8_ARG: i8 = 0x11;
    pub static U8_ARG: u8 = 0x22;
    pub static I16_ARG: i16 = 0x3333;
    pub static U16_ARG: u16 = 0x4444;
    pub static I32_ARG: i32 = 0x5555_5555;
    pub static U32_ARG: u32 = 0x6666_6666;
    pub static I64_ARG: i64 = 0x7777_7777_7777_7777;
    pub static U64_ARG: u64 = 0x8888_8888_8888_8888;
    pub static I128_ARG: i128 = 0x7777_7777_7777_7777_7777_7777_7777_7777;
    pub static U128_ARG: u128 = 0xaaaa_aaaa_aaaa_aaaa_aaaa_aaaa_aaaa_aaaa;
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
    pub static F32_ARG: f32 = core::f32::consts::PI;
    pub static F64_ARG: f64 = core::f64::consts::E;
    pub static PTR_ARG: PointerStruct = PointerStruct((&raw const U32_ARG).cast());
    pub static STRUCT_ARG: TestStruct = TestStruct(
        0x77,
        0x9999_9999_9999_9999,
        0xAAAA_AAAA_AAAA_AAAA,
        0xBBBB_BBBB_BBBB_BBBB,
        0xCCCC_CCCC_CCCC_CCCC,
    );

    pub static SNPRINTF_FORMAT: &CStr = c"1: %d, 2: %u, 3: %lld, 4: %llu, 5: \"%s\", 6: %.1f.\n";
    pub static SNPRINTF_ARG_1: i32 = 1_234_567;
    pub static SNPRINTF_ARG_2: u32 = 9_876_543;
    pub static SNPRINTF_ARG_3: i64 = 12_345_678_900;
    pub static SNPRINTF_ARG_4: u64 = 98_765_432_100;
    pub static SNPRINTF_ARG_5: &CStr = c"This is a &CStr";
    pub static SNPRINTF_ARG_6: f64 = 1.0;
    pub static SNPRINTF_EXPECTED_OUTPUT: &CStr = c"1: 1234567, 2: 9876543, 3: 12345678900, 4: 98765432100, 5: \"This is a &CStr\", 6: 1.0.\n";
    pub static SNPRINTF_EXPECTED_RETURN_VALUE: i32 = 86;

    #[cfg_attr(target_env = "msvc", link(name = "legacy_stdio_definitions"))]
    unsafe extern "C" {
        pub unsafe fn snprintf(s: *mut c_char, n: usize, format: *const c_char, ...) -> i32;
    }
}

// Ensure that the code examples in the README are correct.
#[cfg(all(doc, feature = "closure"))]
#[doc(hidden)]
#[doc = include_str!("../README.md")]
mod _readme {}

// Ensure that the code examples in the libffi-rs-2-fiffi.md are correct.
#[cfg(all(doc, feature = "closure"))]
#[doc(hidden)]
#[doc = include_str!("../libffi-rs-2-fiffi.md")]
mod _libffi_rs_2_fiffi {}
