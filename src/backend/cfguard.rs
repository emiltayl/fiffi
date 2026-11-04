//! Windows Control Flow Guard hooks supplied by the CRT and patched by the loader.
//!
//! Use these even when rustc's CFG instrumentation is disabled: the CRT supplies a no-op check
//! and a direct dispatch for images without CFG enforcement. Enforcement requires compiler
//! metadata (`-C control-flow-guard=yes`) and a CFG-enabled final link. LLVM cannot insert
//! checks into the assembly `call`s.

// These are data symbols, not functions to call directly. Only assembly uses them, with the
// platform's special CFG calling convention. `sym` operands also handle x86 symbol decoration.
unsafe extern "C" {
    pub(super) static __guard_check_icall_fptr: unsafe extern "C" fn();

    #[cfg(target_arch = "x86_64")]
    pub(super) static __guard_dispatch_icall_fptr: unsafe extern "C" fn();
}
