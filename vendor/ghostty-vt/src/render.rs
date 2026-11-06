//! Managing [render states](RenderState) of the terminal.
//!
//! Adapted from libghostty-vt 0.2.2 (MIT OR Apache-2.0).

use std::{convert::Into, marker::PhantomData, mem::MaybeUninit};

use crate::{
    alloc::Object,
    error::{Error, Result, from_optional_result, from_result},
    ffi,
    screen::{Cell, Row},
    style::{RgbColor, Style},
    terminal::Terminal,
};

pub use ffi::RenderStateRowSelection as RowSelection;
mod images;
pub use images::{ImageIterator, ImagePlacement};

/// Represents the state required to render a visible screen (a viewport) of
/// a terminal instance.
///
/// This is stateful and optimized for repeated updates from a single terminal
/// instance and only updating dirty regions of the screen.
///
/// The key design principle of this API is that it only needs read/write
/// access to the terminal instance during the update call. This allows the
/// render state to minimally impact terminal IO performance.
///
/// The basic usage of this API is:
///
///  1. Create an empty render state
///  2. Update it from a terminal instance whenever you need.
///  3. Read from the render state to get the data needed to draw your frame.
///
/// # Dirty Tracking
///
/// Dirty tracking is a key feature of the render state that allows renderers
/// to efficiently determine what parts of the screen have changed and only
/// redraw changed regions.
///
/// The render state API keeps track of dirty state at two independent layers:
/// a global dirty state that indicates whether the entire frame is clean,
/// partially dirty, or fully dirty, and a per-row dirty state that allows
/// tracking which rows in a partially dirty frame have changed.
///
/// The user of the render state API is expected to unset both of these,
/// either with [`Snapshot::clean`] after a full frame or with
/// [`Snapshot::set_dirty`] and [`RowIteration::set_dirty`] individually.
/// The update call does not unset dirty state, it only updates it.
///
/// # Overscan
///
/// A renderer that draws the grid shifted by a fraction of a row can ask for
/// extra rows above and below the viewport with [`RenderState::set_overscan`].
/// The row iterator then visits those rows too, and
/// [`RowIteration::viewport_y`] says where each row belongs.
///
/// # Row identity
///
/// Every row has a [`RowId`] that stays with the row as it moves. A renderer
/// can cache per-row work keyed by the id and reuse it when the same id
/// shows up again and the row is not dirty.
///
/// # Examples
///
/// ```rust
/// use ghostty_vt::{Terminal, TerminalOptions, RenderState};
/// use ghostty_vt::render::{RowIterator, CellIterator};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut terminal = Terminal::new(TerminalOptions { cols: 40, rows: 5, ..Default::default() })?;
/// let mut render_state = RenderState::new()?;
/// let mut rows = RowIterator::new()?;
/// let mut cells = CellIterator::new()?;
///
/// terminal.vt_write(b"Hello, \x1b[1;32mworld\x1b[0m!\r\n");
///
/// let snapshot = render_state.update(&terminal)?;
/// let mut row_iter = rows.update(&snapshot)?;
/// while let Some(row) = row_iter.next() {
///     let mut cell_iter = cells.update(row)?;
///     while let Some(cell) = cell_iter.next() {
///         let _graphemes = cell.graphemes()?;
///     }
/// }
/// snapshot.clean()?;
/// # Ok(())}
/// ```
#[derive(Debug)]
pub struct RenderState(Object<ffi::RenderStateImpl>);

/// A snapshot of the render state after an update.
///
/// This struct exists to guard data accessed from the render state from
/// being accidentally modified after an update. If you find yourself unable
/// to update the render state due to borrow checker errors, make sure to
/// drop the active snapshot (and data that depends on it) before updating.
#[derive(Debug)]
pub struct Snapshot<'s>(&'s mut RenderState);

/// An in-progress render state update.
///
/// This token is returned by [`RenderState::begin_update`] and keeps the render
/// state borrowed until [`Self::end`] completes the deferred update work. This
/// makes it impossible to read from the render state while it is incomplete.
#[derive(Debug)]
pub struct Update<'s> {
    state: Option<&'s mut RenderState>,
}

/// Opaque handle to a render-state row iterator.
///
/// The row iterator must be [updated](RowIterator::update) from a snapshot of
/// the render state in order to function, as most data is only accessible
/// per [iteration](RowIteration).
#[derive(Debug)]
pub struct RowIterator(Object<ffi::RenderStateRowIteratorImpl>);

/// An active iteration over the rows in the render state.
///
/// Row iterations are created by [updating](RowIterator::update) row iterators
/// with a snapshot of the render state. The borrow checker statically
/// guarantees that all accesses of the data do not outlive the given snapshot,
/// at the cost of added lifetime annotations.
#[derive(Debug)]
pub struct RowIteration<'s> {
    iter: &'s mut RowIterator,
    _phan: PhantomData<&'s Snapshot<'s>>,
}

/// Opaque handle to a render state cell iterator.
///
/// The cell iterator must be [updated](CellIterator::update) from a
/// [row](RowIteration) in order to function, as most data is only
/// accessible per [iteration](CellIteration).
#[derive(Debug)]
pub struct CellIterator(Object<ffi::RenderStateRowCellsImpl>);

/// An active iteration over the cells on a given row
/// within the render state.
#[derive(Debug)]
pub struct CellIteration<'s> {
    iter: &'s mut CellIterator,
    _phan: PhantomData<&'s RowIteration<'s>>,
}

//--------------------------
// Impl blocks
//--------------------------

impl RenderState {
    /// Create a new render state instance.
    pub fn new() -> Result<Self> {
        let mut raw: ffi::RenderState = std::ptr::null_mut();
        let result = unsafe { ffi::ghostty_render_state_new(std::ptr::null(), &raw mut raw) };
        from_result(result)?;
        Ok(Self(Object::new(raw)?))
    }

    /// Request extra rows above and below the viewport on every later update.
    ///
    /// Extra rows are only captured when they exist. After an update,
    /// [`Snapshot::overscan`] reports how many rows were captured on each
    /// side.
    pub fn set_overscan(&mut self, overscan: Overscan) -> Result<()> {
        let raw = ffi::RenderStateOverscan {
            above: overscan.above,
            below: overscan.below,
        };
        let result = unsafe {
            ffi::ghostty_render_state_set(
                self.0.as_raw(),
                ffi::RenderStateOption::OVERSCAN,
                std::ptr::from_ref(&raw).cast(),
            )
        };
        from_result(result)
    }

    /// Update a render state instance from a terminal,
    /// returning a new [snapshot](Snapshot).
    ///
    /// This consumes terminal/screen dirty state in the same way as the
    /// internal render state update path.
    ///
    /// # Errors
    ///
    /// Returns `Err(Error::OutOfMemory)` if updating the state requires
    /// allocation and that allocation fails.
    pub fn update(&mut self, terminal: &Terminal) -> Result<Snapshot<'_>> {
        let result =
            unsafe { ffi::ghostty_render_state_update(self.0.as_raw(), terminal.as_raw()) };
        from_result(result)?;
        Ok(Snapshot(self))
    }

    /// Begin an update of a render state instance from a terminal.
    ///
    /// Every begin must be completed with [`Update::end`] before the render
    /// state is read.
    ///
    /// Only this function requires terminal access. The end phase exclusively
    /// reads and writes memory owned by the render state.
    ///
    /// This consumes terminal and screen dirty state in the same way as the
    /// internal render state update path.
    pub fn begin_update(&mut self, terminal: &Terminal) -> Result<Update<'_>> {
        let result =
            unsafe { ffi::ghostty_render_state_begin_update(self.0.as_raw(), terminal.as_raw()) };
        from_result(result)?;
        Ok(Update { state: Some(self) })
    }
}

impl Drop for RenderState {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_render_state_free(self.0.as_raw()) }
    }
}

impl<'s> Update<'s> {
    /// Complete a prior [`RenderState::begin_update`] call by performing any deferred work.
    ///
    /// This only reads and writes memory owned by the render state. Consumes
    /// the update token and returns a snapshot that can be read to draw the
    /// frame.
    pub fn end(mut self) -> Result<Snapshot<'s>> {
        let Some(state) = self.state.take() else {
            return Err(Error::InvalidValue);
        };
        let result = unsafe { ffi::ghostty_render_state_end_update(state.0.as_raw()) };
        from_result(result)?;
        Ok(Snapshot(state))
    }
}

impl Drop for Update<'_> {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            let _ = unsafe { ffi::ghostty_render_state_end_update(state.0.as_raw()) };
        }
    }
}

impl Snapshot<'_> {
    fn get<T>(&self, tag: ffi::RenderStateData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::zeroed();
        let result = unsafe {
            ffi::ghostty_render_state_get(self.0.0.as_raw(), tag, value.as_mut_ptr().cast())
        };
        // Since we manually model every possible query, this should never fail.
        from_result(result)?;
        // SAFETY: Value should be initialized after successful call.
        Ok(unsafe { value.assume_init() })
    }

    fn set<T>(&self, tag: ffi::RenderStateOption::Type, value: &T) -> Result<()> {
        let result = unsafe {
            ffi::ghostty_render_state_set(self.0.0.as_raw(), tag, std::ptr::from_ref(value).cast())
        };
        from_result(result)
    }

    /// Get the current dirty state.
    pub fn dirty(&self) -> Result<Dirty> {
        self.get::<ffi::RenderStateDirty::Type>(ffi::RenderStateData::DIRTY)
            .and_then(|v| v.try_into().map_err(|_| Error::InvalidValue))
    }

    /// Get the viewport width.
    pub fn cols(&self) -> Result<u16> {
        self.get(ffi::RenderStateData::COLS)
    }

    /// Get the viewport height.
    pub fn rows(&self) -> Result<u16> {
        self.get(ffi::RenderStateData::ROWS)
    }

    /// Get the number of overscan rows captured above and below the viewport
    /// by the last update.
    pub fn overscan(&self) -> Result<Overscan> {
        let raw = self.get::<ffi::RenderStateOverscan>(ffi::RenderStateData::OVERSCAN)?;
        Ok(Overscan {
            above: raw.above,
            below: raw.below,
        })
    }

    /// Get the overscan rows requested with [`RenderState::set_overscan`].
    pub fn overscan_request(&self) -> Result<Overscan> {
        let raw = self.get::<ffi::RenderStateOverscan>(ffi::RenderStateData::OVERSCAN_REQUEST)?;
        Ok(Overscan {
            above: raw.above,
            below: raw.below,
        })
    }

    /// Get the cursor color that may have been explicitly set by the terminal state.
    pub fn cursor_color(&self) -> Result<Option<RgbColor>> {
        let has_value = self.get(ffi::RenderStateData::COLOR_CURSOR_HAS_VALUE)?;
        if has_value {
            let color = self.get(ffi::RenderStateData::COLOR_CURSOR)?;
            Ok(Some(color))
        } else {
            Ok(None)
        }
    }

    /// Whether the cursor is currently visible based on terminal modes.
    pub fn cursor_visible(&self) -> Result<bool> {
        self.get(ffi::RenderStateData::CURSOR_VISIBLE)
    }

    /// Whether the cursor is currently blinking based on terminal modes.
    pub fn cursor_blinking(&self) -> Result<bool> {
        self.get(ffi::RenderStateData::CURSOR_BLINKING)
    }

    /// Whether the cursor is at a password input field.
    pub fn cursor_password_input(&self) -> Result<bool> {
        self.get(ffi::RenderStateData::CURSOR_PASSWORD_INPUT)
    }

    /// Get the visual style of the cursor.
    pub fn cursor_visual_style(&self) -> Result<CursorVisualStyle> {
        self.get::<ffi::RenderStateCursorVisualStyle::Type>(
            ffi::RenderStateData::CURSOR_VISUAL_STYLE,
        )
        .and_then(|v| v.try_into().map_err(|_| Error::InvalidValue))
    }

    /// Get the relative position of the cursor and other information
    /// if it is currently visible within the viewport.
    pub fn cursor_viewport(&self) -> Result<Option<CursorViewport>> {
        let has_value = self.get(ffi::RenderStateData::CURSOR_VIEWPORT_HAS_VALUE)?;
        if has_value {
            let x = self.get(ffi::RenderStateData::CURSOR_VIEWPORT_X)?;
            let y = self.get(ffi::RenderStateData::CURSOR_VIEWPORT_Y)?;
            let at_wide_tail = self.get(ffi::RenderStateData::CURSOR_VIEWPORT_WIDE_TAIL)?;
            Ok(Some(CursorViewport { x, y, at_wide_tail }))
        } else {
            Ok(None)
        }
    }

    /// Get all cursor information in one call.
    pub fn cursor(&self) -> Result<Cursor> {
        let mut raw = ffi::sized!(ffi::RenderStateCursor);
        let result = unsafe {
            ffi::ghostty_render_state_get(
                self.0.0.as_raw(),
                ffi::RenderStateData::CURSOR,
                std::ptr::from_mut(&mut raw).cast(),
            )
        };
        from_result(result)?;
        Ok(Cursor {
            viewport: raw.viewport_has_value.then_some(CursorViewport {
                x: raw.viewport_x,
                y: raw.viewport_y,
                at_wide_tail: raw.wide_tail,
            }),
            visible: raw.visible,
            blinking: raw.blinking,
            password_input: raw.password_input,
            visual_style: raw
                .visual_style
                .try_into()
                .map_err(|_| Error::InvalidValue)?,
        })
    }

    /// Get the current color information from a render state.
    pub fn colors(&self) -> Result<Colors> {
        let mut colors = ffi::sized!(ffi::RenderStateColors);
        let result = unsafe {
            ffi::ghostty_render_state_get(
                self.0.0.as_raw(),
                ffi::RenderStateData::COLORS,
                std::ptr::from_mut(&mut colors).cast(),
            )
        };
        from_result(result)?;

        Ok(Colors {
            background: colors.background.into(),
            foreground: colors.foreground.into(),
            cursor: if colors.cursor_has_value {
                Some(colors.cursor.into())
            } else {
                None
            },
            palette: colors.palette.map(Into::into),
        })
    }

    /// Set dirty state.
    pub fn set_dirty(&self, dirty: Dirty) -> Result<()> {
        self.set(
            ffi::RenderStateOption::DIRTY,
            &(dirty as ffi::RenderStateDirty::Type),
        )
    }

    /// Mark the whole render state clean: the global dirty state and every
    /// row's dirty flag.
    ///
    /// Call this after drawing a complete frame.
    pub fn clean(&self) -> Result<()> {
        let result = unsafe { ffi::ghostty_render_state_clean(self.0.0.as_raw()) };
        from_result(result)
    }
}

impl RowIterator {
    /// Create a new row iterator instance.
    pub fn new() -> Result<Self> {
        let mut raw: ffi::RenderStateRowIterator = std::ptr::null_mut();
        let result =
            unsafe { ffi::ghostty_render_state_row_iterator_new(std::ptr::null(), &raw mut raw) };
        from_result(result)?;
        Ok(Self(Object::new(raw)?))
    }

    /// Update the row iterator for a snapshot of the render state,
    /// returning a new row iteration.
    pub fn update<'s>(&'s mut self, snapshot: &'s Snapshot<'s>) -> Result<RowIteration<'s>> {
        let result = unsafe {
            ffi::ghostty_render_state_get(
                snapshot.0.0.as_raw(),
                ffi::RenderStateData::ROW_ITERATOR,
                std::ptr::from_mut(&mut self.0.ptr).cast(),
            )
        };
        from_result(result)?;

        Ok(RowIteration {
            iter: self,
            _phan: PhantomData,
        })
    }
}

impl Drop for RowIterator {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_render_state_row_iterator_free(self.0.as_raw()) }
    }
}

impl RowIteration<'_> {
    /// Move a row iteration to the next row.
    ///
    /// Returns `Some(row)` if the iteration moved successfully and row
    /// data is available to read at the new position using `row`.
    #[allow(
        clippy::should_implement_trait,
        reason = "lending `next` cannot implement trait"
    )]
    pub fn next(&mut self) -> Option<&Self> {
        if unsafe { ffi::ghostty_render_state_row_iterator_next(self.iter.0.as_raw()) } {
            Some(self)
        } else {
            None
        }
    }

    /// Move a row iteration to the next dirty row.
    ///
    /// If the global dirty state is [`Dirty::Clean`], this returns `None`.
    /// If it is [`Dirty::Partial`], clean rows are skipped. If it is
    /// [`Dirty::Full`], every remaining row is returned. The returned value
    /// is the row's viewport y position.
    pub fn next_dirty(&mut self) -> Option<(u16, &Self)> {
        let mut y = 0u16;
        if unsafe {
            ffi::ghostty_render_state_row_iterator_next_dirty(self.iter.0.as_raw(), &raw mut y)
        } {
            Some((y, self))
        } else {
            None
        }
    }

    fn get<T>(&self, tag: ffi::RenderStateRowData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::zeroed();
        let result = unsafe {
            ffi::ghostty_render_state_row_get(self.iter.0.as_raw(), tag, value.as_mut_ptr().cast())
        };
        // Since we manually model every possible query, this should never fail.
        from_result(result)?;
        // SAFETY: Value should be initialized after successful call.
        Ok(unsafe { value.assume_init() })
    }

    fn set<T>(&self, tag: ffi::RenderStateRowOption::Type, value: &T) -> Result<()> {
        let result = unsafe {
            ffi::ghostty_render_state_row_set(
                self.iter.0.as_raw(),
                tag,
                std::ptr::from_ref(value).cast(),
            )
        };
        from_result(result)
    }

    /// Whether the current row is dirty.
    pub fn dirty(&self) -> Result<bool> {
        self.get(ffi::RenderStateRowData::DIRTY)
    }

    /// The raw row value.
    pub fn raw_row(&self) -> Result<Row> {
        self.get(ffi::RenderStateRowData::RAW).map(Row)
    }

    /// The row's position relative to the top of the viewport.
    ///
    /// Viewport rows are 0 through rows - 1. Overscan rows above the
    /// viewport are negative, and overscan rows below it start at rows.
    pub fn viewport_y(&self) -> Result<i32> {
        self.get(ffi::RenderStateRowData::VIEWPORT_Y)
    }

    /// The row's identity across updates.
    pub fn id(&self) -> Result<RowId> {
        self.get::<ffi::RenderStateRowId>(ffi::RenderStateRowData::ID)
            .map(|raw| RowId(raw.bits))
    }

    /// Set dirty state for the current row.
    pub fn set_dirty(&self, dirty: bool) -> Result<()> {
        self.set(ffi::RenderStateRowOption::DIRTY, &dirty)
    }

    /// Row-local selected cell range.
    pub fn selection(&self) -> Result<Option<RowSelection>> {
        let mut value = ffi::sized!(RowSelection);
        let result = unsafe {
            ffi::ghostty_render_state_row_get(
                self.iter.0.as_raw(),
                ffi::RenderStateRowData::SELECTION,
                std::ptr::from_mut(&mut value).cast(),
            )
        };
        from_optional_result(result, value)
    }
}

impl CellIterator {
    /// Create a new cell iterator instance.
    pub fn new() -> Result<Self> {
        let mut raw: ffi::RenderStateRowCells = std::ptr::null_mut();
        let result =
            unsafe { ffi::ghostty_render_state_row_cells_new(std::ptr::null(), &raw mut raw) };
        from_result(result)?;
        Ok(Self(Object::new(raw)?))
    }

    /// Update the cell iterator for a new row iteration,
    /// returning a new cell iteration.
    pub fn update<'s>(&'s mut self, row: &'s RowIteration<'s>) -> Result<CellIteration<'s>> {
        let result = unsafe {
            ffi::ghostty_render_state_row_get(
                row.iter.0.as_raw(),
                ffi::RenderStateRowData::CELLS,
                std::ptr::from_mut(&mut self.0.ptr).cast(),
            )
        };
        from_result(result)?;

        Ok(CellIteration {
            iter: self,
            _phan: PhantomData,
        })
    }
}

impl Drop for CellIterator {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_render_state_row_cells_free(self.0.as_raw()) }
    }
}

impl CellIteration<'_> {
    /// Move a cell iteration to the next cell.
    ///
    /// Returns `Some(cell)` if the iteration moved successfully and cell
    /// data is available to read at the new position using `cell`.
    #[allow(
        clippy::should_implement_trait,
        reason = "lending `next` cannot implement trait"
    )]
    pub fn next(&mut self) -> Option<&Self> {
        if unsafe { ffi::ghostty_render_state_row_cells_next(self.iter.0.as_raw()) } {
            Some(self)
        } else {
            None
        }
    }

    /// Move a cell iteration to a specific column.
    ///
    /// Positions the iteration at the given x (column) index so that
    /// subsequent reads return data for that cell.
    pub fn select(&mut self, x: u16) -> Result<()> {
        let result = unsafe { ffi::ghostty_render_state_row_cells_select(self.iter.0.as_raw(), x) };
        from_result(result)
    }

    fn get<T>(&self, tag: ffi::RenderStateRowCellsData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::zeroed();
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.iter.0.as_raw(),
                tag,
                value.as_mut_ptr().cast(),
            )
        };
        from_result(result)?;
        // SAFETY: Value should be initialized after successful call.
        Ok(unsafe { value.assume_init() })
    }

    /// The raw cell value.
    pub fn raw_cell(&self) -> Result<Cell> {
        self.get(ffi::RenderStateRowCellsData::RAW).map(Cell)
    }

    /// The style for the current cell.
    pub fn style(&self) -> Result<Style> {
        let mut value = ffi::sized!(ffi::Style);
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.iter.0.as_raw(),
                ffi::RenderStateRowCellsData::STYLE,
                std::ptr::from_mut(&mut value).cast(),
            )
        };
        from_result(result)?;
        Style::try_from(value)
    }

    /// The resolved foreground color of the cell.
    ///
    /// Resolves palette indices through the palette. Bold color handling
    /// is not applied; the caller should handle bold styling separately.
    ///
    /// Returns `None` if the cell has no explicit foreground color, in which
    /// case the caller should use whatever default foreground color it want
    /// (e.g. the terminal foreground).
    pub fn fg_color(&self) -> Result<Option<RgbColor>> {
        let res = self.get::<ffi::ColorRgb>(ffi::RenderStateRowCellsData::FG_COLOR);
        match res {
            Ok(o) => Ok(Some(o.into())),
            Err(Error::InvalidValue) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The resolved background color of the cell.
    ///
    /// Flattens the three possible sources: [`Cell::bg_color_rgb`],
    /// [`Cell::bg_color_palette`] (looked up in the palette), or the
    /// style's [`bg_color`][Style::bg_color].
    ///
    /// Returns `None` if the cell has no background color, in which case the
    /// caller should use whatever default background color it wants
    /// (e.g. the terminal background).
    pub fn bg_color(&self) -> Result<Option<RgbColor>> {
        let res = self.get::<ffi::ColorRgb>(ffi::RenderStateRowCellsData::BG_COLOR);
        match res {
            Ok(o) => Ok(Some(o.into())),
            Err(Error::InvalidValue) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Get the grapheme codepoints.
    ///
    /// The base codepoint is placed first, followed by any extra codepoints.
    pub fn graphemes(&self) -> Result<Vec<char>> {
        let len = self.graphemes_len()?;
        let mut graphemes = vec!['\0'; len];
        self.graphemes_buf(&mut graphemes)?;
        Ok(graphemes)
    }

    /// The total number of grapheme codepoints including the base codepoint.
    ///
    /// Returns 0 if the cell has no text.
    pub fn graphemes_len(&self) -> Result<usize> {
        self.get::<u32>(ffi::RenderStateRowCellsData::GRAPHEMES_LEN)
            .map(|len| len as usize)
    }

    /// Write grapheme codepoints into a caller-provided buffer.
    ///
    /// The buffer must be at least [`CellIteration::graphemes_len`] elements.
    /// The base codepoint is written first, followed by any extra codepoints.
    pub fn graphemes_buf(&self, buf: &mut [char]) -> Result<()> {
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.iter.0.as_raw(),
                ffi::RenderStateRowCellsData::GRAPHEMES_BUF,
                buf.as_mut_ptr().cast(),
            )
        };
        from_result(result)
    }

    /// Append the current cell's full grapheme cluster as UTF-8 to a string.
    ///
    /// The base codepoint is encoded first, followed by any extra grapheme
    /// codepoints. Nothing is appended for a cell without text.
    pub fn graphemes_utf8(&self, buf: &mut String) -> Result<()> {
        let mut scratch = [0u8; 64];
        let mut cbuf = ffi::Buffer {
            ptr: scratch.as_mut_ptr(),
            cap: scratch.len(),
            len: 0,
        };
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.iter.0.as_raw(),
                ffi::RenderStateRowCellsData::GRAPHEMES_UTF8,
                std::ptr::from_mut(&mut cbuf).cast(),
            )
        };
        let bytes: Vec<u8>;
        let written: &[u8] = match result {
            ffi::Result::SUCCESS => &scratch[..cbuf.len],
            ffi::Result::OUT_OF_SPACE => {
                let mut grown = vec![0u8; cbuf.len];
                cbuf = ffi::Buffer {
                    ptr: grown.as_mut_ptr(),
                    cap: grown.len(),
                    len: 0,
                };
                let result = unsafe {
                    ffi::ghostty_render_state_row_cells_get(
                        self.iter.0.as_raw(),
                        ffi::RenderStateRowCellsData::GRAPHEMES_UTF8,
                        std::ptr::from_mut(&mut cbuf).cast(),
                    )
                };
                from_result(result)?;
                grown.truncate(cbuf.len);
                bytes = grown;
                &bytes
            }
            other => return from_result(other),
        };
        buf.push_str(std::str::from_utf8(written).map_err(|_| Error::InvalidValue)?);
        Ok(())
    }

    /// Whether the cell is contained within the current selection.
    ///
    /// This returns true when the cell's column is within the current row's
    /// row-local selection range, and false otherwise. Rendering policy for
    /// selected cells (colors, inversion, etc.) is left to the caller.
    ///
    /// Renderers that can draw cells in spans may be more efficient calling
    /// [`RowIteration::selection`] once per row and applying that range
    /// directly, avoiding one C API call per cell for selection state.
    pub fn is_selected(&self) -> Result<bool> {
        self.get(ffi::RenderStateRowCellsData::SELECTED)
    }

    /// Whether the cell has any explicit styling.
    ///
    /// This is equivalent to querying the raw cell's [`Cell::has_styling`]
    /// value, but avoids materializing the raw [`Cell`] for renderers that
    /// only need to know whether fetching the full style is necessary.
    pub fn has_styling(&self) -> Result<bool> {
        self.get(ffi::RenderStateRowCellsData::HAS_STYLING)
    }
}

//---------------------------
// Auxiliary types
//---------------------------

/// A number of rows above and below the viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Overscan {
    /// Rows above the top of the viewport.
    pub above: u16,
    /// Rows below the bottom of the viewport.
    pub below: u16,
}

/// The identity of a row across render state updates.
///
/// Treat this value as opaque. A zero-initialized id is never valid, so it
/// can be used to mean "no row".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RowId(pub [u64; 2]);

impl RowId {
    /// Whether this id is the "no row" sentinel.
    #[must_use]
    pub fn is_none(self) -> bool {
        self.0 == [0, 0]
    }
}

/// Cursor viewport position information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorViewport {
    /// Cursor viewport x position in cells.
    pub x: u16,
    /// Cursor viewport y position in cells.
    pub y: u16,
    /// Whether the cursor is on the tail of a wide character.
    pub at_wide_tail: bool,
}

/// All render-state cursor information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// The cursor position when it is inside the viewport.
    pub viewport: Option<CursorViewport>,
    /// Whether the cursor is visible based on terminal modes.
    pub visible: bool,
    /// Whether the cursor should blink based on terminal modes.
    pub blinking: bool,
    /// Whether the cursor is at a password input field.
    pub password_input: bool,
    /// The visual style of the cursor.
    pub visual_style: CursorVisualStyle,
}

/// Render-state color information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Colors {
    /// The default/current background color for the render state.
    pub background: RgbColor,
    /// The default/current foreground color for the render state.
    pub foreground: RgbColor,
    /// The cursor color which may be explicitly set by terminal state.
    pub cursor: Option<RgbColor>,
    /// The active 256-color palette for this render state.
    pub palette: [RgbColor; 256],
}

/// Dirty state of a render state after update.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, int_enum::IntEnum)]
pub enum Dirty {
    /// Not dirty at all; rendering can be skipped.
    Clean = ffi::RenderStateDirty::FALSE,
    /// Some rows changed; renderer can redraw incrementally.
    Partial = ffi::RenderStateDirty::PARTIAL,
    /// Global state changed; renderer should redraw everything.
    Full = ffi::RenderStateDirty::FULL,
}

/// Visual style of the cursor.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, int_enum::IntEnum)]
#[non_exhaustive]
pub enum CursorVisualStyle {
    /// Bar cursor (DECSCUSR 5, 6).
    Bar = ffi::RenderStateCursorVisualStyle::BAR,
    /// Block cursor (DECSCUSR 1, 2).
    Block = ffi::RenderStateCursorVisualStyle::BLOCK,
    /// Underline cursor (DECSCUSR 3, 4).
    Underline = ffi::RenderStateCursorVisualStyle::UNDERLINE,
    /// Hollow block cursor.
    BlockHollow = ffi::RenderStateCursorVisualStyle::BLOCK_HOLLOW,
}
