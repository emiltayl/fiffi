extern crate alloc;

#[cfg(not(test))]
use alloc::boxed::Box;
use core::ffi::c_void;
use core::fmt::Debug;
use core::ptr::{NonNull, null_mut};

use libffi_sys::{ffi_cif, ffi_closure, ffi_closure_alloc, ffi_closure_free};

use crate::FnPtr;
use crate::errors::ClosureAllocationError;

pub type ClosureCallback = unsafe extern "C" fn(
    cif: *mut ffi_cif,
    return_value: *mut c_void,
    arguments: *mut *mut c_void,
    user_data: *mut c_void,
);

#[derive(Debug)]
pub struct LibffiClosure {
    closure: NonNull<ffi_closure>,
    fn_ptr: FnPtr,
}

/// RAII wrapper for `ffi_closure`.
impl LibffiClosure {
    pub fn try_new() -> Result<Self, ClosureAllocationError> {
        let mut fn_ptr = null_mut::<c_void>();

        // SAFETY:
        //   * The test `verify_ffi_closure_size` ensures that `ffi_closure` is the correct size so
        //     sufficient memory is allocated.
        //   * `fn_ptr` is a `mut *mut c_void`.
        let closure = unsafe {
            ffi_closure_alloc(size_of::<ffi_closure>(), &raw mut fn_ptr).cast::<ffi_closure>()
        };

        match (NonNull::new(closure), FnPtr::from_raw_ptr(fn_ptr)) {
            (Some(closure), Some(fn_ptr)) => Ok(Self { closure, fn_ptr }),
            (Some(closure), None) => {
                // SAFETY: `closure` was allocated by `ffi_closure_alloc` above, but no
                // `LibffiClosure` will be created to own it because it failed.
                unsafe {
                    ffi_closure_free(closure.as_ptr().cast());
                }

                Err(ClosureAllocationError)
            }
            _ => Err(ClosureAllocationError),
        }
    }

    pub fn as_ffi_closure_mut_ptr(&mut self) -> *mut ffi_closure {
        self.closure.as_ptr()
    }

    pub fn as_fn_ptr(&self) -> FnPtr {
        self.fn_ptr
    }
}

impl Drop for LibffiClosure {
    fn drop(&mut self) {
        // SAFETY: `self.closure` is the `ffi_closure` allocated and managed by `self`. It must be
        // freed using `ffi_closure_free` to avoid leaking memory.
        unsafe {
            ffi_closure_free(self.closure.as_ptr().cast());
        }
    }
}

/// Wrapper for the "context" that libffi passes to closures.
///
/// This wrapper allocates space for the context and makes sure it does not move.
pub struct Context<PAYLOAD> {
    payload: NonNull<PAYLOAD>,
}

impl<PAYLOAD> Context<PAYLOAD> {
    pub fn new(payload: PAYLOAD) -> Self {
        // SAFETY: `Box::into_raw` always returns a non-null pointer.
        let payload = unsafe { NonNull::new_unchecked(Box::into_raw(Box::new(payload))) };

        Self { payload }
    }

    pub fn as_ptr(&self) -> *const PAYLOAD {
        self.payload.as_ptr()
    }
}

impl<PAYLOAD> Drop for Context<PAYLOAD> {
    fn drop(&mut self) {
        // SAFETY: `self.payload` was created from `Box::into_raw` in `Context::new`. The `Box` must
        // be re-created using `Box::from_raw` to properly free the memory.
        let _box = unsafe { Box::<PAYLOAD>::from_raw(self.payload.as_ptr()) };
    }
}

// SAFETY: Moving a `Context` moves only RAII owners for stable heap and libffi allocations. Sending
// it is sound when its payload is `Send`.
unsafe impl<PAYLOAD> Send for Context<PAYLOAD> where PAYLOAD: Send {}

// SAFETY: A `Context` exposes only a copyable function pointer and a shared payload reference.
// Sharing it is sound when its payload is `Sync`.
unsafe impl<PAYLOAD> Sync for Context<PAYLOAD> where PAYLOAD: Sync {}

impl<PAYLOAD> Debug for Context<PAYLOAD> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Context")
            .field("payload", &self.payload)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" {
        safe fn ffi_get_closure_size() -> usize;
    }

    #[test]
    fn verify_ffi_closure_size() {
        assert_eq!(size_of::<ffi_closure>(), ffi_get_closure_size());
    }
}
