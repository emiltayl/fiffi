#![no_std]
#![no_main]

extern crate alloc;

use core::panic::PanicInfo;

#[global_allocator]
static GLOBAL: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[cfg(target_os = "windows")]
mod native {
    use core::ffi::c_char;

    unsafe extern "C" {
        unsafe fn _exit(exit_code: i32) -> !;
        unsafe fn _cputs(s: *const c_char) -> i32;
    }

    pub fn exit_process(exit_code: i32) -> ! {
        unsafe { _exit(exit_code) }
    }

    pub unsafe fn cstring_print(s: &core::ffi::CStr) -> i32 {
        unsafe { _cputs(s.as_ptr()) }
    }
}
#[cfg(not(target_os = "windows"))]
mod native {
    use core::ffi::c_char;

    unsafe extern "C" {
        unsafe fn exit(exit_code: i32) -> !;
        unsafe fn puts(s: *const c_char) -> i32;
    }

    pub fn exit_process(exit_code: i32) -> ! {
        unsafe { exit(exit_code) }
    }

    pub unsafe fn cstring_print(s: &core::ffi::CStr) -> i32 {
        unsafe { puts(s.as_ptr()) }
    }
}

use fiffi::closure::Closure;
use fiffi::function::{Function, arg, ret};
use fiffi::types::Type;
use native::*;

#[unsafe(no_mangle)]
extern "C" fn rust_eh_personality() {}

#[allow(non_snake_case)]
#[unsafe(no_mangle)]
extern "C" fn _Unwind_Resume() {}

#[panic_handler]
fn panic(panic_info: &PanicInfo) -> ! {
    let string = alloc::format!("{panic_info}\n");
    let cstring = alloc::ffi::CString::new(string).unwrap_or_else(|_| {
        let bytes = b"Unable to convert panic message to CString!\n\0";
        unsafe { alloc::ffi::CString::from_vec_with_nul_unchecked(alloc::vec::Vec::from(bytes)) }
    });

    unsafe {
        cstring_print(&cstring);
    }
    exit_process(1)
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    let a = 3i32;
    let add_a = |b: i32| a + b;
    let closure = Closure::new(add_a);

    let function = Function::new(closure.as_fn_ptr(), &[Type::I32], Some(&Type::I32));

    let mut result = 0i32;

    unsafe { function.call([arg(&4i32)], ret(&mut result)) };

    assert_eq!(result, 7);

    exit_process(0)
}
