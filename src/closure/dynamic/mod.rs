//! Helper types used by [`DynamicClosure`](`crate::closure::DynamicClosure`) callbacks.

pub(super) mod closure;

use core::ffi::c_void;
use core::fmt::Debug;
use core::iter::FusedIterator;
use core::mem::MaybeUninit;
use core::slice;

use crate::return_buffer::{write_closure_result, write_closure_result_bytes};
use crate::types::{FfiTypeLayout, Type};
#[cfg(msan)]
use crate::{__msan_poison, __msan_test_shadow, __msan_unpoison};

/// Information provided to the callback when a [`DynamicClosure`] is executed.
///
/// # Example
///
/// ```
/// use fiffi::closure::dynamic::DynamicClosureCall;
///
/// fn callback(mut call: DynamicClosureCall<i32>) {
///     for args in call.args() {
///         // Process args here, inspecting their types and reading their values.
///     }
///
///     if let Some(ret) = call.ret() {
///         // Process the result here, inspecting its type and writing it.
///     }
///
///     // A reference to the context provided when creating the `DynamicClosure` calling this
///     // callback.
///     let ctx = call.context();
/// }
/// ```
///
/// [`DynamicClosure`]: `crate::closure::DynamicClosure`
pub struct DynamicClosureCall<'call, CONTEXT> {
    args: DynamicClosureArgs<'call>,
    ret: Option<DynamicClosureRet<'call>>,
    context: &'call CONTEXT,
}

impl<'call, CONTEXT> DynamicClosureCall<'call, CONTEXT> {
    /// Returns the arguments passed to this closure invocation.
    pub fn args(&self) -> &DynamicClosureArgs<'call> {
        &self.args
    }

    /// Returns the return value storage or `None` if the closure has no return value.
    pub fn ret(&mut self) -> Option<&mut DynamicClosureRet<'call>> {
        self.ret.as_mut()
    }

    /// Returns the context provided when the [`DynamicClosure`] was created.
    ///
    /// [`DynamicClosure`]: `crate::closure::DynamicClosure`
    pub fn context(&self) -> &CONTEXT {
        self.context
    }
}

impl<CONTEXT> Debug for DynamicClosureCall<'_, CONTEXT>
where
    CONTEXT: Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DynamicClosureCall")
            .field("args", &self.args)
            .field("ret", &self.ret)
            .field("context", &self.context)
            .finish()
    }
}

/// Arguments passed to a [`DynamicClosure`] invocation.
///
/// Each argument contains the configured [`Type`] and the raw pointer to the argument provided by
/// libffi. The pointers are valid only for the duration of the callback invocation.
///
/// `DynamicClosureArgs` implements `IntoIterator`, which can be used to iterate over all arguments
/// to a callback.
///
/// # Example
///
/// ```
/// use fiffi::closure::DynamicClosure;
/// use fiffi::closure::dynamic::DynamicClosureCall;
/// use fiffi::types::Type;
///
/// fn callback(call: DynamicClosureCall<()>) {
///     let args = call.args();
///
///     assert_eq!(args.len(), 2);
///     assert_eq!(args.get(0).expect("first argument").ty(), &Type::I32);
///     assert_eq!(args.get(1).expect("second argument").ty(), &Type::U64);
///
///     // `DynamicClosureArgs` also implements `IntoIterator` which can be used to iterate through
///     // all the arguments.
/// }
///
/// let closure = DynamicClosure::new(callback, &[Type::I32, Type::U64], None, ());
///
/// // SAFETY: `closure` represents an `extern "C"` function that accepts an `i32` and a `u64` and
/// // returns no value. `closure` remains alive while it is called.
/// let function = unsafe { closure.as_fn_ptr().into_fn::<extern "C" fn(i32, u64)>() };
///
/// function(1, 2);
/// ```
///
/// [`DynamicClosure`]: `crate::closure::DynamicClosure`
#[derive(Debug)]
pub struct DynamicClosureArgs<'call> {
    argument_types: &'call [Type],
    argument_layouts: &'call [FfiTypeLayout],
    argument_array: *const *const c_void,
}

impl<'call> DynamicClosureArgs<'call> {
    /// Returns the number of arguments.
    pub fn len(&self) -> usize {
        self.argument_types.len()
    }

    /// Returns `true` if the closure has no arguments.
    pub fn is_empty(&self) -> bool {
        self.argument_types.is_empty()
    }

    /// Returns the argument at `index` or `None` if `index` is out of bounds.
    pub fn get(&self, index: usize) -> Option<DynamicClosureArg<'call>> {
        let ty = self.argument_types.get(index)?;
        let layout = self.argument_layouts.get(index)?;

        // SAFETY: `argument_array` contains one pointer for every item in `argument_types`, and
        // the slice lookups above verified that `index` is in bounds.
        let ptr = unsafe { *self.argument_array.add(index) };

        Some(DynamicClosureArg { ty, layout, ptr })
    }

    /// Returns an iterator over the closure arguments in parameter order.
    pub fn iter(&self) -> DynamicClosureArgsIter<'_, 'call> {
        self.into_iter()
    }
}

impl<'args, 'call> IntoIterator for &'args DynamicClosureArgs<'call> {
    type Item = DynamicClosureArg<'call>;

    type IntoIter = DynamicClosureArgsIter<'args, 'call>;

    fn into_iter(self) -> Self::IntoIter {
        DynamicClosureArgsIter {
            args: self,
            index: 0,
        }
    }
}

/// Iterator over the arguments passed to a [`DynamicClosure`] invocation.
///
/// [`DynamicClosure`]: `crate::closure::DynamicClosure`
#[derive(Debug)]
pub struct DynamicClosureArgsIter<'args, 'call> {
    args: &'args DynamicClosureArgs<'call>,
    index: usize,
}

impl<'call> Iterator for DynamicClosureArgsIter<'_, 'call> {
    type Item = DynamicClosureArg<'call>;

    fn next(&mut self) -> Option<Self::Item> {
        let arg = self.args.get(self.index)?;
        self.index += 1;

        Some(arg)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.args.len() - self.index;

        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for DynamicClosureArgsIter<'_, '_> {}

impl FusedIterator for DynamicClosureArgsIter<'_, '_> {}

/// One argument passed to a [`DynamicClosure`] invocation.
///
/// # Example
///
/// ```
/// use core::mem::MaybeUninit;
///
/// use fiffi::closure::DynamicClosure;
/// use fiffi::closure::dynamic::DynamicClosureCall;
/// use fiffi::types::Type;
///
/// fn callback(call: DynamicClosureCall<()>) {
///     let arg = call
///         .args()
///         .get(0)
///         .expect("This callback expects one argument.");
///
///     if arg.ty() != &Type::I32 {
///         // **NOTE** This will abort instead of unwinding regardless of the crate's setting
///         // because Rust cannot unwind past libffi's `extern "C"` functions.
///         panic!("This callback can only be used with an `i32` argument.");
///     }
///
///     let mut value = MaybeUninit::<i32>::uninit();
///
///     // SAFETY: The argument's type is `i32`. The `copy_to` fully initializes `value`.
///     unsafe {
///         arg.copy_to(&mut value);
///         assert_eq!(value.assume_init(), 42);
///     }
///
///     // Arguments can also be read directly through pointers.
///     let ptr = arg.as_ptr().cast::<i32>();
///     // SAFETY: `ptr` points to a `i32`.
///     unsafe {
///         assert_eq!(*ptr, 42);
///     }
///
///     // It is also possible to get a byte slice to the underlying bytes of an argument. Note that
///     // this may copy uninitialized bytes (for example padding), reading those will invoke
///     // undefined behavior.
///     // SAFETY: `int_bytes` are the bytes of an `i32` where every byte is initialized.
///     let int_bytes = unsafe { arg.as_bytes().assume_init_ref() };
///     assert_eq!(&42i32.to_ne_bytes(), int_bytes);
/// }
///
/// let closure = DynamicClosure::new(callback, &[Type::I32], None, ());
///
/// // SAFETY: `closure` represents an `extern "C"` function that accepts one `i32` and returns no
/// // value. `closure` remains alive while it is called.
/// let function = unsafe { closure.as_fn_ptr().into_fn::<extern "C" fn(i32)>() };
///
/// function(42);
/// ```
///
/// [`DynamicClosure`]: `crate::closure::DynamicClosure`
#[derive(Debug)]
pub struct DynamicClosureArg<'call> {
    ty: &'call Type,
    layout: &'call FfiTypeLayout,
    ptr: *const c_void,
}

impl DynamicClosureArg<'_> {
    /// Returns this argument's type.
    pub fn ty(&self) -> &Type {
        self.ty
    }

    /// Returns the size and alignment of this argument's type.
    pub fn layout(&self) -> &FfiTypeLayout {
        self.layout
    }

    /// Returns a pointer to the argument storage provided by libffi.
    ///
    /// The pointer is valid only for the duration of the callback invocation. On certain
    /// platforms, the argument is not guaranteed to be correctly aligned.
    pub fn as_ptr(&self) -> *const c_void {
        self.ptr
    }

    /// Return a byte slice of this argument's underlying bytes.
    ///
    /// This function returns a slice of `MaybeUninit<u8>` as the slice may contain uninitialized
    /// data, for example in padding bytes.
    ///
    /// # Example
    ///
    /// ```
    /// use fiffi::closure::DynamicClosure;
    /// use fiffi::closure::dynamic::DynamicClosureCall;
    /// use fiffi::types::Type;
    ///
    /// fn callback(call: DynamicClosureCall<()>) {
    ///     let arg = call
    ///         .args()
    ///         .get(0)
    ///         .expect("This callback expects one argument.");
    ///
    ///     if arg.ty() != &Type::I32 {
    ///         // **NOTE** This will abort instead of unwinding regardless of the crate's setting
    ///         // because Rust cannot unwind past libffi's `extern "C"` functions.
    ///         panic!("This callback can only be used with an `i32` argument.");
    ///     }
    ///
    ///     // SAFETY: The argument's type is `i32`, which has no padding, so every byte is
    ///     // initialized.
    ///     let bytes = unsafe { arg.as_bytes().assume_init_ref() };
    ///
    ///     assert_eq!(bytes, &42i32.to_ne_bytes());
    /// }
    ///
    /// let closure = DynamicClosure::new(callback, &[Type::I32], None, ());
    ///
    /// // SAFETY: `closure` represents an `extern "C"` function that accepts one `i32` and returns
    /// // no value. `closure` remains alive while it is called.
    /// let function = unsafe { closure.as_fn_ptr().into_fn::<extern "C" fn(i32)>() };
    ///
    /// function(42);
    /// ```
    pub fn as_bytes(&self) -> &[MaybeUninit<u8>] {
        // SAFETY:
        // * libffi has allocated `self.ptr` and written an argument that is `self.layout.size`
        //   bytes long to the address.
        // * The memory allocated by libffi is alive at least as long as the `DynamicClosureCall`
        //   is.
        unsafe { slice::from_raw_parts(self.ptr.cast(), self.layout.size) }
    }

    /// Copy the argument to a variable.
    ///
    /// After this function has been called, it can be assumed that `dest` is fully initialized as
    /// long as the safety requirements are fulfilled.
    ///
    /// # Safety
    ///
    /// * `ARG` must have the same size, alignment, and layout as `self.ty` for the given ABI.
    /// * The argument supplied by the caller must be a valid value of type `ARG`.
    ///
    /// # Example
    ///
    /// ```
    /// use core::mem::MaybeUninit;
    ///
    /// use fiffi::closure::DynamicClosure;
    /// use fiffi::closure::dynamic::DynamicClosureCall;
    /// use fiffi::types::Type;
    ///
    /// fn callback(call: DynamicClosureCall<()>) {
    ///     let arg = call
    ///         .args()
    ///         .get(0)
    ///         .expect("This callback expects one argument.");
    ///
    ///     if arg.ty() != &Type::I32 {
    ///         // **NOTE** This will abort instead of unwinding regardless of the crate's setting
    ///         // because Rust cannot unwind past libffi's `extern "C"` functions.
    ///         panic!("This callback can only be used with an `i32` argument.");
    ///     }
    ///
    ///     let mut value = MaybeUninit::<i32>::uninit();
    ///
    ///     // SAFETY: The argument's type is `i32`. The `copy_to` fully initializes `value`.
    ///     unsafe {
    ///         arg.copy_to(&mut value);
    ///         assert_eq!(value.assume_init(), 42);
    ///     }
    /// }
    ///
    /// let closure = DynamicClosure::new(callback, &[Type::I32], None, ());
    ///
    /// // SAFETY: `closure` represents an `extern "C"` function that accepts one `i32` and returns
    /// // no value. `closure` remains alive while it is called.
    /// let function = unsafe { closure.as_fn_ptr().into_fn::<extern "C" fn(i32)>() };
    ///
    /// function(42);
    /// ```
    pub unsafe fn copy_to<ARG>(&self, dest: &mut MaybeUninit<ARG>)
    where
        ARG: Copy,
    {
        let arg_ptr = self.ptr.cast::<ARG>();

        // SAFETY:
        // * It is up to the caller to ensure that the argument can be read as an `ARG` and that an
        //   `ARG` can be written to `dest`.
        // * On 32-bit Windows, the argument is not guaranteed to be correctly aligned, so an
        //   unaligned read followed by a write is used as the caller should ensure that the
        //   destination is aligned properly.
        unsafe {
            // Memory may point to space allocated by libffi that needs unpoisoning for msan.
            #[cfg(msan)]
            let poisoned = {
                if __msan_test_shadow(arg_ptr.cast(), size_of::<ARG>()) != -1 {
                    __msan_unpoison(arg_ptr.cast(), size_of::<ARG>());

                    true
                } else {
                    false
                }
            };

            if cfg!(not(all(target_arch = "x86", target_os = "windows"))) {
                arg_ptr.copy_to(dest.as_mut_ptr(), 1);
            } else {
                dest.as_mut_ptr().write(arg_ptr.read_unaligned());
            }

            #[cfg(msan)]
            if poisoned {
                __msan_poison(arg_ptr.cast(), size_of::<ARG>());
            }
        }
    }
}

/// Return value storage for a [`DynamicClosure`] invocation.
///
/// The storage is valid only for the duration of the callback invocation.
///
/// # Example
///
/// ```
/// use fiffi::closure::DynamicClosure;
/// use fiffi::closure::dynamic::DynamicClosureCall;
/// use fiffi::types::Type;
///
/// fn callback(mut call: DynamicClosureCall<()>) {
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
///         ret.write(42i32);
///     }
/// }
///
/// let closure = DynamicClosure::new(callback, &[], Some(&Type::I32), ());
///
/// // SAFETY: `closure` represents an `extern "C"` function that accepts no arguments and returns
/// // an `i32`. `closure` remains alive while it is called.
/// let function = unsafe { closure.as_fn_ptr().into_fn::<extern "C" fn() -> i32>() };
///
/// assert_eq!(function(), 42);
/// ```
///
/// [`DynamicClosure`]: `crate::closure::DynamicClosure`
#[derive(Debug)]
pub struct DynamicClosureRet<'call> {
    ty: &'call Type,
    layout: &'call FfiTypeLayout,
    ptr: *mut c_void,
}

impl DynamicClosureRet<'_> {
    /// Returns the closure's return type.
    pub fn ty(&self) -> &Type {
        self.ty
    }

    /// Returns the size and alignment of the closure's return type.
    pub fn layout(&self) -> &FfiTypeLayout {
        self.layout
    }

    /// Returns a pointer to where the return value should be written.
    ///
    /// The pointer is valid only for the duration of the callback invocation.
    ///
    /// ## Warning
    ///
    /// Libffi expects integer return values smaller than a register to be promoted to a full
    /// register width. Writing to this pointer directly must handle this promotion correctly. Use
    /// [`DynamicClosureRet::write`] or [`DynamicClosureRet::write_bytes`] when possible because
    /// they handle these cases correctly.
    pub fn as_mut_ptr(&self) -> *mut c_void {
        self.ptr
    }

    /// Write the result value from a slice of bytes.
    ///
    /// This function automatically handles integer promotion if the integer is smaller than a full
    /// register.
    ///
    /// # Safety
    ///
    /// * `result_bytes` must have the same length as `self.layout.size`.
    /// * The bytes in `result_bytes` must represent a valid value of the declared return type.
    /// * No other references to the memory pointed to by `self.ptr` must exist.
    ///
    /// # Example
    ///
    /// ```
    /// use fiffi::closure::DynamicClosure;
    /// use fiffi::closure::dynamic::DynamicClosureCall;
    /// use fiffi::types::Type;
    ///
    /// fn callback(mut call: DynamicClosureCall<i32>) {
    ///     let result_bytes = call.context().to_ne_bytes();
    ///     let ret = call.ret().expect("This callback expects a return value.");
    ///
    ///     if ret.ty() != &Type::I32 {
    ///         // **NOTE** This will abort instead of unwinding regardless of the crate's setting
    ///         // because Rust cannot unwind past libffi's `extern "C"` functions.
    ///         panic!("This callback can only be used with an `i32` return value.");
    ///     }
    ///
    ///     // SAFETY: The return value's type is `i32`, and `result_bytes` contains a valid `i32`.
    ///     unsafe {
    ///         ret.write_bytes(&result_bytes);
    ///     }
    /// }
    ///
    /// let closure = DynamicClosure::new(callback, &[], Some(&Type::I32), 42);
    ///
    /// // SAFETY: `closure` represents an `extern "C"` function that accepts no arguments and
    /// // returns an `i32`. `closure` remains alive while it is called.
    /// let function = unsafe { closure.as_fn_ptr().into_fn::<extern "C" fn() -> i32>() };
    ///
    /// assert_eq!(function(), 42);
    /// ```
    pub unsafe fn write_bytes(&mut self, result_bytes: &[u8]) {
        // SAFETY: It is up to the caller to ensure that `result_bytes` contains a valid return
        // value with the expected length and that no other references to the result storage exist.
        unsafe {
            write_closure_result_bytes(result_bytes, self.ty, self.layout.size, self.ptr);
        }
    }

    /// Write the result value.
    ///
    /// This function automatically handles integer promotion if the integer is smaller than a full
    /// register.
    ///
    /// # Safety
    ///
    /// * `RET` must have the same size, alignment, and layout as `self.ty` for the given ABI.
    /// * `result` must be a valid value that can be returned to the caller of this closure.
    /// * No references to the memory pointed to by `self.ptr` must exist.
    ///
    /// # Example
    ///
    /// ```
    /// use fiffi::closure::DynamicClosure;
    /// use fiffi::closure::dynamic::DynamicClosureCall;
    /// use fiffi::types::Type;
    ///
    /// fn callback(mut call: DynamicClosureCall<i32>) {
    ///     let result = *call.context();
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
    ///         ret.write(result);
    ///     }
    /// }
    ///
    /// let closure = DynamicClosure::new(callback, &[], Some(&Type::I32), 42);
    ///
    /// // SAFETY: `closure` represents an `extern "C"` function that accepts no arguments and
    /// // returns an `i32`. `closure` remains alive while it is called.
    /// let function = unsafe { closure.as_fn_ptr().into_fn::<extern "C" fn() -> i32>() };
    ///
    /// assert_eq!(function(), 42);
    /// ```
    pub unsafe fn write<RET>(&mut self, result: RET)
    where
        RET: Copy,
    {
        // SAFETY: It is up to the caller to ensure that it is safe to write `RET`.
        unsafe {
            write_closure_result(result, self.ty, self.ptr.cast());
        }
    }
}

#[cfg(test)]
mod tests {
    use core::ptr;

    use super::*;
    use crate::test_utils::{F64_ARG, I8_ARG, I16_ARG, U8_ARG, U16_ARG, U64_ARG};
    #[cfg(target_pointer_width = "64")]
    use crate::test_utils::{I32_ARG, U32_ARG};

    fn verify_full_register_was_written(
        return_buffer: MaybeUninit<usize>,
        expected_result_bytes: &[u8],
    ) {
        // SAFETY: Small integer result writers must initialize the full promoted return buffer.
        let actual_bytes = unsafe { return_buffer.assume_init() }.to_ne_bytes();
        let mut expected_bytes = [0; size_of::<usize>()];
        let result_offset = if cfg!(target_endian = "little") {
            0
        } else {
            size_of::<usize>() - expected_result_bytes.len()
        };
        let result_end = result_offset + expected_result_bytes.len();
        expected_bytes[result_offset..result_end].copy_from_slice(expected_result_bytes);

        assert_eq!(actual_bytes, expected_bytes);
    }

    #[test]
    fn args_expose_expected_memory_and_iterate_in_order() {
        let u8_value = U8_ARG;
        let i16_value = I16_ARG;
        let f64_value = F64_ARG;
        let argument_types = [Type::U8, Type::I16, Type::F64];
        let argument_layouts = [
            argument_types[0].layout(),
            argument_types[1].layout(),
            argument_types[2].layout(),
        ];
        let argument_array = [
            (&raw const u8_value).cast::<c_void>(),
            (&raw const i16_value).cast::<c_void>(),
            (&raw const f64_value).cast::<c_void>(),
        ];
        let args = DynamicClosureArgs {
            argument_types: &argument_types,
            argument_layouts: &argument_layouts,
            argument_array: argument_array.as_ptr(),
        };

        assert_eq!(args.len(), 3);
        assert!(!args.is_empty());
        assert!(args.get(3).is_none());

        macro_rules! assert_argument {
            ($index:expr, $value:ident, $type:ty) => {{
                let arg = args.get($index).unwrap();
                assert_eq!(arg.ty(), &argument_types[$index]);
                assert_eq!(arg.layout(), &argument_layouts[$index]);
                assert_eq!(arg.as_ptr(), ptr::from_ref(&$value).cast());

                // SAFETY: Every value used by this test is fully initialized and contains no
                // padding bytes.
                unsafe {
                    let expected_bytes =
                        slice::from_raw_parts((&raw const $value).cast::<u8>(), size_of::<$type>());
                    assert_eq!(arg.as_bytes().assume_init_ref(), expected_bytes);
                };

                let mut copied = MaybeUninit::<$type>::uninit();
                // SAFETY: The argument type and explicit layout describe `$type`.
                unsafe {
                    arg.copy_to(&mut copied);
                }
                // SAFETY: `copy_to` initialized the complete value.
                assert_eq!(unsafe { copied.assume_init() }, $value);
            }};
        }

        assert_argument!(0, u8_value, u8);
        assert_argument!(1, i16_value, i16);
        assert_argument!(2, f64_value, f64);

        let mut iter = args.iter();
        assert_eq!(iter.len(), 3);
        assert_eq!(iter.size_hint(), (3, Some(3)));
        assert_eq!(iter.next().unwrap().ty(), &argument_types[0]);
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.size_hint(), (2, Some(2)));
        assert_eq!(iter.next().unwrap().ty(), &argument_types[1]);
        assert_eq!(iter.next().unwrap().ty(), &argument_types[2]);
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
    }

    #[test]
    fn ret_exposes_metadata_and_write_writes_scalar() {
        let return_type = Type::U64;
        let return_layout = return_type.layout();
        let mut return_value = MaybeUninit::<u64>::uninit();
        let return_ptr = return_value.as_mut_ptr().cast();
        let mut ret = DynamicClosureRet {
            ty: &return_type,
            layout: &return_layout,
            ptr: return_ptr,
        };

        assert_eq!(ret.ty(), &return_type);
        assert_eq!(ret.layout(), &return_layout);
        assert_eq!(ret.as_mut_ptr(), return_ptr);

        // SAFETY: `return_type` and `return_layout` describe `u64`, and `return_value` is writable.
        unsafe {
            ret.write(U64_ARG);
        }

        // SAFETY: `ret.write` initialized `return_value`.
        assert_eq!(unsafe { return_value.assume_init() }, U64_ARG);
    }

    #[test]
    fn write_promotes_small_integers_to_full_register() {
        fn verify<RET>(result: RET, return_type: &Type, expected_result_bytes: &[u8])
        where
            RET: Copy,
        {
            let return_layout = return_type.layout();
            let mut return_buffer = MaybeUninit::new(usize::MAX);
            let mut ret = DynamicClosureRet {
                ty: return_type,
                layout: &return_layout,
                ptr: (&raw mut return_buffer).cast(),
            };

            // SAFETY: `return_type` and `return_layout` describe `RET`, and `return_buffer` is
            // writable, register-sized, and register-aligned storage.
            unsafe {
                ret.write(result);
            }

            verify_full_register_was_written(return_buffer, expected_result_bytes);
        }

        verify(I8_ARG, &Type::I8, &I8_ARG.to_ne_bytes());
        verify(U8_ARG, &Type::U8, &U8_ARG.to_ne_bytes());
        verify(I16_ARG, &Type::I16, &I16_ARG.to_ne_bytes());
        verify(U16_ARG, &Type::U16, &U16_ARG.to_ne_bytes());

        #[cfg(target_pointer_width = "64")]
        {
            verify(I32_ARG, &Type::I32, &I32_ARG.to_ne_bytes());
            verify(U32_ARG, &Type::U32, &U32_ARG.to_ne_bytes());
        }
    }

    #[test]
    fn write_bytes_promotes_small_integers_to_full_register() {
        fn verify(result_bytes: &[u8], return_type: &Type) {
            let return_layout = return_type.layout();
            let mut return_buffer = MaybeUninit::new(usize::MAX);
            let mut ret = DynamicClosureRet {
                ty: return_type,
                layout: &return_layout,
                ptr: (&raw mut return_buffer).cast(),
            };

            // SAFETY: `result_bytes` contains a valid `RET`, and `return_buffer` is writable,
            // register-sized, and register-aligned storage.
            unsafe {
                ret.write_bytes(result_bytes);
            }

            verify_full_register_was_written(return_buffer, result_bytes);
        }

        verify(&I8_ARG.to_ne_bytes(), &Type::I8);
        verify(&U8_ARG.to_ne_bytes(), &Type::U8);
        verify(&I16_ARG.to_ne_bytes(), &Type::I16);
        verify(&U16_ARG.to_ne_bytes(), &Type::U16);

        #[cfg(target_pointer_width = "64")]
        {
            verify(&I32_ARG.to_ne_bytes(), &Type::I32);
            verify(&U32_ARG.to_ne_bytes(), &Type::U32);
        }
    }
}
