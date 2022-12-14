//use crate::{get_osiproc_info, get_osithread_info, CosiProc, OsiThread};
use crate::{CosiProc, CosiThread};

use panda::prelude::*;
use std::{ffi::CString, os::raw::c_char};

/// Gets a reference to the current process which can be freed with `free_process`
#[no_mangle]
pub extern "C" fn get_current_process(cpu: &mut CPUState) -> Option<Box<CosiProc>> {
    CosiProc::get_current_process(cpu).map(Box::new)
}

/// Free an allocated reference to a process
#[no_mangle]
pub extern "C" fn free_process(proc: Option<Box<CosiProc>>) {
    drop(proc);
}

/// Get the name of a process from a reference to it as a C string. Must be freed using
/// the `free_cosi_str` function.
#[no_mangle]
pub extern "C" fn cosi_proc_name(proc: &CosiProc) -> *mut c_char {
    CString::new(proc.name.clone())
        .ok()
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// Get the current thread, must be freed using `free_thread`
#[no_mangle]
pub extern "C" fn get_current_thread(cpu: &mut CPUState) -> Option<Box<CosiThread>> {
    CosiThread::get_current_thread(cpu).map(Box::new)
}

/// Free an allocated reference to a thread
#[no_mangle]
pub extern "C" fn free_thread(thread: Option<Box<CosiThread>>) {
    drop(thread);
}
