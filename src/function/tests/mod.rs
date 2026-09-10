mod call_shapes;
mod edge_values;
mod helpers;
mod pass_by_value;
mod register_passing;
mod single_value;
mod unwind;

use crate::function::{Function, arg, ret};
use crate::types::Type;

#[test]
fn default_abi_matches_extern_c() {

    extern "C" fn default_abi_fn(int_1: u32, float_1: f64, int_2: u32, float_2: f64, int_3: u32,) -> f64 {
        f64::from(int_1)
            + 2.0 * float_1
            + 3.0 * f64::from(int_2)
            + 4.0 * float_2
            + 5.0 * f64::from(int_3)
    }

    let expected = default_abi_fn(1, 2.0, 3, 4.0, 5);

    // Exercise `Abi::default()` through the public default constructor.
    let function = Function::new(
        crate::fn_ptrize!(default_abi_fn),
        &[Type::U32, Type::F64, Type::U32, Type::F64, Type::U32],
        Some(&Type::F64),
    );
    let mut result = f64::NAN;

    // SAFETY: The default ABI must match `extern "C"`; the signature and live argument/return
    // storage match `probe`.
    unsafe {
        function.call(
            &[
                arg(&1u32),
                arg(&2.0f64),
                arg(&3u32),
                arg(&4.0f64),
                arg(&5u32),
            ],
            Some(ret(&mut result)),
        );
    }

    assert_eq!(result, expected);
}

macro_rules! function_tests_for_abi {
    (
        mod $module_name:ident {
            abi: Abi::$abi_variant:ident,
            extern_abi: $extern_abi:literal,
            gpr_regs: [$($gpr_reg:ident),* $(,)?],
            float_regs: [$($float_reg:ident),* $(,)?] $(,)?
        }
    ) => {
        mod $module_name {
            use crate::function::tests::call_shapes::call_shape_tests_for_abi;
            use crate::function::tests::edge_values::edge_value_tests_for_abi;
            use crate::function::tests::pass_by_value::pass_by_value_tests_for_abi;
            use crate::function::tests::single_value::single_value_tests_for_abi;
            use crate::function::tests::register_passing::register_passing_tests_for_abi;
            use crate::function::tests::unwind::unwind_tests_for_abi;

            call_shape_tests_for_abi! {
                abi: crate::Abi::$abi_variant,
                extern_abi: $extern_abi,
            }

            edge_value_tests_for_abi! {
                abi: crate::Abi::$abi_variant,
                extern_abi: $extern_abi,
            }

            single_value_tests_for_abi! {
                abi: crate::Abi::$abi_variant,
                extern_abi: $extern_abi,
            }

            pass_by_value_tests_for_abi! {
                abi: crate::Abi::$abi_variant,
                extern_abi: $extern_abi,
            }

            register_passing_tests_for_abi! {
                abi: crate::Abi::$abi_variant,
                extern_abi: $extern_abi,
                gpr_regs: [$($gpr_reg),*],
                float_regs: [$($float_reg),*]
            }

            unwind_tests_for_abi! {
                abi: crate::Abi::$abi_variant,
                extern_abi: $extern_abi,
            }

            #[test]
            fn void_callback() {
                extern $extern_abi fn test_callback() {}

                crate::function::tests::helpers::call_ffi_fn!(
                    abi: crate::Abi::$abi_variant,
                    test_callback()
                );
            }
        }
    };
}

#[cfg(target_arch = "x86_64")]
function_tests_for_abi! {
    mod x86_64_sysv {
        abi: Abi::SysV,
        extern_abi: "sysv64-unwind",
        gpr_regs: [rdi, rsi, rdx, rcx, r8, r9],
        float_regs: [xmm0, xmm1, xmm2, xmm3, xmm4, xmm5, xmm6, xmm7],
    }
}

#[cfg(target_arch = "x86_64")]
function_tests_for_abi! {
    mod x86_64_win64 {
        abi: Abi::Win64,
        extern_abi: "win64-unwind",
        gpr_regs: [rcx, rdx, r8, r9],
        float_regs: [xmm0, xmm1, xmm2, xmm3],
    }
}
