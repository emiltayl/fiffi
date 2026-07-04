This document shows how code using `libffi-rs` would be written using `fiffi`. This file was last
updated with code samples from `libffi-rs` v5.1.0 and `fiffi` v0.1.0.

## `Function`

`fiffi`'s `Function` is more or less equivalent to `libffi-rs`'s `middle::Cif`.

### `libffi-rs::middle::Cif`
Example code copied from
[`libffi-rs` v5.1.0's docs](https://docs.rs/libffi/5.1.0/libffi/middle/struct.Cif.html#examples)
with a single exception. References are generally not safe for FFI usage, so this example does not
use a reference for `y`.

```rust,ignore
extern "C" fn add(x: f64, y: f64) -> f64 {
    x + y
}

use libffi::middle::*;

let args = vec![Type::f64(), Type::f64()];
let cif = Cif::new(args.into_iter(), Type::f64());

let n = unsafe { cif.call(CodePtr(add as *mut _), &[arg(&5f64), arg(&6f64)]) };
assert_eq!(11f64, n);
```

### `fiffi::function::Function`

```rust
extern "C" fn add(x: f64, y: f64) -> f64 {
    x + y
}

use fiffi::function::{Function, arg, ret};
use fiffi::types::Type;

let function = Function::new(
    fiffi::fn_ptrize!(add),
    &[Type::F64, Type::F64],
    Some(&Type::F64),
);

let mut result = 0f64;

// SAFETY: `function` was built from `add` with matching argument and return
// types, and `result` is valid storage for the return value.
unsafe {
    function.call([arg(&5f64), arg(&6f64)], ret(&mut result));
}

assert_eq!(result, 11f64);
```

## `Closure`
### `libffi::high::Closure2`
Example code copied from [`libffi-rs` v5.1.0's docs](https://docs.rs/libffi/5.1.0/libffi/#examples).

```rust,ignore
use libffi::high::Closure2;

let x = 5u64;
let f = |y: u64, z: u64| x + y + z;

let closure = Closure2::new(&f);
let fun     = closure.code_ptr();

assert_eq!(18, fun.call(6, 7));
```

### `fiffi::closure::Closure`

```rust
use fiffi::closure::Closure;

let x = 5u64;
let f = |y: u64, z: u64| x + y + z;

let closure = Closure::new(f);
// SAFETY: `closure` accepts two `u64` arguments, returns `u64`, and remains alive while the
// function pointer is used.
let fun = unsafe { closure.as_fn_ptr().into_fn::<extern "C" fn(u64, u64) -> u64>() };

assert_eq!(18, fun(6, 7));
```
