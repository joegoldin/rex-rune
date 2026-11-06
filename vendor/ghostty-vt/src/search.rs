//! Search terminal contents, including scrollback, for a string.
//!
//! A [`Search`] is bound to the terminal it was created with. Setting a
//! needle starts the search, changing it restarts from scratch, and
//! clearing it returns the search to idle. Matching is byte-exact except
//! ASCII letters, which compare case-insensitively.
//!
//! Searching a large scrollback takes time, so the work is split into
//! steps the caller drives:
//!
//! - [`Search::tick`] makes bounded progress on data the search has already
//!   copied and never touches the terminal.
//! - [`Search::feed`] reads the terminal to copy in more data and to pick
//!   up terminal changes. Feeding is the only way the search learns that the
//!   terminal changed, so keep feeding while the search is in use.
//! - [`Search::run`] feeds and ticks until the search is caught up.
//!
//! Every match is a [`Selection`] snapshot, so the selection APIs work on
//! matches: format them, hit test them, or install one as the terminal's
//! selection. Matches follow the usual snapshot lifetime rules: they are
//! only valid until the terminal changes, which the borrow on the terminal
//! enforces.
//!
//! ```
//! use ghostty_vt::{Terminal, TerminalOptions, search::Search};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut terminal = Terminal::new(TerminalOptions { cols: 20, rows: 3, ..Default::default() })?;
//! terminal.vt_write(b"one two\r\nthree two\r\n");
//!
//! let mut search = Search::new(&terminal)?;
//! search.set_needle(&terminal, Some("two"))?;
//! search.run(&terminal)?;
//! assert_eq!(search.total_matches()?, 2);
//! # Ok(())}
//! ```

use crate::{
    alloc::Object,
    error::{Error, Result, from_optional_result_uninit, from_result},
    ffi,
    selection::Selection,
    terminal::Terminal,
};
use std::mem::MaybeUninit;

/// A search over one terminal.
///
/// The search and its terminal can be dropped in either order. After the
/// terminal is dropped, calls that need it return [`Error::InvalidValue`]
/// and reads return what the search last saw.
#[derive(Debug)]
pub struct Search(Object<ffi::SearchImpl>);

impl Search {
    /// Create a search bound to a terminal.
    ///
    /// The search starts idle with no needle. Set one with
    /// [`Search::set_needle`] to start searching.
    pub fn new(terminal: &Terminal) -> Result<Self> {
        let mut raw: ffi::Search = std::ptr::null_mut();
        let result =
            unsafe { ffi::ghostty_search_new(std::ptr::null(), &raw mut raw, terminal.as_raw()) };
        from_result(result)?;
        Ok(Self(Object::new(raw)?))
    }

    fn set<T>(&mut self, option: ffi::SearchOption::Type, value: Option<&T>) -> Result<()> {
        let ptr = value.map_or(std::ptr::null(), |v| std::ptr::from_ref(v).cast());
        let result = unsafe { ffi::ghostty_search_set(self.0.as_raw(), option, ptr) };
        from_result(result)
    }

    fn get<T>(&self, data: ffi::SearchData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::zeroed();
        let result =
            unsafe { ffi::ghostty_search_get(self.0.as_raw(), data, value.as_mut_ptr().cast()) };
        from_result(result)?;
        // SAFETY: Value should be initialized after successful call.
        Ok(unsafe { value.assume_init() })
    }

    fn get_optional<T>(&self, data: ffi::SearchData::Type) -> Result<Option<T>> {
        let mut value = MaybeUninit::<T>::zeroed();
        let result =
            unsafe { ffi::ghostty_search_get(self.0.as_raw(), data, value.as_mut_ptr().cast()) };
        from_optional_result_uninit(result, value)
    }

    /// Set the needle to search for.
    ///
    /// Changing the needle restarts the search from scratch and drops all
    /// results, except that setting a needle equal to the current one keeps
    /// existing results. `None` or an empty needle clears it and returns
    /// the search to idle.
    ///
    /// This touches the terminal, hence the terminal borrow.
    pub fn set_needle(&mut self, _terminal: &Terminal, needle: Option<&str>) -> Result<()> {
        let raw = needle.map(ffi::String::from);
        self.set(ffi::SearchOption::NEEDLE, raw.as_ref())
    }

    /// Make a bounded amount of search progress without reading the terminal.
    ///
    /// Call it in a loop while the status is [`Status::Running`]. When it
    /// becomes [`Status::FeedRequired`], call [`Search::feed`].
    pub fn tick(&mut self) -> Result<Status> {
        let mut status = ffi::SearchStatus::COMPLETE;
        let result = unsafe { ffi::ghostty_search_tick(self.0.as_raw(), &raw mut status) };
        from_result(result)?;
        Status::try_from(status).map_err(|_| Error::InvalidValue)
    }

    /// Read the terminal to update the search.
    ///
    /// Each feed catches the search up with the terminal in a bounded amount
    /// of work. Feeding is the only way the search learns about terminal
    /// changes, so keep feeding periodically while the search is in use,
    /// even after it reports complete.
    pub fn feed(&mut self, _terminal: &Terminal) -> Result<()> {
        let result = unsafe { ffi::ghostty_search_feed(self.0.as_raw()) };
        from_result(result)
    }

    /// Feed and tick until the search is caught up with the terminal.
    ///
    /// This blocks for a large scrollback. Interactive embedders should
    /// drive [`Search::tick`] and [`Search::feed`] themselves.
    pub fn run(&mut self, _terminal: &Terminal) -> Result<()> {
        let result = unsafe { ffi::ghostty_search_run(self.0.as_raw()) };
        from_result(result)
    }

    /// The current search status.
    pub fn status(&self) -> Result<Status> {
        let raw = self.get::<ffi::SearchStatus::Type>(ffi::SearchData::STATUS)?;
        Status::try_from(raw).map_err(|_| Error::InvalidValue)
    }

    /// The needle this search is looking for, or `None` when unset.
    pub fn needle(&self) -> Result<Option<&str>> {
        let raw = self.get_optional::<ffi::String>(ffi::SearchData::NEEDLE)?;
        raw.map(|s| {
            // SAFETY: The bytes are borrowed from the search until the needle
            // changes, which requires `&mut self`.
            std::str::from_utf8(unsafe { s.to_bytes() }).map_err(|_| Error::InvalidValue)
        })
        .transpose()
    }

    /// Total matches found so far on the active screen. Zero until the
    /// first feed.
    pub fn total_matches(&self) -> Result<usize> {
        self.get(ffi::SearchData::TOTAL_MATCHES)
    }

    /// Index of the selected match in newest-to-oldest order, where 0 is
    /// the newest match. `None` when nothing is selected.
    pub fn selected_index(&self) -> Result<Option<usize>> {
        self.get_optional(ffi::SearchData::SELECTED_INDEX)
    }

    /// The selected match, or `None` when nothing is selected.
    pub fn selected_match<'t>(&self, _terminal: &'t Terminal) -> Result<Option<Selection<'t>>> {
        let mut value = ffi::sized!(ffi::Selection);
        let result = unsafe {
            ffi::ghostty_search_get(
                self.0.as_raw(),
                ffi::SearchData::SELECTED_MATCH,
                std::ptr::from_mut(&mut value).cast(),
            )
        };
        crate::error::from_optional_result(result, value)
            .map(|v| v.map(|raw| unsafe { Selection::from_raw(raw) }))
    }

    /// All matches on the active screen, ordered newest to oldest, from the
    /// bottom of the active area up through scrollback.
    pub fn matches<'t>(&self, terminal: &'t Terminal) -> Result<Vec<Selection<'t>>> {
        self.selections(terminal, ffi::SearchData::MATCHES)
    }

    /// Matches on the pages covering the viewport as of the last feed, for
    /// drawing highlight rectangles.
    ///
    /// The list can include matches slightly outside the visible viewport
    /// when they share a page with it. Convert each match to viewport
    /// coordinates with [`Terminal::point_from_grid_ref`] and skip the ones
    /// that fail the conversion.
    pub fn viewport_matches<'t>(&self, terminal: &'t Terminal) -> Result<Vec<Selection<'t>>> {
        self.selections(terminal, ffi::SearchData::VIEWPORT_MATCHES)
    }

    fn selections<'t>(
        &self,
        _terminal: &'t Terminal,
        data: ffi::SearchData::Type,
    ) -> Result<Vec<Selection<'t>>> {
        let mut buffer = ffi::SelectionBuffer {
            ptr: std::ptr::null_mut(),
            cap: 0,
            len: 0,
        };
        let result = unsafe {
            ffi::ghostty_search_get(
                self.0.as_raw(),
                data,
                std::ptr::from_mut(&mut buffer).cast(),
            )
        };
        match from_result(result) {
            Ok(()) => return Ok(Vec::new()),
            Err(Error::OutOfSpace { .. }) => {}
            Err(e) => return Err(e),
        }
        if buffer.len == 0 {
            return Ok(Vec::new());
        }
        let mut raw = vec![ffi::sized!(ffi::Selection); buffer.len];
        buffer = ffi::SelectionBuffer {
            ptr: raw.as_mut_ptr(),
            cap: raw.len(),
            len: 0,
        };
        let result = unsafe {
            ffi::ghostty_search_get(
                self.0.as_raw(),
                data,
                std::ptr::from_mut(&mut buffer).cast(),
            )
        };
        from_result(result)?;
        raw.truncate(buffer.len);
        Ok(raw
            .into_iter()
            .map(|sel| unsafe { Selection::from_raw(sel) })
            .collect())
    }

    /// Select the next match, moving toward older content, wrapping around
    /// past the oldest match. The viewport scrolls according to the
    /// [scroll policy](Search::set_select_scroll).
    ///
    /// Returns `false` when there are no matches.
    pub fn select_next(&mut self, _terminal: &Terminal) -> Result<bool> {
        let result = unsafe {
            ffi::ghostty_search_set(
                self.0.as_raw(),
                ffi::SearchOption::SELECT_NEXT,
                std::ptr::null(),
            )
        };
        crate::error::from_optional_result(result, ()).map(|v| v.is_some())
    }

    /// Select the previous match, moving toward newer content, wrapping
    /// around past the newest match.
    ///
    /// Returns `false` when there are no matches.
    pub fn select_prev(&mut self, _terminal: &Terminal) -> Result<bool> {
        let result = unsafe {
            ffi::ghostty_search_set(
                self.0.as_raw(),
                ffi::SearchOption::SELECT_PREV,
                std::ptr::null(),
            )
        };
        crate::error::from_optional_result(result, ()).map(|v| v.is_some())
    }

    /// Set the scroll policy applied when a match becomes selected.
    /// `None` resets it to [`Scroll::IfNeeded`].
    pub fn set_select_scroll(&mut self, scroll: Option<Scroll>) -> Result<()> {
        let raw = scroll.map(|s| s as ffi::SearchScroll::Type);
        self.set(ffi::SearchOption::SELECT_SCROLL, raw.as_ref())
    }

    /// The current scroll policy.
    pub fn select_scroll(&self) -> Result<Scroll> {
        let raw = self.get::<ffi::SearchScroll::Type>(ffi::SearchData::SELECT_SCROLL)?;
        Scroll::try_from(raw).map_err(|_| Error::InvalidValue)
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_search_free(self.0.as_raw()) }
    }
}

/// Progress state of a search.
#[derive(Clone, Copy, Debug, PartialEq, Eq, int_enum::IntEnum)]
#[repr(i32)]
pub enum Status {
    /// [`Search::tick`] can make progress without terminal access.
    Running = ffi::SearchStatus::RUNNING,
    /// Blocked until [`Search::feed`]. This is also the state right after a
    /// needle is set.
    FeedRequired = ffi::SearchStatus::FEED_REQUIRED,
    /// Caught up with the terminal as of the last feed. Later terminal
    /// writes require another feed to be seen.
    Complete = ffi::SearchStatus::COMPLETE,
}

/// Scroll policy applied when a match becomes selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, int_enum::IntEnum)]
#[repr(i32)]
pub enum Scroll {
    /// Scroll the viewport so the match is visible, only if it is not
    /// already visible.
    #[default]
    IfNeeded = ffi::SearchScroll::IF_NEEDED,
    /// Never scroll the viewport.
    None = ffi::SearchScroll::NONE,
}
