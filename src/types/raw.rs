//! Raw libffi type ownership helpers.

extern crate alloc;

#[cfg(not(test))]
use alloc::boxed::Box;
#[cfg(not(test))]
use alloc::vec::Vec;
use core::mem;
use core::mem::ManuallyDrop;
use core::ptr::{NonNull, null_mut, slice_from_raw_parts_mut};

use libffi_sys::{FFI_TYPE_STRUCT, ffi_type};

use crate::raw::{
    ffi_type_double, ffi_type_float, ffi_type_pointer, ffi_type_sint8, ffi_type_sint16,
    ffi_type_sint32, ffi_type_sint64, ffi_type_uint8, ffi_type_uint16, ffi_type_uint32,
    ffi_type_uint64, ffi_type_void,
};
use crate::types::{FfiTypeLayout, Type};

/// Raw libffi type pointer used for call interfaces.
///
/// Note that a `ffi_type`'s fields may depend on the `ffi_cif` owning the `ffi_type`. A cloned
/// `ffi_type` should only be used with a `ffi_cif` identical to the one that owned the original
/// `ffi_type`.
#[derive(Debug)]
#[repr(transparent)]
pub struct LibffiType(pub NonNull<ffi_type>);

impl LibffiType {
    // SAFETY: `ffi_type_void` should always be defined when linking with libffi, so this should
    // never be NULL.
    pub const VOID: LibffiType =
        unsafe { LibffiType(NonNull::new_unchecked(&raw mut ffi_type_void)) };

    pub fn new(ty: &Type) -> Self {
        let ptr = match ty {
            Type::I8 => &raw mut ffi_type_sint8,
            Type::U8 => &raw mut ffi_type_uint8,
            Type::I16 => &raw mut ffi_type_sint16,
            Type::U16 => &raw mut ffi_type_uint16,
            Type::I32 => &raw mut ffi_type_sint32,
            Type::U32 => &raw mut ffi_type_uint32,
            Type::I64 => &raw mut ffi_type_sint64,
            Type::U64 => &raw mut ffi_type_uint64,
            Type::F32 => &raw mut ffi_type_float,
            Type::F64 => &raw mut ffi_type_double,
            Type::Pointer => &raw mut ffi_type_pointer,

            #[cfg(target_pointer_width = "32")]
            Type::Isize => &raw mut ffi_type_sint32,
            #[cfg(target_pointer_width = "64")]
            Type::Isize => &raw mut ffi_type_sint64,

            #[cfg(target_pointer_width = "32")]
            Type::Usize => &raw mut ffi_type_uint32,
            #[cfg(target_pointer_width = "64")]
            Type::Usize => &raw mut ffi_type_uint64,

            Type::Struct(types) => {
                // Note we keep `elements` until `libffi_type` has been created, at which point we
                // assume that nothing can panic, and we "forget" `elements` so it is not
                // deallocated when it goes out of scope after this function.
                // When the new `LibffiType` is dropped, we need to make sure to recreate the
                // `LibFfiTypeArray` to make sure it also can be deallocated properly.
                let elements = LibffiTypeArray::new(types.as_vec());

                let mut libffi_type = Box::new(ffi_type {
                    // Will be set by libffi during `ffi_prep_cif`.
                    size: 0,
                    // Will be set by libffi during `ffi_prep_cif`.
                    alignment: 0,
                    type_: FFI_TYPE_STRUCT,
                    // Casting `*mut Option<NonNull<ffi_type>>` to `*mut *mut ffi_type`
                    elements: null_mut(),
                });

                libffi_type.elements = elements.into_raw();

                Box::into_raw(libffi_type)
            }
        };

        // SAFETY: `ptr` is either a pointer to a static variable or the result of a
        // `Box::into_raw`, which is guaranteed to be non-null.
        unsafe { Self(NonNull::new_unchecked(ptr)) }
    }

    pub fn as_ffi_type_ptr(&self) -> *mut ffi_type {
        self.0.as_ptr()
    }

    pub fn layout(&self) -> FfiTypeLayout {
        let align: usize;
        let size: usize;

        // SAFETY: `self.0` is an initialized `ffi_type` managed by `self`.
        unsafe {
            align = usize::from((*self.0.as_ptr()).alignment);
            size = (*self.0.as_ptr()).size;
        }

        FfiTypeLayout { align, size }
    }
}

impl Clone for LibffiType {
    fn clone(&self) -> Self {
        // SAFETY: `self.0` is not NULL and should point to a `ffi_type`.
        let self_type = unsafe { (*self.0.as_ptr()).type_ };

        if self_type == FFI_TYPE_STRUCT {
            // SAFETY: `self.0` is a non-null pointer and `LibffiType.0` is only ever set to a
            // pointer to a `ffi_type`.
            let mut self_copy = unsafe { Box::new(*self.0.as_ptr()) };

            // SAFETY: `self` represents a struct, which has an `elements` array defined.
            let self_elements =
                unsafe { ManuallyDrop::new(LibffiTypeArray::from_raw(self_copy.elements)) };

            let elements_clone = LibffiTypeArray::clone(&self_elements);

            self_copy.elements = elements_clone.into_raw();

            // SAFETY: `Box::into_raw` is guaranteed to return a non-null pointer.
            unsafe { Self(NonNull::new_unchecked(Box::into_raw(self_copy))) }
        } else {
            // If `self` does not represent a struct, `self.0` contains a pointer to a static
            // `ffi_type` which we can simply copy to the clone.
            Self(self.0)
        }
    }
}

impl Drop for LibffiType {
    fn drop(&mut self) {
        // SAFETY: `self.0` is not NULL and should point to a `ffi_type`.
        let self_type = unsafe { (*self.0.as_ptr()).type_ };

        // We only need to actually drop something if `self` represents a struct.
        if self_type == FFI_TYPE_STRUCT {
            // SAFETY: `self.0` is a non-null pointer and `LibffiType.0` is only ever set to a
            // pointer to a `ffi_type`.
            let self_box = unsafe { Box::from_raw(self.0.as_ptr()) };

            // SAFETY: `self` represents a struct, which has an `elements` array defined.
            let self_elements = unsafe { LibffiTypeArray::from_raw(self_box.elements) };

            drop(self_elements);
            drop(self_box);
        }
    }
}

#[repr(transparent)]
pub struct LibffiTypeArray(NonNull<Option<NonNull<ffi_type>>>);

impl LibffiTypeArray {
    pub fn new(types: &[Type]) -> Self {
        // First collect all `LibffiType`s (this may panic if an allocation fails).
        let libffi_types: Vec<LibffiType> = types.iter().map(LibffiType::new).collect();

        Self::libffi_vec_to_self(libffi_types)
    }

    pub fn into_raw(self) -> *mut *mut ffi_type {
        let ptr = self.0.as_ptr().cast();
        mem::forget(self);

        ptr
    }

    /// Reclaims ownership of a sentinel-terminated libffi type array.
    ///
    /// # Safety
    ///
    /// `ptr` must have been returned by [`LibffiTypeArray::into_raw`], must still be uniquely
    /// owned by the caller, and must point to an array ending in a null sentinel.
    pub unsafe fn from_raw(ptr: *mut *mut ffi_type) -> Self {
        // SAFETY: It is up to the caller to uphold the safety guarantees.
        unsafe { Self(NonNull::new_unchecked(ptr.cast())) }
    }

    fn libffi_vec_to_self(mut vec: Vec<LibffiType>) -> Self {
        // If the vec has no spare capacity, reserve capacity for a new element to avoid panics when
        // pushing a new element to the vec after it has been transformed into a
        // `Vec<Option<NonNull<ffi_type>>>`.
        if vec.capacity() == vec.len() {
            vec.reserve(1);
        }

        let vec_ptr = vec.as_mut_ptr();
        let vec_len = vec.len();
        let vec_capacity = vec.capacity();

        // Forget the original vec so it is not dropped.
        mem::forget(vec);

        // These asserts will be optimized away if both values are equal.
        assert_eq!(
            align_of::<LibffiType>(),
            align_of::<Option<NonNull<ffi_type>>>()
        );
        assert_eq!(
            size_of::<LibffiType>(),
            size_of::<Option<NonNull<ffi_type>>>()
        );

        // SAFETY: The size and alignment of the types are checked above. All other conditions
        // should be satisfied as the pointer, length and capacity comes from the `Vec` allocated
        // above.
        let mut type_array: Vec<Option<NonNull<ffi_type>>> =
            unsafe { Vec::from_raw_parts(vec_ptr.cast(), vec_len, vec_capacity) };

        // A sentinel null must be added to the `Vec` so libffi knows when the array stops.
        type_array.push(None);

        let slice_ptr = Box::into_raw(type_array.into_boxed_slice());

        // SAFETY: `slice_ptr` and the slice's pointer are both allocated in the code above and
        // guaranteed to not be null.
        unsafe { Self(NonNull::new_unchecked((*slice_ptr).as_mut_ptr())) }
    }
}

impl Clone for LibffiTypeArray {
    fn clone(&self) -> Self {
        let mut cloned_vec: Vec<LibffiType> = Vec::new();
        let mut offset = 0;

        // SAFETY: `self.0` is an array of `Option<NonNull<ffi_type>>` guaranteed to end in `None`.
        unsafe {
            while let Some(ty) = self.0.offset(offset).read() {
                // Ensure that the original `LibffiType` is `ManuallyDrop` so it is not dropped
                // twice in case of a panic.
                let ffi_type = ManuallyDrop::new(LibffiType(ty));

                offset += 1;
                cloned_vec.push(LibffiType::clone(&ffi_type));
            }
        }

        Self::libffi_vec_to_self(cloned_vec)
    }
}

impl Drop for LibffiTypeArray {
    fn drop(&mut self) {
        let mut len = 0;

        // SAFETY: `self.0` points to an array of `Option<NonNull<ffi_type>>` guaranteed to end in
        // `None`.
        unsafe {
            while let Some(ty) = self.0.offset(len).read() {
                // Drop the `LibffiType` and replace the pointer in the array with a NULL to avoid
                // accidental reads.
                drop(LibffiType(ty));
                self.0.offset(len).write(None);

                len += 1;
            }
        }

        // We did not count the last element in the array (NULL).
        len += 1;

        // SAFETY: `self.0` was created from a `Box::into_raw` with a boxed slice of the
        // `LibffiType`s in this `LibffiTypeArray`.
        let boxed_slice = unsafe {
            Box::from_raw(slice_from_raw_parts_mut(
                self.0.as_ptr(),
                len.try_into().unwrap_unchecked(),
            ))
        };
        drop(boxed_slice);
    }
}

#[cfg(test)]
mod test {
    use alloc::vec;

    use super::*;

    unsafe fn ffi_types_equal(type_1: &ffi_type, type_2: &ffi_type) -> bool {
        let size_equal = type_1.size == type_2.size;
        let align_equal = type_1.alignment == type_2.alignment;
        let type_equal = type_1.type_ == type_2.type_;

        if !size_equal || !align_equal || !type_equal {
            return false;
        }

        if type_1.type_ == FFI_TYPE_STRUCT {
            // `elements` should **NOT** be NULL for a struct. If either of the structs have a NULL
            // `elements`, something is wrong and we return `false` to not accept / pass invalid
            // structs.
            if type_1.elements.is_null() || type_2.elements.is_null() {
                return false;
            }

            // SAFETY: It is up to the caller to ensure that `type_1` and `type_2` are valid
            // `ffi_type`s that it is safe to read all `elements` from. If any `elements` contains
            // structs, it must also be safe to read that `ffi_type`'s `elements`.
            unsafe {
                let mut offset = 0;
                let mut elements_1_ptr = *type_1.elements;
                let mut elements_2_ptr = *type_2.elements;

                while !elements_1_ptr.is_null() && !elements_2_ptr.is_null() {
                    let element_1 = &*(elements_1_ptr);
                    let element_2 = &*(elements_2_ptr);

                    if !ffi_types_equal(element_1, element_2) {
                        return false;
                    }

                    offset += 1;
                    elements_1_ptr = *(type_1.elements.offset(offset));
                    elements_2_ptr = *(type_2.elements.offset(offset));
                }

                if !elements_1_ptr.is_null() || !elements_2_ptr.is_null() {
                    return false;
                }
            }
        }

        true
    }

    #[test]
    fn clone_keeps_same_pointer_for_non_struct_types() {
        let types = vec![
            Type::I8,
            Type::U8,
            Type::I16,
            Type::U16,
            Type::I32,
            Type::U32,
            Type::I64,
            Type::U64,
            Type::F32,
            Type::F64,
            Type::Pointer,
            Type::Isize,
            Type::Usize,
        ];

        for t in types {
            let raw_type = LibffiType::new(&t);
            let raw_type_clone = raw_type.clone();

            let type_ptr = raw_type.as_ffi_type_ptr();
            let clone_ptr = raw_type_clone.as_ffi_type_ptr();

            assert_eq!(
                type_ptr, clone_ptr,
                "Pointer to `ffi_type` changed when cloning built-in scalar libffi type: {t:?}"
            );
        }
    }

    #[test]
    fn clone_changes_pointer_for_struct_types() {
        let t = Type::create_struct(vec![Type::I32, Type::U32]).unwrap();

        let raw_type = LibffiType::new(&t);
        let raw_type_clone = raw_type.clone();

        let type_ptr = raw_type.as_ffi_type_ptr();
        let clone_ptr = raw_type_clone.as_ffi_type_ptr();

        assert_ne!(
            type_ptr, clone_ptr,
            "Pointer to `ffi_type` did not change when cloning struct type."
        );
    }

    #[test]
    fn test_clone_drop_impls() {
        let struct_a = Type::create_struct(vec![
            Type::I64,
            Type::U64,
            Type::Isize,
            Type::Usize,
            Type::F32,
            Type::F64,
            Type::Pointer,
        ])
        .unwrap();

        let struct_b = Type::create_struct(vec![Type::I32, Type::U32, struct_a]).unwrap();

        let struct_c = Type::create_struct(vec![Type::I16, struct_b, Type::U16]).unwrap();

        let final_struct = Type::create_struct(vec![struct_c, Type::I8, Type::U8]).unwrap();

        let root_type = LibffiType::new(&final_struct);
        let cloned_type = root_type.clone();

        drop(root_type);

        let cloned_type_2 = cloned_type.clone();
        let cloned_type_3 = cloned_type_2.clone();
        let _cloned_type_4 = cloned_type.clone();

        drop(cloned_type_2);

        let _cloned_type_5 = cloned_type_3.clone();
    }

    #[test]
    fn scalar_types_resolve_to_correct_ffi_type() {
        // SAFETY: `LibffiType` should only return pointers to well-formed `ffi_type`s. The
        // `ffi_type`s are either allocated specifically for its `LibffiType` or one of the static
        // `ffi_type`s defined by libffi that should not change.
        unsafe {
            macro_rules! assert_resolves_to {
                ($type_:expr, $expected:expr) => {{
                    let raw_type = LibffiType::new(&$type_);
                    let raw_ref = &*raw_type.0.as_ptr();
                    let expected = $expected;
                    assert!(ffi_types_equal(raw_ref, &expected));
                }};
            }

            assert_resolves_to!(Type::I8, ffi_type_sint8);
            assert_resolves_to!(Type::U8, ffi_type_uint8);
            assert_resolves_to!(Type::I16, ffi_type_sint16);
            assert_resolves_to!(Type::U16, ffi_type_uint16);
            assert_resolves_to!(Type::I32, ffi_type_sint32);
            assert_resolves_to!(Type::U32, ffi_type_uint32);
            assert_resolves_to!(Type::I64, ffi_type_sint64);
            assert_resolves_to!(Type::U64, ffi_type_uint64);
            assert_resolves_to!(Type::F32, ffi_type_float);
            assert_resolves_to!(Type::F64, ffi_type_double);
            assert_resolves_to!(Type::Pointer, ffi_type_pointer);

            #[cfg(target_pointer_width = "32")]
            assert_resolves_to!(Type::Isize, ffi_type_sint32);
            #[cfg(target_pointer_width = "32")]
            assert_resolves_to!(Type::Usize, ffi_type_uint32);
            #[cfg(target_pointer_width = "64")]
            assert_resolves_to!(Type::Isize, ffi_type_sint64);
            #[cfg(target_pointer_width = "64")]
            assert_resolves_to!(Type::Usize, ffi_type_uint64);
        }
    }

    #[test]
    fn struct_resolves_to_correct_ffi_type() {
        let mut struct_elements: [*mut ffi_type; 4] = [
            &raw mut ffi_type_sint8,
            &raw mut ffi_type_uint16,
            &raw mut ffi_type_float,
            null_mut(),
        ];

        // The expected behavior for structs is that the size and alignment are initialized to 0
        // before `ffi_prep_cif`.
        let target_struct = ffi_type {
            type_: FFI_TYPE_STRUCT,
            elements: struct_elements.as_mut_ptr(),
            ..Default::default()
        };

        // SAFETY: The `Vec` provided to `Type::create_struct_unchecked` is not empty.
        let struct_type = unsafe {
            LibffiType::new(&Type::create_struct_unchecked(vec![
                Type::I8,
                Type::U16,
                Type::F32,
            ]))
        };

        // SAFETY: `struct_type` contains a pointer to a valid `ffi_type` that nobody else has a
        // pointer to.
        let struct_ref = unsafe { &*struct_type.0.as_ptr() };

        // SAFETY: Both references should be to valid `ffi_type`s that should not change. Both
        // should have well-formed `elements` arrays with pointers to `ffi_type`s.
        assert!(unsafe { ffi_types_equal(struct_ref, &target_struct) });
    }
}
