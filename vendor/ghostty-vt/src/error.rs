//! Error handling.
//!
//! Adapted from libghostty-vt 0.2.2 (MIT OR Apache-2.0).
use std::mem::MaybeUninit;

use crate::ffi;

/// Convenient alias for fallible return values from libghostty-vt.
pub type Result<T> = std::result::Result<T, Error>;

/// Possible errors libghostty-vt may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Out of memory.
    OutOfMemory,
    /// Invalid value was specified or returned.
    InvalidValue,
    /// Ran out of space when writing to a buffer.
    OutOfSpace {
        /// Required minimum size of the buffer.
        required: usize,
    },
    /// A reader or writer callback failed.
    IoError,
    /// Encoded input exceeded a configured limit.
    LimitExceeded,
    /// A safety check rejected the operation, for example pasted text that
    /// could inject commands. Nothing was done.
    Rejected,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfMemory => write!(f, "out of memory"),
            Self::InvalidValue => write!(f, "invalid value"),
            Self::OutOfSpace { required } => {
                write!(f, "out of space, {required} bytes required")
            }
            Self::IoError => write!(f, "i/o error"),
            Self::LimitExceeded => write!(f, "limit exceeded"),
            Self::Rejected => write!(f, "rejected by a safety check"),
        }
    }
}

impl std::error::Error for Error {}

fn other(code: ffi::Result::Type, required: usize) -> Error {
    match code {
        ffi::Result::OUT_OF_MEMORY => Error::OutOfMemory,
        ffi::Result::OUT_OF_SPACE => Error::OutOfSpace { required },
        ffi::Result::IO_ERROR => Error::IoError,
        ffi::Result::LIMIT_EXCEEDED => Error::LimitExceeded,
        ffi::Result::REJECTED => Error::Rejected,
        _ => Error::InvalidValue,
    }
}

pub(crate) fn from_result(code: ffi::Result::Type) -> Result<()> {
    match code {
        ffi::Result::SUCCESS => Ok(()),
        code => Err(other(code, 0)),
    }
}

pub(crate) fn from_optional_result_uninit<T>(
    code: ffi::Result::Type,
    v: MaybeUninit<T>,
) -> Result<Option<T>> {
    match code {
        // SAFETY: Value should be initialized after successful call.
        ffi::Result::SUCCESS => Ok(Some(unsafe { v.assume_init() })),
        ffi::Result::NO_VALUE => Ok(None),
        code => Err(other(code, 0)),
    }
}

pub(crate) fn from_optional_result<T>(code: ffi::Result::Type, v: T) -> Result<Option<T>> {
    match code {
        ffi::Result::SUCCESS => Ok(Some(v)),
        ffi::Result::NO_VALUE => Ok(None),
        code => Err(other(code, 0)),
    }
}

pub(crate) fn from_result_with_len(code: ffi::Result::Type, len: usize) -> Result<usize> {
    match code {
        ffi::Result::SUCCESS => Ok(len),
        code => Err(other(code, len)),
    }
}

pub(crate) fn from_optional_result_with_len(
    code: ffi::Result::Type,
    len: usize,
) -> Result<Option<usize>> {
    match code {
        ffi::Result::SUCCESS => Ok(Some(len)),
        ffi::Result::NO_VALUE => Ok(None),
        code => Err(other(code, len)),
    }
}
