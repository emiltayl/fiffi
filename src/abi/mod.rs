//! This module defines ABIs supported by libffi.
//!
//! Use the `Abi` type in this module and not from any of the submodules. The `Abi` types in the
//! submodules are only defined when building documentation to show all supported ABIs for all
//! platforms.
//!
//! `Abi` provides a `Default` implementation that returns the default C ABI for the given target.
//!
//! This code is based on:
//! <https://github.com/libffi-rs/libffi-rs/blob/5eb8bfd9eee19e5cfcd41a8b1d8239292f6ecde9/libffi-sys-rs/src/arch.rs>

use libffi_sys::ffi_abi;

/// ABI constants for AArch64 targets.
#[cfg(docsrs)]
pub mod aarch64;
#[cfg(all(target_arch = "aarch64", not(docsrs)))]
mod aarch64;

/// ABI constants for ARMv7 targets.
#[cfg(docsrs)]
pub mod armv7;
#[cfg(all(target_arch = "arm", not(docsrs)))]
mod armv7;

/// ABI constants for LoongArch64 targets.
#[cfg(docsrs)]
pub mod loongarch64;
#[cfg(all(target_arch = "loongarch64", not(docsrs)))]
mod loongarch64;

/// ABI constants for 32-bit PowerPC targets.
#[cfg(docsrs)]
pub mod powerpc;
#[cfg(all(target_arch = "powerpc", not(docsrs)))]
mod powerpc;

/// ABI constants for 64-bit PowerPC targets.
#[cfg(docsrs)]
pub mod powerpc64;
#[cfg(all(target_arch = "powerpc64", not(docsrs)))]
mod powerpc64;

/// ABI constants for RISC-V targets.
#[cfg(docsrs)]
pub mod riscv;
#[cfg(all(any(target_arch = "riscv32", target_arch = "riscv64"), not(docsrs)))]
mod riscv;

/// ABI constants for s390x targets.
#[cfg(docsrs)]
pub mod s390x;
#[cfg(all(target_arch = "s390x", not(docsrs)))]
mod s390x;

/// ABI constants for 64-bit SPARC targets.
#[cfg(docsrs)]
pub mod sparc64;
#[cfg(all(target_arch = "sparc64", not(docsrs)))]
mod sparc64;

/// ABI constants for non-Windows 32-bit x86 targets.
#[cfg(docsrs)]
pub mod x86_32;
#[cfg(all(target_arch = "x86", not(windows), not(docsrs)))]
mod x86_32;

/// ABI constants for Windows 32-bit x86 targets.
#[cfg(docsrs)]
pub mod x86_32_win;
#[cfg(all(target_arch = "x86", windows, not(docsrs)))]
mod x86_32_win;

/// ABI constants for non-Windows 64-bit x86 targets.
#[cfg(docsrs)]
pub mod x86_64;
#[cfg(all(target_arch = "x86_64", not(windows), not(docsrs)))]
mod x86_64;

/// ABI constants for Windows 64-bit x86 targets.
#[cfg(docsrs)]
pub mod x86_64_win;
#[cfg(all(target_arch = "x86_64", windows, not(docsrs)))]
mod x86_64_win;

/// A libffi ABI identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Abi(ffi_abi);

impl Abi {
    /// Returns the raw libffi ABI value.
    pub fn to_ffi_abi(&self) -> ffi_abi {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use core::ffi::{CStr, c_char, c_uint, c_void};

    use libffi_sys::{ffi_call, ffi_cif, ffi_prep_cif, ffi_prep_cif_var};

    use super::*;
    use crate::abi::test_utils::*;
    use crate::errors::LibffiError;
    use crate::raw::{
        ffi_type_double, ffi_type_pointer, ffi_type_sint32, ffi_type_sint64, ffi_type_uint32,
        ffi_type_uint64,
    };
    use crate::test_utils::{
        SNPRINTF_ARG_1, SNPRINTF_ARG_2, SNPRINTF_ARG_3, SNPRINTF_ARG_4, SNPRINTF_ARG_5,
        SNPRINTF_ARG_6, SNPRINTF_EXPECTED_OUTPUT, SNPRINTF_EXPECTED_RETURN_VALUE, SNPRINTF_FORMAT,
        snprintf,
    };

    #[test]
    fn verify_default_abi() {
        unsafe extern "C" {
            safe fn ffi_get_default_abi() -> c_uint;
        }

        assert_eq!(Abi::default().to_ffi_abi(), ffi_get_default_abi());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_ffi_call_default_abi_variadic() {
        #[cfg(target_pointer_width = "32")]
        let size_t_type = &raw mut ffi_type_uint32;
        #[cfg(target_pointer_width = "64")]
        let size_t_type = &raw mut ffi_type_uint64;

        #[rustfmt::skip]
        let mut arg_types = [
            // Fixed args
            &raw mut ffi_type_pointer, size_t_type, &raw mut ffi_type_pointer,
            // Variadic args
            &raw mut ffi_type_sint32, &raw mut ffi_type_uint32, &raw mut ffi_type_sint64,
            &raw mut ffi_type_uint64, &raw mut ffi_type_pointer, &raw mut ffi_type_double,
        ];

        let mut buffer = [0u8; 128];
        let mut buffer_ptr = buffer.as_mut_ptr();
        let mut max_bytes = buffer.len();

        let mut args: [*mut c_void; 9] = [
            (&raw mut buffer_ptr).cast(),
            (&raw mut max_bytes).cast(),
            (&raw const SNPRINTF_FORMAT).cast_mut().cast(),
            (&raw const SNPRINTF_ARG_1).cast_mut().cast(),
            (&raw const SNPRINTF_ARG_2).cast_mut().cast(),
            (&raw const SNPRINTF_ARG_3).cast_mut().cast(),
            (&raw const SNPRINTF_ARG_4).cast_mut().cast(),
            (&raw const SNPRINTF_ARG_5).cast_mut().cast(),
            (&raw const SNPRINTF_ARG_6).cast_mut().cast(),
        ];

        let mut cif = ffi_cif::default();

        // SAFETY:
        // * `cif` points to a writable `ffi_cif`.
        // * `argument_array` points to an array with pointers to valid `ffi_type`s.
        // * `rtype` points to a valid `ffi_type`.
        let status = unsafe {
            ffi_prep_cif_var(
                &raw mut cif,
                Abi::default().to_ffi_abi(),
                3,
                9,
                &raw mut ffi_type_sint32,
                arg_types.as_mut_ptr(),
            )
        };

        assert!(LibffiError::from_status(status).is_none());

        let mut return_value: isize = 0;

        // SAFETY:
        // * `cif` points to a valid, initialized `ffi_cif`.
        // * The signature described in the `ffi_cif` is identical to `snprintf`'s signature.
        // * Transmuting from a function pointer to a Option<function pointer> is valid because
        //   function pointers are guaranteed to not be NULL.
        // * `rvalue` points to a `usize` sized slice of mutable memory.
        // * `args` contains 14 pointers to arguments of the expected type.
        // * `snprintf` is provided the length of the output buffer and will not write out of
        //   bounds.
        unsafe {
            #[rustfmt::skip]
            ffi_call(
                &raw mut cif,
                core::mem::transmute::<
                    unsafe extern "C" fn(*mut c_char, usize, *const c_char, ...) -> i32,
                    Option<unsafe extern "C" fn()>,
                >(snprintf),
                (&raw mut return_value).cast(),
                args.as_mut_ptr(),
            );
        }

        let output_str = CStr::from_bytes_until_nul(&buffer).unwrap();

        assert_eq!(
            return_value,
            isize::try_from(SNPRINTF_EXPECTED_RETURN_VALUE).unwrap(),
            "`snprintf` did not write the expected number of bytes."
        );

        assert_eq!(
            output_str, SNPRINTF_EXPECTED_OUTPUT,
            "Output from `snprintf` was not as expected."
        );
    }

    generate_call_test_for_abi!("C", Abi::default(), test_ffi_call_default_abi);

    #[test]
    #[cfg_attr(miri, ignore)]
    fn create_closure_and_call_with_default_abi() {
        test_create_closure_and_call_with_abi(Abi::default());
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    use core::ffi::c_void;
    use core::ptr::null_mut;

    use libffi_sys::{
        ffi_call, ffi_cif, ffi_closure, ffi_closure_alloc, ffi_closure_free, ffi_prep_cif,
        ffi_prep_closure_loc, ffi_type,
    };

    use super::*;
    use crate::errors::LibffiError;
    use crate::raw::{
        ffi_type_double, ffi_type_float, ffi_type_pointer, ffi_type_sint8, ffi_type_sint16,
        ffi_type_sint32, ffi_type_sint64, ffi_type_uint8, ffi_type_uint16, ffi_type_uint32,
        ffi_type_uint64, ffi_type_void,
    };
    use crate::test_utils::{
        F32_ARG, F64_ARG, I8_ARG, I16_ARG, I32_ARG, I64_ARG, ISIZE_ARG, PTR_ARG, STRUCT_ARG,
        TestStruct, U8_ARG, U16_ARG, U32_ARG, U64_ARG, USIZE_ARG, get_test_struct_ffi_type,
    };

    macro_rules! generate_verify_fn_for_abi {
        ($abi: literal, $call_fn: ident) => {
            extern $abi fn $call_fn(
                i8: i8, u8: u8, i16: i16, u16: u16, i32: i32, u32: u32, i64: i64, u64: u64,
                isize: isize, usize: usize, f32: f32, f64: f64, ptr: *const c_void,
                test_struct: crate::test_utils::TestStruct,
            ) -> usize {
                use crate::test_utils;

                assert_eq!(i8, test_utils::I8_ARG, "`i8` argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(u8, test_utils::U8_ARG, "`u8` argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(i16, test_utils::I16_ARG, "`i16` argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(u16, test_utils::U16_ARG, "`u16` argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(i32, test_utils::I32_ARG, "`i32` argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(u32, test_utils::U32_ARG, "`u32` argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(i64, test_utils::I64_ARG, "`i64` argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(u64, test_utils::U64_ARG, "`u64` argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(isize, test_utils::ISIZE_ARG, "`isize` argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(usize, test_utils::USIZE_ARG, "`usize` argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(f32, test_utils::F32_ARG, "`f32` argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(f64, test_utils::F64_ARG, "`f64` argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(ptr, test_utils::PTR_ARG.0, "pointer argument to {} was not as expected.", stringify!($call_fn));
                assert_eq!(test_struct, test_utils::STRUCT_ARG, "struct argument to {} was not as expected.", stringify!($call_fn));

                test_utils::USIZE_ARG
            }
        };
    }

    pub(crate) use generate_verify_fn_for_abi;

    macro_rules! generate_call_test_for_abi {
        ($abi: literal, $abi_enum: expr, $test_fn: ident) => {
            #[test]
            #[cfg_attr(miri, ignore)]
            fn $test_fn() {
                use crate::test_utils as crate_test_utils;
                use crate::abi::test_utils as abi_test_utils;

                abi_test_utils::generate_verify_fn_for_abi!($abi, verify_fn_args);

                let mut struct_type = crate_test_utils::get_test_struct_ffi_type();
                let mut argument_array = abi_test_utils::get_test_fn_argument_array(&raw mut struct_type);
                let mut cif = ffi_cif::default();

                #[cfg(target_pointer_width = "32")]
                let rtype = &raw mut ffi_type_uint32;
                #[cfg(target_pointer_width = "64")]
                let rtype = &raw mut ffi_type_uint64;

                let mut args: [*mut c_void; 14] = [
                    (&raw const crate_test_utils::I8_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::U8_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::I16_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::U16_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::I32_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::U32_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::I64_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::U64_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::ISIZE_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::USIZE_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::F32_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::F64_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::PTR_ARG).cast_mut().cast(),
                    (&raw const crate_test_utils::STRUCT_ARG).cast_mut().cast(),
                ];

                #[rustfmt::skip]
                let fn_ptr: unsafe extern $abi fn(
                    i8, u8, i16, u16, i32, u32, i64, u64,
                    isize, usize, f32, f64, *const c_void, crate_test_utils::TestStruct,
                ) -> usize = verify_fn_args;

                // SAFETY:
                // * `cif` points to a writable `ffi_cif`.
                // * `argument_array` points to an array with pointers to valid `ffi_type`s.
                // * `rtype` points to a valid `ffi_type`.
                let status = unsafe {
                    ffi_prep_cif(
                        &raw mut cif,
                        $abi_enum.to_ffi_abi(),
                        argument_array.len().try_into().unwrap(),
                        rtype,
                        argument_array.as_mut_ptr(),
                    )
                };

                assert!(crate::errors::LibffiError::from_status(status).is_none());

                let mut return_buffer: usize = 0;

                // SAFETY:
                // * `cif` points to a valid, initialized `ffi_cif`.
                // * The signature described in the `ffi_cif` is identical to `test_abi_fn`'s signature.
                // * Transmuting from a function pointer to a Option<function pointer> is valid because
                //   function pointers are guaranteed to not be NULL.
                // * `rvalue` points to a `usize` sized slice of mutable memory.
                // * `args` contains 14 pointers to arguments of the expected type.
                unsafe {
                    #[rustfmt::skip]
                    ffi_call(
                        &raw mut cif,
                        core::mem::transmute::<
                            unsafe extern $abi fn(
                                i8, u8, i16, u16, i32, u32, i64, u64,
                                isize, usize, f32, f64, *const c_void, crate_test_utils::TestStruct,
                            ) -> usize,
                            Option<unsafe extern "C" fn()>,
                        >(fn_ptr),
                        (&raw mut return_buffer).cast(),
                        args.as_mut_ptr(),
                    );
                }

                assert_eq!(return_buffer, crate_test_utils::USIZE_ARG, "Calling {} did not return the correct value.", stringify!($call_fn));
            }
        };
    }

    pub(crate) use generate_call_test_for_abi;

    #[rustfmt::skip]
    pub fn get_test_fn_argument_array(test_struct_type_ptr: *mut ffi_type) -> [*mut ffi_type; 14] {
        #[cfg(target_pointer_width = "32")]
        let isize_ptr = &raw mut ffi_type_sint32;
        #[cfg(target_pointer_width = "32")]
        let usize_ptr = &raw mut ffi_type_uint32;
        #[cfg(target_pointer_width = "64")]
        let isize_ptr = &raw mut ffi_type_sint64;
        #[cfg(target_pointer_width = "64")]
        let usize_ptr = &raw mut ffi_type_uint64;

        [
            &raw mut ffi_type_sint8, &raw mut ffi_type_uint8, &raw mut ffi_type_sint16,
            &raw mut ffi_type_uint16, &raw mut ffi_type_sint32, &raw mut ffi_type_uint32,
            &raw mut ffi_type_sint64, &raw mut ffi_type_uint64, isize_ptr,
            usize_ptr, &raw mut ffi_type_float, &raw mut ffi_type_double,
            &raw mut ffi_type_pointer, test_struct_type_ptr,
        ]
    }

    unsafe extern "C" fn closure_test_fn(
        _cif: *mut ffi_cif,
        _ret_ptr: *mut c_void,
        args_ptr: *mut *mut c_void,
        _userdata: *mut c_void,
    ) {
        // SAFETY: The tests pass an `args_ptr` array with 14 pointers in the order described by
        // `get_test_fn_argument_array`, and each pointer refers to initialized storage of the
        // expected type.
        unsafe {
            let i8_arg = *((*args_ptr).cast::<i8>());
            assert_eq!(i8_arg, I8_ARG);
            let u8_arg = *((*args_ptr.add(1)).cast::<u8>());
            assert_eq!(u8_arg, U8_ARG);
            let i16_arg = *((*args_ptr.add(2)).cast::<i16>());
            assert_eq!(i16_arg, I16_ARG);
            let u16_arg = *((*args_ptr.add(3)).cast::<u16>());
            assert_eq!(u16_arg, U16_ARG);
            let i32_arg = *((*args_ptr.add(4)).cast::<i32>());
            assert_eq!(i32_arg, I32_ARG);
            let u32_arg = *((*args_ptr.add(5)).cast::<u32>());
            assert_eq!(u32_arg, U32_ARG);
            let i64_arg = *((*args_ptr.add(6)).cast::<i64>());
            assert_eq!(i64_arg, I64_ARG);
            let u64_arg = *((*args_ptr.add(7)).cast::<u64>());
            assert_eq!(u64_arg, U64_ARG);
            let isize_arg = *((*args_ptr.add(8)).cast::<isize>());
            assert_eq!(isize_arg, ISIZE_ARG);
            let usize_arg = *((*args_ptr.add(9)).cast::<usize>());
            assert_eq!(usize_arg, USIZE_ARG);
            let f32_arg = *((*args_ptr.add(10)).cast::<f32>());
            assert_eq!(f32_arg, F32_ARG);
            let f64_arg = *((*args_ptr.add(11)).cast::<f64>());
            assert_eq!(f64_arg, F64_ARG);
            let ptr_arg = *((*args_ptr.add(12)).cast::<*const c_void>());
            assert_eq!(ptr_arg, PTR_ARG.0);
            let struct_arg = *((*args_ptr.add(13)).cast::<TestStruct>());
            assert_eq!(struct_arg, STRUCT_ARG);
        }
    }

    pub fn test_create_closure_and_call_with_abi(abi: Abi) {
        let mut closure_struct_type = get_test_struct_ffi_type();
        let mut closure_cif_argument_types =
            get_test_fn_argument_array(&raw mut closure_struct_type);
        let mut closure_cif = ffi_cif::default();

        // SAFETY:
        // * `closure_cif` is a writable `ffi_cif`.
        // * `closure_cif_argument_types` is an array with pointers to valid `ffi_type`s.
        // * `rtype` points to a valid `ffi_type`.
        let status = unsafe {
            ffi_prep_cif(
                &raw mut closure_cif,
                abi.to_ffi_abi(),
                closure_cif_argument_types.len().try_into().unwrap(),
                &raw mut ffi_type_void,
                closure_cif_argument_types.as_mut_ptr(),
            )
        };

        assert!(LibffiError::from_status(status).is_none());

        let mut call_closure_addr: *mut c_void = null_mut();

        // SAFETY:
        // * `call_closure_addr` is a `c_void` pointer, which is what `ffi_closure_alloc` writes.
        let closure =
            unsafe { ffi_closure_alloc(size_of::<ffi_closure>(), &raw mut call_closure_addr) };

        assert!(!closure.is_null());

        // SAFETY:
        // * `closure` is a `ffi_closure` allocated by `ffi_closure_alloc`.
        // * `closure_cif` is a initialized `ffi_cif`.
        // * `call_closure_addr` is the pointer written by `ffi_closure_alloc`.
        let status = unsafe {
            ffi_prep_closure_loc(
                closure.cast(),
                &raw mut closure_cif,
                Some(closure_test_fn),
                null_mut(),
                call_closure_addr,
            )
        };

        assert!(LibffiError::from_status(status).is_none());

        let mut call_struct_type = get_test_struct_ffi_type();
        let mut call_cif_argument_types = get_test_fn_argument_array(&raw mut call_struct_type);
        let mut call_cif = ffi_cif::default();

        // SAFETY:
        // * `call_cif` is a writable `ffi_cif`.
        // * `call_cif_argument_types` is an array with pointers to valid `ffi_type`s.
        // * `rtype` points to a valid `ffi_type`.
        let status = unsafe {
            ffi_prep_cif(
                &raw mut call_cif,
                abi.to_ffi_abi(),
                call_cif_argument_types.len().try_into().unwrap(),
                &raw mut ffi_type_void,
                call_cif_argument_types.as_mut_ptr(),
            )
        };

        assert!(LibffiError::from_status(status).is_none());

        let mut args: [*mut c_void; 14] = [
            (&raw const I8_ARG).cast_mut().cast(),
            (&raw const U8_ARG).cast_mut().cast(),
            (&raw const I16_ARG).cast_mut().cast(),
            (&raw const U16_ARG).cast_mut().cast(),
            (&raw const I32_ARG).cast_mut().cast(),
            (&raw const U32_ARG).cast_mut().cast(),
            (&raw const I64_ARG).cast_mut().cast(),
            (&raw const U64_ARG).cast_mut().cast(),
            (&raw const ISIZE_ARG).cast_mut().cast(),
            (&raw const USIZE_ARG).cast_mut().cast(),
            (&raw const F32_ARG).cast_mut().cast(),
            (&raw const F64_ARG).cast_mut().cast(),
            (&raw const PTR_ARG).cast_mut().cast(),
            (&raw const STRUCT_ARG).cast_mut().cast(),
        ];
        //
        // SAFETY:
        // * `call_cif` is to a valid, initialized `ffi_cif`.
        // * The signature described in the `ffi_cif` is identical to the signature defined when
        //   creating the closure.
        // * Transmuting from a function pointer to a Option<function pointer> is valid because
        //   function pointers are guaranteed to not be NULL.
        // * `call_cif` is defined with a `void` return type, so `rvalue` can be `NULL`.
        // * `args` contains 14 pointers to arguments of the expected type.
        unsafe {
            #[rustfmt::skip]
            ffi_call(
                &raw mut call_cif,
                core::mem::transmute::<
                    *mut c_void,
                    Option<unsafe extern "C" fn()>,
                >(call_closure_addr),
                null_mut(),
                args.as_mut_ptr(),
            );
        }

        // SAFETY: `closure` is a `ffi_closure` allocated by `ffi_closure_alloc`.
        unsafe {
            ffi_closure_free(closure);
        }
    }
}
