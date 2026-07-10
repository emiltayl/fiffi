macro_rules! arg_only_test {
    (abi: $abi:path, extern_abi: $extern_abi:literal, fn $name:ident($ty:ty = $val:expr)) => {
        #[test]
        fn $name() {
            extern $extern_abi fn test_callback(arg: $ty) {
                assert!(arg == $val);
            }

            call_ffi_fn!(abi: $abi, test_callback($ty = $val));
        }
    };
}

macro_rules! return_only_test {
    (abi: $abi:path, extern_abi: $extern_abi:literal, fn $name:ident() -> $ty:ty = $val:expr) => {
        #[test]
        fn $name() {
            extern $extern_abi fn test_callback() -> $ty {
                $val
            }

            let return_value = call_ffi_fn!(abi: $abi, test_callback() -> $ty);
            assert!(return_value == $val);
        }
    };
}

macro_rules! roundtrip_test {
    (abi: $abi:path, extern_abi: $extern_abi:literal, fn $name:ident($ty:ty = $val:expr)) => {
        #[test]
        fn $name() {
            extern $extern_abi fn test_callback(arg: $ty) -> $ty {
                assert!(arg == $val);
                arg
            }

            let return_value = call_ffi_fn!(abi: $abi, test_callback($ty = $val) -> $ty);
            assert!(return_value == $val);
        }
    };
}

macro_rules! single_value_test_cases {
    (
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        $(
            $module_name:ident: $ty:ty = $expected:path;
        )+
    ) => {
        $(
            mod $module_name {
                use crate::function::tests::helpers::call_ffi_fn;
                use crate::function::tests::single_value::{
                    arg_only_test, return_only_test, roundtrip_test,
                };
                use $expected as EXPECTED_VALUE;

                arg_only_test!(
                    abi: $abi,
                    extern_abi: $extern_abi,
                    fn arg_only($ty = EXPECTED_VALUE)
                );
                return_only_test!(
                    abi: $abi,
                    extern_abi: $extern_abi,
                    fn return_only() -> $ty = EXPECTED_VALUE
                );
                roundtrip_test!(
                    abi: $abi,
                    extern_abi: $extern_abi,
                    fn roundtrip($ty = EXPECTED_VALUE)
                );
            }
        )+
    };
}

macro_rules! single_value_tests_for_abi {
    (abi: $abi:path, extern_abi: $extern_abi:literal $(,)?) => {
        mod single_value {
            use crate::function::tests::single_value::single_value_test_cases;

            single_value_test_cases! {
                abi: $abi,
                extern_abi: $extern_abi,
                i8: i8 = crate::test_utils::I8_ARG;
                i16: i16 = crate::test_utils::I16_ARG;
                i32: i32 = crate::test_utils::I32_ARG;
                i64: i64 = crate::test_utils::I64_ARG;
                i128: i128 = crate::test_utils::I128_ARG;
                isize: isize = crate::test_utils::ISIZE_ARG;
                u8: u8 = crate::test_utils::U8_ARG;
                u16: u16 = crate::test_utils::U16_ARG;
                u32: u32 = crate::test_utils::U32_ARG;
                u64: u64 = crate::test_utils::U64_ARG;
                u128: u128 = crate::test_utils::U128_ARG;
                usize: usize = crate::test_utils::USIZE_ARG;
                f32: f32 = crate::test_utils::F32_ARG;
                f64: f64 = crate::test_utils::F64_ARG;
                ptr: crate::test_utils::Ptr = crate::test_utils::PTR_ARG;
                struct_u8: crate::test_utils::structs::U8 = crate::test_utils::structs::U8_ARG;
                struct_u8x2: crate::test_utils::structs::U8x2 =
                    crate::test_utils::structs::U8X2_ARG;
                struct_u8x3: crate::test_utils::structs::U8x3 =
                    crate::test_utils::structs::U8X3_ARG;
                struct_u8x7: crate::test_utils::structs::U8x7 =
                    crate::test_utils::structs::U8X7_ARG;
                struct_u8x15: crate::test_utils::structs::U8x15 =
                    crate::test_utils::structs::U8X15_ARG;
                struct_u16x3: crate::test_utils::structs::U16x3 =
                    crate::test_utils::structs::U16X3_ARG;
                struct_u32x2: crate::test_utils::structs::U32x2 =
                    crate::test_utils::structs::U32X2_ARG;
                struct_u32x3: crate::test_utils::structs::U32x3 =
                    crate::test_utils::structs::U32X3_ARG;
                struct_u32x4: crate::test_utils::structs::U32x4 =
                    crate::test_utils::structs::U32X4_ARG;
                struct_u32x17: crate::test_utils::structs::U32x17 =
                    crate::test_utils::structs::U32X17_ARG;
                struct_u64: crate::test_utils::structs::U64 = crate::test_utils::structs::U64_ARG;
                struct_u64x2: crate::test_utils::structs::U64x2 =
                    crate::test_utils::structs::U64X2_ARG;
                struct_u64x3: crate::test_utils::structs::U64x3 =
                    crate::test_utils::structs::U64X3_ARG;
                struct_u64x4: crate::test_utils::structs::U64x4 =
                    crate::test_utils::structs::U64X4_ARG;
                struct_u128: crate::test_utils::structs::U128 =
                    crate::test_utils::structs::U128_ARG;
                struct_u128x2: crate::test_utils::structs::U128x2 =
                    crate::test_utils::structs::U128X2_ARG;
                struct_f32: crate::test_utils::structs::F32 = crate::test_utils::structs::F32_ARG;
                struct_f32x2: crate::test_utils::structs::F32x2 =
                    crate::test_utils::structs::F32X2_ARG;
                struct_f32x3: crate::test_utils::structs::F32x3 =
                    crate::test_utils::structs::F32X3_ARG;
                struct_f32x4: crate::test_utils::structs::F32x4 =
                    crate::test_utils::structs::F32X4_ARG;
                struct_f64: crate::test_utils::structs::F64 = crate::test_utils::structs::F64_ARG;
                struct_f64x2: crate::test_utils::structs::F64x2 =
                    crate::test_utils::structs::F64X2_ARG;
                struct_f64x3: crate::test_utils::structs::F64x3 =
                    crate::test_utils::structs::F64X3_ARG;
                struct_f64x4: crate::test_utils::structs::F64x4 =
                    crate::test_utils::structs::F64X4_ARG;
                struct_u64_f64: crate::test_utils::structs::U64F64 =
                    crate::test_utils::structs::U64_F64_ARG;
                struct_f64_u64: crate::test_utils::structs::F64U64 =
                    crate::test_utils::structs::F64_U64_ARG;
                struct_u32_f32: crate::test_utils::structs::U32F32 =
                    crate::test_utils::structs::U32_F32_ARG;
                struct_f32x3_u32: crate::test_utils::structs::F32x3U32 =
                    crate::test_utils::structs::F32X3_U32_ARG;
                struct_u32_f32x3: crate::test_utils::structs::U32F32x3 =
                    crate::test_utils::structs::U32_F32X3_ARG;
                struct_f64_f32: crate::test_utils::structs::F64F32 =
                    crate::test_utils::structs::F64_F32_ARG;
                struct_u8_u16: crate::test_utils::structs::U8U16 =
                    crate::test_utils::structs::U8_U16_ARG;
                struct_u8_u64: crate::test_utils::structs::U8U64 =
                    crate::test_utils::structs::U8_U64_ARG;
                struct_u64_u8: crate::test_utils::structs::U64U8 =
                    crate::test_utils::structs::U64_U8_ARG;
                struct_u8_f64: crate::test_utils::structs::U8F64 =
                    crate::test_utils::structs::U8_F64_ARG;
                struct_u8_f64_u8: crate::test_utils::structs::U8F64U8 =
                    crate::test_utils::structs::U8_F64_U8_ARG;
                struct_u32_u64_u32: crate::test_utils::structs::U32U64U32 =
                    crate::test_utils::structs::U32_U64_U32_ARG;
                struct_u8_u128: crate::test_utils::structs::U8U128 =
                    crate::test_utils::structs::U8_U128_ARG;
                struct_u128_u8: crate::test_utils::structs::U128U8 =
                    crate::test_utils::structs::U128_U8_ARG;
                struct_u8_u128_u8: crate::test_utils::structs::U8U128U8 =
                    crate::test_utils::structs::U8_U128_U8_ARG;
                struct_nested_u8_u32x2: crate::test_utils::structs::NestedU8U32x2 =
                    crate::test_utils::structs::NESTED_U8_U32X2_ARG;
                struct_nested_f32x2x2: crate::test_utils::structs::NestedF32x2x2 =
                    crate::test_utils::structs::NESTED_F32X2X2_ARG;
                struct_nested_f64x2x2: crate::test_utils::structs::NestedF64x2x2 =
                    crate::test_utils::structs::NESTED_F64X2X2_ARG;
                struct_nested_u8_u64x2: crate::test_utils::structs::NestedU8U64x2 =
                    crate::test_utils::structs::NESTED_U8_U64X2_ARG;
                struct_nested_union_u32_f32: crate::test_utils::structs::NestedUnionU32F32 =
                    crate::test_utils::structs::NESTED_UNION_U32_F32_ARG;
                struct_nested_union_u32_f32x2: crate::test_utils::structs::NestedUnionU32F32x2 =
                    crate::test_utils::structs::NESTED_UNION_U32_F32X2_ARG;
                struct_nested_u8_union_u64_f64: crate::test_utils::structs::NestedU8UnionU64F64 =
                    crate::test_utils::structs::NESTED_U8_UNION_U64_F64_ARG;
                struct_nested_union_u8_u128_u8: crate::test_utils::structs::NestedUnionU8U128U8 =
                    crate::test_utils::structs::NESTED_UNION_U8_U128_U8_ARG;
                struct_nested_u8_union_u128_u8: crate::test_utils::structs::NestedU8UnionU128U8 =
                    crate::test_utils::structs::NESTED_U8_UNION_U128_U8_ARG;
                struct_usize_pointer: crate::test_utils::structs::UsizePointer =
                    crate::test_utils::structs::USIZE_POINTER_ARG;
                union_i32_u32: crate::test_utils::unions::UnionI32U32 =
                    crate::test_utils::unions::UNION_I32_U32_ARG;
                union_i64_u64: crate::test_utils::unions::UnionI64U64 =
                    crate::test_utils::unions::UNION_I64_U64_ARG;
                union_u128: crate::test_utils::unions::UnionU128 =
                    crate::test_utils::unions::UNION_U128_ARG;
                union_u8_u128: crate::test_utils::unions::UnionU8U128 =
                    crate::test_utils::unions::UNION_U8_U128_ARG;
                union_u128_u8: crate::test_utils::unions::UnionU128U8 =
                    crate::test_utils::unions::UNION_U128_U8_ARG;
                union_u32_f32: crate::test_utils::unions::UnionU32F32 =
                    crate::test_utils::unions::UNION_U32_F32_ARG;
                union_u64_f64: crate::test_utils::unions::UnionU64F64 =
                    crate::test_utils::unions::UNION_U64_F64_ARG;
                union_nested_u8x3_u64: crate::test_utils::unions::UnionNestedU8x3U64 =
                    crate::test_utils::unions::UNION_NESTED_U8X3_U64_ARG;
                union_nested_u8x3_f32x2: crate::test_utils::unions::UnionNestedU8x3F32x2 =
                    crate::test_utils::unions::UNION_NESTED_U8X3_F32X2_ARG;
                union_nested_u16x3_f64x2: crate::test_utils::unions::UnionNestedU16x3F64x2 =
                    crate::test_utils::unions::UNION_NESTED_U16X3_F64X2_ARG;
                union_nested_u64x2: crate::test_utils::unions::UnionNestedU64x2 =
                    crate::test_utils::unions::UNION_NESTED_U64X2_ARG;
                union_nested_f64x2: crate::test_utils::unions::UnionNestedF64x2 =
                    crate::test_utils::unions::UNION_NESTED_F64X2_ARG;
                union_nested_u8_u16_u64: crate::test_utils::unions::UnionNestedU8U16U64 =
                    crate::test_utils::unions::UNION_NESTED_U8_U16_U64_ARG;
                union_nested_u64_f64: crate::test_utils::unions::UnionNestedU64F64 =
                    crate::test_utils::unions::UNION_NESTED_U64_F64_ARG;
                union_nested_f32x4_u32x4: crate::test_utils::unions::UnionNestedF32x4U32x4 =
                    crate::test_utils::unions::UNION_NESTED_F32X4_U32X4_ARG;
                union_nested_f64x2_u64x2: crate::test_utils::unions::UnionNestedF64x2U64x2 =
                    crate::test_utils::unions::UNION_NESTED_F64X2_U64X2_ARG;
                union_nested_f32x2_u64: crate::test_utils::unions::UnionNestedF32x2U64 =
                    crate::test_utils::unions::UNION_NESTED_F32X2_U64_ARG;
                union_nested_f64x4_u64x4: crate::test_utils::unions::UnionNestedF64x4U64x4 =
                    crate::test_utils::unions::UNION_NESTED_F64X4_U64X4_ARG;
                union_nested_u64x4_f64x4: crate::test_utils::unions::UnionNestedU64x4F64x4 =
                    crate::test_utils::unions::UNION_NESTED_U64X4_F64X4_ARG;
            }
        }
    };
}

pub(crate) use arg_only_test;
pub(crate) use return_only_test;
pub(crate) use roundtrip_test;
pub(crate) use single_value_test_cases;
pub(crate) use single_value_tests_for_abi;
