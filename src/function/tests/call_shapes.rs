macro_rules! call_shape_tests_for_abi {
    (abi: $abi:path, extern_abi: $extern_abi:literal $(,)?) => {
        #[allow(
            clippy::too_many_arguments,
            reason = "Tests of high-arity functions."
        )]
        mod call_shapes {
            use core::ffi::c_void;

            use crate::function::tests::helpers::call_ffi_fn;
            use crate::test_utils::I128_ARG;
            use crate::test_utils::structs::{
                NESTED_F32X2X2_ARG, NestedF32x2x2, U32X2_ARG, U32x2, U64x3,
            };

            #[rustfmt::skip]
            #[test]
            fn sixteen_i32_arguments_return_normally() {
                extern $extern_abi fn test_callback(
                    arg01: i32, arg02: i32, arg03: i32, arg04: i32,
                    arg05: i32, arg06: i32, arg07: i32, arg08: i32,
                    arg09: i32, arg10: i32, arg11: i32, arg12: i32,
                    arg13: i32, arg14: i32, arg15: i32, arg16: i32,
                ) -> i32 {
                    let args = [
                        arg01, arg02, arg03, arg04, arg05, arg06, arg07, arg08,
                        arg09, arg10, arg11, arg12, arg13, arg14, arg15, arg16,
                    ];
                    assert_eq!(args, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
                    args.into_iter().sum()
                }

                let return_value = call_ffi_fn!(
                    abi: $abi,
                    test_callback(
                        i32 = 1, i32 = 2, i32 = 3, i32 = 4,
                        i32 = 5, i32 = 6, i32 = 7, i32 = 8,
                        i32 = 9, i32 = 10, i32 = 11, i32 = 12,
                        i32 = 13, i32 = 14, i32 = 15, i32 = 16,
                    ) -> i32
                );
                assert_eq!(return_value, 136);
            }

            #[rustfmt::skip]
            #[test]
            fn twenty_four_f32_arguments_return_normally() {
                extern $extern_abi fn test_callback(
                    arg01: f32, arg02: f32, arg03: f32, arg04: f32,
                    arg05: f32, arg06: f32, arg07: f32, arg08: f32,
                    arg09: f32, arg10: f32, arg11: f32, arg12: f32,
                    arg13: f32, arg14: f32, arg15: f32, arg16: f32,
                    arg17: f32, arg18: f32, arg19: f32, arg20: f32,
                    arg21: f32, arg22: f32, arg23: f32, arg24: f32,
                ) -> f32 {
                    let args = [
                        arg01, arg02, arg03, arg04, arg05, arg06, arg07, arg08,
                        arg09, arg10, arg11, arg12, arg13, arg14, arg15, arg16,
                        arg17, arg18, arg19, arg20, arg21, arg22, arg23, arg24,
                    ];
                    assert_eq!(
                        args,
                        [
                            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0,
                            9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0,
                            17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0,
                        ]
                    );
                    args.into_iter().sum()
                }

                let return_value = call_ffi_fn!(
                    abi: $abi,
                    test_callback(
                        f32 = 1.0, f32 = 2.0, f32 = 3.0, f32 = 4.0,
                        f32 = 5.0, f32 = 6.0, f32 = 7.0, f32 = 8.0,
                        f32 = 9.0, f32 = 10.0, f32 = 11.0, f32 = 12.0,
                        f32 = 13.0, f32 = 14.0, f32 = 15.0, f32 = 16.0,
                        f32 = 17.0, f32 = 18.0, f32 = 19.0, f32 = 20.0,
                        f32 = 21.0, f32 = 22.0, f32 = 23.0, f32 = 24.0,
                    ) -> f32
                );
                assert_eq!(return_value, 300.0);
            }

            #[rustfmt::skip]
            #[test]
            fn sixteen_f64_arguments_return_normally() {
                extern $extern_abi fn test_callback(
                    arg01: f64, arg02: f64, arg03: f64, arg04: f64,
                    arg05: f64, arg06: f64, arg07: f64, arg08: f64,
                    arg09: f64, arg10: f64, arg11: f64, arg12: f64,
                    arg13: f64, arg14: f64, arg15: f64, arg16: f64,
                ) -> f64 {
                    let args = [
                        arg01, arg02, arg03, arg04, arg05, arg06, arg07, arg08,
                        arg09, arg10, arg11, arg12, arg13, arg14, arg15, arg16,
                    ];
                    assert_eq!(
                        args,
                        [
                            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0,
                            9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0,
                        ]
                    );
                    args.into_iter().sum()
                }

                let return_value = call_ffi_fn!(
                    abi: $abi,
                    test_callback(
                        f64 = 1.0, f64 = 2.0, f64 = 3.0, f64 = 4.0,
                        f64 = 5.0, f64 = 6.0, f64 = 7.0, f64 = 8.0,
                        f64 = 9.0, f64 = 10.0, f64 = 11.0, f64 = 12.0,
                        f64 = 13.0, f64 = 14.0, f64 = 15.0, f64 = 16.0,
                    ) -> f64
                );
                assert_eq!(return_value, 136.0);
            }

            #[allow(
                clippy::cast_precision_loss,
                reason = "The small integer fixtures are exactly representable as f64."
            )]
            #[rustfmt::skip]
            #[test]
            fn many_mixed_arguments_return_normally() {
                extern $extern_abi fn test_callback(
                    arg01: f64, arg02: f64, arg03: isize,
                    arg04: f64, arg05: f64, arg06: isize,
                    arg07: f64, arg08: f64, arg09: isize,
                    arg10: f64, arg11: f64, arg12: isize,
                    arg13: f64, arg14: f64, arg15: isize,
                    arg16: f64, arg17: f64, arg18: isize,
                    arg19: f64,
                ) -> f64 {
                    let args = [
                        arg01, arg02, arg03 as f64, arg04, arg05, arg06 as f64,
                        arg07, arg08, arg09 as f64, arg10, arg11, arg12 as f64,
                        arg13, arg14, arg15 as f64, arg16, arg17, arg18 as f64,
                        arg19,
                    ];
                    assert_eq!(
                        args,
                        [
                            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0,
                            11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0,
                        ]
                    );
                    args.into_iter().sum()
                }

                let return_value = call_ffi_fn!(
                    abi: $abi,
                    test_callback(
                        f64 = 1.0, f64 = 2.0, isize = 3isize,
                        f64 = 4.0, f64 = 5.0, isize = 6isize,
                        f64 = 7.0, f64 = 8.0, isize = 9isize,
                        f64 = 10.0, f64 = 11.0, isize = 12isize,
                        f64 = 13.0, f64 = 14.0, isize = 15isize,
                        f64 = 16.0, f64 = 17.0, isize = 18isize,
                        f64 = 19.0,
                    ) -> f64
                );
                assert_eq!(return_value, 190.0);
            }

            #[rustfmt::skip]
            #[test]
            fn i128_roundtrip_after_six_integers() {
                extern $extern_abi fn test_callback(
                    arg1: i32, arg2: i32, arg3: i32, arg4: i32, arg5: i32, arg6: i32,
                    value: i128,
                ) -> i128 {
                    assert_eq!([arg1, arg2, arg3, arg4, arg5, arg6], [1, 2, 3, 4, 5, 6]);
                    assert_eq!(value, I128_ARG);
                    value
                }

                let return_value = call_ffi_fn!(
                    abi: $abi,
                    test_callback(
                        i32 = 1, i32 = 2, i32 = 3, i32 = 4, i32 = 5, i32 = 6,
                        i128 = I128_ARG,
                    ) -> i128
                );
                assert_eq!(return_value, I128_ARG);
            }

            #[rustfmt::skip]
            #[test]
            fn forty_small_struct_arguments() {
                extern $extern_abi fn test_callback(
                    arg00: U32x2, arg01: U32x2, arg02: U32x2, arg03: U32x2,
                    arg04: U32x2, arg05: U32x2, arg06: U32x2, arg07: U32x2,
                    arg08: U32x2, arg09: U32x2, arg10: U32x2, arg11: U32x2,
                    arg12: U32x2, arg13: U32x2, arg14: U32x2, arg15: U32x2,
                    arg16: U32x2, arg17: U32x2, arg18: U32x2, arg19: U32x2,
                    arg20: U32x2, arg21: U32x2, arg22: U32x2, arg23: U32x2,
                    arg24: U32x2, arg25: U32x2, arg26: U32x2, arg27: U32x2,
                    arg28: U32x2, arg29: U32x2, arg30: U32x2, arg31: U32x2,
                    arg32: U32x2, arg33: U32x2, arg34: U32x2, arg35: U32x2,
                    arg36: U32x2, arg37: U32x2, arg38: U32x2, arg39: U32x2,
                ) -> U32x2 {
                    let args = [
                        arg00, arg01, arg02, arg03, arg04, arg05, arg06, arg07,
                        arg08, arg09, arg10, arg11, arg12, arg13, arg14, arg15,
                        arg16, arg17, arg18, arg19, arg20, arg21, arg22, arg23,
                        arg24, arg25, arg26, arg27, arg28, arg29, arg30, arg31,
                        arg32, arg33, arg34, arg35, arg36, arg37, arg38, arg39,
                    ];
                    for (index, arg) in args.into_iter().enumerate() {
                        let index = u32::try_from(index).expect("argument index fits in u32");
                        assert_eq!(arg, U32x2 { a: index + 1, b: 40 - index });
                    }

                    U32x2 { a: 820, b: 820 }
                }

                let values: [U32x2; 40] = core::array::from_fn(|index| {
                    let index = u32::try_from(index).expect("argument index fits in u32");
                    U32x2 { a: index + 1, b: 40 - index }
                });
                let return_value = call_ffi_fn!(
                    abi: $abi,
                    test_callback(
                        U32x2 = values[0], U32x2 = values[1],
                        U32x2 = values[2], U32x2 = values[3],
                        U32x2 = values[4], U32x2 = values[5],
                        U32x2 = values[6], U32x2 = values[7],
                        U32x2 = values[8], U32x2 = values[9],
                        U32x2 = values[10], U32x2 = values[11],
                        U32x2 = values[12], U32x2 = values[13],
                        U32x2 = values[14], U32x2 = values[15],
                        U32x2 = values[16], U32x2 = values[17],
                        U32x2 = values[18], U32x2 = values[19],
                        U32x2 = values[20], U32x2 = values[21],
                        U32x2 = values[22], U32x2 = values[23],
                        U32x2 = values[24], U32x2 = values[25],
                        U32x2 = values[26], U32x2 = values[27],
                        U32x2 = values[28], U32x2 = values[29],
                        U32x2 = values[30], U32x2 = values[31],
                        U32x2 = values[32], U32x2 = values[33],
                        U32x2 = values[34], U32x2 = values[35],
                        U32x2 = values[36], U32x2 = values[37],
                        U32x2 = values[38], U32x2 = values[39],
                    ) -> U32x2
                );
                assert_eq!(return_value, U32x2 { a: 820, b: 820 });
            }

            #[test]
            fn aggregate_arguments_with_callable_pointer_and_hidden_return() {
                extern $extern_abi fn increment(value: u32) -> u32 {
                    value + 1
                }

                extern $extern_abi fn test_callback(
                    nested: NestedF32x2x2,
                    callback: *mut c_void,
                    integers: U32x2,
                ) -> U64x3 {
                    assert_eq!(nested, NESTED_F32X2X2_ARG);
                    assert_eq!(integers, U32X2_ARG);

                    let callback = crate::FnPtr::from_raw_ptr(callback)
                        .expect("test callback pointer must be non-null");
                    // SAFETY: The pointer was created from `increment`, which has this exact ABI
                    // and signature and remains alive for the duration of the call.
                    let callback = unsafe {
                        callback.into_fn::<extern $extern_abi fn(u32) -> u32>()
                    };
                    let callback_result = callback(41);
                    assert_eq!(callback_result, 42);

                    U64x3 {
                        a: u64::from(callback_result),
                        b: u64::from(integers.a),
                        c: u64::from(integers.b),
                    }
                }

                let callback = crate::fn_ptrize!(increment).as_c_void_ptr();
                let return_value = call_ffi_fn!(
                    abi: $abi,
                    test_callback(
                        NestedF32x2x2 = NESTED_F32X2X2_ARG,
                        *mut c_void = callback,
                        U32x2 = U32X2_ARG
                    ) -> U64x3
                );
                assert_eq!(
                    return_value,
                    U64x3 {
                        a: 42,
                        b: u64::from(U32X2_ARG.a),
                        c: u64::from(U32X2_ARG.b),
                    }
                );
            }
        }
    };
}

pub(crate) use call_shape_tests_for_abi;
