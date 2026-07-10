macro_rules! register_passing_test {
    // Main match arm, divides supplied arguments in two groups divided by `;`. Arguments before
    // the semicolon are increased by 1 for each arguments, while the ones after are not. This is
    // done to support struct and unions that do not support a `+ i` as arguments.
    (
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        fn $name:ident(
            $($arg:ident: $arg_ty:ty = $expected:expr),* $(,)? ;
            $($extra_arg:ident: $extra_ty:ty = $extra_expected:expr),* $(,)?
        )
    ) => {
        #[test]
        fn $name() {
            extern $extern_abi fn test_callback($($arg: $arg_ty,)* $($extra_arg: $extra_ty),*) -> usize {
                let mut correct_args = 0;

                $(
                    assert_eq!($arg, $expected + (correct_args as $arg_ty));
                    correct_args += 1;
                )*
                $(
                    assert!($extra_arg == $extra_expected);
                    correct_args += 1;
                )*

                correct_args
            }

            let mut i = 0;
            $(
                let $arg = $expected + (i as $arg_ty);
                i += 1;
            )*
            $(
                let $extra_arg = $extra_expected;
                i += 1;
            )*

            let correct_args = call_ffi_fn!(abi: $abi, test_callback($($arg_ty = $arg,)* $($extra_ty = $extra_arg),*) -> usize);

            assert_eq!(correct_args, i);
        }
    };

    // If there is no semicolon in the argument list, simply add a semicolon to match this macro's
    // main rule.
    (
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        fn $name:ident($($arg:ident: $arg_ty:ty = $expected:expr),* $(,)?)
    ) => {
        register_passing_test! {
            abi: $abi,
            extern_abi: $extern_abi,
            fn $name($($arg: $arg_ty = $expected),* ;)
        }
    };

}

// Tests register passing when the return value is written through a hidden pointer. The return
// type used with this macro must be large enough for the tested ABI to return it indirectly.
macro_rules! hidden_return_pointer_test {
    (
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        fn $name:ident(
            $($arg:ident: $arg_ty:ty = $expected:expr),* $(,)?
        ) -> $return_ty:ty = $return_value:expr
    ) => {
        #[test]
        fn $name() {
            extern $extern_abi fn test_callback($($arg: $arg_ty),*) -> $return_ty {
                let mut correct_args = 0;

                $(
                    assert_eq!($arg, $expected + (correct_args as $arg_ty));
                    correct_args += 1;
                )*

                let _ = correct_args;
                $return_value
            }

            let mut i = 0;
            $(
                let $arg = $expected + (i as $arg_ty);
                i += 1;
            )*

            let return_value = call_ffi_fn!(
                abi: $abi,
                test_callback($($arg_ty = $arg),*) -> $return_ty
            );

            let _ = i;
            assert!(return_value == $return_value);
        }
    };
}

// Removes up to the requested number of arguments from the end of a token list, then invokes
// register_pressure_test! with the retained arguments.
macro_rules! trim_register_args {
    (
        count: 0,
        args: [$($arg:tt),* $(,)?],
        then: { $($state:tt)* }
    ) => {
        register_pressure_test! {
            $($state)*
            trimmed_args: [$($arg),*],
        }
    };

    (
        count: $count:tt,
        args: [$($arg:tt),* $(,)?],
        then: { $($state:tt)* }
    ) => {
        trim_register_args! {
            @validate $count,
            args: [$($arg),*],
            retained_args: [],
            then: { $($state)* }
        }
    };

    (
        @validate 1,
        $($state:tt)*
    ) => {
        trim_register_args! {
            @drop 1,
            $($state)*
        }
    };

    (
        @validate 2,
        $($state:tt)*
    ) => {
        trim_register_args! {
            @drop 2,
            $($state)*
        }
    };

    (
        @validate $unsupported:tt,
        $($state:tt)*
    ) => {
        compile_error!("Register pressure tests only support leaving 0, 1, or 2 registers free");
    };

    (
        @drop 1,
        args: [$last_arg:tt $(,)?],
        retained_args: [$($retained_arg:tt),* $(,)?],
        then: { $($state:tt)* }
    ) => {
        register_pressure_test! {
            $($state)*
            trimmed_args: [$($retained_arg),*],
        }
    };

    (
        @drop 2,
        args: [$second_last_arg:tt, $last_arg:tt $(,)?],
        retained_args: [$($retained_arg:tt),* $(,)?],
        then: { $($state:tt)* }
    ) => {
        register_pressure_test! {
            $($state)*
            trimmed_args: [$($retained_arg),*],
        }
    };

    (
        @drop $count:tt,
        args: [$first_arg:tt, $($arg:tt),+ $(,)?],
        retained_args: [$($retained_arg:tt),* $(,)?],
        then: { $($state:tt)* }
    ) => {
        trim_register_args! {
            @drop $count,
            args: [$($arg),*],
            retained_args: [$($retained_arg,)* $first_arg],
            then: { $($state)* }
        }
    };

    (
        @drop $count:tt,
        args: [$($arg:tt),* $(,)?],
        retained_args: [$($retained_arg:tt),* $(,)?],
        then: { $($state:tt)* }
    ) => {
        register_pressure_test! {
            $($state)*
            trimmed_args: [$($retained_arg),*],
        }
    };
}

macro_rules! register_pressure_test {
    (
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        gpr_args: [$($gpr_arg:tt),* $(,)?],
        float_args: [$($float_arg:tt),* $(,)?],
        free_gpr: $free_gpr:tt,
        free_float: $free_float:tt,
        fn $name:ident(
            $($extra_arg:ident: $extra_ty:ty = $extra_expected:expr),* $(,)?
        )
    ) => {
        trim_register_args! {
            count: $free_gpr,
            args: [$($gpr_arg),*],
            then: {
                @gpr_trimmed
                abi: $abi,
                extern_abi: $extern_abi,
                float_args: [$($float_arg),*],
                free_float: $free_float,
                fn $name($($extra_arg: $extra_ty = $extra_expected),*),
            }
        }
    };

    (
        @gpr_trimmed
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        float_args: [$($float_arg:tt),* $(,)?],
        free_float: $free_float:tt,
        fn $name:ident(
            $($extra_arg:ident: $extra_ty:ty = $extra_expected:expr),* $(,)?
        ),
        trimmed_args: [$($gpr_arg:tt),* $(,)?],
    ) => {
        trim_register_args! {
            count: $free_float,
            args: [$($float_arg),*],
            then: {
                @float_trimmed
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$($gpr_arg),*],
                fn $name($($extra_arg: $extra_ty = $extra_expected),*),
            }
        }
    };

    (
        @float_trimmed
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        gpr_args: [$($gpr_arg:tt),* $(,)?],
        fn $name:ident(
            $($extra_arg:ident: $extra_ty:ty = $extra_expected:expr),* $(,)?
        ),
        trimmed_args: [$($float_arg:tt),* $(,)?],
    ) => {
        register_pressure_test! {
            @interleave
            abi: $abi,
            extern_abi: $extern_abi,
            gpr_args: [$($gpr_arg),*],
            float_args: [$($float_arg),*],
            interleaved_args: [],
            fn $name($($extra_arg: $extra_ty = $extra_expected),*)
        }
    };

    (
        @interleave
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        gpr_args: [$first_gpr_arg:tt $(, $gpr_arg:tt)* $(,)?],
        float_args: [$first_float_arg:tt $(, $float_arg:tt)* $(,)?],
        interleaved_args: [$($interleaved_arg:tt),* $(,)?],
        fn $name:ident(
            $($extra_arg:ident: $extra_ty:ty = $extra_expected:expr),* $(,)?
        )
    ) => {
        register_pressure_test! {
            @interleave
            abi: $abi,
            extern_abi: $extern_abi,
            gpr_args: [$($gpr_arg),*],
            float_args: [$($float_arg),*],
            interleaved_args: [
                $($interleaved_arg,)*
                $first_gpr_arg,
                $first_float_arg
            ],
            fn $name($($extra_arg: $extra_ty = $extra_expected),*)
        }
    };

    (
        @interleave
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        gpr_args: [$($gpr_arg:tt),* $(,)?],
        float_args: [],
        interleaved_args: [$($interleaved_arg:tt),* $(,)?],
        fn $name:ident(
            $($extra_arg:ident: $extra_ty:ty = $extra_expected:expr),* $(,)?
        )
    ) => {
        register_pressure_test! {
            @emit
            abi: $abi,
            extern_abi: $extern_abi,
            args: [$($interleaved_arg,)* $($gpr_arg),*],
            fn $name($($extra_arg: $extra_ty = $extra_expected),*)
        }
    };

    (
        @interleave
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        gpr_args: [],
        float_args: [$($float_arg:tt),* $(,)?],
        interleaved_args: [$($interleaved_arg:tt),* $(,)?],
        fn $name:ident(
            $($extra_arg:ident: $extra_ty:ty = $extra_expected:expr),* $(,)?
        )
    ) => {
        register_pressure_test! {
            @emit
            abi: $abi,
            extern_abi: $extern_abi,
            args: [$($interleaved_arg,)* $($float_arg),*],
            fn $name($($extra_arg: $extra_ty = $extra_expected),*)
        }
    };

    (
        @emit
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        args: [$(($arg:ident: $arg_ty:ty = $expected:expr)),* $(,)?],
        fn $name:ident(
            $($extra_arg:ident: $extra_ty:ty = $extra_expected:expr),* $(,)?
        )
    ) => {
        register_passing_test! {
            abi: $abi,
            extern_abi: $extern_abi,
            fn $name(
                $($arg: $arg_ty = $expected,)* ;
                $($extra_arg: $extra_ty = $extra_expected),*
            )
        }
    };
}

macro_rules! register_passing_tests_for_abi {
    (
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        gpr_regs: [],
        float_regs: [] $(,)?
    ) => {
        // If no arguments are passed in registers, we skip these tests
    };

    (
        abi: $abi:path,
        extern_abi: $extern_abi:literal,
        gpr_regs: [$($gpr_reg:ident),* $(,)?],
        float_regs: [$($float_reg:ident),* $(,)?] $(,)?
    ) => {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            reason = "Allowing possibly lost casts for tests. Both compared values will use same
                      number, so they should still match"
        )]
        mod register_passing {
            use crate::function::tests::helpers::call_ffi_fn;
            use crate::function::tests::register_passing::{
                hidden_return_pointer_test, register_passing_test, register_pressure_test,
                trim_register_args,
            };
            use crate::test_utils::{F32_ARG, I16_ARG, U8_ARG, U128_ARG, USIZE_ARG};
            use crate::test_utils::structs::{
                F64x2, F64X2_ARG, U32F32, U32x2, U32X2_ARG, U32_F32_ARG, U64F64,
                U64x3, U64x4, U64X3_ARG, U64X4_ARG, U64_F64_ARG,
            };
            use crate::test_utils::unions::{
                UnionI64U64, UnionNestedF64x2, UnionNestedU8x3F32x2,
                UnionNestedU16x3F64x2, UnionU128, UNION_I64_U64_ARG,
                UNION_NESTED_F64X2_ARG, UNION_NESTED_U8X3_F32X2_ARG,
                UNION_NESTED_U16X3_F64X2_ARG, UNION_U128_ARG,
            };

            register_passing_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn all_gpr_registers($($gpr_reg: usize = USIZE_ARG),*)
            }

            register_passing_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn gpr_register_spill($($gpr_reg: usize = USIZE_ARG,)* spill: usize = USIZE_ARG)
            }

            register_passing_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn gpr_register_spill_small_ints($($gpr_reg: usize = USIZE_ARG,)* spill_1: u8 = U8_ARG, spill_2: i16 = I16_ARG)
            }

            register_passing_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn gpr_full_float_does_not_spill($($gpr_reg: usize = USIZE_ARG,)* spill: f32 = F32_ARG)
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [],
                free_gpr: 1,
                free_float: 0,
                fn large_stack_struct_preserves_remaining_gpr(
                    large: U64x4 = U64X4_ARG,
                    trailing: usize = USIZE_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [],
                free_gpr: 0,
                free_float: 0,
                fn large_stack_struct_with_full_gpr_bank(
                    large: U64x4 = U64X4_ARG,
                    trailing: usize = USIZE_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [],
                free_gpr: 1,
                free_float: 0,
                fn gpr_one_slot_free_u128(tail: u128 = U128_ARG, reg: usize = USIZE_ARG)
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [],
                free_gpr: 2,
                free_float: 0,
                fn gpr_two_slots_free_u128(tail: u128 = U128_ARG)
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [],
                free_gpr: 1,
                free_float: 0,
                fn gpr_one_slot_free_u32x2(tail: U32x2 = U32X2_ARG)
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [],
                free_gpr: 2,
                free_float: 0,
                fn gpr_two_slots_free_u32x2(tail: U32x2 = U32X2_ARG)
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [],
                free_gpr: 1,
                free_float: 0,
                fn gpr_one_slot_free_union_u128(tail: UnionU128 = UNION_U128_ARG)
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [],
                free_gpr: 2,
                free_float: 0,
                fn gpr_two_slots_free_union_u128(tail: UnionU128 = UNION_U128_ARG)
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [],
                free_gpr: 1,
                free_float: 0,
                fn gpr_one_slot_free_union_i64_u64(
                    tail: UnionI64U64 = UNION_I64_U64_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [],
                free_gpr: 2,
                free_float: 0,
                fn gpr_two_slots_free_union_i64_u64(
                    tail: UnionI64U64 = UNION_I64_U64_ARG
                )
            }

            register_passing_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn all_float_registers($($float_reg: f32 = F32_ARG),*)
            }

            register_passing_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn float_register_spill($($float_reg: f32 = F32_ARG,)* spill: f32 = F32_ARG)
            }

            register_passing_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn float_full_int_does_not_spill($($float_reg: f32 = F32_ARG,)* spill: usize = USIZE_ARG)
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 0,
                free_float: 1,
                fn float_one_slot_free_f64x2(tail: F64x2 = F64X2_ARG, reg: f32 = F32_ARG)
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 0,
                free_float: 2,
                fn float_two_slots_free_f64x2(tail: F64x2 = F64X2_ARG)
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 0,
                free_float: 1,
                fn float_one_slot_free_union_nested_f64x2(
                    tail: UnionNestedF64x2 = UNION_NESTED_F64X2_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 0,
                free_float: 2,
                fn float_two_slots_free_union_nested_f64x2(
                    tail: UnionNestedF64x2 = UNION_NESTED_F64X2_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 1,
                free_float: 1,
                fn mixed_one_slot_each_u32_f32(
                    tail: U32F32 = U32_F32_ARG,
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 1,
                free_float: 0,
                fn mixed_only_gpr_slot_u32_f32(
                    tail: U32F32 = U32_F32_ARG,
                    remaining_gpr: usize = USIZE_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 0,
                free_float: 1,
                fn mixed_only_float_slot_u32_f32(
                    tail: U32F32 = U32_F32_ARG,
                    remaining_float: f32 = F32_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 1,
                free_float: 1,
                fn mixed_one_slot_each_u64_f64(tail: U64F64 = U64_F64_ARG)
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 1,
                free_float: 0,
                fn mixed_only_gpr_slot_u64_f64(
                    tail: U64F64 = U64_F64_ARG,
                    remaining_gpr: usize = USIZE_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 0,
                free_float: 1,
                fn mixed_only_float_slot_u64_f64(
                    tail: U64F64 = U64_F64_ARG,
                    remaining_float: f32 = F32_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 1,
                free_float: 1,
                fn mixed_one_slot_each_union_nested_u8x3_f32x2(
                    tail: UnionNestedU8x3F32x2 = UNION_NESTED_U8X3_F32X2_ARG,
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 1,
                free_float: 0,
                fn mixed_only_gpr_slot_union_nested_u8x3_f32x2(
                    tail: UnionNestedU8x3F32x2 = UNION_NESTED_U8X3_F32X2_ARG,
                    remaining_gpr: usize = USIZE_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 0,
                free_float: 1,
                fn mixed_only_float_slot_union_nested_u8x3_f32x2(
                    tail: UnionNestedU8x3F32x2 = UNION_NESTED_U8X3_F32X2_ARG,
                    remaining_float: f32 = F32_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 1,
                free_float: 1,
                fn mixed_one_slot_each_union_nested_u16x3_f64x2(
                    tail: UnionNestedU16x3F64x2 = UNION_NESTED_U16X3_F64X2_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 1,
                free_float: 0,
                fn mixed_only_gpr_slot_union_nested_u16x3_f64x2(
                    tail: UnionNestedU16x3F64x2 = UNION_NESTED_U16X3_F64X2_ARG,
                    remaining_gpr: usize = USIZE_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 0,
                free_float: 1,
                fn mixed_only_float_slot_union_nested_u16x3_f64x2(
                    tail: UnionNestedU16x3F64x2 = UNION_NESTED_U16X3_F64X2_ARG,
                    remaining_float: f32 = F32_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 0,
                free_float: 1,
                fn mixed_gpr_full_u64_f64_rollback(
                    tail: U64F64 = U64_F64_ARG,
                    available_float: f32 = F32_ARG
                )
            }

            register_pressure_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                gpr_args: [$(($gpr_reg: usize = USIZE_ARG)),*],
                float_args: [$(($float_reg: f32 = F32_ARG)),*],
                free_gpr: 1,
                free_float: 0,
                fn mixed_float_full_u64_f64_rollback(
                    tail: U64F64 = U64_F64_ARG,
                    available_gpr: usize = USIZE_ARG
                )
            }

            hidden_return_pointer_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn hidden_return_pointer_shifts_gpr_arguments(
                    $($gpr_reg: usize = USIZE_ARG),*
                ) -> U64x3 = U64X3_ARG
            }

            hidden_return_pointer_test! {
                abi: $abi,
                extern_abi: $extern_abi,
                fn hidden_return_pointer_preserves_float_registers(
                    $($float_reg: f32 = F32_ARG),*
                ) -> U64x3 = U64X3_ARG
            }
        }
    };
}

pub(crate) use hidden_return_pointer_test;
pub(crate) use register_passing_test;
pub(crate) use register_passing_tests_for_abi;
pub(crate) use register_pressure_test;
pub(crate) use trim_register_args;
