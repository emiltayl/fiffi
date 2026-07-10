macro_rules! edge_value_roundtrip_test {
    (
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        fn $name:ident($ty:ty = $value:expr)
    ) => {
        #[test]
        fn $name() {
            extern $extern_abi fn test_callback(arg: $ty) -> $ty {
                assert_eq!(arg, $value);
                arg
            }

            let return_value =
                call_ffi_fn!(abi: $abi, test_callback($ty = $value) -> $ty);
            assert_eq!(return_value, $value);
        }
    };
}

macro_rules! edge_value_tests_for_abi {
    (abi: $abi:path, extern_abi: $extern_abi:literal $(,)?) => {
        mod edge_values {
            use crate::function::tests::edge_values::edge_value_roundtrip_test;
            use crate::function::tests::helpers::call_ffi_fn;

            static SUBNORMAL_F64: f64 = f64::from_bits(1);

            #[test]
            fn small_integer_boundaries() {
                extern $extern_abi fn test_callback(
                    signed_8: i8,
                    signed_16: i16,
                    unsigned_8: u8,
                    unsigned_16: u16,
                ) -> i32 {
                    let args = (signed_8, signed_16, unsigned_8, unsigned_16);
                    assert!(
                        args == (-1, -12, 0x80, 0xffff)
                            || args == (i8::MIN, i16::MIN, u8::MAX, u16::MAX)
                    );

                    i32::from(signed_8)
                        + i32::from(signed_16)
                        + i32::from(unsigned_8)
                        + i32::from(unsigned_16)
                }

                let return_value = call_ffi_fn!(
                    abi: $abi,
                    test_callback(
                        i8 = -1i8,
                        i16 = -12i16,
                        u8 = 0x80u8,
                        u16 = 0xffffu16
                    ) -> i32
                );
                assert_eq!(return_value, 65_650);

                let return_value = call_ffi_fn!(
                    abi: $abi,
                    test_callback(
                        i8 = i8::MIN,
                        i16 = i16::MIN,
                        u8 = u8::MAX,
                        u16 = u16::MAX
                    ) -> i32
                );
                assert_eq!(return_value, 32_894);
            }

            edge_value_roundtrip_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn i8_min_roundtrip(i8 = i8::MIN)
            }

            edge_value_roundtrip_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn i16_min_roundtrip(i16 = i16::MIN)
            }

            edge_value_roundtrip_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn u8_max_roundtrip(u8 = u8::MAX)
            }

            edge_value_roundtrip_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn u16_max_roundtrip(u16 = u16::MAX)
            }

            edge_value_roundtrip_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn subnormal_f64_roundtrip(f64 = SUBNORMAL_F64)
            }

            #[test]
            fn f32_argument_with_f64_return() {
                extern $extern_abi fn test_callback(arg: f32) -> f64 {
                    assert_eq!(arg, 3.5);
                    f64::from(arg) * 2.0
                }

                let return_value =
                    call_ffi_fn!(abi: $abi, test_callback(f32 = 3.5f32) -> f64);
                assert_eq!(return_value, 7.0);
            }
        }
    };
}

pub(crate) use edge_value_roundtrip_test;
pub(crate) use edge_value_tests_for_abi;
