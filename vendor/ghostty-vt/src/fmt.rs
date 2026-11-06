//! Format terminal content as plain text, VT sequences, or HTML.
//!
//! A formatter captures a reference to a terminal and formatting options.
//! It can be used repeatedly to produce output that reflects the current
//! terminal state at the time of each format call.
//!
//! Adapted from libghostty-vt 0.2.2 (MIT OR Apache-2.0).
use std::{io::Write, marker::PhantomData, ptr::NonNull};

use crate::{
    alloc::{Bytes, Object},
    error::{Error, Result, from_result},
    ffi,
    selection::Selection,
    terminal::Terminal,
};

/// Formatter that formats terminal content.
#[derive(Debug)]
pub struct Formatter<'t> {
    inner: Object<ffi::FormatterImpl>,
    _terminal: PhantomData<&'t Terminal>,
}

/// Options for [creating a terminal formatter](Formatter::new).
#[derive(Debug)]
pub struct FormatterOptions<'t, 's> {
    inner: ffi::FormatterTerminalOptions,
    _phan: PhantomData<&'s Selection<'t>>,
}
impl<'t, 's> FormatterOptions<'t, 's> {
    /// Create a new set of options for [creating a terminal formatter](Formatter::new).
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: ffi::FormatterTerminalOptions {
                extra: ffi::FormatterTerminalExtra {
                    screen: ffi::FormatterScreenExtra {
                        ..ffi::sized!(ffi::FormatterScreenExtra)
                    },
                    ..ffi::sized!(ffi::FormatterTerminalExtra)
                },
                ..ffi::sized!(ffi::FormatterTerminalOptions)
            },
            _phan: PhantomData,
        }
    }
    /// Specify the output format to emit.
    #[must_use]
    pub fn with_format(mut self, value: Format) -> Self {
        self.inner.emit = value.into();
        self
    }
    /// Specify whether to unwrap soft-wrapped lines.
    #[must_use]
    pub fn with_unwrap(mut self, value: bool) -> Self {
        self.inner.unwrap = value;
        self
    }
    /// Specify whether to trim trailing whitespace on non-blank lines.
    #[must_use]
    pub fn with_trim(mut self, value: bool) -> Self {
        self.inner.trim = value;
        self
    }
    /// Specify the selection to restrict output to a range.
    ///
    /// If a selection is not given, the formatter defaults to formatting
    /// the entire screen.
    #[must_use]
    pub fn with_selection(mut self, value: &'s Selection<'t>) -> Self {
        self.inner.selection = &value.inner;
        self
    }

    // --- Extra settings --- //

    /// Specify whether to emit the palette using OSC 4 sequences.
    #[must_use]
    pub fn with_palette(mut self, value: bool) -> Self {
        self.inner.extra.palette = value;
        self
    }
    /// Specify terminal modes that differ from their defaults using CSI h/l.
    #[must_use]
    pub fn with_modes(mut self, value: bool) -> Self {
        self.inner.extra.modes = value;
        self
    }
    /// Specify whether to emit scrolling region state using DECSTBM and DECSLRM sequences.
    #[must_use]
    pub fn with_scrolling_region(mut self, value: bool) -> Self {
        self.inner.extra.scrolling_region = value;
        self
    }
    /// Specify tabstop positions by clearing all tabs and setting each one.
    #[must_use]
    pub fn with_tabstops(mut self, value: bool) -> Self {
        self.inner.extra.tabstops = value;
        self
    }
    /// Specify the present working directory using OSC 7.
    #[must_use]
    pub fn with_pwd(mut self, value: bool) -> Self {
        self.inner.extra.pwd = value;
        self
    }
    /// Specify keyboard modes such as ModifyOtherKeys.
    #[must_use]
    pub fn with_keyboard(mut self, value: bool) -> Self {
        self.inner.extra.keyboard = value;
        self
    }

    // --- Screen settings --- //

    /// Specify whether to emit cursor position using CUP (CSI H).
    #[must_use]
    pub fn with_cursor(mut self, value: bool) -> Self {
        self.inner.extra.screen.cursor = value;
        self
    }
    /// Emit current SGR style state based on the cursor's active style_id.
    #[must_use]
    pub fn with_style(mut self, value: bool) -> Self {
        self.inner.extra.screen.style = value;
        self
    }
    /// Emit current hyperlink state using OSC 8 sequences.
    #[must_use]
    pub fn with_hyperlink(mut self, value: bool) -> Self {
        self.inner.extra.screen.hyperlink = value;
        self
    }
    /// Emit character protection mode using DECSCA.
    #[must_use]
    pub fn with_protection(mut self, value: bool) -> Self {
        self.inner.extra.screen.protection = value;
        self
    }
    /// Emit Kitty keyboard protocol state using CSI > u and CSI = sequences.
    #[must_use]
    pub fn with_kitty_keyboard(mut self, value: bool) -> Self {
        self.inner.extra.screen.kitty_keyboard = value;
        self
    }
    /// Emit character set designations and invocations.
    #[must_use]
    pub fn with_charsets(mut self, value: bool) -> Self {
        self.inner.extra.screen.charsets = value;
        self
    }
}

impl Default for FormatterOptions<'_, '_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'t> Formatter<'t> {
    /// Create a formatter for a terminal's active screen.
    pub fn new(terminal: &'t Terminal, opts: FormatterOptions<'t, '_>) -> Result<Self> {
        let mut raw: ffi::Formatter = std::ptr::null_mut();

        let result = unsafe {
            ffi::ghostty_formatter_terminal_new(
                std::ptr::null(),
                &raw mut raw,
                terminal.as_raw(),
                opts.inner,
            )
        };
        from_result(result)?;

        Ok(Self {
            inner: Object::new(raw)?,
            _terminal: PhantomData,
        })
    }

    /// Run the formatter and return an allocated buffer with the output.
    ///
    /// Each call formats the current terminal state.
    pub fn format_alloc(&mut self) -> Result<Bytes> {
        let mut bytes = std::ptr::null_mut();
        let mut len = 0usize;
        let result = unsafe {
            ffi::ghostty_formatter_format_alloc(
                self.inner.as_raw(),
                std::ptr::null(),
                std::ptr::from_mut(&mut bytes),
                std::ptr::from_mut(&mut len),
            )
        };
        from_result(result)?;

        let ptr = NonNull::new(bytes).ok_or(Error::OutOfMemory)?;
        Ok(unsafe { Bytes::from_raw_parts(ptr, len, std::ptr::null()) })
    }

    /// Run the formatter and stream the output to a writer.
    ///
    /// Each call formats the current terminal state. A write error stops the
    /// format and returns [`Error::IoError`].
    pub fn format_to(&mut self, writer: &mut dyn Write) -> Result<()> {
        crate::io::with_writer(writer, |w| unsafe {
            ffi::ghostty_formatter_format(self.inner.as_raw(), w)
        })
    }

    /// Run the formatter and produce output into the caller-provided buffer.
    ///
    /// Each call formats the current terminal state. If the buffer is too small,
    /// returns `Err(Error::OutOfSpace { required })` where `required` is the
    /// required size. The caller can then retry with a larger buffer.
    pub fn format_buf(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut len = 0usize;
        let result = unsafe {
            ffi::ghostty_formatter_format_buf(
                self.inner.as_raw(),
                std::ptr::from_mut(buf).cast(),
                buf.len(),
                std::ptr::from_mut(&mut len),
            )
        };
        crate::error::from_result_with_len(result, len)
    }

    /// Query the required buffer size for the formatted output.
    ///
    /// The result can be used to create a sufficiently large buffer
    /// for [`Formatter::format_buf`].
    pub fn format_len(&mut self) -> Result<usize> {
        let mut len = 0usize;
        let result = unsafe {
            ffi::ghostty_formatter_format_buf(
                self.inner.as_raw(),
                std::ptr::null_mut(),
                0,
                std::ptr::from_mut(&mut len),
            )
        };
        // This should always fail with OutOfSpace.
        match from_result(result) {
            Err(Error::OutOfSpace { .. }) => Ok(len),
            Err(e) => Err(e),
            Ok(()) => Err(Error::InvalidValue),
        }
    }
}

impl Drop for Formatter<'_> {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_formatter_free(self.inner.as_raw()) }
    }
}

/// Output format.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, int_enum::IntEnum)]
pub enum Format {
    /// Plain text (no escape sequences).
    Plain = ffi::FormatterFormat::PLAIN,
    /// VT sequences preserving colors, styles, URLs, etc.
    Vt = ffi::FormatterFormat::VT,
    /// HTML with inline styles.
    Html = ffi::FormatterFormat::HTML,
}
