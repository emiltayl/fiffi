use core::cell::UnsafeCell;

use crate::function::{Arg, arg};

/// Converts test-owned storage into an argument while constraining ordinary values to `T`.
pub(super) trait CallArgument<T: ?Sized> {
    fn as_argument(&self) -> Arg<'_>;
}

impl<T: ?Sized> CallArgument<T> for T {
    fn as_argument(&self) -> Arg<'_> {
        arg(self)
    }
}

impl<T: ?Sized> CallArgument<T> for UnsafeCell<T> {
    fn as_argument(&self) -> Arg<'_> {
        // `UnsafeCell<T>` has the same representation as `T`. Keeping the argument typed as the
        // cell avoids creating an `&T`, which would defeat tests that intentionally detect writes
        // to the source storage.
        arg(self)
    }
}

pub(super) fn call_argument<'arg, T: ?Sized, Storage: ?Sized>(storage: &'arg Storage) -> Arg<'arg>
where
    Storage: CallArgument<T>,
{
    storage.as_argument()
}

macro_rules! call_ffi_fn {
    (abi: $abi:path, $fn:ident($($ty:ty = $val:expr),* $(,)?)) => {{
        let arg_types = [$(<$ty as $crate::types::FfiType>::ffi_type()),*];
        let function = $crate::function::Function::with_abi(
            $crate::fn_ptrize!($fn),
            &arg_types,
            None,
            $abi,
        );

        let args = [$($crate::function::tests::helpers::call_argument::<$ty, _>(&$val)),*];
        // SAFETY: For testing purposes only. It is assumed that the tests call functions with the
        // correct ABI and argument and return types.
        unsafe {
            function.call(&args, None);
        }
    }};

    (abi: $abi:path, $fn:ident($($ty:ty = $val:expr),* $(,)?) -> $ret_ty:ty) => {{
        let arg_types = [$(<$ty as $crate::types::FfiType>::ffi_type()),*];
        let return_type = <$ret_ty as $crate::types::FfiType>::ffi_type();
        let function = $crate::function::Function::with_abi(
            $crate::fn_ptrize!($fn),
            &arg_types,
            Some(&return_type),
            $abi,
        );

        let mut return_value = $crate::test_utils::GuardedReturn::<$ret_ty>::new();

        let args = [$($crate::function::tests::helpers::call_argument::<$ty, _>(&$val)),*];
        // SAFETY: For testing purposes only. It is assumed that the tests call functions with the
        // correct ABI and argument and return types. After call, `return_value` has been
        // initialized by `Function::call`.
        unsafe {
            function.call(&args, Some(return_value.ret()));
            return_value.get()
        }
    }};
}

pub(crate) use call_ffi_fn;
