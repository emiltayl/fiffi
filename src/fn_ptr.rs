use core::ffi::c_void;
use core::ptr::NonNull;

/// A untyped function pointer used by fiffi's function and closure APIs.
///
/// `FnPtr` does not encode or validate a function's ABI or signature. When passing one to a
/// [`Function`](crate::function::Function), the caller must ensure that it matches the ABI and
/// signature used to construct the function. When converting a pointer returned by one of fiffi's
/// closure APIs, the caller must use the ABI and signature that the closure was created with and
/// keep the closure alive while the pointer is used.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
#[repr(transparent)]
pub struct FnPtr(NonNull<c_void>);

// SAFETY: `FnPtr` only stores a non-null function pointer address. Sending or sharing that address
// does not by itself call the pointed-to function; call safety remains part of the caller's
// contract.
unsafe impl Send for FnPtr {}

// SAFETY: See the `Send` impl. Shared access to `FnPtr` only permits copying or reading the
// function pointer address.
unsafe impl Sync for FnPtr {}

impl FnPtr {
    /// Wraps a non-null raw function pointer.
    ///
    /// It is up to the caller to ensure that `ptr` is a valid function pointer with the appropriate
    /// ABI and signature wherever it is used.
    pub fn new(ptr: NonNull<c_void>) -> Self {
        Self(ptr)
    }

    /// Wraps a raw function pointer, returning `None` if it is null.
    ///
    /// It is up to the caller to ensure that `ptr` is a valid function pointer with the appropriate
    /// ABI and signature wherever it is used.
    pub fn from_raw_ptr(ptr: *mut c_void) -> Option<Self> {
        NonNull::new(ptr).map(Self::new)
    }

    /// Creates a function pointer wrapper without checking for null.
    ///
    /// It is up to the caller to ensure that `ptr` is a valid function pointer with the appropriate
    /// ABI and signature wherever it is used.
    ///
    /// # Safety
    ///
    /// `ptr` must be non-null. Any later call through this value must also ensure that the pointer
    /// is a valid function pointer for the ABI and signature used at the call site.
    pub unsafe fn from_raw_ptr_unchecked(ptr: *mut c_void) -> Self {
        // SAFETY: It is up to the caller to ensure that it is safe to call
        // `NonNull::new_unchecked`.
        unsafe { Self(NonNull::new_unchecked(ptr)) }
    }

    /// Returns the stored pointer as `*mut c_void`.
    pub fn as_c_void_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    /// Helper function to convert a `FnPtr` into an actual `fn` pointer.
    ///
    /// This function uses [`transmute_copy`] to convert the pointer contained in `self` to `F`.
    ///
    /// # Safety
    ///
    /// `into_fn` does not perform any verification, it is up to the caller to ensure that
    /// transmuting `NonNull<c_void>` to `F` is safe.
    ///
    /// Further use of the result of this function will invoke undefined behavior unless `F` matches
    /// `self`'s function signature, including the ABI.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "closure")]
    /// # {
    /// use fiffi::closure::Closure;
    ///
    /// let add = Closure::new(|x: i32, y: i32| x + y);
    ///
    /// // SAFETY: `add` is an `extern "C"` function that accepts two `i32` arguments, returns
    /// // `i32`, and remains alive while the function pointer is used.
    /// let add_fn = unsafe { add.as_fn_ptr().into_fn::<extern "C" fn(i32, i32) -> i32>() };
    ///
    /// assert_eq!(add_fn(1, 2), 3);
    /// # }
    /// ```
    ///
    /// [`transmute_copy`]: `core::mem::transmute_copy`
    pub unsafe fn into_fn<F>(self) -> F {
        // SAFETY: It is up to the caller to ensure that this is safe.
        unsafe { core::mem::transmute_copy::<NonNull<c_void>, F>(&self.0) }
    }

    pub(crate) fn as_libffi_sys_ptr(self) -> Option<unsafe extern "C" fn()> {
        // SAFETY: On systems where fiffi is supported, simply creating a function pointer is not
        // immediate undefined behavior. It is up to the caller to ensure that an actual valid
        // function pointer is provided when creating a `FnPtr`.
        unsafe { core::mem::transmute::<NonNull<c_void>, Option<unsafe extern "C" fn()>>(self.0) }
    }
}

/// Convert a function to a [`FnPtr`].
///
/// This is primarily intended to make it easier to write test cases. If you already have a valid
/// function pointer it is probably better to call it directly instead of through `fiffi`.
///
/// # Example
///
/// ```
/// use fiffi::fn_ptrize;
/// use fiffi::function::{Function, Ret};
///
/// extern "C" fn ping() {}
///
/// let function = Function::new(fn_ptrize!(ping), &[], None);
///
/// // SAFETY: `function` was built from `ping` which has the function signature `extern "C" fn()`.
/// unsafe {
///     function.call([], Ret::void());
/// }
/// ```
#[macro_export]
macro_rules! fn_ptrize {
    ($fn:ident) => {
        // As long as `fn_ptrize` is an actual function pointer, Rust should optimize away the
        // `unwrap`.
        $crate::FnPtr::from_raw_ptr($fn as *mut ::core::ffi::c_void).unwrap()
    };
}
