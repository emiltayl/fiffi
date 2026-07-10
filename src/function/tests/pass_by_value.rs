macro_rules! pass_by_value_tests_for_abi {
    (abi: $abi:path, extern_abi: $extern_abi:literal $(,)?) => {
        mod pass_by_value {
            use core::cell::UnsafeCell;
            use core::ptr;

            use crate::function::tests::helpers::call_ffi_fn;
            use crate::test_utils::structs::{U32X17_ARG, U32x17, U64X4_ARG, U64x4};
            use crate::test_utils::unions::{
                UNION_NESTED_U64X4_F64X4_ARG, UnionNestedU64x4F64x4,
            };

            const MODIFIED_STRUCT: U64x4 = U64x4 {
                a: 0,
                b: 0,
                c: 0,
                d: 0,
            };

            const MODIFIED_VERY_LARGE_STRUCT: U32x17 = U32x17 {
                a: 0,
                b: 0,
                c: 0,
                d: 0,
                e: 0,
                f: 0,
                g: 0,
                h: 0,
                i: 0,
                j: 0,
                k: 0,
                l: 0,
                m: 0,
                n: 0,
                o: 0,
                p: 0,
                q: 0,
            };

            const MODIFIED_UNION: UnionNestedU64x4F64x4 = UnionNestedU64x4F64x4 {
                i: MODIFIED_STRUCT,
            };

            #[test]
            fn large_struct_is_passed_by_value() {
                extern $extern_abi fn test_callback(mut arg: U64x4) {
                    assert_eq!(arg, U64X4_ARG);

                    // SAFETY: `arg` is a valid, aligned local value. A volatile write makes sure
                    // the callback actually modifies the storage used for its by-value argument.
                    unsafe {
                        ptr::write_volatile(&raw mut arg, MODIFIED_STRUCT);
                    }
                }

                // `UnsafeCell` has the same representation as its contents and permits this test
                // to observe an erroneous write through the argument pointer without itself
                // introducing undefined behavior.
                let original = UnsafeCell::new(U64X4_ARG);
                call_ffi_fn!(abi: $abi, test_callback(U64x4 = original));

                assert_eq!(original.into_inner(), U64X4_ARG);
            }

            #[test]
            fn very_large_struct_is_passed_by_value() {
                extern $extern_abi fn test_callback(mut arg: U32x17) {
                    assert_eq!(arg, U32X17_ARG);

                    // SAFETY: `arg` is a valid, aligned local value. A volatile write makes sure
                    // the callback actually modifies the storage used for its by-value argument.
                    unsafe {
                        ptr::write_volatile(&raw mut arg, MODIFIED_VERY_LARGE_STRUCT);
                    }
                }

                // See the large struct test above for why the original is stored in an
                // `UnsafeCell`.
                let original = UnsafeCell::new(U32X17_ARG);
                call_ffi_fn!(abi: $abi, test_callback(U32x17 = original));

                assert_eq!(original.into_inner(), U32X17_ARG);
            }

            #[test]
            fn large_union_is_passed_by_value() {
                extern $extern_abi fn test_callback(mut arg: UnionNestedU64x4F64x4) {
                    assert!(arg == UNION_NESTED_U64X4_F64X4_ARG);

                    // SAFETY: `arg` is a valid, aligned local value. A volatile write makes sure
                    // the callback actually modifies the storage used for its by-value argument.
                    unsafe {
                        ptr::write_volatile(&raw mut arg, MODIFIED_UNION);
                    }
                }

                // See the struct test above for why the original is stored in an `UnsafeCell`.
                let original = UnsafeCell::new(UNION_NESTED_U64X4_F64X4_ARG);
                call_ffi_fn!(
                    abi: $abi,
                    test_callback(UnionNestedU64x4F64x4 = original)
                );

                assert!(original.into_inner() == UNION_NESTED_U64X4_F64X4_ARG);
            }
        }
    };
}

pub(crate) use pass_by_value_tests_for_abi;
