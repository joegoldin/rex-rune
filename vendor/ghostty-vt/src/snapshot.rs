//! Encode and restore complete terminal snapshots.
//!
//! A snapshot is an ordered, CRC-protected binary record stream that holds
//! everything needed to render and resume a terminal, including any
//! unfinished VT parser input, followed by older scrollback pages.
//!
//! Encoding a terminal whose parser is mid-sequence needs
//! [continuation tracking](crate::Terminal::set_continuation_max_bytes)
//! enabled before the input that produced that state was written.
//!
//! ```
//! use ghostty_vt::{Terminal, TerminalOptions, snapshot};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut terminal = Terminal::new(TerminalOptions { cols: 20, rows: 3, ..Default::default() })?;
//! terminal.vt_write(b"hello");
//!
//! let bytes = snapshot::encode(&terminal)?;
//! let restored = snapshot::Decoder::from_slice(&bytes).decode()?;
//! assert_eq!(restored.cursor_x()?, 5);
//! # Ok(())}
//! ```

use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::ptr::NonNull;

use crate::{
    alloc::{Bytes, Object},
    error::{Error, Result, from_optional_result_uninit, from_result},
    ffi,
    screen::Screen,
    terminal::Terminal,
};

/// Encode a complete terminal snapshot into libghostty-allocated bytes.
pub fn encode(terminal: &Terminal) -> Result<Bytes> {
    let mut ptr = std::ptr::null_mut();
    let mut len = 0usize;
    let result = unsafe {
        ffi::ghostty_snapshot_encode_alloc(
            terminal.as_raw(),
            std::ptr::null(),
            &raw mut ptr,
            &raw mut len,
        )
    };
    from_result(result)?;
    let ptr = NonNull::new(ptr).ok_or(Error::OutOfMemory)?;
    Ok(unsafe { Bytes::from_raw_parts(ptr, len, std::ptr::null()) })
}

/// Encode a complete terminal snapshot to a writer.
///
/// A write error stops the encode and returns [`Error::IoError`]; the
/// writer may then hold a partial snapshot.
pub fn encode_to(terminal: &Terminal, writer: &mut dyn Write) -> Result<()> {
    crate::io::with_writer(writer, |w| unsafe {
        ffi::ghostty_snapshot_encode(terminal.as_raw(), w)
    })
}

/// A snapshot decoder.
///
/// Use [`Decoder::decode`] to restore a whole snapshot in one call, or
/// [`Decoder::ready`] to get a renderable terminal first and then restore
/// history one page at a time with [`Decoder::next_page`], which suits a large
/// scrollback restored in the background.
pub struct Decoder<'a> {
    inner: Object<ffi::SnapshotDecoderImpl>,
    terminal: Option<Terminal>,
    // Keeps the reader alive at a stable address while libghostty reads.
    _source: Option<Box<Box<dyn Read + 'a>>>,
}

impl std::fmt::Debug for Decoder<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder")
            .field("inner", &self.inner)
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
    }
}

/// Progress of one [`Decoder::next_page`] step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// The screen the decoded history page belongs to.
    pub screen: Screen,
    /// Rows prepended by the page. Zero means the page was validated but
    /// could not be applied to the live terminal.
    pub rows: usize,
    /// Page records remaining in the same screen's history.
    pub remaining: u32,
}

impl<'a> Decoder<'a> {
    /// Create a decoder over a borrowed byte buffer.
    ///
    /// Bytes after the snapshot are not consumed; [`Decoder::source_offset`]
    /// locates them.
    ///
    /// # Panics
    ///
    /// Panics if libghostty cannot allocate the decoder.
    #[must_use]
    pub fn from_slice(bytes: &'a [u8]) -> Self {
        let mut raw: ffi::SnapshotDecoder = std::ptr::null_mut();
        let result = unsafe {
            ffi::ghostty_snapshot_decoder_new_buf(
                std::ptr::null(),
                &raw mut raw,
                bytes.as_ptr(),
                bytes.len(),
            )
        };
        from_result(result).expect("snapshot decoder allocation");
        Self {
            inner: Object::new(raw).expect("snapshot decoder allocation"),
            terminal: None,
            _source: None,
        }
    }

    /// Create a decoder that reads from a reader.
    ///
    /// A zero-byte read is end of file. A read error reports
    /// [`Error::IoError`], and end of file before a required marker reports
    /// truncated data as [`Error::InvalidValue`].
    pub fn new(reader: impl Read + 'a) -> Result<Self> {
        let mut source: Box<Box<dyn Read + 'a>> = Box::new(Box::new(reader));
        // SAFETY: The inner box is kept alive by `_source` and is only read
        // for the lifetime 'a that the decoder carries.
        let erased: &mut Box<dyn Read> = unsafe {
            &mut *std::ptr::from_mut::<Box<dyn Read + 'a>>(&mut source).cast::<Box<dyn Read>>()
        };
        let raw_reader = crate::io::reader(erased);
        let mut raw: ffi::SnapshotDecoder = std::ptr::null_mut();
        let result = unsafe {
            ffi::ghostty_snapshot_decoder_new(std::ptr::null(), &raw mut raw, raw_reader)
        };
        from_result(result)?;
        Ok(Self {
            inner: Object::new(raw)?,
            terminal: None,
            _source: Some(source),
        })
    }

    fn set<T>(&mut self, option: ffi::SnapshotDecoderOption::Type, value: &T) -> Result<()> {
        let result = unsafe {
            ffi::ghostty_snapshot_decoder_set(
                self.inner.as_raw(),
                option,
                std::ptr::from_ref(value).cast(),
            )
        };
        from_result(result)
    }

    fn get<T>(&self, data: ffi::SnapshotDecoderData::Type) -> Result<Option<T>> {
        let mut value = MaybeUninit::<T>::zeroed();
        let result = unsafe {
            ffi::ghostty_snapshot_decoder_get(self.inner.as_raw(), data, value.as_mut_ptr().cast())
        };
        from_optional_result_uninit(result, value)
    }

    /// Set the largest non-ground continuation the decoder accepts.
    ///
    /// Zero accepts only snapshots whose parser is at ground. The default is
    /// the largest built-in APC buffer limit. Only allowed before decoding
    /// starts.
    pub fn set_max_continuation_bytes(&mut self, max: usize) -> Result<&mut Self> {
        self.set(ffi::SnapshotDecoderOption::MAX_CONTINUATION_BYTES, &max)?;
        Ok(self)
    }

    /// Keep continuation tracking enabled on the decoded terminal, with the
    /// decoder's maximum as the limit, so [`Terminal::continuation`] can
    /// export the restored unfinished input. Off by default. Only allowed
    /// before decoding starts.
    pub fn set_retain_continuation(&mut self, retain: bool) -> Result<&mut Self> {
        self.set(ffi::SnapshotDecoderOption::RETAIN_CONTINUATION, &retain)?;
        Ok(self)
    }

    /// Decode and validate one complete snapshot.
    ///
    /// Bytes following the snapshot are left unread.
    pub fn decode(mut self) -> Result<Terminal> {
        let mut raw: ffi::Terminal = std::ptr::null_mut();
        let result =
            unsafe { ffi::ghostty_snapshot_decoder_decode(self.inner.as_raw(), &raw mut raw) };
        from_result(result)?;
        // SAFETY: A successful decode hands ownership of the terminal to us.
        let terminal = unsafe { Terminal::from_raw(raw) }?;
        self.terminal = None;
        Ok(terminal)
    }

    /// Decode the renderable prefix of the snapshot.
    ///
    /// On success the decoder holds a terminal that is immediately usable
    /// for rendering and live input. Restore older scrollback with
    /// [`Decoder::next_page`], then take the terminal with
    /// [`Decoder::into_terminal`]. This may only be called once.
    pub fn ready(&mut self) -> Result<&mut Terminal> {
        if self.terminal.is_some() {
            return Err(Error::InvalidValue);
        }
        let mut raw: ffi::Terminal = std::ptr::null_mut();
        let result =
            unsafe { ffi::ghostty_snapshot_decoder_ready(self.inner.as_raw(), &raw mut raw) };
        from_result(result)?;
        // SAFETY: A successful ready hands ownership of the terminal to us.
        let terminal = unsafe { Terminal::from_raw(raw) }?;
        Ok(self.terminal.insert(terminal))
    }

    /// The terminal produced by [`Decoder::ready`], if any.
    ///
    /// It may be rendered, resized and fed live input between
    /// [`Decoder::next_page`] calls.
    pub fn terminal_mut(&mut self) -> Option<&mut Terminal> {
        self.terminal.as_mut()
    }

    /// Apply one snapshot record (a history page or image) after READY.
    /// Returns false at validated FINISH without assuming page-only progress fields.
    pub fn next_record(&mut self) -> Result<bool> {
        if self.terminal.is_none() {
            return Err(Error::InvalidValue);
        }
        let result = unsafe { ffi::ghostty_snapshot_decoder_next(self.inner.as_raw()) };
        Ok(from_optional_result_uninit(result, MaybeUninit::new(()))?.is_some())
    }

    /// Decode one history page into the terminal from [`Decoder::ready`].
    ///
    /// Returns `Ok(None)` once the snapshot's end marker was validated, and
    /// keeps returning it afterwards.
    pub fn next_page(&mut self) -> Result<Option<Progress>> {
        if self.terminal.is_none() {
            return Err(Error::InvalidValue);
        }
        let result = unsafe { ffi::ghostty_snapshot_decoder_next(self.inner.as_raw()) };
        match from_optional_result_uninit(result, MaybeUninit::new(()))? {
            None => Ok(None),
            Some(()) => {
                let screen = self
                    .get::<ffi::TerminalScreen::Type>(ffi::SnapshotDecoderData::PROGRESS_SCREEN)?
                    .ok_or(Error::InvalidValue)?;
                let rows = self
                    .get::<usize>(ffi::SnapshotDecoderData::PROGRESS_ROWS)?
                    .ok_or(Error::InvalidValue)?;
                let remaining = self
                    .get::<u32>(ffi::SnapshotDecoderData::PROGRESS_REMAINING)?
                    .ok_or(Error::InvalidValue)?;
                Ok(Some(Progress {
                    screen: Screen::try_from(screen).map_err(|_| Error::InvalidValue)?,
                    rows,
                    remaining,
                }))
            }
        }
    }

    /// Take the terminal produced by [`Decoder::ready`].
    ///
    /// Abandoning an incremental decode leaves the terminal usable with
    /// whatever history had already been restored.
    #[must_use]
    pub fn into_terminal(mut self) -> Option<Terminal> {
        self.terminal.take()
    }

    /// Number of source bytes consumed so far. Unavailable after a decoding
    /// error.
    pub fn source_offset(&self) -> Result<Option<usize>> {
        self.get(ffi::SnapshotDecoderData::SOURCE_OFFSET)
    }

    /// Advisory complete history extent for the primary screen, available
    /// after [`Decoder::ready`].
    pub fn history_rows_primary(&self) -> Result<Option<u64>> {
        self.get(ffi::SnapshotDecoderData::HISTORY_ROWS_PRIMARY)
    }

    /// Advisory complete history extent for the alternate screen, available
    /// after [`Decoder::ready`] when the snapshot declares one.
    pub fn history_rows_alternate(&self) -> Result<Option<u64>> {
        self.get(ffi::SnapshotDecoderData::HISTORY_ROWS_ALTERNATE)
    }

    /// The current maximum accepted continuation size.
    pub fn max_continuation_bytes(&self) -> Result<usize> {
        self.get(ffi::SnapshotDecoderData::MAX_CONTINUATION_BYTES)?
            .ok_or(Error::InvalidValue)
    }

    /// Whether continuation tracking is retained on decoded terminals.
    pub fn retain_continuation(&self) -> Result<bool> {
        self.get(ffi::SnapshotDecoderData::RETAIN_CONTINUATION)?
            .ok_or(Error::InvalidValue)
    }
}

impl Drop for Decoder<'_> {
    fn drop(&mut self) {
        // The decoder never owns the terminal, so freeing it first is safe.
        unsafe { ffi::ghostty_snapshot_decoder_free(self.inner.as_raw()) }
    }
}
