//! Create function pointers backed by Rust closures or callbacks.
//!
//! The function pointers can be used as long as the [`Closure`] or [`DynamicClosure`] they were
//! created from are alive. Attempting to call a function pointer created by a dropped [`Closure`]
//! or [`DynamicClosure`] causes undefined behavior.
//!
//! Closures passed to [`Closure`] must implement `Fn`. They must also be `Send + Sync` because
//! foreign code may pass the generated function pointers to other threads and call the function
//! pointers multiple times concurrently.
//!
//! [`DynamicClosure`] creates function pointers whose signatures are known only at runtime and
//! dispatches calls through a Rust callback. Its callback argument and return helpers are available
//! in the [`dynamic`] module.
//!
//! # Examples
//!
//! ```
//! use fiffi::closure::Closure;
//!
//! let add_value = 1;
//! let add_one = Closure::new(|value: i32| value + add_value);
//!
//! // SAFETY: `add_one` represents an `extern "C"` function that accepts one `i32` and returns an
//! // `i32`. `add_one` remains alive while it is called.
//! let add_one_fn_ptr = unsafe { add_one.as_fn_ptr().into_fn::<extern "C" fn(i32) -> i32>() };
//!
//! assert_eq!(add_one_fn_ptr(1336), 1337);
//! ```
//!
//! Use [`DynamicClosure`] when the function signature is defined at runtime:
//!
//! ```
//! use core::mem::MaybeUninit;
//!
//! use fiffi::closure::DynamicClosure;
//! use fiffi::closure::dynamic::DynamicClosureCall;
//! use fiffi::types::Type;
//!
//! fn callback(mut call: DynamicClosureCall<i32>) {
//!     let arg = call
//!         .args()
//!         .get(0)
//!         .expect("This callback must be used with exactly one argument.");
//!
//!     if arg.ty() != &Type::I32 {
//!         // **NOTE** This will abort instead of unwinding regardless of the crate's setting
//!         // because Rust cannot unwind past libffi's `extern "C"` functions.
//!         panic!("This callback can only be used with an `i32` argument.");
//!     }
//!
//!     let mut value = MaybeUninit::<i32>::uninit();
//!
//!     // SAFETY: The argument's type is `i32`. The `copy_to` fully initializes `value`.
//!     let result = unsafe {
//!         arg.copy_to(&mut value);
//!         value.assume_init()
//!     } + call.context();
//!
//!     let ret = call.ret().expect("This callback expects a return value.");
//!
//!     if ret.ty() != &Type::I32 {
//!         // **NOTE** This will abort instead of unwinding regardless of the crate's setting
//!         // because Rust cannot unwind past libffi's `extern "C"` functions.
//!         panic!("This callback can only be used with an `i32` return value.");
//!     }
//!
//!     // SAFETY: The return value's type is `i32`.
//!     unsafe {
//!         ret.write(result);
//!     }
//! }
//!
//! let add_offset = DynamicClosure::new(callback, &[Type::I32], Some(&Type::I32), 5);
//!
//! // SAFETY: `add_offset` represents an `extern "C"` function that accepts one `i32` and returns
//! // an `i32`. `add_offset` remains alive while it is called.
//! let add_offset_fn = unsafe {
//!     add_offset
//!         .as_fn_ptr()
//!         .into_fn::<extern "C" fn(i32) -> i32>()
//! };
//!
//! assert_eq!(add_offset_fn(37), 42);
//! ```
//!
//! # Panics inside of closures
//!
//! If a closure called through [`Closure`] or a callback called through [`DynamicClosure`] panics,
//! the process will abort instead of unwinding regardless of the project's settings. This is
//! because panics cannot unwind across libffi's `extern "C"` functions.

mod compiletime;
pub mod dynamic;
pub(crate) mod raw;

use core::fmt::Debug;

pub use compiletime::{Closure, traits};
pub use dynamic::closure::DynamicClosure;
use libffi_sys::{ffi_prep_closure_loc, ffi_status_FFI_OK};

use crate::FnPtr;
use crate::closure::raw::{ClosureCallback, Context, LibffiClosure};
use crate::errors::ClosureAllocationError;
use crate::function::raw::Cif;

/// Base closure type that handles common code for fiffi's closure types.
struct BaseClosure<PAYLOAD> {
    libffi_closure: LibffiClosure,
    cif: Cif,
    context: Context<PAYLOAD>,
}

impl<PAYLOAD> BaseClosure<PAYLOAD> {
    /// Allocates and prepares a libffi closure around `payload`.
    ///
    /// # Safety
    ///
    /// `callback` must interpret the arguments, return storage, and `PAYLOAD` pointer according to
    /// `cif`.
    ///
    /// # Errors
    ///
    /// This function returns `ClosureAllocationError` if libffi fails to allocate the closure.
    unsafe fn try_new(
        cif: Cif,
        callback: ClosureCallback,
        context: Context<PAYLOAD>,
    ) -> Result<Self, ClosureAllocationError> {
        let mut libffi_closure = LibffiClosure::try_new()?;

        // SAFETY:
        // * `libffi_closure` owns writable closure storage allocated by libffi.
        // * `cif` is initialized and remains alive in the returned `PreparedClosure`.
        // * The caller guarantees that `callback` matches `cif` and correctly handles the payload.
        // * `payload` owns a stable allocation that remains alive in the returned value.
        // * `libffi_closure.as_fn_ptr()` is the executable address paired with this closure.
        let status = unsafe {
            ffi_prep_closure_loc(
                libffi_closure.as_ffi_closure_mut_ptr(),
                cif.as_ffi_cif_ptr(),
                Some(callback),
                context.as_ptr().cast_mut().cast(),
                libffi_closure.as_fn_ptr().as_c_void_ptr(),
            )
        };

        let closure = Self {
            libffi_closure,
            cif,
            context,
        };

        #[allow(
            clippy::missing_panics_doc,
            reason = "Internal sanity check only fails if there is an internal bug in this crate."
        )]
        {
            // PANIC: `ffi_prep_closure_loc` should never return an error when safe fiffi functions
            // are used. An error signifies a bug in fiffi and we cannot guarantee correct behavior.
            assert_eq!(
                status, ffi_status_FFI_OK,
                "Libffi returned the error code {status} from `ffi_prep_closure_loc`."
            );
        }

        Ok(closure)
    }

    fn as_fn_ptr(&self) -> FnPtr {
        self.libffi_closure.as_fn_ptr()
    }
}

// SAFETY: Moving a `BaseClosure` moves only RAII owners for stable heap and libffi allocations.
// Sending it is sound when its payload is `Send`.
unsafe impl<PAYLOAD> Send for BaseClosure<PAYLOAD> where PAYLOAD: Send {}

// SAFETY: A `BaseClosure` exposes only a copyable function pointer and a shared payload reference.
// Sharing it is sound when its payload is `Sync`.
unsafe impl<PAYLOAD> Sync for BaseClosure<PAYLOAD> where PAYLOAD: Sync {}

impl<PAYLOAD> Debug for BaseClosure<PAYLOAD> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BaseClosure")
            .field("libffi_closure", &self.libffi_closure)
            .field("cif", &self.cif)
            .field("context", &self.context)
            .finish()
    }
}
