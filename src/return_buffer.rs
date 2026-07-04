use core::ffi::c_void;
use core::mem::MaybeUninit;
use core::ptr;

use libffi_sys::{FFI_TYPE_FLOAT, FFI_TYPE_STRUCT, FFI_TYPE_VOID, ffi_type};

#[cfg(any(feature = "closure", test))]
use crate::types::Type;

/// Buffer used by libffi for closures and functions returning small integers.
///
/// Libffi requires integer returns to be widened to a full register's width.
#[repr(transparent)]
pub(crate) struct ReturnBuffer(MaybeUninit<usize>);

const _: () = {
    assert!(size_of::<ReturnBuffer>() == size_of::<usize>());
    assert!(align_of::<ReturnBuffer>() == align_of::<usize>());
};

impl ReturnBuffer {
    /// Returns whether libffi requires a register-sized return buffer for `return_type`.
    ///
    /// Libffi promotes integer return values smaller than a register to a full register width.
    /// Return values of other types can be written using their declared layout.
    #[cfg(any(feature = "closure", test))]
    pub fn is_required_for_type(return_type: &Type) -> bool {
        match return_type {
            Type::I8 | Type::I16 | Type::U8 | Type::U16 => true,
            Type::I32 | Type::U32 => cfg!(target_pointer_width = "64"),
            Type::I64
            | Type::Isize
            | Type::U64
            | Type::Usize
            | Type::F32
            | Type::F64
            | Type::Pointer
            | Type::Struct(_) => false,
        }
    }

    pub fn new() -> Self {
        Self(MaybeUninit::uninit())
    }

    #[cfg(any(feature = "closure", test))]
    pub fn zeroed() -> Self {
        Self(MaybeUninit::new(0))
    }

    pub fn as_ptr(&self) -> *const c_void {
        self.0.as_ptr().cast()
    }

    pub fn as_mut_ptr(&mut self) -> *mut c_void {
        self.0.as_mut_ptr().cast()
    }

    /// Get the byte offset from `self.0` to where an integer of size `value_size` would be written.
    ///
    /// For little endian architectures, this is always 0, while on big endian architectures the
    /// value is written at an offset from the base address.
    ///
    /// For big endian architectures, this function considers the full `ReturnBuffer` to be an array
    /// of `value_size / size_of::<ReturnBuffer>()` sized elements. It calculates the byte offset to
    /// the last (least significant) array element to get the correct address to read from.
    ///
    /// This functions assumes that `size_of::<ReturnBuffer> >= value_size` but does not make any
    /// checks.
    pub fn get_value_offset(value_size: usize) -> usize {
        if cfg!(target_endian = "little") {
            0
        } else {
            size_of::<ReturnBuffer>() - value_size
        }
    }

    /// Write a return value widened by libffi into caller-provided storage.
    ///
    /// # Safety
    ///
    /// * The `ReturnBuffer` must have been fully initialized by libffi.
    /// * `result_ptr` must be valid to write `result_size` bytes to.
    /// * `result_size` must be less than or equal to `size_of::<ReturnBuffer>()`.
    pub unsafe fn write_result(&self, result_ptr: *mut c_void, result_size: usize) {
        debug_assert!(result_size <= size_of::<ReturnBuffer>());

        // SAFETY: `result_size <= size_of::<ReturnBuffer>()`, so this offset stays within the
        // return buffer allocation.
        let src_ptr = unsafe {
            self.as_ptr()
                .cast::<u8>()
                .add(ReturnBuffer::get_value_offset(result_size))
        };

        // SAFETY:
        // * `self.0` was initialized by libffi before this function was called.
        // * `src_ptr` is selected so `result_size` bytes are in bounds of `self.0`.
        // * `result_ptr` is valid for writes of `result_size` bytes by this function's safety
        //   contract.
        unsafe {
            ptr::copy(src_ptr, result_ptr.cast(), result_size);
        }
    }
}

pub(crate) fn ffi_type_requires_return_buffer(return_type: &ffi_type) -> bool {
    return_type.size < size_of::<usize>()
        && return_type.type_ != FFI_TYPE_FLOAT
        && return_type.type_ != FFI_TYPE_STRUCT
        && return_type.type_ != FFI_TYPE_VOID
}

/// Prepares libffi's result storage and returns a pointer to where the result value should be
/// written.
///
/// # Returns
///
/// Returns a pointer to where the result should be written to.
///
/// # Safety
///
/// * `result_space` must be valid to write `result_size` bytes to.
/// * If `result_type` requires a return buffer, `result_space` must be valid and properly aligned
///   to write a [`ReturnBuffer`] to.
/// * `result_size` must be less than or equal to the size of [`ReturnBuffer`] when `result_type`
///   requires a return buffer.
#[cfg(any(feature = "closure", test))]
unsafe fn prepare_closure_result(
    result_type: &Type,
    result_size: usize,
    result_space: *mut c_void,
) -> *mut c_void {
    if ReturnBuffer::is_required_for_type(result_type) {
        // SAFETY:
        // * It is up to the caller to ensure that it is possible to write a full `ReturnBuffer` to
        //   `result_space` for small integers.
        // * `result_size <= size_of::<ReturnBuffer>()`, so this offset stays within the return
        //   buffer allocation.
        unsafe {
            result_space
                .cast::<ReturnBuffer>()
                .write(ReturnBuffer::zeroed());

            result_space.byte_add(ReturnBuffer::get_value_offset(result_size))
        }
    } else {
        result_space
    }
}

/// Writes a closure return value into libffi's result storage.
///
/// # SAFETY
///
/// * `result_space` must point to a memory location that it is valid to write a `RET` to.
/// * `result_type` must describe `RET`.
/// * `RET` must be FFI safe.
///
/// For integers smaller than `isize` / `usize`, it must be possible to write a full `isize` /
/// `usize` to `result_space`. This crate uses [`ReturnBuffer`] in these cases, which is guaranteed
/// to have the same size and alignment as `isize` / `usize`.
#[cfg(any(feature = "closure", test))]
pub(crate) unsafe fn write_closure_result<RET>(
    result: RET,
    result_type: &Type,
    result_space: *mut MaybeUninit<RET>,
) where
    RET: Copy,
{
    // SAFETY: It is up to the caller to ensure that the result storage can hold `RET` or a full
    // `ReturnBuffer` when integer promotion is required.
    let write_ptr =
        unsafe { prepare_closure_result(result_type, size_of::<RET>(), result_space.cast()) };

    // SAFETY: It is up to the caller to ensure that the result can be written to `result_space`.
    // For small integers on big endian architectures, the alignment should be correct as long as
    // `result_space` is a properly aligned `ReturnBuffer`.
    unsafe { write_ptr.cast::<RET>().write(result) }
}

/// Writes closure return value bytes into libffi's result storage.
///
/// # Safety
///
/// * `result_bytes.len()` must be equal to `result_size`.
/// * `result_size` must match the layout size of `result_type`.
/// * The bytes in `result_bytes` must be a valid FFI return value described by `result_type`.
/// * `result_space` must point to a memory location that it is valid to write `result_size` bytes
///   to.
/// * `result_bytes` and `result_space` must not overlap.
///
/// For integers smaller than `isize` / `usize`, it must be possible to write a full `isize` /
/// `usize` to `result_space`. This crate uses [`ReturnBuffer`] in these cases, which is guaranteed
/// to have the same size and alignment as `isize` / `usize`.
#[cfg(any(feature = "closure", test))]
pub(crate) unsafe fn write_closure_result_bytes(
    result_bytes: &[u8],
    result_type: &Type,
    result_size: usize,
    result_space: *mut c_void,
) {
    // SAFETY: It is up to the caller to ensure that the result storage can hold `result_size` bytes
    // or a full `ReturnBuffer` when integer promotion is required.
    let write_ptr =
        unsafe { prepare_closure_result(result_type, result_size, result_space) }.cast::<u8>();

    // SAFETY:
    // * `result_bytes` contains exactly `result_size` readable bytes.
    // * `write_ptr` points to storage for `result_size` bytes.
    // * The source and destination do not overlap.
    unsafe {
        ptr::copy_nonoverlapping(result_bytes.as_ptr(), write_ptr, result_size);
    }
}

#[cfg(test)]
mod tests {
    use libffi_sys::ffi_call;

    use super::*;
    use crate::FnPtr;
    use crate::abi::Abi;
    use crate::function::raw::Cif;
    use crate::function::test_callbacks::{i8_identity, i16_identity, u8_identity, u16_identity};
    #[cfg(target_pointer_width = "64")]
    use crate::function::test_callbacks::{i32_identity, u32_identity};
    use crate::test_utils::{I8_ARG, I16_ARG, U8_ARG, U16_ARG};
    #[cfg(target_pointer_width = "64")]
    use crate::test_utils::{I32_ARG, U32_ARG};
    use crate::types::raw::LibffiType;

    fn requires_buffer_for(ty: &Type) -> bool {
        let ffi_type = LibffiType::new(ty);

        // SAFETY: `LibffiType` always stores a non-null pointer to a valid `ffi_type`.
        unsafe { ffi_type_requires_return_buffer(&*ffi_type.as_ffi_type_ptr()) }
    }

    fn initialized_return_buffer<T>(val: T) -> ReturnBuffer
    where
        T: TryInto<usize>,
    {
        if let Ok(val) = val.try_into() {
            ReturnBuffer(MaybeUninit::new(val))
        } else {
            panic!("Unable to convert type into `usize`.");
        }
    }

    fn verify_written_result_bytes(
        return_buffer: MaybeUninit<usize>,
        expected_result_bytes: &[u8],
    ) {
        // SAFETY: Both closure result writers initialize the full return buffer for promoted
        // integer results.
        let return_buffer_bytes = unsafe { return_buffer.assume_init() }.to_ne_bytes();
        let result_offset = ReturnBuffer::get_value_offset(expected_result_bytes.len());
        let result_end = result_offset + expected_result_bytes.len();

        assert_eq!(
            &return_buffer_bytes[result_offset..result_end],
            expected_result_bytes
        );
        assert!(
            return_buffer_bytes[..result_offset]
                .iter()
                .all(|byte| *byte == 0)
        );
        assert!(
            return_buffer_bytes[result_end..]
                .iter()
                .all(|byte| *byte == 0)
        );
    }

    #[test]
    fn ffi_type_requires_return_buffer_only_for_small_non_float_non_struct_returns() {
        assert!(requires_buffer_for(&Type::I8));
        assert!(requires_buffer_for(&Type::I16));
        assert_eq!(
            requires_buffer_for(&Type::I32),
            cfg!(target_pointer_width = "64")
        );
        assert!(!requires_buffer_for(&Type::I64));
        assert!(!requires_buffer_for(&Type::Isize));

        assert!(requires_buffer_for(&Type::U8));
        assert!(requires_buffer_for(&Type::U16));
        assert_eq!(
            requires_buffer_for(&Type::U32),
            cfg!(target_pointer_width = "64")
        );
        assert!(!requires_buffer_for(&Type::U64));
        assert!(!requires_buffer_for(&Type::Usize));

        assert!(!requires_buffer_for(&Type::F32));
        assert!(!requires_buffer_for(&Type::F64));
        assert!(!requires_buffer_for(&Type::Pointer));
        assert!(!requires_buffer_for(
            &Type::create_struct(vec![Type::I8]).unwrap()
        ));

        let void_type = LibffiType::VOID;

        // SAFETY: `LibffiType::VOID` points to libffi's static void type.
        assert!(!unsafe { ffi_type_requires_return_buffer(&*void_type.as_ffi_type_ptr()) });
    }

    #[test]
    fn is_required_for_type_only_for_small_integers() {
        assert!(ReturnBuffer::is_required_for_type(&Type::I8));
        assert!(ReturnBuffer::is_required_for_type(&Type::I16));
        assert_eq!(
            ReturnBuffer::is_required_for_type(&Type::I32),
            cfg!(target_pointer_width = "64")
        );
        assert!(!ReturnBuffer::is_required_for_type(&Type::I64));
        assert!(!ReturnBuffer::is_required_for_type(&Type::Isize));
        assert!(ReturnBuffer::is_required_for_type(&Type::U8));
        assert!(ReturnBuffer::is_required_for_type(&Type::U16));
        assert_eq!(
            ReturnBuffer::is_required_for_type(&Type::U32),
            cfg!(target_pointer_width = "64")
        );
        assert!(!ReturnBuffer::is_required_for_type(&Type::U64));
        assert!(!ReturnBuffer::is_required_for_type(&Type::Usize));
        assert!(!ReturnBuffer::is_required_for_type(&Type::F32));
        assert!(!ReturnBuffer::is_required_for_type(&Type::F64));
        assert!(!ReturnBuffer::is_required_for_type(&Type::Pointer));
        assert!(!ReturnBuffer::is_required_for_type(
            &Type::create_struct(vec![Type::I8]).unwrap()
        ));
    }

    #[test]
    fn write_result_writes_only_exact_result() {
        macro_rules! assert_write_result_writes_only_exact_result {
            ($result:expr, $guard:expr) => {{
                let return_buffer = initialized_return_buffer($result);
                let mut write_buffer = [$guard; 3];
                let result_size = core::mem::size_of_val(&write_buffer[1]);

                // SAFETY:
                // * `return_buffer` is fully initialized by `initialized_return_buffer`.
                // * `result_size` is smaller than `size_of::<ReturnBuffer>()`.
                // * `write_buffer[1]` has `result_size` writable bytes.
                unsafe {
                    return_buffer.write_result((&raw mut write_buffer[1]).cast(), result_size);
                }

                assert_eq!(write_buffer, [$guard, $result, $guard]);
            }};
        }

        assert_write_result_writes_only_exact_result!(U8_ARG, 0xAAu8);
        assert_write_result_writes_only_exact_result!(I8_ARG, 0x55i8);
        assert_write_result_writes_only_exact_result!(U16_ARG, 0xAAAAu16);
        assert_write_result_writes_only_exact_result!(I16_ARG, 0x5555i16);
        #[cfg(target_pointer_width = "64")]
        assert_write_result_writes_only_exact_result!(U32_ARG, 0xAAAA_AAAAu32);
        #[cfg(target_pointer_width = "64")]
        assert_write_result_writes_only_exact_result!(I32_ARG, 0x5555_5555i32);
    }

    #[test]
    fn write_closure_result_writes_promoted_integers_and_zeroes_padding() {
        fn verify_write_closure_result_promotion<RET>(
            result: RET,
            result_type: &Type,
            expected_result_bytes: &[u8],
        ) where
            RET: Copy,
        {
            let mut return_buffer = MaybeUninit::new(usize::MAX);

            // SAFETY:
            // * `return_buffer` is writable storage for the full promoted return buffer.
            // * `result_type` describes `RET`.
            unsafe {
                write_closure_result(result, result_type, (&raw mut return_buffer).cast());
            }

            verify_written_result_bytes(return_buffer, expected_result_bytes);
        }

        verify_write_closure_result_promotion(I8_ARG, &Type::I8, &I8_ARG.to_ne_bytes());
        verify_write_closure_result_promotion(U8_ARG, &Type::U8, &U8_ARG.to_ne_bytes());
        verify_write_closure_result_promotion(I16_ARG, &Type::I16, &I16_ARG.to_ne_bytes());
        verify_write_closure_result_promotion(U16_ARG, &Type::U16, &U16_ARG.to_ne_bytes());

        #[cfg(target_pointer_width = "64")]
        {
            verify_write_closure_result_promotion(I32_ARG, &Type::I32, &I32_ARG.to_ne_bytes());
            verify_write_closure_result_promotion(U32_ARG, &Type::U32, &U32_ARG.to_ne_bytes());
        }
    }

    #[test]
    fn write_closure_result_bytes_writes_promoted_integers_and_zeroes_padding() {
        fn verify_write_closure_result_bytes_promotion(
            result_bytes: &[u8],
            result_type: &Type,
            result_size: usize,
        ) {
            let mut return_buffer = MaybeUninit::new(usize::MAX);

            // SAFETY:
            // * `result_bytes` contains exactly `result_size` bytes describing `result_type`.
            // * `return_buffer` is writable storage for the full promoted return buffer.
            // * The source bytes and destination do not overlap.
            unsafe {
                write_closure_result_bytes(
                    result_bytes,
                    result_type,
                    result_size,
                    (&raw mut return_buffer).cast(),
                );
            }

            verify_written_result_bytes(return_buffer, result_bytes);
        }

        verify_write_closure_result_bytes_promotion(
            &I8_ARG.to_ne_bytes(),
            &Type::I8,
            size_of::<i8>(),
        );
        verify_write_closure_result_bytes_promotion(
            &U8_ARG.to_ne_bytes(),
            &Type::U8,
            size_of::<u8>(),
        );
        verify_write_closure_result_bytes_promotion(
            &I16_ARG.to_ne_bytes(),
            &Type::I16,
            size_of::<i16>(),
        );
        verify_write_closure_result_bytes_promotion(
            &U16_ARG.to_ne_bytes(),
            &Type::U16,
            size_of::<u16>(),
        );

        #[cfg(target_pointer_width = "64")]
        {
            verify_write_closure_result_bytes_promotion(
                &I32_ARG.to_ne_bytes(),
                &Type::I32,
                size_of::<i32>(),
            );
            verify_write_closure_result_bytes_promotion(
                &U32_ARG.to_ne_bytes(),
                &Type::U32,
                size_of::<u32>(),
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn ffi_call_overwrites_full_return_buffer_for_small_integers() {
        let test_cases = [
            (crate::fn_ptrize!(i8_identity), Type::I8),
            (crate::fn_ptrize!(i16_identity), Type::I16),
            #[cfg(target_pointer_width = "64")]
            (crate::fn_ptrize!(i32_identity), Type::I32),
            (crate::fn_ptrize!(u8_identity), Type::U8),
            (crate::fn_ptrize!(u16_identity), Type::U16),
            #[cfg(target_pointer_width = "64")]
            (crate::fn_ptrize!(u32_identity), Type::U32),
        ];

        let input_arg: usize = 0;
        let input_array = [(&raw const input_arg).cast_mut().cast::<c_void>()];
        let input_ptr = input_array.as_ptr().cast_mut();

        let expected_return_value = 0usize;

        for test_case in test_cases {
            let mut return_buffer = ReturnBuffer::new();
            let return_buffer_ptr = return_buffer.as_mut_ptr();

            // SAFETY: `return_buffer_ptr` points to writable storage for a `usize`.
            unsafe {
                return_buffer_ptr.cast::<usize>().write(usize::MAX);
            }

            let cif = Cif::new(
                Abi::default(),
                core::slice::from_ref(&test_case.1),
                Some(&test_case.1),
            );

            // SAFETY:
            // * The `identity_*` functions do not perform any unsafe actions.
            // * A `FnPtr` contains a function pointer, which has the same layout as
            //   `Option<function pointer>` (function pointers cannot be NULL).
            // * A `usize` is large enough and properly aligned for reads of 8-, 16-, and 32-bit
            //   integers on platforms where fiffi is supported.
            unsafe {
                ffi_call(
                    cif.as_ffi_cif_ptr(),
                    core::mem::transmute::<FnPtr, Option<unsafe extern "C" fn()>>(test_case.0),
                    return_buffer_ptr,
                    input_ptr,
                );

                #[cfg(msan)]
                crate::__msan_unpoison(return_buffer_ptr, size_of::<ReturnBuffer>());
            }

            // SAFETY: The `return_buffer` array was written to just after initialization, so it is
            // safe to assume that it has been initialized.
            let return_value = unsafe { return_buffer_ptr.cast::<usize>().read() };

            assert_eq!(
                return_value, expected_return_value,
                "`ffi_call` did not write to the entire `return_buffer` when calling identity function for type {:?}.",
                test_case.1
            );
        }
    }
}
