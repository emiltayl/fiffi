pub mod traits;

use core::ffi::c_void;
use core::fmt::Debug;
use core::marker::PhantomData;
use core::mem::{MaybeUninit, transmute};

use libffi_sys::ffi_cif;
use traits::{Callback, FfiArgs, FfiReturnType};

use crate::FnPtr;
use crate::abi::Abi;
use crate::closure::BaseClosure;
use crate::closure::raw::{ClosureCallback, Context};
use crate::errors::ClosureAllocationError;
use crate::function::raw::Cif;
use crate::types::Type;

/// A wrapper for Rust closures that enables creating function pointers to the closures.
///
/// The function pointer returned by [`Closure::as_fn_ptr`] remains valid only while the underlying
/// `Closure` is alive.
///
/// The Rust closure must implement `Fn(..) -> .. + Send + Sync`. Every argument must implement
/// [`FfiType`] and if there is a return value, its type must also implement [`FfiType`]. Up to 16
/// arguments are currently supported. Use thread-safe interior mutability such as atomics or locks
/// if your closure requires mutable state.
///
/// # Examples
///
/// ```
/// use fiffi::closure::Closure;
/// use fiffi::types::{FfiType, Type};
///
/// #[derive(Copy, Clone, Debug, PartialEq, Eq)]
/// #[repr(C)]
/// struct FfiStruct(i32, i32);
///
/// // SAFETY: `FfiType` is a `#[repr(C)]` struct that contains two `i32`s.
/// unsafe impl FfiType for FfiStruct {
///     fn ffi_type() -> Type {
///         // SAFETY: The vec provided to `Type::create_struct_unchecked` is not empty.
///         unsafe { Type::create_struct_unchecked(vec![Type::I32, Type::I32]) }
///     }
/// }
///
/// let sum_ffi_struct = Closure::new(|value: FfiStruct| value.0 + value.1);
///
/// // SAFETY: `sum_ffi_struct.as_fn_ptr` returns a function pointer to a extern "C" function that
/// // accepts one `FfiStruct` argument and returns a `i32` value.
/// let add_one_fn_ptr = unsafe {
///     sum_ffi_struct
///         .as_fn_ptr()
///         .into_fn::<extern "C" fn(FfiStruct) -> i32>()
/// };
///
/// let input_struct = FfiStruct(40, 2);
/// assert_eq!(add_one_fn_ptr(input_struct), 42);
/// ```
///
/// `FnMut` closures are rejected:
///
/// ```compile_fail
/// use fiffi::closure::Closure;
///
/// let mut counter = 0usize;
///
/// // The following line fails to compile as the closure is `FnMut`.
/// let closure = Closure::new(|| {
///     counter += 1;
/// });
/// ```
///
/// Closures that capture non-`Send + Sync` state are also rejected:
///
/// ```compile_fail
/// use std::cell::Cell;
/// use std::rc::Rc;
///
/// use fiffi::closure::Closure;
///
/// let counter = Rc::new(Cell::new(0usize));
/// let counter_ref = Rc::clone(&counter);
///
/// // The following line fails to compile as the closure is not `Send + Sync`.
/// let closure = Closure::new(move || {
///     counter_ref.set(counter_ref.get() + 1);
/// });
/// ```
///
/// [`FfiType`]: `crate::types::FfiType`
pub struct Closure<ARGS, RET, FN>
where
    ARGS: FfiArgs,
    RET: FfiReturnType,
    FN: Callback<ARGS, RET>,
{
    closure: BaseClosure<FN>,
    _marker: PhantomData<fn(ARGS) -> RET>,
}

impl<ARGS, RET, FN, const N: usize> Closure<ARGS, RET, FN>
where
    ARGS: FfiArgs<TypeArray = [Type; N]>,
    RET: FfiReturnType,
    FN: Callback<ARGS, RET>,
{
    /// Creates a closure using the target's default ABI.
    ///
    /// # Panics
    ///
    /// Panics if libffi cannot allocate closure storage. Use [`Closure::try_new`] if you need to
    /// avoid the potential panic.
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::atomic::{AtomicUsize, Ordering};
    ///
    /// use fiffi::closure::Closure;
    ///
    /// let counter = AtomicUsize::new(0);
    ///
    /// // The closure must be `Fn`, so a mutable reference to counter cannot be used.
    /// let increment = Closure::new(|| {
    ///     counter.fetch_add(1, Ordering::Relaxed);
    /// });
    ///
    /// // SAFETY: `increment.as_fn_ptr` returns a function pointer to a extern "C" function that
    /// // does not take any arguments and does not return a value.
    /// let increment_fn_ptr = unsafe { increment.as_fn_ptr().into_fn::<extern "C" fn()>() };
    ///
    /// increment_fn_ptr();
    ///
    /// assert_eq!(counter.load(Ordering::Relaxed), 1);
    /// ```
    pub fn new(rust_closure: FN) -> Self {
        Self::try_new(rust_closure).expect("Libffi was unable to allocate the closure.")
    }

    /// Creates a closure using the provided [`Abi`].
    ///
    /// # Panics
    ///
    /// Panics if libffi cannot allocate closure storage. Use [`Closure::try_with_abi`] if you need
    /// to avoid the potential panic.
    ///
    /// # Example
    ///
    /// ```
    /// use fiffi::abi::Abi;
    /// use fiffi::closure::Closure;
    ///
    /// let closure = Closure::with_abi(|value: i32| value, Abi::default());
    /// ```
    pub fn with_abi(rust_closure: FN, abi: Abi) -> Self {
        Self::try_with_abi(rust_closure, abi).expect("Libffi was unable to allocate the closure.")
    }

    /// Creates a closure using the target's default ABI.
    ///
    /// # Errors
    ///
    /// Returns [`ClosureAllocationError`] if libffi cannot allocate closure storage.
    ///
    /// # Example
    ///
    /// ```
    /// use fiffi::closure::Closure;
    ///
    /// let closure = Closure::try_new(|| 42)?;
    /// assert!(!closure.as_fn_ptr().as_c_void_ptr().is_null());
    ///
    /// # Ok::<(), fiffi::errors::ClosureAllocationError>(())
    /// ```
    pub fn try_new(rust_closure: FN) -> Result<Self, ClosureAllocationError> {
        Self::try_with_abi(rust_closure, Abi::default())
    }

    /// Creates a closure using the provided [`Abi`].
    ///
    /// # Errors
    ///
    /// Returns [`ClosureAllocationError`] if libffi cannot allocate closure storage.
    ///
    /// # Example
    ///
    /// ```
    /// use fiffi::abi::Abi;
    /// use fiffi::closure::Closure;
    ///
    /// let closure = Closure::try_with_abi(|| {}, Abi::default())?;
    /// assert!(!closure.as_fn_ptr().as_c_void_ptr().is_null());
    ///
    /// # Ok::<(), fiffi::errors::ClosureAllocationError>(())
    /// ```
    pub fn try_with_abi(rust_closure: FN, abi: Abi) -> Result<Self, ClosureAllocationError> {
        let arg_types = ARGS::as_type_array();

        let return_type = RET::return_type(traits::internal::Token);

        // SAFETY:
        // * The second argument is a pointer to a possibly uninitialized buffer that libffi will
        //   write the return value to.
        // * The fourth argument is a pointer to the Rust closure, which libffi passes to the
        //   callback function `FN::callback`.
        let closure_handler_fn = unsafe {
            transmute::<
                unsafe extern "C" fn(
                    *mut ffi_cif,
                    *mut MaybeUninit<RET>,
                    *mut *mut c_void,
                    *const FN,
                ),
                ClosureCallback,
            >(FN::callback)
        };

        let cif = Cif::new(abi, &arg_types, return_type.as_ref());
        let context = Context::new(rust_closure);

        // SAFETY:
        // * `cif` was prepared for `arg_types`, `return_type`, and `abi`.
        // * `closure_handler_fn` is `FN::callback` with the generic return and payload pointers
        //   erased to the callback types expected by libffi.
        // * `FN::callback` interprets the arguments and return storage according to `cif`.
        let closure = unsafe { BaseClosure::try_new(cif, closure_handler_fn, context)? };

        Ok(Self {
            closure,
            _marker: PhantomData,
        })
    }

    /// Returns a function pointer that can be used to call this closure.
    ///
    /// The pointer is valid only while `self` remains alive.
    ///
    /// # Example
    ///
    /// ```
    /// use fiffi::closure::Closure;
    ///
    /// let closure = Closure::new(|| 42);
    /// let fn_ptr = closure.as_fn_ptr();
    ///
    /// // `fn_ptr` can now be called and sent across FFI boundaries for example as a callback.
    /// ```
    pub fn as_fn_ptr(&self) -> FnPtr {
        self.closure.as_fn_ptr()
    }
}

impl<ARGS, RET, FN> Debug for Closure<ARGS, RET, FN>
where
    ARGS: FfiArgs,
    RET: FfiReturnType,
    FN: Callback<ARGS, RET>,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Closure")
            .field("closure", &self.closure)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use core::cell::UnsafeCell;
    use core::hint::black_box;

    use super::*;
    use crate::function::{Function, Ret, arg, ret};
    use crate::test_utils::{
        F32_ARG, F64_ARG, I8_ARG, I16_ARG, I32_ARG, I64_ARG, ISIZE_ARG, PTR_ARG, STRUCT_ARG,
        TestStruct, U8_ARG, U16_ARG, U32_ARG, U64_ARG, USIZE_ARG,
    };
    use crate::types::FfiType;

    macro_rules! test_identity_closure_for_type_all_abis {
        (fn $testname:ident($type:ty, $value:expr)) => {
            #[test]
            #[cfg_attr(miri, ignore)]
            fn $testname() {
                for abi in Abi::ABIS {
                    let closure = Closure::with_abi(|input: $type| input, abi);

                    let function = Function::with_abi(
                        closure.as_fn_ptr(),
                        &[<$type as FfiType>::ffi_type()],
                        Some(&<$type as FfiType>::ffi_type()),
                        abi,
                    );

                    let mut return_value = MaybeUninit::<$type>::uninit();

                    // SAFETY: The `function` was built from the closure pointer with a matching
                    // signature.
                    unsafe {
                        function.call([arg(&$value)], ret(&mut return_value));
                    }

                    // SAFETY: `Function::call` has written the result to `return_value`.
                    let return_value = unsafe { return_value.assume_init() };

                    assert_eq!(
                        return_value, $value,
                        "Invalid return value from the identity function using the ABI {abi:?}."
                    );
                }
            }
        };
    }

    test_identity_closure_for_type_all_abis!(fn test_i8_identity_closure(i8, I8_ARG));
    test_identity_closure_for_type_all_abis!(fn test_i16_identity_closure(i16, I16_ARG));
    test_identity_closure_for_type_all_abis!(fn test_i32_identity_closure(i32, I32_ARG));
    test_identity_closure_for_type_all_abis!(fn test_i64_identity_closure(i64, I64_ARG));
    test_identity_closure_for_type_all_abis!(fn test_isize_identity_closure(isize, ISIZE_ARG));
    test_identity_closure_for_type_all_abis!(fn test_u8_identity_closure(u8, U8_ARG));
    test_identity_closure_for_type_all_abis!(fn test_u16_identity_closure(u16, U16_ARG));
    test_identity_closure_for_type_all_abis!(fn test_u32_identity_closure(u32, U32_ARG));
    test_identity_closure_for_type_all_abis!(fn test_u64_identity_closure(u64, U64_ARG));
    test_identity_closure_for_type_all_abis!(fn test_usize_identity_closure(usize, USIZE_ARG));
    test_identity_closure_for_type_all_abis!(fn test_f32_identity_closure(f32, F32_ARG));
    test_identity_closure_for_type_all_abis!(fn test_f64_identity_closure(f64, F64_ARG));
    test_identity_closure_for_type_all_abis!(fn test_ptr_identity_closure(*const c_void, PTR_ARG.0));
    test_identity_closure_for_type_all_abis!(fn test_test_struct_identity_closure(TestStruct, STRUCT_ARG));

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_all_types_all_abis() {
        for abi in Abi::ABIS {
            let closure = Closure::with_abi(
                |i8_arg: i8,
                 i16_arg: i16,
                 i32_arg: i32,
                 i64_arg: i64,
                 isize_arg: isize,
                 u8_arg: u8,
                 u16_arg: u16,
                 u32_arg: u32,
                 u64_arg: u64,
                 usize_arg: usize,
                 f32_arg: f32,
                 f64_arg: f64,
                 ptr_arg: *const c_void,
                 struct_arg: TestStruct| {
                    assert_eq!(i8_arg, I8_ARG, "`i8` was not identical for ABI {abi:?}.");
                    assert_eq!(i16_arg, I16_ARG, "`i16` was not identical for ABI {abi:?}.");
                    assert_eq!(i32_arg, I32_ARG, "`i32` was not identical for ABI {abi:?}.");
                    assert_eq!(i64_arg, I64_ARG, "`i64` was not identical for ABI {abi:?}.");
                    assert_eq!(
                        isize_arg, ISIZE_ARG,
                        "`isize` was not identical for ABI {abi:?}."
                    );
                    assert_eq!(u8_arg, U8_ARG, "`u8` was not identical for ABI {abi:?}.");
                    assert_eq!(u16_arg, U16_ARG, "`u16` was not identical for ABI {abi:?}.");
                    assert_eq!(u32_arg, U32_ARG, "`u32` was not identical for ABI {abi:?}.");
                    assert_eq!(u64_arg, U64_ARG, "`u64` was not identical for ABI {abi:?}.");
                    assert_eq!(
                        usize_arg, USIZE_ARG,
                        "`usize` was not identical for ABI {abi:?}."
                    );
                    assert_eq!(f32_arg, F32_ARG, "`f32` was not identical for ABI {abi:?}.");
                    assert_eq!(f64_arg, F64_ARG, "`f64` was not identical for ABI {abi:?}.");
                    assert_eq!(
                        ptr_arg, PTR_ARG.0,
                        "`*const c_void` was not identical for ABI {abi:?}."
                    );
                    assert_eq!(
                        struct_arg, STRUCT_ARG,
                        "`TestStruct` was not identical for ABI {abi:?}."
                    );
                },
                abi,
            );

            let function = Function::with_abi(
                closure.as_fn_ptr(),
                &[
                    Type::I8,
                    Type::I16,
                    Type::I32,
                    Type::I64,
                    Type::Isize,
                    Type::U8,
                    Type::U16,
                    Type::U32,
                    Type::U64,
                    Type::Usize,
                    Type::F32,
                    Type::F64,
                    Type::Pointer,
                    TestStruct::ffi_type(),
                ],
                None,
                abi,
            );

            // SAFETY: The `function` was built from the closure pointer with a matching signature.
            unsafe {
                function.call(
                    [
                        arg(&I8_ARG),
                        arg(&I16_ARG),
                        arg(&I32_ARG),
                        arg(&I64_ARG),
                        arg(&ISIZE_ARG),
                        arg(&U8_ARG),
                        arg(&U16_ARG),
                        arg(&U32_ARG),
                        arg(&U64_ARG),
                        arg(&USIZE_ARG),
                        arg(&F32_ARG),
                        arg(&F64_ARG),
                        arg(&PTR_ARG.0),
                        arg(&STRUCT_ARG),
                    ],
                    Ret::void(),
                );
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_closure_does_not_modify_args() {
        for abi in Abi::ABIS {
            let i8_arg = UnsafeCell::new(I8_ARG);
            let i16_arg = UnsafeCell::new(I16_ARG);
            let i32_arg = UnsafeCell::new(I32_ARG);
            let i64_arg = UnsafeCell::new(I64_ARG);
            let isize_arg = UnsafeCell::new(ISIZE_ARG);
            let u8_arg = UnsafeCell::new(U8_ARG);
            let u16_arg = UnsafeCell::new(U16_ARG);
            let u32_arg = UnsafeCell::new(U32_ARG);
            let u64_arg = UnsafeCell::new(U64_ARG);
            let usize_arg = UnsafeCell::new(USIZE_ARG);
            let f32_arg = UnsafeCell::new(F32_ARG);
            let f64_arg = UnsafeCell::new(F64_ARG);
            let struct_arg = UnsafeCell::new(STRUCT_ARG);
            let ptr_arg = UnsafeCell::new(PTR_ARG);

            #[rustfmt::skip]
            let closure = Closure::with_abi(|
                mut i8_arg: i8, mut i16_arg: i16, mut i32_arg: i32, mut i64_arg: i64,
                mut isize_arg: isize, mut u8_arg: u8, mut u16_arg: u16, mut u32_arg: u32,
                mut u64_arg: u64, mut usize_arg: usize, mut f32_arg: f32, mut f64_arg: f64,
                mut struct_arg: TestStruct, mut ptr_arg: *const c_void,
            | {
                i8_arg += 1; i16_arg += 1; i32_arg += 1; i64_arg += 1; isize_arg += 1; u8_arg += 1;
                u16_arg += 1; u32_arg += 1; u64_arg += 1; usize_arg += 1; f32_arg += 1.; f64_arg += 1.0;
                struct_arg.0 += 1; struct_arg.1 += 1; struct_arg.2 += 1; struct_arg.3 += 1;

                // SAFETY: `ptr_arg` will not overflow by adding 1.
                ptr_arg = unsafe { ptr_arg.byte_add(1) };

                // Silence lint and possibly avoid Rust optimizing away potential bugs?
                black_box((
                    i8_arg, i16_arg, i32_arg, i64_arg, isize_arg, u8_arg, u16_arg, u32_arg, u64_arg,
                    usize_arg, f32_arg, f64_arg, struct_arg, ptr_arg
                ));
            }, abi);

            #[rustfmt::skip]
            let function = Function::with_abi(
                closure.as_fn_ptr(),
                &[
                    Type::I8, Type::I16, Type::I32, Type::I64, Type::Isize, Type::U8, Type::U16,
                    Type::U32, Type::U64, Type::Usize, Type::F32, Type::F64,
                    <TestStruct as FfiType>::ffi_type(), Type::Pointer,
                ],
                None,
                abi,
            );

            #[rustfmt::skip]
            let arg_array = [
                arg(&i8_arg), arg(&i16_arg), arg(&i32_arg), arg(&i64_arg), arg(&isize_arg),
                arg(&u8_arg), arg(&u16_arg), arg(&u32_arg), arg(&u64_arg), arg(&usize_arg),
                arg(&f32_arg), arg(&f64_arg), arg(&struct_arg), arg(&ptr_arg),
            ];

            // SAFETY: The `function` was built with a valid function pointer and matching
            // signature.
            unsafe {
                function.call(arg_array, Ret::void());
            }

            assert_eq!(i8_arg.into_inner(), I8_ARG);
            assert_eq!(i16_arg.into_inner(), I16_ARG);
            assert_eq!(i32_arg.into_inner(), I32_ARG);
            assert_eq!(i64_arg.into_inner(), I64_ARG);
            assert_eq!(isize_arg.into_inner(), ISIZE_ARG);
            assert_eq!(u8_arg.into_inner(), U8_ARG);
            assert_eq!(u16_arg.into_inner(), U16_ARG);
            assert_eq!(u32_arg.into_inner(), U32_ARG);
            assert_eq!(u64_arg.into_inner(), U64_ARG);
            assert_eq!(usize_arg.into_inner(), USIZE_ARG);
            assert_eq!(f32_arg.into_inner(), F32_ARG);
            assert_eq!(f64_arg.into_inner(), F64_ARG);
            assert_eq!(struct_arg.into_inner(), STRUCT_ARG);
            assert_eq!(ptr_arg.into_inner(), PTR_ARG);
        }
    }
}
