use core::ptr;

use crate::function::{Function, arg, ret};
use crate::types::{Type, VariadicType};

const SENTINEL: u64 = 0xd4c3_b2a1_9876_5432;
const TAIL_LEN: usize = 16;
const INTS: [i64; TAIL_LEN] = [
    0x1234_5678_90ab_cdef,
    -0x2345_6789_0abc_def1,
    0x3456_7890_abcd_ef12,
    -0x4567_890a_bcde_f123,
    0x5678_90ab_cdef_1234,
    -0x6789_0abc_def1_2345,
    0x7890_abcd_ef12_3456,
    -0x1890_abcd_ef12_3457,
    0x290a_bcde_f123_4568,
    -0x30ab_cdef_1234_5679,
    0x40bc_def1_2345_678a,
    -0x50cd_ef12_3456_789b,
    0x60de_f123_4567_89ac,
    -0x70ef_1234_5678_9abd,
    0x11f1_2345_6789_abce,
    -0x2212_3456_789a_bcdf,
];
const FLOATS: [f64; TAIL_LEN] = [
    123.25, -234.5, 345.75, -456.125, 567.375, -678.625, 789.875, -890.25, 901.5, -1012.75,
    1123.125, -1234.375, 1345.625, -1456.875, 1567.25, -1678.5,
];
const GP_PREFIX: [i64; 3] = [
    -0x1357_9bdf_2468_ace0,
    0x2468_ace0_1357_9bdf,
    -0x3579_bdf1_468a_ce02,
];
const FP_PREFIX: [f64; 3] = [-9876.25, 8765.5, -7654.75];

fn success_bit(matches: bool, index: usize) -> u64 {
    u64::from(matches) << index
}

fn expected_mask(argument_count: usize) -> u64 {
    (1u64 << argument_count) - 1
}

#[test]
fn empty() {
    unsafe extern "C-unwind" fn callback(sentinel: u64, _: ...) -> u64 {
        success_bit(sentinel == SENTINEL, 0)
    }

    let function = Function::variadic(
        crate::fn_ptrize!(callback),
        &[Type::U64],
        &[],
        Some(&Type::U64),
    );
    let mut result = 0u64;

    // SAFETY: The default ABI matches `C-unwind`. The signature describes the live u64
    // sentinel and u64 return storage, and the callback reads no variadic arguments.
    unsafe {
        function.call(&[arg(&SENTINEL)], Some(ret(&mut result)));
    }

    assert_eq!(result, expected_mask(1));
}

#[test]
fn types() {
    const SIGNED_32: i32 = -0x1234_5678;
    const FLOAT: f64 = -1234.75;
    const UNSIGNED_32: u32 = 0xcdef_9876;
    const SIGNED_64: i64 = -0x2345_6789_0abc_def0;
    const UNSIGNED_64: u64 = 0xfedc_ba98_7654_3210;
    const SIGNED_SIZE: isize = isize::MIN / 2 + 0x1234_5678;
    const UNSIGNED_SIZE: usize = usize::MAX - 0x1234_5678;
    static CONST_POINTEE: u8 = 0x36;
    static MUT_POINTEE: u8 = 0xa9;

    unsafe extern "C-unwind" fn callback(sentinel: u64, mut args: ...) -> u64 {
        let mut mask = success_bit(sentinel == SENTINEL, 0);

        // SAFETY: The caller supplies exactly these nine already-promoted arguments in
        // this order, with the same types used by each read. Pointers are only compared.
        unsafe {
            mask |= success_bit(args.next_arg::<i32>() == SIGNED_32, 1);
            mask |= success_bit(args.next_arg::<f64>() == FLOAT, 2);
            mask |= success_bit(args.next_arg::<u32>() == UNSIGNED_32, 3);
            mask |= success_bit(args.next_arg::<i64>() == SIGNED_64, 4);
            mask |= success_bit(args.next_arg::<u64>() == UNSIGNED_64, 5);
            mask |= success_bit(args.next_arg::<isize>() == SIGNED_SIZE, 6);
            mask |= success_bit(args.next_arg::<usize>() == UNSIGNED_SIZE, 7);
            mask |= success_bit(
                args.next_arg::<*const u8>() == ptr::from_ref(&CONST_POINTEE),
                8,
            );
            mask |= success_bit(
                args.next_arg::<*mut u8>() == ptr::from_ref(&MUT_POINTEE).cast_mut(),
                9,
            );
        }

        mask
    }

    let function = Function::variadic(
        crate::fn_ptrize!(callback),
        &[Type::U64],
        &[
            VariadicType::I32,
            VariadicType::F64,
            VariadicType::U32,
            VariadicType::I64,
            VariadicType::U64,
            VariadicType::Isize,
            VariadicType::Usize,
            VariadicType::Pointer,
            VariadicType::Pointer,
        ],
        Some(&Type::U64),
    );
    let const_pointer = ptr::from_ref(&CONST_POINTEE);
    let mut_pointer = ptr::from_ref(&MUT_POINTEE).cast_mut();
    let arguments = [
        arg(&SENTINEL),
        arg(&SIGNED_32),
        arg(&FLOAT),
        arg(&UNSIGNED_32),
        arg(&SIGNED_64),
        arg(&UNSIGNED_64),
        arg(&SIGNED_SIZE),
        arg(&UNSIGNED_SIZE),
        arg(&const_pointer),
        arg(&mut_pointer),
    ];
    let mut result = 0u64;

    // SAFETY: The default ABI matches `C-unwind`; the fixed sentinel, nine variadic
    // arguments in read order, and u64 return storage match the signature and remain live.
    // Both pointers refer to distinct live statics and are never dereferenced or written.
    unsafe {
        function.call(&arguments, Some(ret(&mut result)));
    }

    assert_eq!(result, expected_mask(arguments.len()));
}

#[test]
fn many_ints() {
    unsafe extern "C-unwind" fn callback(sentinel: u64, mut args: ...) -> u64 {
        let mut mask = success_bit(sentinel == SENTINEL, 0);
        for (index, expected) in INTS.into_iter().enumerate() {
            // SAFETY: The caller supplies exactly TAIL_LEN i64 variadic arguments.
            let value = unsafe { args.next_arg::<i64>() };
            mask |= success_bit(value == expected, index + 1);
        }
        mask
    }

    let function = Function::variadic(
        crate::fn_ptrize!(callback),
        &[Type::U64],
        &[const { VariadicType::I64 }; TAIL_LEN],
        Some(&Type::U64),
    );
    let mut arguments = vec![arg(&SENTINEL)];
    arguments.extend(INTS.iter().map(arg));
    let mut result = 0u64;

    // SAFETY: The default ABI matches `C-unwind`. The live u64 sentinel, TAIL_LEN i64
    // arguments in read order, and u64 return storage match the declared signature.
    unsafe {
        function.call(&arguments, Some(ret(&mut result)));
    }

    assert_eq!(result, expected_mask(arguments.len()));
}

#[test]
fn many_floats() {
    unsafe extern "C-unwind" fn callback(sentinel: u64, mut args: ...) -> u64 {
        let mut mask = success_bit(sentinel == SENTINEL, 0);
        for (index, expected) in FLOATS.into_iter().enumerate() {
            // SAFETY: The caller supplies exactly TAIL_LEN f64 variadic arguments.
            let value = unsafe { args.next_arg::<f64>() };
            mask |= success_bit(value == expected, index + 1);
        }
        mask
    }

    let function = Function::variadic(
        crate::fn_ptrize!(callback),
        &[Type::U64],
        &[const { VariadicType::F64 }; TAIL_LEN],
        Some(&Type::U64),
    );
    let mut arguments = vec![arg(&SENTINEL)];
    arguments.extend(FLOATS.iter().map(arg));
    let mut result = 0u64;

    // SAFETY: The default ABI matches `C-unwind`. The live u64 sentinel, TAIL_LEN f64
    // arguments in read order, and u64 return storage match the declared signature.
    unsafe {
        function.call(&arguments, Some(ret(&mut result)));
    }

    assert_eq!(result, expected_mask(arguments.len()));
}

#[test]
fn mixed() {
    unsafe extern "C-unwind" fn callback(sentinel: u64, mut args: ...) -> u64 {
        let mut mask = success_bit(sentinel == SENTINEL, 0);
        for (index, (integer, float)) in INTS.into_iter().zip(FLOATS).enumerate() {
            // SAFETY: The caller supplies exactly TAIL_LEN alternating i64/f64 pairs.
            let (actual_integer, actual_float) =
                unsafe { (args.next_arg::<i64>(), args.next_arg::<f64>()) };
            mask |= success_bit(actual_integer == integer, 2 * index + 1);
            mask |= success_bit(actual_float == float, 2 * index + 2);
        }
        mask
    }

    let variadic_types: Vec<_> = (0..TAIL_LEN)
        .flat_map(|_| [VariadicType::I64, VariadicType::F64])
        .collect();
    let function = Function::variadic(
        crate::fn_ptrize!(callback),
        &[Type::U64],
        &variadic_types,
        Some(&Type::U64),
    );
    let mut arguments = vec![arg(&SENTINEL)];
    for (integer, float) in INTS.iter().zip(FLOATS.iter()) {
        arguments.extend([arg(integer), arg(float)]);
    }
    let mut result = 0u64;

    // SAFETY: The default ABI matches `C-unwind`. The live u64 sentinel, TAIL_LEN
    // alternating i64/f64 pairs, and u64 return storage match the signature and read order.
    unsafe {
        function.call(&arguments, Some(ret(&mut result)));
    }

    assert_eq!(result, expected_mask(arguments.len()));
}

#[test]
fn gp_prefix() {
    unsafe extern "C-unwind" fn callback(
        first: i64,
        second: i64,
        third: i64,
        mut args: ...
    ) -> u64 {
        let mut mask = success_bit(first == GP_PREFIX[0], 0)
            | success_bit(second == GP_PREFIX[1], 1)
            | success_bit(third == GP_PREFIX[2], 2);
        for (index, expected) in INTS.into_iter().enumerate() {
            // SAFETY: The caller supplies exactly TAIL_LEN i64 variadic arguments.
            let value = unsafe { args.next_arg::<i64>() };
            mask |= success_bit(value == expected, index + 3);
        }
        mask
    }

    let function = Function::variadic(
        crate::fn_ptrize!(callback),
        &[Type::I64, Type::I64, Type::I64],
        &[const { VariadicType::I64 }; TAIL_LEN],
        Some(&Type::U64),
    );
    let arguments: Vec<_> = GP_PREFIX.iter().chain(INTS.iter()).map(arg).collect();
    let mut result = 0u64;

    // SAFETY: The default ABI matches `C-unwind`. The live three fixed i64 values,
    // TAIL_LEN variadic i64 values, and u64 return storage match the signature and read order.
    unsafe {
        function.call(&arguments, Some(ret(&mut result)));
    }

    assert_eq!(result, expected_mask(arguments.len()));
}

#[test]
fn fp_prefix() {
    unsafe extern "C-unwind" fn callback(
        first: f64,
        second: f64,
        third: f64,
        mut args: ...
    ) -> u64 {
        let mut mask = success_bit(first == FP_PREFIX[0], 0)
            | success_bit(second == FP_PREFIX[1], 1)
            | success_bit(third == FP_PREFIX[2], 2);
        for (index, expected) in FLOATS.into_iter().enumerate() {
            // SAFETY: The caller supplies exactly TAIL_LEN f64 variadic arguments.
            let value = unsafe { args.next_arg::<f64>() };
            mask |= success_bit(value == expected, index + 3);
        }
        mask
    }

    let function = Function::variadic(
        crate::fn_ptrize!(callback),
        &[Type::F64, Type::F64, Type::F64],
        &[const { VariadicType::F64 }; TAIL_LEN],
        Some(&Type::U64),
    );
    let arguments: Vec<_> = FP_PREFIX.iter().chain(FLOATS.iter()).map(arg).collect();
    let mut result = 0u64;

    // SAFETY: The default ABI matches `C-unwind`. The live three fixed f64 values,
    // TAIL_LEN variadic f64 values, and u64 return storage match the signature and read order.
    unsafe {
        function.call(&arguments, Some(ret(&mut result)));
    }

    assert_eq!(result, expected_mask(arguments.len()));
}

#[test]
fn mixed_prefix() {
    unsafe extern "C-unwind" fn callback(
        first_integer: i64,
        first_float: f64,
        second_integer: i64,
        second_float: f64,
        mut args: ...
    ) -> u64 {
        let mut mask = success_bit(first_integer == GP_PREFIX[0], 0)
            | success_bit(first_float == FP_PREFIX[0], 1)
            | success_bit(second_integer == GP_PREFIX[1], 2)
            | success_bit(second_float == FP_PREFIX[1], 3);
        for (index, (integer, float)) in INTS.into_iter().zip(FLOATS).enumerate() {
            // SAFETY: The caller supplies exactly TAIL_LEN alternating i64/f64 pairs.
            let (actual_integer, actual_float) =
                unsafe { (args.next_arg::<i64>(), args.next_arg::<f64>()) };
            mask |= success_bit(actual_integer == integer, 2 * index + 4);
            mask |= success_bit(actual_float == float, 2 * index + 5);
        }
        mask
    }

    let variadic_types: Vec<_> = (0..TAIL_LEN)
        .flat_map(|_| [VariadicType::I64, VariadicType::F64])
        .collect();
    let function = Function::variadic(
        crate::fn_ptrize!(callback),
        &[Type::I64, Type::F64, Type::I64, Type::F64],
        &variadic_types,
        Some(&Type::U64),
    );
    let mut arguments = vec![
        arg(&GP_PREFIX[0]),
        arg(&FP_PREFIX[0]),
        arg(&GP_PREFIX[1]),
        arg(&FP_PREFIX[1]),
    ];
    for (integer, float) in INTS.iter().zip(FLOATS.iter()) {
        arguments.extend([arg(integer), arg(float)]);
    }
    let mut result = 0u64;

    // SAFETY: The default ABI matches `C-unwind`. The four fixed arguments precede
    // TAIL_LEN variadic i64/f64 pairs in read order. All argument storage and the u64
    // return storage remain live and match the declared signature.
    unsafe {
        function.call(&arguments, Some(ret(&mut result)));
    }

    assert_eq!(result, expected_mask(arguments.len()));
}
