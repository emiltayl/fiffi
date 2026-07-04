extern crate alloc;

#[cfg(not(test))]
use alloc::boxed::Box;
#[cfg(not(test))]
use alloc::vec::Vec;
use core::ffi::c_uint;
use core::ptr::{NonNull, slice_from_raw_parts_mut};
use core::slice;

use libffi_sys::{ffi_cif, ffi_prep_cif, ffi_prep_cif_var, ffi_type};

use crate::abi::Abi;
use crate::errors::LibffiError;
use crate::types::raw::LibffiType;
use crate::types::{FfiTypeLayout, Type, VariadicType};

const _: () = {
    // This crate is only supported on architectures where a `u32` fits inside a `usize`. This
    // guarantees that the conversion will never fail (and therefore panic) if this crate compiles.
    assert!(size_of::<u32>() <= size_of::<usize>());
    // Extra check in case `c_uint` is something other than `u32`.
    assert!(size_of::<c_uint>() <= size_of::<usize>());
};

// The maximum number of arguments libffi can store in a `ffi_cif`. The static assert above ensures
// that `c_uint::MAX` will fit inside a `usize`.
pub(crate) const MAX_ARGS: usize = c_uint::MAX as usize;

#[derive(Debug)]
pub struct Cif {
    cif: NonNull<ffi_cif>,
    arguments: NonNull<LibffiType>,
    return_type: LibffiType,
    pub(crate) kind: CifKind,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum CifKind {
    Fixed,
    Variadic { n_fixed_args: c_uint },
}

impl Cif {
    pub fn new<'arg_types, I>(abi: Abi, argument_types: I, return_type: Option<&Type>) -> Self
    where
        I: IntoIterator<Item = &'arg_types Type>,
    {
        let cif_box = Box::new(ffi_cif::default());

        let args_slice: Box<[LibffiType]> = argument_types
            .into_iter()
            .take(MAX_ARGS)
            .map(LibffiType::new)
            .collect();

        // PANIC: `args_slice` takes a maximum of `MAX_ARGS` elements, so converting the length from
        // `usize` to `c_uint` should never panic.
        let n_args = c_uint::try_from(args_slice.len()).unwrap();

        // SAFETY: `Box::into_raw` will always provide a non-null pointer.
        let cif = unsafe { NonNull::new_unchecked(Box::into_raw(cif_box)) };

        // SAFETY: `Box::into_raw` will always provide a non-null pointer.
        let args_ptr = unsafe {
            // Cast `*mut [LibffiType]` to a thin `*mut LibffiType` as we do not want to store the
            // number of arguments in both `Cif` and its `ffi_cif`.
            NonNull::new_unchecked(Box::into_raw(args_slice).cast::<LibffiType>())
        };
        let args_raw_ptr = args_ptr
            .as_ptr()
            // Cast `LibffiType` to `*mut ffi_type`.
            .cast::<*mut ffi_type>();

        let return_type = return_type.map_or(LibffiType::VOID, LibffiType::new);

        // SAFETY:
        // * `cif` points to a properly aligned `ffi_cif` allocated by `Box`.
        // * `LibffiType` contains a valid pointer to a `ffi_type`.
        // * `args_raw_ptr` points to the first element in an array of `n_args` `LibffiType`s.
        let status = unsafe {
            ffi_prep_cif(
                cif.as_ptr(),
                abi.to_ffi_abi(),
                n_args,
                return_type.as_ffi_type_ptr(),
                args_raw_ptr,
            )
        };

        // This crate should ensure that `ffi_prep_cif` never returns an error as that may invoke
        // undefined behavior.
        //
        // * FfiBadTypeDef: A `Type` should never produce an invalid typedef, empty structs are not
        //   permitted.
        // * FfiBadAbi: The `Abi` type should not allow representation of invalid ABIs.
        // * FfiBadArgType: Only relevant for variadic functions.
        let error = LibffiError::from_status(status);
        if error.is_some() {
            // Re-materialize the `Box`es to deallocate their memory.

            // SAFETY: `cif` was allocated in a `Box` earlier in this function.
            let _cif_box = unsafe { Box::from_raw(cif.as_ptr()) };

            let args_slice_ptr = slice_from_raw_parts_mut(
                args_raw_ptr.cast::<LibffiType>(),
                usize::try_from(n_args).unwrap(),
            );

            // SAFETY: `args_slice_ptr` was allocated in a `Box` earlier in this function.
            let _args_slice_box = unsafe { Box::from_raw(args_slice_ptr) };

            panic!("Libffi returned the error code {status} from `ffi_prep_cif`.");
        }

        Self {
            cif,
            arguments: args_ptr,
            return_type,
            kind: CifKind::Fixed,
        }
    }

    pub fn variadic<'fixed_arg_types, 'variadic_arg_types, I1, I2>(
        abi: Abi,
        fixed_argument_types: I1,
        variadic_argument_types: I2,
        return_type: Option<&Type>,
    ) -> Self
    where
        I1: IntoIterator<Item = &'fixed_arg_types Type>,
        I2: IntoIterator<Item = &'variadic_arg_types VariadicType>,
    {
        let cif_box = Box::new(ffi_cif::default());

        let mut args_vec: Vec<LibffiType> = fixed_argument_types
            .into_iter()
            .take(MAX_ARGS)
            .map(LibffiType::new)
            .collect();

        let fixed_len = args_vec.len();

        // PANIC: `args_vec` takes a maximum of `MAX_ARGS` elements, so converting the length from
        // `usize` to `c_uint` should never panic.
        let n_fixed_args = c_uint::try_from(fixed_len).unwrap();

        let variadic_max_args = MAX_ARGS - fixed_len;
        let variadic_iter = variadic_argument_types
            .into_iter()
            .take(variadic_max_args)
            .map(|variadic_type| LibffiType::new(&variadic_type.to_type()));

        args_vec.extend(variadic_iter);

        // PANIC: `args_vec` takes a maximum of `MAX_ARGS` elements, so converting the length from
        // `usize` to `c_uint` should never panic.
        let n_total_args = c_uint::try_from(args_vec.len()).unwrap();

        // SAFETY: `Box::into_raw` will always provide a non-null pointer.
        let cif = unsafe { NonNull::new_unchecked(Box::into_raw(cif_box)) };

        // SAFETY: `Box::into_raw` will always provide a non-null pointer.
        let args_ptr = unsafe {
            // Cast `*mut [LibffiType]` to a thin `*mut LibffiType` as we do not want to store the
            // number of arguments in both `Cif` and its `ffi_cif`.
            NonNull::new_unchecked(Box::into_raw(args_vec.into_boxed_slice()).cast::<LibffiType>())
        };
        let args_raw_ptr = args_ptr
            .as_ptr()
            // Cast `LibffiType` to `*mut ffi_type`.
            .cast::<*mut ffi_type>();

        let return_type = return_type.map_or(LibffiType::VOID, LibffiType::new);

        // SAFETY:
        // * `cif` points to a properly aligned `ffi_cif` allocated by `Box`.
        // * `LibffiType` contains a valid pointer to a `ffi_type`.
        // * `args_raw_ptr` points to the first element in an array of `n_total_args` `LibffiType`s.
        let status = unsafe {
            ffi_prep_cif_var(
                cif.as_ptr(),
                abi.to_ffi_abi(),
                n_fixed_args,
                n_total_args,
                return_type.as_ffi_type_ptr(),
                args_raw_ptr,
            )
        };

        if LibffiError::from_status(status).is_some() {
            // Re-materialize the `Box`es to deallocate their memory.

            // SAFETY: `cif` was allocated in a `Box` earlier in this function.
            let _cif_box = unsafe { Box::from_raw(cif.as_ptr()) };

            let args_slice_ptr = slice_from_raw_parts_mut(
                args_raw_ptr.cast::<LibffiType>(),
                usize::try_from(n_total_args).unwrap(),
            );

            // SAFETY: `args_slice_ptr` was allocated from a `Box` earlier in this function.
            let _args_slice_box = unsafe { Box::from_raw(args_slice_ptr) };

            // `Type`, `Abi`, and `VariadicType` should make every libffi error here
            // unrepresentable through safe APIs.
            panic!("Libffi returned the error code {status} from `ffi_prep_cif_var`.");
        }

        Self {
            cif,
            arguments: args_ptr,
            return_type,
            kind: CifKind::Variadic { n_fixed_args },
        }
    }

    pub fn as_ffi_cif_ptr(&self) -> *mut ffi_cif {
        self.cif.as_ptr()
    }

    pub fn argument_count(&self) -> usize {
        // SAFETY: `self.cif` points to a properly initialized `ffi_cif` that should never change
        // as long as `self` is alive.
        let self_cif = unsafe { &*self.cif.as_ptr() };

        // PANICS: The static assertion at the top of this module guarantees that
        // `usize::try_from(u32)` will never fail if the code compiles.
        usize::try_from(self_cif.nargs).unwrap()
    }

    pub fn argument_layouts(&self) -> Vec<FfiTypeLayout> {
        let n_args = self.argument_count();

        // SAFETY: `self.arguments` is an initialized array of `LibffiType`s with `n_args` elements
        // that is managed by `self` and will not change while `self` is alive.
        let ffi_type_slice = unsafe { slice::from_raw_parts(self.arguments.as_ptr(), n_args) };

        let output_vec: Vec<FfiTypeLayout> =
            ffi_type_slice.iter().map(LibffiType::layout).collect();

        output_vec
    }

    pub fn return_layout(&self) -> FfiTypeLayout {
        self.return_type.layout()
    }
}

impl Clone for Cif {
    fn clone(&self) -> Self {
        // SAFETY: `self.cif` points to a properly initialized `ffi_cif` that should never change
        // as long as `self` is alive.
        let self_cif = unsafe { &*self.cif.as_ptr() };
        let n_args = self_cif.nargs;
        let abi = self_cif.abi;

        let cloned_cif_box = Box::new(ffi_cif::default());

        // SAFETY: `self.arguments` is a pointer to the first element in a slice of `LibffiType`
        // with `(*self.cif).nargs` elements allocated by `Box`. `LibffiType`s are not modified
        // after initialization and will be alive as long as `self` as the memory is managed by
        // `self`.
        let cloned_args_slice: Box<[LibffiType]> = unsafe {
            // PANICS: The static assertion at the top of this module guarantees that
            // `usize::try_from(u32)` will never fail if the code compiles.
            slice::from_raw_parts(self.arguments.as_ptr(), usize::try_from(n_args).unwrap())
                .iter()
                .cloned()
                .collect()
        };

        // SAFETY: `Box::into_raw` will always provide a non-null pointer.
        let cloned_cif = unsafe { NonNull::new_unchecked(Box::into_raw(cloned_cif_box)) };

        // SAFETY: `Box::into_raw` will always provide a non-null pointer.
        let cloned_args_ptr = unsafe {
            NonNull::new_unchecked(Box::into_raw(cloned_args_slice).cast::<LibffiType>())
        };
        let cloned_args_raw_ptr = cloned_args_ptr
            // Get the raw fat pointer.
            .as_ptr()
            // Convert `LibffiType` to `*mut ffi_type`.
            .cast::<*mut ffi_type>();

        let cloned_return_type = self.return_type.clone();

        // SAFETY:
        // * `cif` points to a properly aligned `ffi_cif` allocated by `Box`.
        // * `LibffiType` should contain a valid pointer to a `ffi_type`.
        // * `args_raw_ptr` points to the first element in an array of `n_args` `LibffiType`s.
        let status = unsafe {
            match self.kind {
                CifKind::Fixed => ffi_prep_cif(
                    cloned_cif.as_ptr(),
                    abi,
                    n_args,
                    cloned_return_type.as_ffi_type_ptr(),
                    cloned_args_raw_ptr,
                ),
                CifKind::Variadic { n_fixed_args } => ffi_prep_cif_var(
                    cloned_cif.as_ptr(),
                    abi,
                    n_fixed_args,
                    n_args,
                    cloned_return_type.as_ffi_type_ptr(),
                    cloned_args_raw_ptr,
                ),
            }
        };

        // If `self` was successfully created, cloning should never fail.
        let error = LibffiError::from_status(status);
        if error.is_some() {
            // Re-materialize the `Box`es to deallocate their memory.

            // SAFETY: `cloned_cif` was allocated in a `Box` earlier in this function.
            let _cif_box = unsafe { Box::from_raw(cloned_cif.as_ptr()) };

            let cloned_args_slice_ptr = slice_from_raw_parts_mut(
                cloned_args_raw_ptr.cast::<LibffiType>(),
                usize::try_from(n_args).unwrap(),
            );

            // SAFETY: `cloned_args_slice_ptr` was allocated in a `Box` earlier in this function.
            let _args_slice_box = unsafe { Box::from_raw(cloned_args_slice_ptr) };

            panic!("Libffi returned the error code {status} from `ffi_prep_cif`.");
        }

        Self {
            cif: cloned_cif,
            arguments: cloned_args_ptr,
            return_type: cloned_return_type,
            kind: self.kind,
        }
    }
}

impl Drop for Cif {
    fn drop(&mut self) {
        // SAFETY: `self.cif` was created from `Box::into_raw`.
        let cif_box = unsafe { Box::from_raw(self.cif.as_ptr()) };
        let n_args = cif_box.nargs;
        drop(cif_box);

        // SAFETY: `self.arguments` is a pointer to the first element in a slice of `LibffiType`
        // with `(*self.cif).nargs` elements allocated by `Box` and managed by `self`.
        let args_box = unsafe {
            // PANICS: The static assertion at the top of this module guarantees that
            // `usize::try_from(u32)` will never fail if the code compiles.
            let args_slice_ptr =
                slice_from_raw_parts_mut(self.arguments.as_ptr(), usize::try_from(n_args).unwrap());

            Box::from_raw(args_slice_ptr)
        };

        drop(args_box);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_clone_drop_cifs() {
        let void_cif = Cif::new(Abi::default(), &[], None);
        let void_clone = void_cif.clone();
        let _void_clone2 = void_cif.clone();
        drop(void_cif);
        let _void_clone3 = void_clone.clone();

        let everything_cif = Cif::new(
            Abi::default(),
            &[
                Type::I8,
                Type::U8,
                Type::I16,
                Type::U16,
                Type::I32,
                Type::U32,
                Type::I64,
                Type::U64,
                Type::Isize,
                Type::Usize,
                Type::F32,
                Type::F64,
                Type::Pointer,
                Type::create_struct(vec![Type::U32, Type::U32]).unwrap(),
            ],
            Some(&Type::Pointer),
        );

        let everything_clone = everything_cif.clone();
        let _everything_clone2 = everything_cif.clone();
        drop(everything_cif);
        let _everything_clone3 = everything_clone.clone();
    }

    #[test]
    fn create_clone_drop_variadic_cifs() {
        let void_cif = Cif::variadic(
            Abi::default(),
            &[Type::Pointer],
            &[VariadicType::Usize],
            None,
        );
        let void_clone = void_cif.clone();
        let _void_clone2 = void_cif.clone();
        drop(void_cif);
        let _void_clone3 = void_clone.clone();
    }

    #[test]
    fn cif_clone_clones_ffi_cif_and_ffi_type_structs() {
        let cif = Cif::new(
            Abi::default(),
            &[Type::create_struct(vec![Type::I8]).unwrap(), Type::U8],
            Some(&Type::create_struct(vec![Type::U8]).unwrap()),
        );

        let cif_clone = cif.clone();

        // Compare the pointers to ensure that they do not point to the same structs.
        assert_ne!(
            cif.cif, cif_clone.cif,
            "Pointer to `ffi_cif` not changed after cloning `Cif`."
        );
        assert_ne!(
            cif.arguments, cif_clone.arguments,
            "Pointer to array of `ffi_type` argument definitions not changed after cloning `Cif`."
        );
        assert_ne!(
            cif.return_type.as_ffi_type_ptr(),
            cif_clone.return_type.as_ffi_type_ptr(),
            "Pointer to `ffi_type` return type definition not changed after cloning `Cif` even though the type is a struct."
        );

        // Make sure that the struct in `cif.arguments` is a clone.
        // SAFETY: `cif` and `cif_clone` manages a slice of two `LibffiType`s each. Nobody else can
        // access these slices.
        unsafe {
            let original_type_ptr = *cif.arguments.as_ptr().cast::<*mut ffi_type>();
            let clone_type_ptr = *cif_clone.arguments.as_ptr().cast::<*mut ffi_type>();

            assert_ne!(
                original_type_ptr, clone_type_ptr,
                "Pointer to `ffi_type` argument type definition not changed after cloning `Cif` even though the type is a struct.",
            );
        }
    }
}
