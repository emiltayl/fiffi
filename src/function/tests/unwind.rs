macro_rules! unwind_tests_for_abi {
    (abi: $abi:path, extern_abi: $extern_abi:literal $(,)?) => {
        #[cfg(panic = "unwind")]
        mod unwind {
            use core::sync::atomic::{AtomicBool, Ordering};
            use std::panic::{catch_unwind, panic_any};

            use crate::function::tests::helpers::call_ffi_fn;

            #[derive(Debug)]
            struct UnwindMarker;

            struct DropGuard<'flag>(&'flag AtomicBool);

            impl Drop for DropGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::Relaxed);
                }
            }

            fn assert_unwind_marker(result: std::thread::Result<()>) {
                let payload = result.expect_err("The callback returned instead of unwinding");
                assert!(
                    payload.is::<UnwindMarker>(),
                    "The call propagated an unexpected panic payload"
                );
            }

            #[test]
            fn panic_unwinds_through_call() {
                extern $extern_abi fn test_callback() {
                    panic_any(UnwindMarker);
                }

                let result = catch_unwind(|| {
                    call_ffi_fn!(abi: $abi, test_callback());
                });

                assert_unwind_marker(result);
            }

            #[rustfmt::skip]
            #[test]
            fn panic_unwinds_with_stack_arguments() {
                extern $extern_abi fn test_callback(
                    arg0: usize, arg1: usize, arg2: usize, arg3: usize, arg4: usize, arg5: usize,
                    arg6: usize, arg7: usize, arg8: usize, arg9: usize, arg10: usize, arg11: usize,
                    arg12: usize, arg13: usize, arg14: usize, arg15: usize,
                ) {
                    assert_eq!(
                        [
                            arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10,
                            arg11, arg12, arg13, arg14, arg15,
                        ],
                        [
                            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
                        ]
                    );

                    panic_any(UnwindMarker);
                }

                let result = catch_unwind(|| {
                    call_ffi_fn!(
                        abi: $abi,
                        test_callback(
                            usize = 0, usize = 1, usize = 2, usize = 3, usize = 4, usize = 5,
                            usize = 6, usize = 7, usize = 8, usize = 9, usize = 10, usize = 11,
                            usize = 12, usize = 13, usize = 14, usize = 15,
                        )
                    );
                });

                assert_unwind_marker(result);
            }

            #[test]
            fn caller_cleanup_runs_after_unwind() {
                extern $extern_abi fn unwind_callback() {
                    panic_any(UnwindMarker);
                }

                extern $extern_abi fn identity_callback(value: usize) -> usize {
                    value
                }

                let guard_dropped = AtomicBool::new(false);
                let result = catch_unwind(|| {
                    let _guard = DropGuard(&guard_dropped);
                    call_ffi_fn!(abi: $abi, unwind_callback());
                });

                assert_unwind_marker(result);
                assert!(guard_dropped.load(Ordering::Relaxed));

                let result = call_ffi_fn!(
                    abi: $abi,
                    identity_callback(usize = 42) -> usize
                );
                assert_eq!(result, 42);
            }

            #[test]
            fn panic_unwinds_through_nested_calls() {
                extern $extern_abi fn inner_callback() {
                    panic_any(UnwindMarker);
                }

                extern $extern_abi fn outer_callback() {
                    call_ffi_fn!(abi: $abi, inner_callback());
                }

                let result = catch_unwind(|| {
                    call_ffi_fn!(abi: $abi, outer_callback());
                });

                assert_unwind_marker(result);
            }
        }
    };
}

pub(crate) use unwind_tests_for_abi;
