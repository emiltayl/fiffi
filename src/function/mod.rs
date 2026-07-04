//! Functionality to call external FFI functions when the function signature is not necessarily
//! known at compile time.
//!
//! # Example
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
//! // SAFETY: `function` was built from `add` which has the function signature
//! // `extern "C" fn(i32, i32) -> i32`.
//! let mut return_value = 0i32;
//! unsafe {
//!     function.call([arg(&1), arg(&2)], ret(&mut return_value));
//! }
//!
//! assert_eq!(return_value, 3);
//! ```

extern crate alloc;

use alloc::vec::Vec;
use core::ffi::c_void;
use core::marker::PhantomData;
use core::ptr::{self, null_mut};

use libffi_sys::ffi_call;

#[cfg(msan)]
use crate::__msan_unpoison;
use crate::FnPtr;
use crate::abi::Abi;
use crate::function::raw::Cif;
use crate::return_buffer::{ReturnBuffer, ffi_type_requires_return_buffer};
use crate::types::{FfiTypeLayout, Type, VariadicType};

pub(crate) mod raw;

/// Reference to an argument to pass to [`Function::call`].
///
/// Note that while `Arg` ensures that arguments are alive when they are passed, `Arg` does not
/// perform any verification to ensure that the argument is the correct type. It is up to the caller
/// to ensure that valid arguments are passed to functions.
///
/// # Example
///
/// ```
/// use fiffi::function::Arg;
///
/// let value = 123i32;
/// let arg = Arg::new(&value);
///
/// // `Arg` may also be passed a reference to a slice of memory, but it must be large enough and
/// // properly aligned. This is not checked by fiffi or libffi.
/// let arg_buffer = [0u8; 4];
/// let arg = Arg::new(&arg_buffer);
/// ```
#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct Arg<'arg>(*mut c_void, PhantomData<&'arg ()>);

impl<'arg> Arg<'arg> {
    /// Creates an `Arg` from a reference.
    ///
    /// It is up to the caller to ensure that the reference points to valid data with proper size
    /// and alignment.
    pub fn new<T>(arg_ref: &'arg T) -> Self
    where
        T: ?Sized,
    {
        let arg_ptr = ptr::from_ref(arg_ref)
            .cast::<c_void>()
            // libffi-sys expects a `*mut c_void`, so we must cast to a `mut` pointer.
            .cast_mut();

        Self(arg_ptr, PhantomData)
    }

    fn into_ptr(self) -> *mut c_void {
        self.0
    }
}

/// Creates an [`Arg`] from a reference.
///
/// This function is an alias for [`Arg::new`].
pub fn arg<T>(arg_ref: &'_ T) -> Arg<'_>
where
    T: ?Sized,
{
    Arg::new(arg_ref)
}

/// Mutable reference to where the result of [`Function::call`] should be stored.
///
/// For functions that do not return any value, [`Ret::void`] can be used to avoid having to provide
/// a mutable reference. Note that [`Ret::void`] must only be used with functions that do not return
/// a value. `Ret` does not perform any validation to ensure that it is valid to write the return
/// value to the provided mutable reference.
///
/// # Example
///
/// ```
/// use std::mem::MaybeUninit;
///
/// use fiffi::function::Ret;
///
/// let mut value = MaybeUninit::<i32>::uninit();
/// let ret = Ret::new(&mut value);
/// // After a `Function` has written to `ret` the return value can be read by calling
/// // `value.assume_init()`.
///
/// // `Ret` may also be passed a mutable reference to a slice of memory, but it must be large
/// // enough and properly aligned. This is not checked by fiffi or libffi.
/// let mut ret_buffer = [0u8; 4];
/// let ret = Ret::new(&mut ret_buffer);
/// ```
#[derive(Debug)]
#[repr(transparent)]
pub struct Ret<'ret>(*mut c_void, PhantomData<&'ret mut ()>);

impl<'ret> Ret<'ret> {
    /// Creates a `Ret` from a mutable reference.
    ///
    /// It is up to the caller to ensure that it is valid to store the result at the referenced
    /// location.
    pub fn new<T>(ret_ref: &'ret mut T) -> Self
    where
        T: ?Sized,
    {
        let ret_ptr = ptr::from_mut(ret_ref).cast::<c_void>();

        Self(ret_ptr, PhantomData)
    }

    /// Used to create a `Ret` for functions that do not return any value.
    ///
    /// Using a `Ret::void()` with a [`Function`] that returns a value will result in a segmentation
    /// fault as libffi attempts to write the result to a NULL pointer.
    pub fn void() -> Self {
        Self(null_mut(), PhantomData)
    }

    fn into_ptr(self) -> *mut c_void {
        self.0
    }
}

/// Creates a [`Ret`] from a mutable reference.
///
/// This function is an alias for [`Ret::new`].
pub fn ret<T>(ret_ref: &'_ mut T) -> Ret<'_>
where
    T: ?Sized,
{
    Ret::new(ret_ref)
}

/// An callable FFI function.
///
/// `Function` can be used to call FFI functions in cases where the signature is not known at
/// compile-time.
///
/// # Example
///
/// ```
/// use fiffi::function::{Function, arg, ret};
/// use fiffi::types::Type;
///
/// extern "C" fn double(value: i32) -> i32 {
///     value * 2
/// }
///
/// let fn_ptr = fiffi::fn_ptrize!(double);
/// let function = Function::new(fn_ptr, &[Type::I32], Some(&Type::I32));
///
/// let input = 21i32;
/// let mut output = 0i32;
///
/// // SAFETY: The function signature used to construct `function` matches `double`.
/// unsafe {
///     function.call([arg(&input)], ret(&mut output));
/// }
///
/// assert_eq!(output, 42);
/// ```
#[derive(Clone, Debug)]
pub struct Function {
    cif: Cif,
    fn_ptr: FnPtr,
}

impl Function {
    /// Create a `Function` using the target's default ABI.
    ///
    /// # Warning
    ///
    /// libffi stores the number of arguments in a C `unsigned int`. If more than `c_uint::MAX`
    /// argument types are provided, only the first `c_uint::MAX` are retained in the prepared
    /// function signature.
    pub fn new<'args, I>(fn_ptr: FnPtr, argument_types: I, return_type: Option<&Type>) -> Self
    where
        I: IntoIterator<Item = &'args Type>,
    {
        Self::with_abi(fn_ptr, argument_types, return_type, Abi::default())
    }

    /// Create a variadic `Function` using the target's default ABI.
    ///
    /// `fixed_argument_types` must describe the fixed parameters, and `variadic_argument_types`
    /// must describe the variadic arguments supplied for a call.
    ///
    /// # Warning
    ///
    /// libffi stores the number of arguments in a C `unsigned int`. If more than `c_uint::MAX`
    /// argument types are provided, only the first `c_uint::MAX` are retained in the prepared
    /// function signature.
    ///
    /// Fixed arguments are retained before variadic arguments if the signature is truncated.
    ///
    /// # Example
    ///
    /// ```
    /// use std::ffi::c_char;
    ///
    /// use fiffi::function::{Function, arg, ret};
    /// use fiffi::types::{Type, VariadicType};
    ///
    /// #[cfg_attr(target_env = "msvc", link(name = "legacy_stdio_definitions"))]
    /// unsafe extern "C" {
    ///     pub unsafe fn snprintf(s: *mut c_char, n: usize, format: *const c_char, ...) -> i32;
    /// }
    ///
    /// let function = Function::variadic(
    ///     fiffi::fn_ptrize!(snprintf),
    ///     // Fixed arguments
    ///     //Output buffer  Buffer size  Format string
    ///     &[Type::Pointer, Type::Usize, Type::Pointer],
    ///     // Variadic arguments
    ///     &[VariadicType::I32],
    ///     Some(&Type::I32),
    /// );
    ///
    /// let format = c"Num: %d";
    /// let num = 1337i32;
    /// let expected = c"Num: 1337".to_bytes_with_nul();
    ///
    /// let mut output_buffer = [0u8; 128];
    /// let mut return_value = 0i32;
    ///
    /// // SAFETY: `function` was built with a valid `snprintf` function pointer and the correct
    /// // function signature for `snprintf` with a single `i32` variadic argument.
    /// unsafe {
    ///     function.call(
    ///         [
    ///             arg(&output_buffer.as_mut_ptr()),
    ///             arg(&output_buffer.len()),
    ///             arg(&format),
    ///             arg(&num),
    ///         ],
    ///         ret(&mut return_value),
    ///     );
    /// }
    ///
    /// // `snprintf`'s return value return the written length without the final NULL byte.
    /// assert_eq!(return_value as usize, expected.len() - 1);
    /// assert_eq!(expected, &output_buffer[0..expected.len()]);
    /// ```
    pub fn variadic<'fixed_args, 'variadic_args, I1, I2>(
        fn_ptr: FnPtr,
        fixed_argument_types: I1,
        variadic_argument_types: I2,
        return_type: Option<&Type>,
    ) -> Self
    where
        I1: IntoIterator<Item = &'fixed_args Type>,
        I2: IntoIterator<Item = &'variadic_args VariadicType>,
    {
        Self::variadic_with_abi(
            fn_ptr,
            fixed_argument_types,
            variadic_argument_types,
            return_type,
            Abi::default(),
        )
    }

    /// Creates a `Function` using the provided [`Abi`].
    ///
    /// # Warning
    ///
    /// libffi stores the number of arguments in a C `unsigned int`. If more than `c_uint::MAX`
    /// argument types are provided, only the first `c_uint::MAX` are retained in the prepared
    /// function signature.
    pub fn with_abi<'args, I>(
        fn_ptr: FnPtr,
        argument_types: I,
        return_type: Option<&Type>,
        abi: Abi,
    ) -> Self
    where
        I: IntoIterator<Item = &'args Type>,
    {
        let cif = Cif::new(abi, argument_types, return_type);

        Self { cif, fn_ptr }
    }

    /// Creates a variadic `Function` using the provided [`Abi`].
    ///
    /// `fixed_argument_types` must describe the fixed parameters, and `variadic_argument_types`
    /// must describe the variadic arguments supplied for a call.
    ///
    /// # Warning
    ///
    /// libffi stores the number of arguments in a C `unsigned int`. If more than `c_uint::MAX`
    /// argument types are provided, only the first `c_uint::MAX` are retained in the prepared
    /// function signature.
    ///
    /// Fixed arguments are retained before variadic arguments if the signature is truncated.
    pub fn variadic_with_abi<'fixed_args, 'variadic_args, I1, I2>(
        fn_ptr: FnPtr,
        fixed_argument_types: I1,
        variadic_argument_types: I2,
        return_type: Option<&Type>,
        abi: Abi,
    ) -> Self
    where
        I1: IntoIterator<Item = &'fixed_args Type>,
        I2: IntoIterator<Item = &'variadic_args VariadicType>,
    {
        let cif = Cif::variadic(
            abi,
            fixed_argument_types,
            variadic_argument_types,
            return_type,
        );

        Self { cif, fn_ptr }
    }

    /// Create a [`FunctionBuilder`] used to build a [`Function`].
    ///
    /// # Example
    ///
    /// ```
    /// use fiffi::function::Function;
    /// use fiffi::types::Type;
    ///
    /// extern "C" fn add(a: i32, b: i64) -> i64 {
    ///     i64::from(a) + b
    /// }
    ///
    /// let function = Function::builder()
    ///     .arg(Type::I32)
    ///     .arg(Type::I64)
    ///     .ret(Some(Type::I64))
    ///     .fn_ptr(fiffi::fn_ptrize!(add))
    ///     .build();
    /// ```
    pub fn builder() -> FunctionBuilder<FnPtrNotSet> {
        FunctionBuilder {
            fn_ptr: FnPtrNotSet,
            argument_types: Vec::new(),
            return_type: None,
            abi: Abi::default(),
        }
    }

    /// Create a [`VariadicFunctionBuilder`] used to build a variadic [`Function`].
    pub fn variadic_builder() -> VariadicFunctionBuilder<FnPtrNotSet> {
        VariadicFunctionBuilder {
            fn_ptr: FnPtrNotSet,
            fixed_argument_types: Vec::new(),
            variadic_argument_types: Vec::new(),
            return_type: None,
            abi: Abi::default(),
        }
    }

    /// Calls the wrapped function pointer through libffi.
    ///
    /// # Safety
    ///
    /// * The wrapped [`FnPtr`] must be valid to call with this function's ABI and signature.
    /// * Every [`Arg`] in `args` must point to an initialized value matching the corresponding
    ///   [`Type`] the function expects and remain alive for the duration of the call.
    /// * All arguments expected by the called function must be provided.
    /// * `ret` must be valid to write the return type to, unless the function was created with no
    ///   return type.
    /// * Calling the target function must not violate Rust aliasing rules for any referenced
    ///   memory.
    ///
    /// If more than `c_uint::MAX` argument types were provided during construction, libffi will use
    /// the truncated signature and may not read every argument pointer provided in `args`.
    ///
    /// # Example
    ///
    /// ```
    /// use fiffi::function::{Function, arg, ret};
    /// use fiffi::types::Type;
    ///
    /// extern "C" fn add_one(value: i32) -> i32 {
    ///     value + 1
    /// }
    ///
    /// let function = Function::new(fiffi::fn_ptrize!(add_one), &[Type::I32], Some(&Type::I32));
    /// let input = 41i32;
    /// let mut output = 0i32;
    ///
    /// // SAFETY: The function pointer, argument type, return type, and storage match `add_one`.
    /// unsafe {
    ///     function.call([arg(&input)], ret(&mut output));
    /// }
    ///
    /// assert_eq!(output, 42);
    /// ```
    pub unsafe fn call<'arg, I>(&self, args: I, ret: Ret)
    where
        I: IntoIterator<Item = Arg<'arg>>,
    {
        // libffi may modify the pointers in `avalue` array, so we mark it as `mut`.
        let mut args: Vec<*mut c_void> = args.into_iter().map(Arg::into_ptr).collect();
        let args_ptr = args.as_mut_ptr();

        let fn_ptr = self.fn_ptr.as_libffi_sys_ptr();

        // SAFETY: `rtype` is a pointer to an initialized `ffi_type` that will not change while
        // `self` is alive.
        let return_type = unsafe { &*(*self.cif.as_ffi_cif_ptr()).rtype };
        let ret_ptr = ret.into_ptr();

        if ffi_type_requires_return_buffer(return_type) {
            // For integers smaller than a register, libffi still writes a full register to
            // `rvalue`.
            let mut return_buffer = ReturnBuffer::new();

            // SAFETY: It is up to the caller to ensure that it is safe to call the function. See
            // `call`'s safety section for more details.
            unsafe {
                ffi_call(
                    self.cif.as_ffi_cif_ptr(),
                    fn_ptr,
                    return_buffer.as_mut_ptr(),
                    args_ptr,
                );

                #[cfg(msan)]
                __msan_unpoison(return_buffer.as_mut_ptr(), size_of::<ReturnBuffer>());
            }

            // SAFETY: `ffi_call` initialized `return_buffer`, and `call`'s safety contract requires
            // `ret_ptr` to be valid writable storage for this function's return type.
            unsafe {
                return_buffer.write_result(ret_ptr, return_type.size);
            }
        } else {
            // For sufficiently large integers, floats, structs and void return values, we can
            // simply pass the pointer in `ret` to `ffi_call`.
            //
            // SAFETY: It is up to the caller to ensure that it is safe to call the function. See
            // `call`'s safety section for more details.
            unsafe {
                ffi_call(self.cif.as_ffi_cif_ptr(), fn_ptr, ret_ptr, args_ptr);

                #[cfg(msan)]
                __msan_unpoison(ret_ptr, return_type.size);
            }
        }
    }

    /// Returns the memory layout of this `Function`'s arguments.
    ///
    /// # Example
    ///
    /// ```
    /// use fiffi::function::Function;
    /// use fiffi::types::Type;
    ///
    /// extern "C" fn identity(value: i32) -> i32 {
    ///     value
    /// }
    ///
    /// let function = Function::new(fiffi::fn_ptrize!(identity), &[Type::I32], Some(&Type::I32));
    ///
    /// assert_eq!(function.argument_layouts(), vec![Type::I32.layout()]);
    /// ```
    pub fn argument_layouts(&self) -> Vec<FfiTypeLayout> {
        self.cif.argument_layouts()
    }

    /// Returns the memory layout of this `Function`'s return value.
    ///
    /// # Example
    ///
    /// ```
    /// use fiffi::function::Function;
    /// use fiffi::types::Type;
    ///
    /// extern "C" fn identity(value: i32) -> i32 {
    ///     value
    /// }
    ///
    /// let function = Function::new(fiffi::fn_ptrize!(identity), &[Type::I32], Some(&Type::I32));
    ///
    /// assert_eq!(function.return_layout(), Type::I32.layout());
    /// ```
    pub fn return_layout(&self) -> FfiTypeLayout {
        self.cif.return_layout()
    }
}

// SAFETY: `Function` itself is safe to be sent to a different thread, although it should be noted
// that it might not be safe to call the provided function pointer from another thread.
unsafe impl Send for Function {}

// SAFETY: `Function` itself is safe to be sent to a different thread, although it should be noted
// that it might not be safe to call the provided function pointer from another thread.
unsafe impl Sync for Function {}

/// Builder state used before a function pointer has been set.
///
/// If a `FnPtr` has not been set, the [`Function`] cannot be created.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FnPtrNotSet;

/// Builder state used after a function pointer has been set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct FnPtrSet(FnPtr);

/// Builder for a non-variadic [`Function`].
///
/// Argument types are appended in call order. [`Function`]s created using `FunctionBuilder` use the
/// default [`Abi`] unless another ABI is explicitly set.
///
/// # Example
///
/// ```
/// use fiffi::function::Function;
/// use fiffi::types::Type;
///
/// extern "C" fn identity(a: i32) -> i32 {
///     a
/// }
///
/// let function = Function::builder()
///     .arg(Type::I32)
///     .ret(Some(Type::I32))
///     .fn_ptr(fiffi::fn_ptrize!(identity))
///     .build();
/// ```
#[derive(Clone, Debug)]
pub struct FunctionBuilder<State> {
    fn_ptr: State,
    argument_types: Vec<Type>,
    return_type: Option<Type>,
    abi: Abi,
}

impl<State> FunctionBuilder<State> {
    /// Set the function pointer.
    #[must_use]
    pub fn fn_ptr(self, fn_ptr: FnPtr) -> FunctionBuilder<FnPtrSet> {
        FunctionBuilder {
            fn_ptr: FnPtrSet(fn_ptr),
            argument_types: self.argument_types,
            return_type: self.return_type,
            abi: self.abi,
        }
    }

    /// Set the function's ABI.
    #[must_use]
    pub fn abi(mut self, abi: Abi) -> Self {
        self.abi = abi;
        self
    }

    /// Set the function's return type.
    #[must_use]
    pub fn ret(mut self, return_type: Option<Type>) -> Self {
        self.return_type = return_type;
        self
    }

    /// Add a single argument to the function signature.
    #[must_use]
    pub fn arg(mut self, ty: Type) -> Self {
        self.argument_types.push(ty);
        self
    }

    /// Add multiple arguments to the function signature.
    #[must_use]
    pub fn args<I>(mut self, types: I) -> Self
    where
        I: IntoIterator<Item = Type>,
    {
        self.argument_types.extend(types);
        self
    }
}

impl FunctionBuilder<FnPtrSet> {
    /// Build the [`Function`].
    ///
    /// # Warning
    ///
    /// libffi stores the number of arguments in a C `unsigned int`. If more than `c_uint::MAX`
    /// argument types are provided, only the first `c_uint::MAX` are retained in the prepared
    /// function signature.
    pub fn build(self) -> Function {
        Function::with_abi(
            self.fn_ptr.0,
            &self.argument_types,
            self.return_type.as_ref(),
            self.abi,
        )
    }
}

/// Builder for a variadic [`Function`].
///
/// [`Function`]s created using `VariadicFunctionBuilder` use the default [`Abi`] unless another ABI
/// is explicitly set.
///
/// Fixed and variadic argument types are appended in call order within their respective groups.
/// All fixed arguments are always provided before all variadic arguments when calling the function.
///
/// If the signature is truncated, fixed arguments are retained before variadic arguments.
///
/// # Example
///
/// ```
/// use std::ffi::c_char;
///
/// use fiffi::function::Function;
/// use fiffi::types::{Type, VariadicType};
///
/// #[cfg_attr(target_env = "msvc", link(name = "legacy_stdio_definitions"))]
/// unsafe extern "C" {
///     pub unsafe fn snprintf(s: *mut c_char, n: usize, format: *const c_char, ...) -> i32;
/// }
///
/// // Prepare to call `snprintf` with a single `i32` variadic argument.
/// let function = Function::variadic_builder()
///     .fixed_arg(Type::Pointer)
///     .fixed_arg(Type::Usize)
///     .fixed_arg(Type::Pointer)
///     .variadic_arg(VariadicType::I32)
///     .ret(Some(Type::I32))
///     .fn_ptr(fiffi::fn_ptrize!(snprintf))
///     .build();
/// ```
#[derive(Clone, Debug)]
pub struct VariadicFunctionBuilder<State> {
    fn_ptr: State,
    fixed_argument_types: Vec<Type>,
    variadic_argument_types: Vec<VariadicType>,
    return_type: Option<Type>,
    abi: Abi,
}

impl<State> VariadicFunctionBuilder<State> {
    /// Set the function pointer.
    #[must_use]
    pub fn fn_ptr(self, fn_ptr: FnPtr) -> VariadicFunctionBuilder<FnPtrSet> {
        VariadicFunctionBuilder {
            fn_ptr: FnPtrSet(fn_ptr),
            fixed_argument_types: self.fixed_argument_types,
            variadic_argument_types: self.variadic_argument_types,
            return_type: self.return_type,
            abi: self.abi,
        }
    }

    /// Set the function's ABI.
    #[must_use]
    pub fn abi(mut self, abi: Abi) -> Self {
        self.abi = abi;
        self
    }

    /// Set the function's return type.
    #[must_use]
    pub fn ret(mut self, return_type: Option<Type>) -> Self {
        self.return_type = return_type;
        self
    }

    /// Add a single fixed argument to the function signature.
    #[must_use]
    pub fn fixed_arg(mut self, ty: Type) -> Self {
        self.fixed_argument_types.push(ty);
        self
    }

    /// Add multiple fixed arguments to the function signature.
    #[must_use]
    pub fn fixed_args<I>(mut self, types: I) -> Self
    where
        I: IntoIterator<Item = Type>,
    {
        self.fixed_argument_types.extend(types);
        self
    }

    /// Add a single variadic argument to the function signature.
    #[must_use]
    pub fn variadic_arg(mut self, ty: VariadicType) -> Self {
        self.variadic_argument_types.push(ty);
        self
    }

    /// Add multiple variadic arguments to the function signature.
    #[must_use]
    pub fn variadic_args<I>(mut self, types: I) -> Self
    where
        I: IntoIterator<Item = VariadicType>,
    {
        self.variadic_argument_types.extend(types);
        self
    }
}

impl VariadicFunctionBuilder<FnPtrSet> {
    /// Build the variadic [`Function`].
    ///
    /// # Warning
    ///
    /// libffi stores the number of arguments in a C `unsigned int`. If more than `c_uint::MAX`
    /// argument types are provided, only the first `c_uint::MAX` are retained in the prepared
    /// function signature.
    pub fn build(self) -> Function {
        Function::variadic_with_abi(
            self.fn_ptr.0,
            &self.fixed_argument_types,
            &self.variadic_argument_types,
            self.return_type.as_ref(),
            self.abi,
        )
    }
}

#[cfg(test)]
mod tests {
    use core::cell::UnsafeCell;
    use core::ffi::CStr;
    use core::mem::MaybeUninit;

    use test_callbacks::*;

    use super::*;
    use crate::fn_ptrize;
    use crate::function::raw::CifKind;
    use crate::test_utils::{
        F32_ARG, F64_ARG, I8_ARG, I16_ARG, I32_ARG, I64_ARG, ISIZE_ARG, PTR_ARG, SNPRINTF_ARG_1,
        SNPRINTF_ARG_2, SNPRINTF_ARG_3, SNPRINTF_ARG_4, SNPRINTF_ARG_5, SNPRINTF_ARG_6,
        SNPRINTF_EXPECTED_OUTPUT, SNPRINTF_EXPECTED_RETURN_VALUE, SNPRINTF_FORMAT, STRUCT_ARG,
        TestStruct, U8_ARG, U16_ARG, U32_ARG, U64_ARG, USIZE_ARG, snprintf,
    };
    use crate::types::FfiType;

    macro_rules! test_identity_function {
        ($ty:ty, $identity_fn:ident, $val:expr) => {{
            let ffi_type = <$ty as FfiType>::ffi_type();
            let function = Function::new(
                fn_ptrize!($identity_fn),
                core::slice::from_ref(&ffi_type),
                Some(&ffi_type),
            );

            let mut return_buffer = MaybeUninit::<$ty>::uninit();

            let fn_args = [arg(&$val)];
            let fn_ret = ret(&mut return_buffer);

            // SAFETY: The `function` was built with a valid function pointer and matching
            // signature. `return_buffer` is initialized by the call before it is read.
            let return_value = unsafe {
                function.call(fn_args, fn_ret);
                return_buffer.assume_init()
            };

            assert_eq!(
                return_value,
                $val,
                "Unexpected return while calling identity function {}.",
                stringify!($identity_fn)
            );
        }};
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn call_void_fn() {
        let function = Function::new(fn_ptrize!(void_fn), &[], None);

        // SAFETY: The `function` was built with a valid function pointer and matching signature.
        unsafe {
            function.call([], Ret::void());
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_identity_functions() {
        test_identity_function!(i8, i8_identity, I8_ARG);
        test_identity_function!(i16, i16_identity, I16_ARG);
        test_identity_function!(i32, i32_identity, I32_ARG);
        test_identity_function!(i64, i64_identity, I64_ARG);
        test_identity_function!(isize, isize_identity, ISIZE_ARG);
        test_identity_function!(u8, u8_identity, U8_ARG);
        test_identity_function!(u16, u16_identity, U16_ARG);
        test_identity_function!(u32, u32_identity, U32_ARG);
        test_identity_function!(u64, u64_identity, U64_ARG);
        test_identity_function!(usize, usize_identity, USIZE_ARG);
        test_identity_function!(f32, f32_identity, F32_ARG);
        test_identity_function!(f64, f64_identity, F64_ARG);
        test_identity_function!(*const c_void, ptr_identity, PTR_ARG.0);
        test_identity_function!(TestStruct, test_struct_identity, STRUCT_ARG);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_function_does_not_modify_args() {
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
        let function = Function::new(
            fn_ptrize!(modifying_fn),
            &[
                Type::I8, Type::I16, Type::I32, Type::I64, Type::Isize, Type::U8, Type::U16,
                Type::U32, Type::U64, Type::Usize, Type::F32, Type::F64,
                <TestStruct as FfiType>::ffi_type(), Type::Pointer,
            ],
            None,
        );

        #[rustfmt::skip]
        let arg_array = [
            arg(&i8_arg), arg(&i16_arg), arg(&i32_arg), arg(&i64_arg), arg(&isize_arg),
            arg(&u8_arg), arg(&u16_arg), arg(&u32_arg), arg(&u64_arg), arg(&usize_arg),
            arg(&f32_arg), arg(&f64_arg), arg(&struct_arg), arg(&ptr_arg),
        ];

        // SAFETY: The `function` was built with a valid function pointer and matching signature.
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

    #[test]
    fn test_type_layout_fns() {
        #[rustfmt::skip]
        let modifying_fn_arg_types = [
            Type::I8, Type::I16, Type::I32, Type::I64, Type::Isize, Type::U8, Type::U16, Type::U32,
            Type::U64, Type::Usize, Type::F32, Type::F64, <TestStruct as FfiType>::ffi_type(),
            Type::Pointer,
        ];

        let expected_layouts: Vec<FfiTypeLayout> =
            modifying_fn_arg_types.iter().map(Type::layout).collect();

        let function = Function::new(fn_ptrize!(modifying_fn), &modifying_fn_arg_types, None);

        assert_eq!(expected_layouts, function.argument_layouts());

        for ty in &modifying_fn_arg_types {
            // Note that this `function` points to a function with a different signature, so it
            // should never be `call`ed.
            let function = Function::new(fn_ptrize!(void_fn), core::slice::from_ref(ty), Some(ty));

            assert_eq!(function.argument_layouts(), vec![ty.layout()],);
            assert_eq!(function.return_layout(), ty.layout());
        }
    }

    #[test]
    fn function_new_with_abi_works_with_all_abis() {
        for abi in Abi::ABIS {
            Function::with_abi(fn_ptrize!(void_fn), &[], None, abi);
        }
    }

    #[test]
    fn variadic_function_supports_all_variadic_types() {
        for abi in Abi::ABIS {
            // Do not call `void_fn` prepared like this.
            let variadic_fn = Function::variadic_with_abi(
                fn_ptrize!(void_fn),
                &[Type::Pointer],
                &[
                    VariadicType::I32,
                    VariadicType::U32,
                    VariadicType::I64,
                    VariadicType::U64,
                    VariadicType::Isize,
                    VariadicType::Usize,
                    VariadicType::F64,
                    VariadicType::Pointer,
                    VariadicType::create_struct(vec![Type::I8]).unwrap(),
                ],
                None,
                abi,
            );

            assert!(matches!(variadic_fn.cif.kind, CifKind::Variadic { .. }));
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_variadic_snprintf() {
        let snprintf_function = Function::variadic(
            fn_ptrize!(snprintf),
            &[Type::Pointer, Type::Usize, Type::Pointer],
            &[
                VariadicType::I32,
                VariadicType::U32,
                VariadicType::I64,
                VariadicType::U64,
                VariadicType::Pointer,
                VariadicType::F64,
            ],
            Some(&Type::I32),
        );

        let mut buffer = [0u8; 128];
        let buffer_ptr = buffer.as_mut_ptr();
        let buffer_size = buffer.len();
        let mut return_value = 0i32;

        // SAFETY: `snprintf_function` was built with a valid `snprintf` function pointer and
        // matching fixed/variadic signatures expected by the call site.
        unsafe {
            snprintf_function.call(
                [
                    arg(&buffer_ptr),
                    arg(&buffer_size),
                    arg(&SNPRINTF_FORMAT),
                    arg(&SNPRINTF_ARG_1),
                    arg(&SNPRINTF_ARG_2),
                    arg(&SNPRINTF_ARG_3),
                    arg(&SNPRINTF_ARG_4),
                    arg(&SNPRINTF_ARG_5),
                    arg(&SNPRINTF_ARG_6),
                ],
                ret(&mut return_value),
            );
        }

        let output_str = CStr::from_bytes_until_nul(&buffer).unwrap();

        assert_eq!(
            return_value, SNPRINTF_EXPECTED_RETURN_VALUE,
            "`snprintf` did not write the expected number of bytes."
        );

        assert_eq!(
            output_str, SNPRINTF_EXPECTED_OUTPUT,
            "Output from `snprintf` was not as expected."
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn builder_builds_and_calls_void() {
        let function = Function::builder().fn_ptr(fn_ptrize!(void_fn)).build();

        // SAFETY: The `function` was built with a valid function pointer and matching signature.
        unsafe {
            function.call([], Ret::void());
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn builder_with_args_and_multiple_fn_ptr_calls() {
        let builder = Function::builder()
            .arg(Type::I32)
            .ret(Some(Type::I32))
            .fn_ptr(fn_ptrize!(f64_identity))
            .fn_ptr(fn_ptrize!(i32_identity));

        let function = builder.build();

        let mut return_buffer = MaybeUninit::<i32>::uninit();
        let arg_val: i32 = I32_ARG;

        // SAFETY: `function` was built with the correct signature and a valid function pointer.
        unsafe {
            function.call([arg(&arg_val)], ret(&mut return_buffer));
        }

        // SAFETY: `return_buffer` was initialized by the call above.
        let return_value = unsafe { return_buffer.assume_init() };

        assert_eq!(return_value, arg_val);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn variadic_builder_snprintf_works() {
        let snprintf_function = Function::variadic_builder()
            .fixed_args(vec![Type::Pointer, Type::Usize, Type::Pointer])
            .variadic_args(vec![
                VariadicType::I32,
                VariadicType::U32,
                VariadicType::I64,
                VariadicType::U64,
                VariadicType::Pointer,
                VariadicType::F64,
            ])
            .ret(Some(Type::I32))
            .fn_ptr(fn_ptrize!(snprintf))
            .build();

        let mut buffer = [0u8; 128];
        let buffer_ptr = buffer.as_mut_ptr();
        let buffer_size = buffer.len();
        let mut return_value = 0i32;

        // SAFETY: `snprintf_function` was built with a valid `snprintf` function pointer and
        // matching fixed/variadic signatures expected by the snprintf with the given format string.
        unsafe {
            snprintf_function.call(
                [
                    arg(&buffer_ptr),
                    arg(&buffer_size),
                    arg(&SNPRINTF_FORMAT),
                    arg(&SNPRINTF_ARG_1),
                    arg(&SNPRINTF_ARG_2),
                    arg(&SNPRINTF_ARG_3),
                    arg(&SNPRINTF_ARG_4),
                    arg(&SNPRINTF_ARG_5),
                    arg(&SNPRINTF_ARG_6),
                ],
                ret(&mut return_value),
            );
        }

        let output_str = CStr::from_bytes_until_nul(&buffer).unwrap();

        assert_eq!(
            return_value, SNPRINTF_EXPECTED_RETURN_VALUE,
            "`snprintf` did not write the expected number of bytes."
        );

        assert_eq!(
            output_str, SNPRINTF_EXPECTED_OUTPUT,
            "Output from `snprintf` was not as expected."
        );
    }
}

#[cfg(test)]
#[rustfmt::skip]
pub(crate) mod test_callbacks {
    use core::ffi::c_void;
    use core::hint::black_box;

    use crate::test_utils::TestStruct;

    pub extern "C" fn void_fn() {}

    pub extern "C" fn i8_identity(arg: i8) -> i8 { arg }
    pub extern "C" fn i16_identity(arg: i16) -> i16 { arg }
    pub extern "C" fn i32_identity(arg: i32) -> i32 { arg }
    pub extern "C" fn i64_identity(arg: i64) -> i64 { arg }
    pub extern "C" fn isize_identity(arg: isize) -> isize { arg }
    pub extern "C" fn u8_identity(arg: u8) -> u8 { arg }
    pub extern "C" fn u16_identity(arg: u16) -> u16 { arg }
    pub extern "C" fn u32_identity(arg: u32) -> u32 { arg }
    pub extern "C" fn u64_identity(arg: u64) -> u64 { arg }
    pub extern "C" fn usize_identity(arg: usize) -> usize { arg }
    pub extern "C" fn f32_identity(arg: f32) -> f32 { arg }
    pub extern "C" fn f64_identity(arg: f64) -> f64 { arg }
    pub extern "C" fn test_struct_identity(arg: TestStruct) -> TestStruct { arg }
    pub extern "C" fn ptr_identity(arg: *const c_void) -> *const c_void { arg }

    pub extern "C" fn modifying_fn(
        mut i8_arg: i8, mut i16_arg: i16, mut i32_arg: i32, mut i64_arg: i64, mut isize_arg: isize,
        mut u8_arg: u8, mut u16_arg: u16, mut u32_arg: u32, mut u64_arg: u64, mut usize_arg: usize,
        mut f32_arg: f32, mut f64_arg: f64, mut struct_arg: TestStruct, mut ptr_arg: *const c_void,
    ) {
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
    }
}
