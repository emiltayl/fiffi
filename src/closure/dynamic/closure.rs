extern crate alloc;

#[cfg(not(test))]
use alloc::vec::Vec;
use core::ffi::c_void;
use core::fmt::Debug;

use libffi_sys::ffi_cif;

use super::{DynamicClosureArgs, DynamicClosureCall, DynamicClosureRet};
use crate::FnPtr;
use crate::abi::Abi;
use crate::closure::BaseClosure;
use crate::closure::raw::Context;
use crate::errors::ClosureAllocationError;
use crate::function::raw::{Cif, MAX_ARGS};
use crate::types::{FfiTypeLayout, Type};

struct DynamicClosureContext<FN, CONTEXT>
where
    FN: Fn(DynamicClosureCall<CONTEXT>),
{
    callback: FN,
    argument_types: Vec<Type>,
    argument_layouts: Vec<FfiTypeLayout>,
    return_type_and_layout: Option<(Type, FfiTypeLayout)>,
    context: CONTEXT,
}

/// A wrapper for creating function pointers to callbacks whose signatures are not known at
/// compile-time.
///
/// The generated function pointer remains valid only while the `DynamicClosure` is alive. It must
/// be called using the ABI, argument types, and return type provided when the closure was created.
///
/// If a return type has been defined, the callback function **must** write a valid result before
/// returning. Failing to do so will result in undefined behavior when the callback returns.
///
/// The callback and context must be thread-safe because foreign code may invoke the generated
/// function pointer concurrently from multiple threads.
///
/// # Example
///
/// Creating a callback that can accept an arbitrary number of `i32`s and returns an `i32` with the
/// sum of the arguments.
///
/// ```
/// use core::mem::MaybeUninit;
///
/// use fiffi::closure::DynamicClosure;
/// use fiffi::closure::dynamic::DynamicClosureCall;
/// use fiffi::types::Type;
///
/// fn summing_callback(mut call: DynamicClosureCall<()>) {
///     let mut sum = 0i32;
///     let mut num = MaybeUninit::<i32>::uninit();
///
///     for arg in call.args() {
///         if arg.ty() != &Type::I32 {
///             // **NOTE** This will abort instead of unwinding regardless of the crate's setting
///             // because Rust cannot unwind past libffi's `extern "C"` functions.
///             panic!("This callback can only be used with `i32` arguments.");
///         }
///
///         // SAFETY: The argument's type is `i32`. The `copy_to` fully initializes `num`.
///         unsafe {
///             arg.copy_to(&mut num);
///             sum += num.assume_init();
///         }
///     }
///
///     let ret = call.ret().expect("This callback expects a return value.");
///
///     if ret.ty() != &Type::I32 {
///         // **NOTE** This will abort instead of unwinding regardless of the crate's setting
///         // because Rust cannot unwind past libffi's `extern "C"` functions.
///         panic!("This callback can only be used with an `i32` return value.");
///     }
///
///     // SAFETY: The return value's type is `i32`.
///     unsafe {
///         ret.write(sum);
///     }
/// }
///
/// let sum_i32s = DynamicClosure::new(
///     summing_callback,
///     &[Type::I32, Type::I32, Type::I32],
///     Some(&Type::I32),
///     (),
/// );
///
/// // SAFETY: `sum_i32s` represents an `extern "C"` function that accepts three `i32`s and
/// // returns an `i32`. `sum_i32s` remains alive while it is called.
/// let sum_i32s_fn = unsafe {
///     sum_i32s
///         .as_fn_ptr()
///         .into_fn::<extern "C" fn(i32, i32, i32) -> i32>()
/// };
///
/// assert_eq!(sum_i32s_fn(1, 2, 3), 6);
/// ```
pub struct DynamicClosure<FN, CONTEXT>
where
    FN: Fn(DynamicClosureCall<CONTEXT>) + Send + Sync,
    CONTEXT: Send + Sync,
{
    closure: BaseClosure<DynamicClosureContext<FN, CONTEXT>>,
}

impl<FN, CONTEXT> DynamicClosure<FN, CONTEXT>
where
    FN: Fn(DynamicClosureCall<CONTEXT>) + Send + Sync,
    CONTEXT: Send + Sync,
{
    /// Creates a dynamic closure using the target's default ABI.
    ///
    /// If a return type is defined, the callback function must write a valid result before
    /// returning. Failing to do so will result in undefined behavior when the callback returns.
    ///
    /// # Warning
    ///
    /// libffi stores the number of arguments in a C `unsigned int`. If more than `c_uint::MAX`
    /// argument types are provided, only the first `c_uint::MAX` are retained in the prepared
    /// function signature.
    ///
    /// # Panics
    ///
    /// Panics if libffi cannot allocate closure storage. Use [`DynamicClosure::try_new`] to handle
    /// allocation failure.
    pub fn new<'args, I>(
        callback: FN,
        argument_types: I,
        return_type: Option<&Type>,
        context: CONTEXT,
    ) -> Self
    where
        I: IntoIterator<Item = &'args Type>,
    {
        Self::try_new(callback, argument_types, return_type, context)
            .expect("Libffi was unable to allocate the closure.")
    }

    /// Creates a dynamic closure using the provided [`Abi`].
    ///
    /// # Warning
    ///
    /// libffi stores the number of arguments in a C `unsigned int`. If more than `c_uint::MAX`
    /// argument types are provided, only the first `c_uint::MAX` are retained in the prepared
    /// function signature.
    ///
    /// # Panics
    ///
    /// Panics if libffi cannot allocate closure storage. Use [`DynamicClosure::try_with_abi`] to
    /// handle allocation failure.
    pub fn with_abi<'args, I>(
        callback: FN,
        argument_types: I,
        return_type: Option<&Type>,
        abi: Abi,
        context: CONTEXT,
    ) -> Self
    where
        I: IntoIterator<Item = &'args Type>,
    {
        Self::try_with_abi(callback, argument_types, return_type, abi, context)
            .expect("Libffi was unable to allocate the closure.")
    }

    /// Tries to create a dynamic closure using the target's default ABI.
    ///
    /// # Warning
    ///
    /// libffi stores the number of arguments in a C `unsigned int`. If more than `c_uint::MAX`
    /// argument types are provided, only the first `c_uint::MAX` are retained in the prepared
    /// function signature.
    ///
    /// # Errors
    ///
    /// Returns [`ClosureAllocationError`] if libffi cannot allocate closure storage.
    pub fn try_new<'args, I>(
        callback: FN,
        argument_types: I,
        return_type: Option<&Type>,
        context: CONTEXT,
    ) -> Result<Self, ClosureAllocationError>
    where
        I: IntoIterator<Item = &'args Type>,
    {
        Self::try_with_abi(
            callback,
            argument_types,
            return_type,
            Abi::default(),
            context,
        )
    }

    /// Tries to create a dynamic closure using the provided [`Abi`].
    ///
    /// # Warning
    ///
    /// libffi stores the number of arguments in a C `unsigned int`. If more than `c_uint::MAX`
    /// argument types are provided, only the first `c_uint::MAX` are retained in the prepared
    /// function signature.
    ///
    /// # Errors
    ///
    /// Returns [`ClosureAllocationError`] if libffi cannot allocate closure storage.
    pub fn try_with_abi<'args, I>(
        callback: FN,
        argument_types: I,
        return_type: Option<&Type>,
        abi: Abi,
        context: CONTEXT,
    ) -> Result<Self, ClosureAllocationError>
    where
        I: IntoIterator<Item = &'args Type>,
    {
        let argument_types: Vec<Type> =
            argument_types.into_iter().take(MAX_ARGS).cloned().collect();

        let cif = Cif::new(abi, &argument_types, return_type);

        let context = Context::new(DynamicClosureContext {
            callback,
            argument_layouts: cif.argument_layouts(),
            return_type_and_layout: return_type
                .map(|return_type| (return_type.clone(), cif.return_layout())),
            argument_types,
            context,
        });

        // SAFETY:
        // * The type information used to create `cif` is stored for further usage by the callback.
        // * `dynamic_closure_callback` interprets its payload as the matching
        //   `DynamicClosureContext<CONTEXT>` and uses the same types to describe the raw libffi
        //   argument and return pointers.
        // * `Context` keeps its payload allocation stable and alive for the lifetime of `closure`.
        let closure =
            unsafe { BaseClosure::try_new(cif, dynamic_closure_callback::<FN, CONTEXT>, context)? };

        Ok(Self { closure })
    }

    /// Returns a function pointer that can be used to call this closure.
    ///
    /// The pointer remains valid only while `self` is alive and must be called using the ABI and
    /// signature provided when the closure was created.
    ///
    /// Invoking the function pointer is in general unsafe. The callback must ensure that reading
    /// arguments and writing results are done safely. If a return type has been set, the callback
    /// must also ensure that a result value has been written before returning, as failing to do
    /// this results in undefined behavior.
    pub fn as_fn_ptr(&self) -> FnPtr {
        self.closure.as_fn_ptr()
    }
}

impl<FN, CONTEXT> Debug for DynamicClosure<FN, CONTEXT>
where
    FN: Fn(DynamicClosureCall<CONTEXT>) + Send + Sync,
    CONTEXT: Send + Sync,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DynamicClosure")
            .field("closure", &self.closure)
            .finish()
    }
}

unsafe extern "C" fn dynamic_closure_callback<FN, CONTEXT>(
    _cif: *mut ffi_cif,
    result_ptr: *mut c_void,
    arguments_ptr: *mut *mut c_void,
    context_ptr: *mut c_void,
) where
    FN: Fn(DynamicClosureCall<CONTEXT>) + Send + Sync,
    CONTEXT: Send + Sync,
{
    // SAFETY:
    // * `DynamicClosure::try_with_abi` registered this callback with a pointer to a
    //   `DynamicClosureContext<CONTEXT>` using the same `CONTEXT` type.
    // * The owning `BaseClosure` keeps that allocation alive for every callback invocation.
    // * No mutable references are created to `context_ptr`.
    let context = unsafe { &*context_ptr.cast::<DynamicClosureContext<FN, CONTEXT>>() };

    let ret = context
        .return_type_and_layout
        .as_ref()
        .map(|(return_type, return_layout)| DynamicClosureRet {
            ty: return_type,
            layout: return_layout,
            ptr: result_ptr,
        });

    let args = DynamicClosureArgs {
        argument_types: &context.argument_types,
        argument_layouts: &context.argument_layouts,
        argument_array: arguments_ptr.cast_const().cast(),
    };

    let call = DynamicClosureCall {
        args,
        ret,
        context: &context.context,
    };

    (context.callback)(call);
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use core::mem::MaybeUninit;
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::function::{Function, Ret, arg, ret};
    use crate::test_utils::{
        F32_ARG, F64_ARG, I8_ARG, I16_ARG, I32_ARG, I64_ARG, ISIZE_ARG, PTR_ARG, STRUCT_ARG,
        TestStruct, U8_ARG, U16_ARG, U32_ARG, U64_ARG, USIZE_ARG,
    };
    use crate::types::FfiType;

    fn noop_callback(_: DynamicClosureCall<Arc<()>>) {}

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_no_args_no_return_context_lifetime_all_abis() {
        for abi in Abi::ABIS {
            let context = Arc::new(());
            let context_probe = Arc::downgrade(&context);

            let closure = DynamicClosure::with_abi(noop_callback, &[], None, abi, context);

            assert!(context_probe.upgrade().is_some());

            let function = Function::with_abi(closure.as_fn_ptr(), &[], None, abi);

            // SAFETY: The `function` was built from the closure pointer with a matching
            // zero-argument, void-returning signature.
            unsafe {
                function.call([], Ret::void());
            }

            assert!(context_probe.upgrade().is_some());

            drop(closure);

            assert!(context_probe.upgrade().is_none());
        }
    }

    /// This callback compares every type argument provided with the equivalent test constant and
    /// writes the result with the argument if both have the same type. If multiple arguments' types
    /// match the result type, the last argument will be written as the result.
    ///
    /// The context is increased by one for each call.
    fn test_callback(call: DynamicClosureCall<&AtomicUsize>) {
        macro_rules! test_argument {
            ($arg:ident, $ret:ident, $arg_type:ty, $expected:expr) => {{
                // SAFETY:
                // * The matched `Type` has the same layout as `$arg_type`.
                // * `copy_to` fully initializes `actual_arg`.
                let actual_arg = unsafe {
                    let mut actual_arg = MaybeUninit::<$arg_type>::uninit();
                    $arg.copy_to(&mut actual_arg);

                    actual_arg.assume_init()
                };

                assert_eq!(actual_arg, $expected);

                if let Some(ret) = $ret.as_mut()
                    && ret.ty() == $arg.ty()
                {
                    // SAFETY: The return type matches the argument type and `$arg_type` is FFI
                    // safe.
                    unsafe {
                        ret.write(actual_arg);
                    }
                }
            }};
        }

        let DynamicClosureCall {
            args,
            mut ret,
            context,
        } = call;

        for arg in &args {
            match arg.ty() {
                Type::I8 => test_argument!(arg, ret, i8, I8_ARG),
                Type::I16 => test_argument!(arg, ret, i16, I16_ARG),
                Type::I32 => test_argument!(arg, ret, i32, I32_ARG),
                Type::I64 => test_argument!(arg, ret, i64, I64_ARG),
                Type::Isize => test_argument!(arg, ret, isize, ISIZE_ARG),
                Type::U8 => test_argument!(arg, ret, u8, U8_ARG),
                Type::U16 => test_argument!(arg, ret, u16, U16_ARG),
                Type::U32 => test_argument!(arg, ret, u32, U32_ARG),
                Type::U64 => test_argument!(arg, ret, u64, U64_ARG),
                Type::Usize => test_argument!(arg, ret, usize, USIZE_ARG),
                Type::F32 => test_argument!(arg, ret, f32, F32_ARG),
                Type::F64 => test_argument!(arg, ret, f64, F64_ARG),
                Type::Pointer => test_argument!(arg, ret, *const c_void, PTR_ARG.0),
                Type::Struct(_) => {
                    assert_eq!(arg.ty(), &TestStruct::ffi_type());
                    test_argument!(arg, ret, TestStruct, STRUCT_ARG);
                }
            }
        }

        context.fetch_add(1, Ordering::Relaxed);
    }

    macro_rules! test_identity_closure_for_type_all_abis {
        (fn $testname:ident($type:ty, $value:expr)) => {
            #[test]
            #[cfg_attr(miri, ignore)]
            fn $testname() {
                let ffi_type = <$type as FfiType>::ffi_type();
                for abi in Abi::ABIS {
                    let context = AtomicUsize::new(0);

                    let closure = DynamicClosure::with_abi(
                        test_callback,
                        core::slice::from_ref(&ffi_type),
                        Some(&ffi_type),
                        abi,
                        &context,
                    );

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

                    assert_eq!(context.load(Ordering::Relaxed), 1);
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
        #[rustfmt::skip]
        let arg_types = [
            Type::I8, Type::I16, Type::I32, Type::I64, Type::Isize, Type::U8, Type::U16, Type::U32,
            Type::U64, Type::Usize, Type::F32, Type::F64, Type::Pointer,
            <TestStruct as FfiType>::ffi_type(),
        ];

        #[rustfmt::skip]
        let args = [
            arg(&I8_ARG), arg(&I16_ARG), arg(&I32_ARG), arg(&I64_ARG), arg(&ISIZE_ARG),
            arg(&U8_ARG), arg(&U16_ARG), arg(&U32_ARG), arg(&U64_ARG), arg(&USIZE_ARG),
            arg(&F32_ARG), arg(&F64_ARG), arg(&PTR_ARG.0), arg(&STRUCT_ARG),
        ];

        for abi in Abi::ABIS {
            let context = AtomicUsize::new(0);

            let closure = DynamicClosure::with_abi(test_callback, &arg_types, None, abi, &context);

            let function = Function::with_abi(closure.as_fn_ptr(), &arg_types, None, abi);

            // SAFETY: The `function` was built from the closure pointer with a matching
            // signature.
            unsafe {
                function.call(args.clone(), Ret::void());
            }
        }
    }
}
