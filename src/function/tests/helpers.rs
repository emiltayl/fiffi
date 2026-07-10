macro_rules! call_ffi_fn {
    (abi: $abi:path, $fn:ident($($ty:ty = $val:expr),* $(,)?)) => {{
        use crate::fn_ptrize;
        #[allow(unused, reason = "Void test does not use `arg`.")]
        use crate::function::{Function, Ret, arg};
        #[allow(unused, reason = "Void test does not use `FfiType`.")]
        use crate::types::FfiType;

        let arg_types = [$(<$ty as FfiType>::ffi_type()),*];
        let function = Function::with_abi(
            fn_ptrize!($fn),
            &arg_types,
            None,
            $abi,
        );

        let args = [$(arg(&$val)),*];
        // SAFETY: For testing purposes only. It is assumed that the tests call functions with the
        // correct ABI and argument and return types.
        unsafe {
            function.call(args, Ret::void());
        }
    }};

    (abi: $abi:path, $fn:ident($($ty:ty = $val:expr),* $(,)?) -> $ret_ty:ty) => {{
        use core::cell::UnsafeCell;
        use core::mem::{offset_of, size_of, MaybeUninit};

        use crate::fn_ptrize;
        #[allow(unused, reason = "`arg` not used when there are no arguments.")]
        use crate::function::{Function, arg, ret};
        use crate::types::FfiType;

        /// Surround the return location in `0xA5` bytes to discover if a result has written out of
        /// bounds of the return buffer.
        #[repr(C)]
        struct ReturnBuffer {
            guard_1: UnsafeCell::<[u8; 16]>,
            buffer: MaybeUninit::<$ret_ty>,
            guard_2: UnsafeCell::<[u8; 16]>,
        }

        let arg_types = [$(<$ty as FfiType>::ffi_type()),*];
        let return_type = <$ret_ty as FfiType>::ffi_type();
        let function = Function::with_abi(
            fn_ptrize!($fn),
            &arg_types,
            Some(&return_type),
            $abi,
        );

        let mut return_value = ReturnBuffer {
            guard_1: UnsafeCell::new([0xA5; 16]),
            buffer: MaybeUninit::<$ret_ty>::uninit(),
            guard_2: UnsafeCell::new([0x5A; 16]),
        };

        // Ensure guard arrays are positioned right next to `buffer`.
        assert_eq!(offset_of!(ReturnBuffer, buffer), 16);
        assert_eq!(
            offset_of!(ReturnBuffer, guard_2),
            offset_of!(ReturnBuffer, buffer) + size_of::<$ret_ty>()
        );

        let args = [$(arg(&$val)),*];
        // SAFETY: For testing purposes only. It is assumed that the tests call functions with the
        // correct ABI and argument and return types. After call, `return_value` has been
        // initialized by `Function::call`.
        unsafe {
            function.call(args, ret(&mut return_value.buffer));

            assert_eq!(
                return_value.guard_1.into_inner(),
                [0xA5; 16],
                "`Function::call` wrote outside of bounds."
            );
            assert_eq!(
                return_value.guard_2.into_inner(),
                [0x5A; 16],
                "`Function::call` wrote outside of bounds."
            );

            return_value.buffer.assume_init()
        }
    }};
}

pub(crate) use call_ffi_fn;
