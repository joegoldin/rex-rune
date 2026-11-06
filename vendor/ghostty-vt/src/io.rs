//! Adapters between `std::io` and libghostty's reader and writer callbacks.

use std::ffi::c_void;
use std::io::{Read, Write};

use crate::{
    error::{Result, from_result},
    ffi,
};

unsafe extern "C" fn write_cb(userdata: *mut c_void, data: *const u8, len: usize) -> bool {
    // SAFETY: `userdata` is the `&mut dyn Write` installed by `with_writer`
    // and libghostty only calls this during that call.
    let writer = unsafe { &mut **userdata.cast::<&mut dyn Write>() };
    let data = unsafe { std::slice::from_raw_parts(data, len) };
    writer.write_all(data).is_ok()
}

/// Run `f` with a libghostty writer that forwards to `writer`.
pub(crate) fn with_writer(
    writer: &mut dyn Write,
    f: impl FnOnce(ffi::Writer) -> ffi::Result::Type,
) -> Result<()> {
    let mut slot: &mut dyn Write = writer;
    let raw = ffi::Writer {
        write: Some(write_cb),
        userdata: std::ptr::from_mut(&mut slot).cast(),
    };
    from_result(f(raw))
}

unsafe extern "C" fn read_cb(
    userdata: *mut c_void,
    buffer: *mut u8,
    capacity: usize,
    out_read: *mut usize,
) -> bool {
    // SAFETY: `userdata` is the boxed reader owned by the caller, which keeps
    // it alive for as long as libghostty may call back.
    let reader = unsafe { &mut **userdata.cast::<Box<dyn Read>>() };
    let buffer = unsafe { std::slice::from_raw_parts_mut(buffer, capacity) };
    loop {
        match reader.read(buffer) {
            Ok(n) => {
                unsafe { *out_read = n };
                return true;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
}

/// A libghostty reader over a boxed `std::io::Read`.
///
/// The box must stay alive and unmoved while libghostty holds the reader.
pub(crate) fn reader(source: &mut Box<dyn Read>) -> ffi::Reader {
    ffi::Reader {
        read: Some(read_cb),
        userdata: std::ptr::from_mut(source).cast(),
    }
}
