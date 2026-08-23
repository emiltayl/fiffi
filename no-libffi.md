# Writing a libffi alternative in plain Rust (and assembly)

fiffi started as an alternative libffi wrapper to libffi-rs. However, after having worked with this
for a while I wanted to have a go at creating an alternative to libffi written in Rust (and
assembly). I hope to create a ffi library that can be built without requiring any other external
tools than the Rust compiler.

Fiffi will not support as many architectures and ABIs as libffi. The main goal is to support common
ABIs on common platforms, and then branch out from there as needed.

# Plan

* [x] `Type` size and layout functionality
* [x] Initial test suite for function calls
* [x] 64-bit x86 SysV ABI for function calls
  * [x] Argument, return value classification
  * [x] Plan for marshalling arguments
  * [x] Assembly: argument marshalling, the call and return value handling
* [ ] Support discarding return value, add tests for this
* [ ] Take args by reference in `Function::call`
* [ ] Review test suite for missing test cases
* [ ] Plan how to handle overflow in size calculations, document it explicitly
  * [ ] Types
  * [ ] Marshalling plan (stack buffer)
  * [ ] Anything else?
* [ ] 64-bit x86 Win64 ABI for function calls
* [ ] 32-bit x86 ABIs for function calls
* [ ] Aarch64 ABIs for function calls
* [ ] Review status
* [ ] Variadic test suite (in C?) and support for variadics
* [ ] Closures?

# Work log

* 2026-08-23 x86_64 SysV function call support.
* 2026-07-12 Got started with argument marshalling plan generation.
* 2026-07-10 Added function call test suite added with 308 failing tests. Got started preparing for
  code for x86_64 SysV calls.
* 2026-07-06 Added union support to `Type` and implemented layout and offset calculation for `Type`.
* 2026-07-04 Started this document