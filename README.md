# fiffi

[![Crates.io][crates-badge]][crates-url]
[![Docs][docs-image]][docs-link]
[![Apache 2.0 licensed][apache-badge]][apache-url]
[![MIT licensed][mit-badge]][mit-url]
[![Build Status][build-image]][build-link]

**This is a project branch attempting to rewrite fiffi without libffi. See [`no-libffi.md`] for more information.**

`fiffi` (*\[fi\`f:i]*) provides Rust bindings for [`libffi`]. [`libffi-rs`] already provides Rust
bindings for `libffi`, but `fiffi` aims to provide more ergonomic bindings that makes it easier to
avoid safety issues when using functionality provided by `libffi`.

The crate uses [`libffi-sys-rs`] to build and link the underlying libffi library. `fiffi` is
`no_std` but requires `alloc`.

## Use Cases

`fiffi` is built around three main use cases:

- Calling foreign functions whose signatures are not fully known at compile time with `Function`.
- Creating function pointers to Rust closures with `Closure`.
- Creating function pointers with arbitrary signatures chosen at run time backed by Rust callbacks
  with `DynamicClosure`.

## Examples

Add `fiffi` to your `Cargo.toml`:

```toml
[dependencies]
fiffi = "0.1"
```

### Calling an FFI function with `Function`

```rust
use fiffi::function::{Function, arg, ret};
use fiffi::types::Type;

// Typically when using fiffi, the function's signature is not known at compile-time. If you know
// the function signature at compile-time, it is probably preferable to call the function pointer
// directly instead of using an external library for it.
extern "C" fn add(a: i32, b: i32) -> i32 {
    a + b
}

let function = Function::new(
    fiffi::fn_ptrize!(add),
    &[Type::I32, Type::I32],
    Some(&Type::I32),
);

let a = 10i32;
let b = 32i32;
let mut result = 0i32;

// SAFETY: `function` was built from `add` with matching argument and return
// types, and `result` is valid storage for the return value.
unsafe {
    function.call(&[arg(&a), arg(&b)], ret(&mut result));
}

assert_eq!(result, 42);
```

### Converting a Rust closure to a function pointer with `Closure`

```rust
use fiffi::closure::Closure;

let offset = 5i32;
let add_offset = Closure::new(move |value: i32| value + offset);

// `add_offset.as_fn_ptr()` can now be used to retrieve an `extern "C"` function pointer to call the
// closure.

// SAFETY: `add_offset` accepts one `i32` argument, returns `i32`, and remains alive while the
// function pointer is used.
let add_offset_fn = unsafe { add_offset.as_fn_ptr().into_fn::<extern "C" fn(i32) -> i32>() };

assert_eq!(add_offset_fn(1), 6);
```

### Creating a function pointer with a function signature defined at run time with `DynamicClosure`

```rust
use core::mem::MaybeUninit;

use fiffi::closure::dynamic::DynamicClosureCall;
use fiffi::closure::DynamicClosure;
use fiffi::types::Type;

fn add_offset_callback(mut call: DynamicClosureCall<i32>) {
    let arg = call
        .args()
        .get(0)
        .expect("This callback must be used with exactly one argument.");

    if arg.ty() != &Type::I32 {
        // **NOTE** This will abort instead of unwinding regardless of the crate's setting
        // because Rust cannot unwind past libffi's `extern "C"` functions.
        panic!("This callback can only be used with an `i32` argument.");
    }

    let mut value = MaybeUninit::<i32>::uninit();

    // SAFETY: The argument's type is `i32`. The `copy_to` fully initializes `value`.
    let result = unsafe {
        arg.copy_to(&mut value);
        value.assume_init()
    } + call.context();

    let ret = call
        .ret()
        .expect("This callback expects a return value.");

    if ret.ty() != &Type::I32 {
        // **NOTE** This will abort instead of unwinding regardless of the crate's setting
        // because Rust cannot unwind past libffi's `extern "C"` functions.
        panic!("This callback can only be used with an `i32` return value.");
    }

    // SAFETY: The return value's type is `i32`.
    unsafe {
        ret.write(result);
    }
}

let add_offset = DynamicClosure::new(
    add_offset_callback,
    &[Type::I32],
    Some(&Type::I32),
    5,
);

// SAFETY: `add_offset` represents an `extern "C"` function that accepts one `i32` and returns an
// `i32`. `add_offset` remains alive while it is called.
let add_offset_fn = unsafe {
    add_offset
        .as_fn_ptr()
        .into_fn::<extern "C" fn(i32) -> i32>()
};

assert_eq!(add_offset_fn(37), 42);
```

## Features

- `closure`: includes `Closure` and `DynamicClosure` function-pointer support. This feature is
  enabled by default.
- `check_only`: forwards `libffi-sys`'s `check_only` feature to speed up `cargo check` by not
  compiling libffi.
- `system`: forwards `libffi-sys`'s `system` feature to dynamically link to a system libffi instead
  of compiling a static library. Note that some functionality may require libffi 3.5.0.

## `libffi-rs` or `fiffi`?

See [`libffi-rs-2-fiffi.md`] for a comparison of how two simple code snippets using `libffi-rs`
would be written using `fiffi`.

### Reasons to prefer `libffi-rs`

* Mature project with more than 1 maintainer
* Used by large projects such as [`miri`] and [`deno`]
* Prioritizes stability and support for older Rust versions
* Several abstraction levels (`low`, `middle`, `high`) for different needs

### Reasons to prefer `fiffi`

* Smaller API with one way to do something rather than different abstraction levels
* Designed to not allow invalid function signatures or ABIs, which may cause `libffi-rs` to return an error or panic

## Supported targets

The targets below are tested in `fiffi`'s CI tests. Additional targets may work, but are not tested
in CI; run the full test suite before using `fiffi` on another platform to ensure that everything
works as intended.

Development currently occurs mainly on x86_64 Linux and Windows.

* `aarch64-apple-darwin`
* `aarch64-pc-windows-gnullvm`
* `aarch64-pc-windows-msvc`
* `aarch64-unknown-linux-gnu`
* `armv7-unknown-linux-gnueabihf`
* `i686-pc-windows-gnu`
* `i686-pc-windows-msvc`
* `i686-unknown-linux-gnu`
* `loongarch64-unknown-linux-gnu`
* `powerpc-unknown-linux-gnu`
* `powerpc64-unknown-linux-gnu`
* `powerpc64le-unknown-linux-gnu`
* `riscv64gc-unknown-linux-gnu`
* `s390x-unknown-linux-gnu`
* `sparc64-unknown-linux-gnu`
* `x86_64-apple-darwin`
* `x86_64-pc-windows-gnu`
* `x86_64-pc-windows-gnullvm`
* `x86_64-pc-windows-msvc`
* `x86_64-unknown-linux-gnu`
* `x86_64-unknown-linux-musl`

## `no_alloc` support?

Fiffi does not currently support `no_alloc` and there is no concrete plans for implementing this. If
you need to support `no_alloc` please open an issue with your use case, and if possible suggestions
of how a `no_alloc` API could look.

[`libffi`]: https://sourceware.org/libffi/
[`libffi-rs`]: https://github.com/libffi-rs/libffi-rs/
[`libffi-sys-rs`]: https://crates.io/crates/libffi-sys/
[`miri`]: https://github.com/rust-lang/miri/
[`deno`]: https://github.com/denoland/deno/
[`no-libffi.md`]: ./no-libffi.md
[`libffi-rs-2-fiffi.md`]: ./libffi-rs-2-fiffi.md
[`Function`]: https://docs.rs/fiffi/latest/fiffi/function/struct.Function.html
[`Closure`]: https://docs.rs/fiffi/latest/fiffi/closure/struct.Closure.html
[`DynamicClosure`]: https://docs.rs/fiffi/latest/fiffi/closure/struct.DynamicClosure.html
[crates-badge]: https://img.shields.io/crates/v/fiffi.svg
[crates-url]: https://crates.io/crates/fiffi
[docs-image]: https://img.shields.io/docsrs/fiffi.svg
[docs-link]: https://docs.rs/fiffi/
[apache-badge]: https://img.shields.io/badge/license-Apache%20License%202.0-blue
[apache-url]: ./LICENSE-APACHE
[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: ./LICENSE-MIT
[build-image]: https://github.com/emiltayl/fiffi/actions/workflows/tests.yml/badge.svg
[build-link]: https://github.com/emiltayl/fiffi/actions/workflows/tests.yml
