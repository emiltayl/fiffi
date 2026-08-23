mod call_shapes;
mod edge_values;
mod helpers;
mod pass_by_value;
mod register_passing;
mod single_value;
mod unwind;

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
