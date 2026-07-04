#[cfg(not(miri))]
pub use libffi_sys::{
    ffi_type_double, ffi_type_float, ffi_type_pointer, ffi_type_sint8, ffi_type_sint16,
    ffi_type_sint32, ffi_type_sint64, ffi_type_uint8, ffi_type_uint16, ffi_type_uint32,
    ffi_type_uint64, ffi_type_void,
};
#[cfg(miri)]
pub use miri::{
    ffi_type_double, ffi_type_float, ffi_type_pointer, ffi_type_sint8, ffi_type_sint16,
    ffi_type_sint32, ffi_type_sint64, ffi_type_uint8, ffi_type_uint16, ffi_type_uint32,
    ffi_type_uint64, ffi_type_void,
};

#[cfg(any(test, miri))]
#[expect(
    non_upper_case_globals,
    reason = "Using the same names as statics in libffi_sys."
)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "The alignments used fit into a c_ushort/u16."
)]
mod miri {
    use core::ffi::{c_ushort, c_void};
    use core::ptr::null_mut;

    use libffi_sys::ffi_type;

    pub static mut ffi_type_double: ffi_type = ffi_type {
        size: size_of::<f64>(),
        alignment: align_of::<f64>() as c_ushort,
        type_: libffi_sys::FFI_TYPE_DOUBLE,
        elements: null_mut(),
    };

    pub static mut ffi_type_float: ffi_type = ffi_type {
        size: size_of::<f32>(),
        alignment: align_of::<f32>() as c_ushort,
        type_: libffi_sys::FFI_TYPE_FLOAT,
        elements: null_mut(),
    };

    pub static mut ffi_type_pointer: ffi_type = ffi_type {
        size: size_of::<*mut c_void>(),
        alignment: align_of::<*mut c_void>() as c_ushort,
        type_: libffi_sys::FFI_TYPE_POINTER,
        elements: null_mut(),
    };

    pub static mut ffi_type_sint8: ffi_type = ffi_type {
        size: size_of::<i8>(),
        alignment: align_of::<i8>() as c_ushort,
        type_: libffi_sys::FFI_TYPE_SINT8,
        elements: null_mut(),
    };

    pub static mut ffi_type_sint16: ffi_type = ffi_type {
        size: size_of::<i16>(),
        alignment: align_of::<i16>() as c_ushort,
        type_: libffi_sys::FFI_TYPE_SINT16,
        elements: null_mut(),
    };

    pub static mut ffi_type_sint32: ffi_type = ffi_type {
        size: size_of::<i32>(),
        alignment: align_of::<i32>() as c_ushort,
        type_: libffi_sys::FFI_TYPE_SINT32,
        elements: null_mut(),
    };

    pub static mut ffi_type_sint64: ffi_type = ffi_type {
        size: size_of::<i64>(),
        alignment: align_of::<i64>() as c_ushort,
        type_: libffi_sys::FFI_TYPE_SINT64,
        elements: null_mut(),
    };

    pub static mut ffi_type_uint8: ffi_type = ffi_type {
        size: size_of::<u8>(),
        alignment: align_of::<u8>() as c_ushort,
        type_: libffi_sys::FFI_TYPE_UINT8,
        elements: null_mut(),
    };

    pub static mut ffi_type_uint16: ffi_type = ffi_type {
        size: size_of::<u16>(),
        alignment: align_of::<u16>() as c_ushort,
        type_: libffi_sys::FFI_TYPE_UINT16,
        elements: null_mut(),
    };

    pub static mut ffi_type_uint32: ffi_type = ffi_type {
        size: size_of::<u32>(),
        alignment: align_of::<u32>() as c_ushort,
        type_: libffi_sys::FFI_TYPE_UINT32,
        elements: null_mut(),
    };

    pub static mut ffi_type_uint64: ffi_type = ffi_type {
        size: size_of::<u64>(),
        alignment: align_of::<u64>() as c_ushort,
        type_: libffi_sys::FFI_TYPE_UINT64,
        elements: null_mut(),
    };

    pub static mut ffi_type_void: ffi_type = ffi_type {
        size: 1,
        alignment: 1,
        type_: libffi_sys::FFI_TYPE_VOID,
        elements: null_mut(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! assert_ffi_types_equal {
        ($ty:ident) => {
            let type_a = $ty;
            let type_b = miri::$ty;

            assert_eq!(
                type_a.size,
                type_b.size,
                "{}'s size is different.",
                stringify!($ty)
            );

            assert_eq!(
                type_a.alignment,
                type_b.alignment,
                "{}'s alignment is different.",
                stringify!($ty)
            );

            assert_eq!(
                type_a.type_,
                type_b.type_,
                "{}'s type tag is different.",
                stringify!($ty)
            );

            assert_eq!(
                type_a.elements,
                type_b.elements,
                "{}'s element array is different.",
                stringify!($ty)
            );
        };
    }

    #[test]
    fn test_ffi_types_equal() {
        // SAFETY: Although the `ffi_type_` variables are declared `static mut`, they should not be
        // modified.
        unsafe {
            assert_ffi_types_equal!(ffi_type_double);
            assert_ffi_types_equal!(ffi_type_float);
            assert_ffi_types_equal!(ffi_type_pointer);
            assert_ffi_types_equal!(ffi_type_sint8);
            assert_ffi_types_equal!(ffi_type_sint16);
            assert_ffi_types_equal!(ffi_type_sint32);
            assert_ffi_types_equal!(ffi_type_sint64);
            assert_ffi_types_equal!(ffi_type_uint8);
            assert_ffi_types_equal!(ffi_type_uint16);
            assert_ffi_types_equal!(ffi_type_uint32);
            assert_ffi_types_equal!(ffi_type_uint64);
            assert_ffi_types_equal!(ffi_type_void);
        }
    }
}
