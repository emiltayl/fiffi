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

#[cfg(msan)]
use crate::__msan_unpoison;
use crate::FnPtr;
use crate::abi::Abi;
use crate::types::{FfiTypeLayout, Type, VariadicType};

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
    // TODO cif: Cif,
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
        _fn_ptr: FnPtr,
        _argument_types: I,
        _return_type: Option<&Type>,
        _abi: Abi,
    ) -> Self
    where
        I: IntoIterator<Item = &'args Type>,
    {
        todo!();
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
        _fn_ptr: FnPtr,
        _fixed_argument_types: I1,
        _variadic_argument_types: I2,
        _return_type: Option<&Type>,
        _abi: Abi,
    ) -> Self
    where
        I1: IntoIterator<Item = &'fixed_args Type>,
        I2: IntoIterator<Item = &'variadic_args VariadicType>,
    {
        todo!();
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
    pub unsafe fn call<'arg, I>(&self, _args: I, _ret: Ret)
    where
        I: IntoIterator<Item = Arg<'arg>>,
    {
        todo!();
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
        todo!();
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
        todo!();
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
