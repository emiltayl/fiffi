//! Traits that implement functionality required to use Rust closures with
//! [`Closure`](crate::closure::Closure).
//!
//! These traits are implemented by the crate for supported closure signatures. The traits in this
//! module is for internal use only.

use core::ffi::c_void;
use core::mem::MaybeUninit;

use libffi_sys::ffi_cif;

use crate::return_buffer::write_closure_result;
use crate::types::{FfiType, Type};
#[cfg(msan)]
use crate::{__msan_poison, __msan_test_shadow, __msan_unpoison};

pub(crate) mod internal {
    pub struct Token;

    pub trait FfiReturnTypeSuper {}
    pub trait FfiArgsSuper {}
    pub trait CallbackSuper<ARGS, RET> {}
}

/// A type that can be returned from a libffi closure callback.
///
/// This trait cannot be implemented manually, but blanket implementations are provided for all
/// types that implement `FfiType` as well as for `()` to support `void` return values.
///
/// # Safety
///
/// See [`FfiType`].
pub unsafe trait FfiReturnType: internal::FfiReturnTypeSuper {
    /// Returns the [`Type`] the type implementing `FfiReturnType` corresponds to.
    ///
    /// Uses the type's [`FfiType`] implementation to retrieve this except for `()`, which returns
    /// `None`.
    #[doc(hidden)]
    fn return_type(_: internal::Token) -> Option<Type>;
}

impl<T> internal::FfiReturnTypeSuper for T where T: FfiType {}

// SAFETY: This blanket implementation assumes that all implementations of `FfiType` are correct.
unsafe impl<T> FfiReturnType for T
where
    T: FfiType,
{
    fn return_type(_: internal::Token) -> Option<Type> {
        Some(<T as FfiType>::ffi_type())
    }
}

impl internal::FfiReturnTypeSuper for () {}

// SAFETY: `()` maps to libffi's void return type.
unsafe impl FfiReturnType for () {
    fn return_type(_: internal::Token) -> Option<Type> {
        None
    }
}

/// A tuple of argument types that can be passed to a libffi closure callback.
///
/// This trait cannot be implemented manually, but is implemented for all tuples of `FfiType`s up to
/// 16 elements.
///
/// # Safety
///
/// Implementors must return argument type descriptions in the exact order and count that libffi
/// will provide to the callback. Each described Rust type must satisfy [`FfiType`].
pub unsafe trait FfiArgs: internal::FfiArgsSuper {
    /// Array with a [`Type`] for each argument passed to the closure.
    #[doc(hidden)]
    type TypeArray;

    /// Returns the libffi type descriptions for this argument tuple.
    #[doc(hidden)]
    fn as_type_array() -> Self::TypeArray;
}

impl internal::FfiArgsSuper for () {}

// SAFETY: The empty tuple describes a callback with no arguments.
unsafe impl FfiArgs for () {
    type TypeArray = [Type; 0];

    fn as_type_array() -> Self::TypeArray {
        []
    }
}

macro_rules! count_idents {
    () => {0usize};
    ($head:ident $(,)*) => {1usize};
    ($head:ident , $($tail:ident),+ $(,)*) => {1usize + count_idents!($($tail),*)};
}

// Ensure `count_idents` counts correctly.
const _: () = {
    assert!(count_idents!() == 0);
    assert!(count_idents!(a) == 1);
    assert!(count_idents!(a, b) == 2);
    assert!(count_idents!(a, b, c, d, e) == 5);
};

macro_rules! impl_ffi_args_for_tuple {
    ($($placeholder:ident),* $(,)*) => {
        impl<$($placeholder,)*> internal::FfiArgsSuper for ($($placeholder,)*)
        where
            $($placeholder: FfiType,)*
        {}

        // SAFETY: Every tuple element implements `FfiType`, and the generated array preserves the
        // tuple's order and argument count.
        unsafe impl<$($placeholder,)*> FfiArgs for ($($placeholder,)*)
        where
            $($placeholder: FfiType,)*
        {
            type TypeArray = [Type; count_idents!($($placeholder,)*)];

            fn as_type_array() -> Self::TypeArray {
                [
                    $(<$placeholder as FfiType>::ffi_type(),)*
                ]
            }
        }
    };
}

impl_ffi_args_for_tuple!(T0);
impl_ffi_args_for_tuple!(T0, T1);
impl_ffi_args_for_tuple!(T0, T1, T2);
impl_ffi_args_for_tuple!(T0, T1, T2, T3);
impl_ffi_args_for_tuple!(T0, T1, T2, T3, T4);
impl_ffi_args_for_tuple!(T0, T1, T2, T3, T4, T5);
impl_ffi_args_for_tuple!(T0, T1, T2, T3, T4, T5, T6);
impl_ffi_args_for_tuple!(T0, T1, T2, T3, T4, T5, T6, T7);
impl_ffi_args_for_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8);
impl_ffi_args_for_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9);
impl_ffi_args_for_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
impl_ffi_args_for_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
impl_ffi_args_for_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12);
impl_ffi_args_for_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13);
impl_ffi_args_for_tuple!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14
);
impl_ffi_args_for_tuple!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15
);

/// A Rust closure that can be invoked from a libffi closure trampoline.
///
/// This trait cannot be implemented manually, but is implemented for all closures with up to 16
/// arguments that each implement [`FfiType`] and a return value that implements [`FfiReturnType`].
///
/// Note that `Callback` is only implemented for `Fn + Send + Sync` closures.
///
/// # Safety
///
/// The closures implementing this trait must be `Fn + Send + Sync` closures that accept arguments
/// as described by `ARGS` and return a `RET` value.
pub unsafe trait Callback<ARGS, RET>:
    internal::CallbackSuper<ARGS, RET> + Sized + Send + Sync
where
    ARGS: FfiArgs,
    RET: FfiReturnType,
{
    /// Callback for libffi's closure trampoline that retrieves the arguments and calls the Rust
    /// closure.
    ///
    /// This function should not be called manually, but only by libffi.
    ///
    /// # Safety
    ///
    /// * `args` must point to an array of argument pointers with one pointer for every type in
    ///   `ARGS`.
    /// * `result_ptr` must be writable for `RET`, or in the case of integers that are smaller than
    ///   a full register, writable for a [`ReturnBuffer`]. Note that while this function signature
    ///   is incorrect for small integers, libffi allocates sufficient storage.
    /// * `closure` must point to the Rust closure owned by a [`crate::closure::Closure`].
    #[doc(hidden)]
    unsafe extern "C" fn callback(
        cif: *mut ffi_cif,
        result_ptr: *mut MaybeUninit<RET>,
        args: *mut *mut c_void,
        closure: *const Self,
    );
}

macro_rules! read_single_argument {
    ($args_ptr:ident, $name:ident: $type:ident) => {
        // SAFETY: `$args_ptr` is provided by libffi and points to this argument's entry in
        // the closure argument pointer array. The entry points to initialized storage for
        // `$type`. On 32-bit Windows, libffi does not guarantee natural alignment for every
        // argument value, so the read must be unaligned there.
        let $name: $type = unsafe {
            let arg_ptr = *$args_ptr;

            // Memory may point to space allocated by libffi that needs unpoisoning for msan.
            #[cfg(msan)]
            let poisoned = {
                if __msan_test_shadow(arg_ptr, size_of::<$type>()) != -1 {
                    __msan_unpoison(arg_ptr.cast(), size_of::<$type>());

                    true
                } else {
                    false
                }
            };

            let val: $type = if cfg!(all(target_arch = "x86", target_os = "windows")) {
                arg_ptr.cast::<$type>().read_unaligned()
            } else {
                arg_ptr.cast::<$type>().read()
            };

            #[cfg(msan)]
            if poisoned {
                __msan_poison(arg_ptr.cast(), size_of::<$type>());
            }

            val
        };
    };
}

macro_rules! read_next_arguments {
    ($args_ptr:ident) => {};
    ($args_ptr:ident, $name:ident: $type:ident $(, $rest_name:ident: $rest_type:ident)* $(,)*) => {
        // SAFETY: Libffi provides one pointer-array entry for every argument in the closure
        // signature, and this expansion advances from the previous argument to the next one.
        let $args_ptr = unsafe { $args_ptr.add(1) };
        read_single_argument!($args_ptr, $name: $type);
        read_next_arguments!($args_ptr $(, $rest_name: $rest_type)*);
    };
}

macro_rules! read_arguments {
    ($args_ptr:ident,) => {};
    ($args_ptr:ident, $name:ident: $type:ident $(, $rest_name:ident: $rest_type:ident)* $(,)*) => {
        read_single_argument!($args_ptr, $name: $type);
        read_next_arguments!($args_ptr $(, $rest_name: $rest_type)*);
    };
}

macro_rules! impl_callback_for_fn {
    (fn($($name:ident: $type:ident),* $(,)*)) => {
        impl<$($type,)* FN> internal::CallbackSuper<($($type,)*), ()> for FN
        where
            $($type: FfiType,)*
            FN: Fn($($type),*) + Send + Sync,
        {}

        // SAFETY: The generated callback reads arguments according to their `FfiType`
        // descriptions and calls `FN` without writing a return value.
        unsafe impl<$($type,)* FN> Callback<($($type,)*), ()> for FN
        where
            $($type: FfiType,)*
            FN: Fn($($type),*) + Send + Sync,
        {
            unsafe extern "C" fn callback(
                _cif: *mut ffi_cif,
                _result_ptr: *mut MaybeUninit<()>,
                _args: *mut *mut c_void,
                closure: *const FN,
            ) {
                read_arguments!(_args, $($name: $type),*);

                // SAFETY: This function should only be called by libffi with a pointer to a closure
                // managed by a `Closure`.
                unsafe { (*closure)($($name,)*) }
            }
        }

        impl<$($type,)* RET, FN> internal::CallbackSuper<($($type,)*), RET> for FN
        where
            $($type: FfiType,)*
            RET: FfiType,
            FN: Fn($($type),*) -> RET + Send + Sync,
        {}

        // SAFETY: The generated callback reads arguments according to their `FfiType`
        // descriptions, calls `FN`, and writes the returned `RET` using `write_closure_result`.
        unsafe impl<$($type,)* RET, FN> Callback<($($type,)*), RET> for FN
        where
            $($type: FfiType,)*
            RET: FfiType,
            FN: Fn($($type),*) -> RET + Send + Sync,
        {
            unsafe extern "C" fn callback(
                _cif: *mut ffi_cif,
                result_ptr: *mut MaybeUninit<RET>,
                _args: *mut *mut c_void,
                closure: *const FN,
            ) {
                read_arguments!(_args, $($name: $type),*);

                // SAFETY: `callback` should only be called by libffi for `Closure`s that own the
                // allocation of `closure`.
                let return_value = unsafe { (*closure)($($name,)*) };

                // SAFETY: Libffi is responsible for making sure that it is valid to write the
                // result to `result_ptr`.
                unsafe { write_closure_result(return_value, &<RET as FfiType>::ffi_type(), result_ptr) }
            }
        }
    };
}

impl_callback_for_fn!(fn());
impl_callback_for_fn!(fn(v0: T0));
impl_callback_for_fn!(fn(v0: T0, v1: T1));
impl_callback_for_fn!(fn(v0: T0, v1: T1, v2: T2));
impl_callback_for_fn!(fn(v0: T0, v1: T1, v2: T2, v3: T3));
impl_callback_for_fn!(fn(v0: T0, v1: T1, v2: T2, v3: T3, v4: T4));
impl_callback_for_fn!(fn(v0: T0, v1: T1, v2: T2, v3: T3, v4: T4, v5: T5));
impl_callback_for_fn!(fn(v0: T0, v1: T1, v2: T2, v3: T3, v4: T4, v5: T5, v6: T6));
impl_callback_for_fn!(fn(v0: T0, v1: T1, v2: T2, v3: T3, v4: T4, v5: T5, v6: T6, v7: T7));
impl_callback_for_fn!(fn(v0: T0, v1: T1, v2: T2, v3: T3, v4: T4, v5: T5, v6: T6, v7: T7, v8: T8));
impl_callback_for_fn!(
    fn(v0: T0, v1: T1, v2: T2, v3: T3, v4: T4, v5: T5, v6: T6, v7: T7, v8: T8, v9: T9)
);
impl_callback_for_fn!(
    fn(v0: T0, v1: T1, v2: T2, v3: T3, v4: T4, v5: T5, v6: T6, v7: T7, v8: T8, v9: T9, v10: T10)
);
impl_callback_for_fn!(
    fn(v0: T0, v1: T1, v2: T2, v3: T3, v4: T4, v5: T5, v6: T6, v7: T7, v8: T8, v9: T9, v10: T10,
        v11: T11)
);
impl_callback_for_fn!(
    fn(v0: T0, v1: T1, v2: T2, v3: T3, v4: T4, v5: T5, v6: T6, v7: T7, v8: T8, v9: T9, v10: T10,
        v11: T11, v12: T12)
);
impl_callback_for_fn!(
    fn(v0: T0, v1: T1, v2: T2, v3: T3, v4: T4, v5: T5, v6: T6, v7: T7, v8: T8, v9: T9, v10: T10,
        v11: T11, v12: T12, v13: T13)
);
impl_callback_for_fn!(
    fn(v0: T0, v1: T1, v2: T2, v3: T3, v4: T4, v5: T5, v6: T6, v7: T7, v8: T8, v9: T9, v10: T10,
        v11: T11, v12: T12, v13: T13, v14: T14)
);
impl_callback_for_fn!(
    fn(v0: T0, v1: T1, v2: T2, v3: T3, v4: T4, v5: T5, v6: T6, v7: T7, v8: T8, v9: T9, v10: T10,
        v11: T11, v12: T12, v13: T13, v14: T14, v15: T15)
);

#[cfg(test)]
mod tests {
    use core::ptr::null_mut;
    use std::ptr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::test_utils::{I16_ARG, U8_ARG, U32_ARG, U64_ARG};

    /// Calls a generated callback directly from tests.
    ///
    /// # Safety
    ///
    /// The `cif`, `result_ptr`, `args`, and `closure` arguments must satisfy
    /// [`Callback::callback`]'s contract for `ARGS`, `RET`, and `FN`.
    unsafe fn call_callback<ARGS, RET, FN>(
        cif: *mut ffi_cif,
        result_ptr: *mut MaybeUninit<RET>,
        args: *mut *mut c_void,
        closure: &FN,
    ) where
        ARGS: FfiArgs,
        RET: FfiReturnType,
        FN: Callback<ARGS, RET>,
    {
        // SAFETY: It is up to the caller to ensure that safety requirements are fulfilled.
        unsafe { FN::callback(cif, result_ptr, args, ptr::from_ref(closure)) }
    }

    #[test]
    fn callback_no_args_no_ret_runs_closure() {
        let flag = AtomicUsize::new(0);

        let closure = || flag.store(0xABCD, Ordering::Relaxed);

        // SAFETY: This callback has no arguments and no return value, and `closure` lives for the
        // duration of the call.
        unsafe {
            call_callback(null_mut(), null_mut(), null_mut(), &closure);
        }

        assert_eq!(flag.load(Ordering::Relaxed), 0xABCD);
    }

    #[test]
    fn callback_no_args_with_return_writes_result() {
        let closure = || 0xDEAD_BEEFu32;

        let mut ret = MaybeUninit::<usize>::uninit();

        // SAFETY: This callback has no arguments, `ret` is a writable full return buffer for the
        // small `u32` return, and `closure` lives for the duration of the call.
        unsafe {
            call_callback(null_mut(), ret.as_mut_ptr().cast(), null_mut(), &closure);
        }

        // SAFETY: The callback initialized `ret` above.
        let value = unsafe { ret.assume_init() };
        let u32_value = u32::try_from(value).unwrap();
        assert_eq!(u32_value, 0xDEAD_BEEF);
    }

    #[test]
    fn callback_two_args_with_return_reads_args_and_writes_result() {
        let closure = |a: u32, b: u64| u64::from(a) + b;

        let args: [*mut c_void; 2] = [
            (&raw const U32_ARG).cast_mut().cast(),
            (&raw const U64_ARG).cast_mut().cast(),
        ];

        let mut ret = MaybeUninit::<u64>::uninit();

        // SAFETY: `args` contains pointers to the two argument values expected by `closure`, `ret`
        // is writable storage for the `u64` return, and `closure` lives for the duration of the
        // call.
        unsafe {
            call_callback(
                null_mut(),
                ret.as_mut_ptr().cast(),
                args.as_ptr().cast_mut(),
                &closure,
            );
        }

        // SAFETY: The callback initialized `ret` above.
        let value = unsafe { ret.assume_init() };
        let expected = u64::from(U32_ARG) + U64_ARG;
        assert_eq!(value, expected);
    }

    #[test]
    fn callback_three_args_no_ret_reads_args() {
        let closure = |a: u8, b: i16, c: u32| {
            assert_eq!(a, U8_ARG);
            assert_eq!(b, I16_ARG);
            assert_eq!(c, U32_ARG);
        };

        let args: [*mut c_void; 3] = [
            (&raw const U8_ARG).cast_mut().cast(),
            (&raw const I16_ARG).cast_mut().cast(),
            (&raw const U32_ARG).cast_mut().cast(),
        ];

        // SAFETY: `args` contains pointers to the three argument values expected by `closure`, and
        // `closure` lives for the duration of the call.
        unsafe {
            call_callback(null_mut(), null_mut(), args.as_ptr().cast_mut(), &closure);
        }
    }

    #[test]
    fn callback_sixteen_args_with_return_reads_args_and_writes_result() {
        let values = [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

        let args: [*mut c_void; 16] =
            core::array::from_fn(|index| ptr::from_ref(&values[index]).cast_mut().cast());

        #[rustfmt::skip]
        let closure =
            |v0: u8, v1: u8, v2: u8, v3: u8, v4: u8, v5: u8, v6: u8, v7: u8, v8: u8, v9: u8, v10: u8,
            v11: u8, v12: u8, v13: u8, v14: u8, v15: u8| -> u64 {
                let received = [
                    v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14, v15,
                ];
                assert_eq!(received, values);

                received.into_iter().map(u64::from).sum()
            };

        let mut ret = MaybeUninit::<u64>::uninit();

        // SAFETY: `args` contains pointers to the 16 argument values expected by `closure`, `ret`
        // is writable storage for the `u64` return, and `closure` lives for the duration of the
        // call.
        unsafe {
            #[rustfmt::skip]
            call_callback::<
                (u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8),
                u64,
                _,
            >(
                null_mut(),
                ret.as_mut_ptr().cast(),
                args.as_ptr().cast_mut(),
                &closure,
            );
        }

        // SAFETY: The callback initialized `ret` above.
        let value = unsafe { ret.assume_init() };
        let expected = values.into_iter().map(u64::from).sum();

        assert_eq!(value, expected);
    }
}
