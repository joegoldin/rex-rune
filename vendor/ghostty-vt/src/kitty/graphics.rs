//! API for inspecting images and placements stored via the
//! [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/).
//!
//! The central object is [`Graphics`], an opaque handle to the image storage
//! associated with a terminal's active screen. From it you can iterate over
//! placements and look up individual images.
//!
//! ## Obtaining a [`Graphics`] handle
//!
//! A [`Graphics`] handle is obtained from a terminal via
//! [`Terminal::kitty_graphics`]. The handle is borrowed from the terminal and
//! remains valid until the next mutating terminal call (e.g.
//! [`Terminal::vt_write`] or [`Terminal::reset`]).
//!
//! Before images can be stored, Kitty graphics must be enabled on the
//! terminal by setting a non-zero storage limit with
//! [`Terminal::set_kitty_image_storage_limit`] and a PNG
//! decoder must be installed via [`set_png_decoder`].
//!
//! ## Iterating placements
//!
//! Placements are inspected through a [`PlacementIterator`].
//! The typical workflow is:
//!   1. Create an iterator with [`PlacementIterator::new`].
//!   2. Populate it from the storage with [`PlacementIterator::update`],
//!      returning a [`PlacementIteration`] object.
//!   3. Optionally filter by z-layer with [`PlacementIteration::set_layer`].
//!   4. Advance with [`PlacementIteration::next`] and read
//!      per-placement data with various methods on [`PlacementIteration`],
//!      such as [`PlacementIteration::image_id`].
//!   5. For each placement, look up its image with [`Graphics::image`] to
//!      access pixel data and dimensions.
//!
//! ## Lifetimes and thread-safety
//!
//! All handles borrowed from the terminal ([`Graphics`],
//! [`Image`]) are invalidated by any mutating terminal
//! call. The placement iterator is independently owned and must be freed
//! by the caller, but the data it yields is only valid while the
//! underlying terminal is not mutated.
//!
//! Adapted from libghostty-vt 0.2.2 (MIT OR Apache-2.0).

#![cfg(feature = "kitty-graphics")]

use std::{
    cell::RefCell,
    mem::{ManuallyDrop, MaybeUninit},
};

use crate::{
    Terminal,
    alloc::{Allocator, Bytes, Object, Ref},
    error::{Error, Result, from_optional_result_uninit, from_result},
    ffi,
    selection::Selection,
};

#[doc(inline)]
pub use ffi::KittyGraphicsPlacementRenderInfo as PlacementRenderInfo;

/// Opaque reference to a Kitty graphics image storage.
///
/// Obtained via [`Terminal::kitty_graphics`]. The reference is borrowed from
/// the terminal with lifetime `'t` and remains valid until the next mutating
/// terminal call (e.g. [`Terminal::vt_write`] or [`Terminal::reset`]).
#[derive(Debug)]
pub struct Graphics<'t> {
    inner: Ref<'t, ffi::KittyGraphicsImpl>,
}

/// Opaque reference to a Kitty graphics image.
///
/// Obtained via [`Graphics::image`] with an image ID. The reference is
/// borrowed from the storage with lifetime `'t` and remains valid until
/// the next mutating terminal call.
#[derive(Debug)]
pub struct Image<'t> {
    inner: Ref<'t, ffi::KittyGraphicsImageImpl>,
}

/// Opaque reference to a Kitty graphics placement iterator.
#[derive(Debug)]
pub struct PlacementIterator {
    inner: Object<ffi::KittyGraphicsPlacementIteratorImpl>,
}

/// Obtained via [`PlacementIterator::update`]. The reference is
/// borrowed from the storage with lifetime `'t` and remains valid until
/// the next mutating terminal call.
#[derive(Debug)]
pub struct PlacementIteration<'t>(&'t mut PlacementIterator);

/// Methods related to the [Kitty graphics protocol](crate::kitty::graphics).
impl Terminal {
    /// The Kitty graphics image storage for the active screen.
    ///
    /// Returns a borrowed reference to the image storage.
    /// The pointer is valid until the next mutating terminal call (e.g.
    /// [`Terminal::vt_write`] or [`Terminal::reset`]).
    pub fn kitty_graphics(&self) -> Result<Graphics<'_>> {
        let inner = self.get::<ffi::KittyGraphics>(ffi::TerminalData::KITTY_GRAPHICS)?;
        Ok(Graphics {
            inner: Ref::new(inner)?,
        })
    }

    /// The Kitty image storage limit in bytes for the active screen.
    ///
    /// A value of zero means the Kitty graphics protocol is disabled.
    pub fn kitty_image_storage_limit(&self) -> Result<u64> {
        self.get(ffi::TerminalData::KITTY_IMAGE_STORAGE_LIMIT)
    }
    /// Whether the file medium is enabled for Kitty image loading on the
    /// active screen.
    pub fn is_kitty_image_from_file_allowed(&self) -> Result<bool> {
        self.get(ffi::TerminalData::KITTY_IMAGE_MEDIUM_FILE)
    }
    /// Whether the temporary file medium is enabled for Kitty image loading
    /// on the active screen.
    pub fn is_kitty_image_from_temp_file_allowed(&self) -> Result<bool> {
        let directory = self.get::<ffi::String>(ffi::TerminalData::KITTY_IMAGE_MEDIUM_TEMP_FILE)?;
        Ok(directory.len != 0)
    }
    /// Whether the shared memory medium is enabled for Kitty image loading
    /// on the active screen.
    pub fn is_kitty_image_from_shared_mem_allowed(&self) -> Result<bool> {
        self.get(ffi::TerminalData::KITTY_IMAGE_MEDIUM_SHARED_MEM)
    }
    /// Set the Kitty image storage limit in bytes.
    ///
    /// Applied to all initialized screens (primary and alternate).
    /// A value of zero disables the Kitty graphics protocol entirely,
    /// deleting all stored images and placements.
    pub fn set_kitty_image_storage_limit(&mut self, limit: u64) -> Result<&mut Self> {
        self.set(ffi::TerminalOption::KITTY_IMAGE_STORAGE_LIMIT, &limit)?;
        Ok(self)
    }
    /// Enable or disable Kitty image loading via the file medium.
    pub fn set_kitty_image_from_file_allowed(&mut self, allowed: bool) -> Result<&mut Self> {
        self.set(ffi::TerminalOption::KITTY_IMAGE_MEDIUM_FILE, &allowed)?;
        Ok(self)
    }
    /// Restrict temporary-file image loading to this directory, or disable it.
    /// The C API copies the directory; None passes a null pointer to disable it.
    pub fn set_kitty_image_temp_directory(&mut self, directory: Option<&str>) -> Result<&mut Self> {
        let directory = directory.map(|s| ffi::String {
            ptr: s.as_ptr(),
            len: s.len(),
        });
        self.set_optional(
            ffi::TerminalOption::KITTY_IMAGE_MEDIUM_TEMP_FILE,
            directory.as_ref(),
        )?;
        Ok(self)
    }
    /// Enable or disable Kitty image loading via the shared memory medium.
    pub fn set_kitty_image_from_shared_mem_allowed(&mut self, allowed: bool) -> Result<&mut Self> {
        self.set(ffi::TerminalOption::KITTY_IMAGE_MEDIUM_SHARED_MEM, &allowed)?;
        Ok(self)
    }

    /// Set the maximum bytes the APC handler will buffer for Kitty graphics
    /// protocol data.
    ///
    /// This prevents malicious input from causing unbounded memory allocation.
    /// A `None` value removes all overrides, reverting to the built-in defaults.
    pub fn set_apc_max_bytes_kitty(&mut self, max: Option<usize>) -> Result<&mut Self> {
        self.set_optional(ffi::TerminalOption::APC_MAX_BYTES_KITTY, max.as_ref())?;
        Ok(self)
    }
}

impl<'t> Graphics<'t> {
    fn get<T>(&self, tag: ffi::KittyGraphicsData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::zeroed();
        let result = unsafe {
            ffi::ghostty_kitty_graphics_get(self.inner.as_raw(), tag, value.as_mut_ptr().cast())
        };
        from_result(result)?;
        Ok(unsafe { value.assume_init() })
    }

    /// Look up a Kitty graphics image by its image ID.
    ///
    /// Returns `None` if no image with the given ID exists.
    #[must_use]
    pub fn image(&self, id: u32) -> Option<Image<'t>> {
        let image = unsafe { ffi::ghostty_kitty_graphics_image(self.inner.as_raw(), id) };

        Some(Image {
            inner: Ref::new(image.cast_mut()).ok()?,
        })
    }

    /// Generation stamp of the last content mutation to this storage.
    ///
    /// Content mutations are any image transmit or replace, placement add, or
    /// delete. Zero means the storage has never been mutated and is therefore
    /// empty.
    ///
    /// If the generation is unchanged since a previous query, the set of
    /// placements and all image data are identical, so placement iteration and
    /// image staleness checks can be skipped entirely. Note that placement
    /// geometry may still have changed, since scrolling and resizing move
    /// placements without changing the storage contents, so rendering geometry
    /// must still be recomputed for frames marked dirty.
    ///
    /// Stamps are unique and monotonically increasing process-wide: a value
    /// observed from any storage never recurs for different content, even across
    /// screen switches or terminal resets. It is therefore safe to key caches on
    /// this value alone.
    pub fn generation(&self) -> Result<u64> {
        self.get(ffi::KittyGraphicsData::GENERATION)
    }
}

impl<'t> Image<'t> {
    fn get<T>(&self, tag: ffi::KittyGraphicsImageData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::zeroed();
        let result = unsafe {
            ffi::ghostty_kitty_graphics_image_get(
                self.inner.as_raw(),
                tag,
                value.as_mut_ptr().cast(),
            )
        };
        // Since we manually model every possible query, this should never fail.
        from_result(result)?;
        // SAFETY: Value should be initialized after successful call.
        Ok(unsafe { value.assume_init() })
    }

    /// The image ID.
    pub fn id(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsImageData::ID)
    }
    /// The image number.
    pub fn number(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsImageData::NUMBER)
    }
    /// Image width in pixels.
    pub fn width(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsImageData::WIDTH)
    }
    /// Image height in pixels.
    pub fn height(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsImageData::HEIGHT)
    }
    /// Generation stamp assigned when this image was added to or replaced in the storage.
    ///
    /// A changed generation for a given image ID means the pixel contents may
    /// have changed even when the dimensions, format, and data length are
    /// identical, for example a retransmission of the same image ID, so texture
    /// caches must key staleness on this value rather than on size heuristics.
    ///
    /// Stamps are unique and monotonically increasing process-wide and are drawn
    /// from the same sequence as [`Graphics::generation`]. Never zero for a
    /// stored image, so zero can be used as an empty sentinel by callers.
    pub fn generation(&self) -> Result<u64> {
        self.get(ffi::KittyGraphicsImageData::GENERATION)
    }
    /// Pixel format of the image.
    pub fn format(&self) -> Result<ImageFormat> {
        self.get::<ffi::KittyImageFormat::Type>(ffi::KittyGraphicsImageData::FORMAT)
            .and_then(|v| v.try_into().map_err(|_| Error::InvalidValue))
    }
    /// Compression of the image.
    pub fn compression(&self) -> Result<Compression> {
        self.get::<ffi::KittyImageCompression::Type>(ffi::KittyGraphicsImageData::COMPRESSION)
            .and_then(|v| v.try_into().map_err(|_| Error::InvalidValue))
    }
    /// Borrowed pointer to the raw pixel data.
    ///
    /// Valid as long as the underlying terminal is not mutated.
    pub fn data(&self) -> Result<&'t [u8]> {
        let ptr = self.get::<*const u8>(ffi::KittyGraphicsImageData::DATA_PTR)?;
        let len = self.get::<usize>(ffi::KittyGraphicsImageData::DATA_LEN)?;
        if ptr.is_null() || len == 0 {
            return Ok(&[]);
        }

        // SAFETY: We trust libghostty to return valid results
        Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
    }
}

impl PlacementIterator {
    /// Create a new placement iterator instance.
    pub fn new() -> Result<Self> {
        let mut inner: ffi::KittyGraphicsPlacementIterator = std::ptr::null_mut();
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_iterator_new(std::ptr::null(), &raw mut inner)
        };
        from_result(result)?;
        Ok(Self {
            inner: Object::new(inner)?,
        })
    }

    /// Update the placement iterator with the given graphics storage,
    /// returning a new placement iteration.
    pub fn update(&mut self, graphics: &Graphics<'_>) -> Result<PlacementIteration<'_>> {
        let result = unsafe {
            ffi::ghostty_kitty_graphics_get(
                graphics.inner.as_raw(),
                ffi::KittyGraphicsData::PLACEMENT_ITERATOR,
                (&raw mut self.inner.ptr).cast(),
            )
        };
        from_result(result)?;
        Ok(PlacementIteration(self))
    }
}

impl Drop for PlacementIterator {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_kitty_graphics_placement_iterator_free(self.inner.as_raw());
        }
    }
}

impl<'t> PlacementIteration<'t> {
    /// Advance the placement iterator to the next placement.
    ///
    /// If a layer filter has been set via [`PlacementIteration::set_layer`],
    /// only placements matching that layer are returned.
    #[allow(
        clippy::should_implement_trait,
        reason = "lending `next` cannot implement trait"
    )]
    pub fn next(&mut self) -> Option<&Self> {
        if unsafe { ffi::ghostty_kitty_graphics_placement_next(self.0.inner.as_raw()) } {
            Some(self)
        } else {
            None
        }
    }

    fn set<T>(
        &self,
        tag: ffi::KittyGraphicsPlacementIteratorOption::Type,
        value: &T,
    ) -> Result<()> {
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_iterator_set(
                self.0.inner.as_raw(),
                tag,
                std::ptr::from_ref(value).cast(),
            )
        };
        from_result(result)
    }
    fn get<T>(&self, tag: ffi::KittyGraphicsPlacementData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::zeroed();
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_get(
                self.0.inner.as_raw(),
                tag,
                value.as_mut_ptr().cast(),
            )
        };
        // Since we manually model every possible query, this should never fail.
        from_result(result)?;
        // SAFETY: Value should be initialized after successful call.
        Ok(unsafe { value.assume_init() })
    }

    /// Set the z-layer filter for the iterator.
    pub fn set_layer(&self, layer: Layer) -> Result<()> {
        self.set::<ffi::KittyPlacementLayer::Type>(
            ffi::KittyGraphicsPlacementIteratorOption::LAYER,
            &layer.into(),
        )
    }

    /// Compute the rendered pixel size of the current placement.
    ///
    /// Takes into account the placement's source rectangle, specified
    /// columns/rows, and aspect ratio to calculate the final rendered pixel
    /// dimensions.
    pub fn pixel_size(&self, image: &Image<'t>, terminal: &'t Terminal) -> Result<PixelSize> {
        let mut size = PixelSize::default();
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_pixel_size(
                self.0.inner.as_raw(),
                image.inner.as_raw(),
                terminal.as_raw(),
                &raw mut size.width,
                &raw mut size.height,
            )
        };
        from_result(result)?;
        Ok(size)
    }

    /// Compute the rendered grid size of the current placement.
    ///
    /// Takes into account the placement's source rectangle, specified
    /// columns/rows, and aspect ratio to calculate the final rendered grid
    /// dimensions.
    pub fn grid_size(&self, image: &Image<'t>, terminal: &'t Terminal) -> Result<GridSize> {
        let mut size = GridSize::default();
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_grid_size(
                self.0.inner.as_raw(),
                image.inner.as_raw(),
                terminal.as_raw(),
                &raw mut size.cols,
                &raw mut size.rows,
            )
        };
        from_result(result)?;
        Ok(size)
    }

    /// Get the viewport-relative grid position of the current placement.
    ///
    /// Converts the placement's internal pin to viewport-relative column and
    /// row coordinates. The returned coordinates represent the top-left
    /// corner of the placement in the viewport's grid coordinate space.
    ///
    /// The row value can be negative when the placement's origin has
    /// scrolled above the top of the viewport. Embedders should use these
    /// coordinates directly when computing the destination rectangle for
    /// rendering; the embedder is responsible for clipping the portion of
    /// the image that falls outside the viewport.
    ///
    /// Returns `None` when the placement is completely outside the viewport
    /// (its bottom edge is above the viewport or its top edge is at or below
    /// the last viewport row), or when the placement is a virtual (unicode
    /// placeholder) placement.
    pub fn viewport_pos(
        &self,
        image: &Image<'t>,
        terminal: &'t Terminal,
    ) -> Result<Option<ViewportPos>> {
        let mut pos = ViewportPos::default();
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_viewport_pos(
                self.0.inner.as_raw(),
                image.inner.as_raw(),
                terminal.as_raw(),
                &raw mut pos.col,
                &raw mut pos.row,
            )
        };
        from_optional_result_uninit(result, MaybeUninit::new(pos))
    }

    /// Get the resolved source rectangle for the current placement.
    ///
    /// Applies kitty protocol semantics: a width or height of 0 in the
    /// placement means "use the full image dimension", and the resulting
    /// rectangle is clamped to the actual image bounds. The returned values
    /// are in pixels and are ready to use for texture sampling.
    pub fn source_rect(&self, image: &Image<'t>) -> Result<SourceRect> {
        let mut rect = SourceRect::default();
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_source_rect(
                self.0.inner.as_raw(),
                image.inner.as_raw(),
                &raw mut rect.x,
                &raw mut rect.y,
                &raw mut rect.width,
                &raw mut rect.height,
            )
        };
        from_result(result)?;
        Ok(rect)
    }

    /// Get the bounding rectangle of the current placement as a selection.
    pub fn rect(&self, image: &Image<'t>, terminal: &'t Terminal) -> Result<Selection<'t>> {
        let mut sel = MaybeUninit::<ffi::Selection>::zeroed();
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_rect(
                self.0.inner.as_raw(),
                image.inner.as_raw(),
                terminal.as_raw(),
                sel.as_mut_ptr(),
            )
        };
        from_result(result)?;
        // SAFETY: Selection should be initialized and valid on success
        Ok(unsafe { Selection::from_raw(sel.assume_init()) })
    }

    /// Get all rendering geometry for a placement in a single call.
    ///
    /// Combines pixel size, grid size, viewport position, and source
    /// rectangle into one struct.
    ///
    /// When `viewport_visible` is false, the placement is fully off-screen
    /// or is a virtual placement; `viewport_col` and `viewport_row` may
    /// contain meaningless values in that case.
    pub fn placement_render_info(
        &self,
        image: &Image<'t>,
        terminal: &'t Terminal,
    ) -> Result<PlacementRenderInfo> {
        let mut info = ffi::sized!(PlacementRenderInfo);
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_render_info(
                self.0.inner.as_raw(),
                image.inner.as_raw(),
                terminal.as_raw(),
                &raw mut info,
            )
        };
        from_result(result)?;
        Ok(info)
    }

    /// The image ID this placement belongs to.
    pub fn image_id(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsPlacementData::IMAGE_ID)
    }
    /// The placement ID.
    pub fn placement_id(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsPlacementData::PLACEMENT_ID)
    }
    /// Whether this is a virtual placement (unicode placeholder).
    pub fn is_virtual(&self) -> Result<bool> {
        self.get(ffi::KittyGraphicsPlacementData::IS_VIRTUAL)
    }
    /// Pixel offset from the left edge of the cell.
    pub fn x_offset(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsPlacementData::X_OFFSET)
    }
    /// Pixel offset from the top edge of the cell.
    pub fn y_offset(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsPlacementData::Y_OFFSET)
    }
    /// Source rectangle x origin in pixels.
    pub fn source_x(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsPlacementData::SOURCE_X)
    }
    /// Source rectangle y origin in pixels.
    pub fn source_y(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsPlacementData::SOURCE_Y)
    }
    /// Source rectangle width in pixels (0 = full image width).
    pub fn source_width(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsPlacementData::SOURCE_WIDTH)
    }
    /// Source rectangle height in pixels (0 = full image height).
    pub fn source_height(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsPlacementData::SOURCE_HEIGHT)
    }
    /// Number of columns this placement occupies.
    pub fn columns(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsPlacementData::COLUMNS)
    }
    /// Number of rows this placement occupies.
    pub fn rows(&self) -> Result<u32> {
        self.get(ffi::KittyGraphicsPlacementData::ROWS)
    }
    /// Z-index for this placement.
    pub fn z(&self) -> Result<i32> {
        self.get(ffi::KittyGraphicsPlacementData::Z)
    }
}

/// The size of an image in pixel coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PixelSize {
    /// The width in number of pixels.
    pub width: u32,
    /// The height in number of pixels.
    pub height: u32,
}

/// The size of an image in grid coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GridSize {
    /// The number of columns.
    pub cols: u32,
    /// The number of rows.
    pub rows: u32,
}

/// The position of an image in the viewport.
///
/// The row value can be negative when the placement's origin has
/// scrolled above the top of the viewport.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewportPos {
    /// The column index relative to the viewport.
    pub col: i32,
    /// The row index relative to the viewport.
    pub row: i32,
}

/// The pixel position and size of a source rectangle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceRect {
    /// The x origin in pixels.
    pub x: u32,
    /// The y origin in pixels.
    pub y: u32,
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
}

/// Z-layer classification for kitty graphics placements.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, int_enum::IntEnum)]
#[repr(i32)]
pub enum Layer {
    /// Match all placements; apply no filtering (default behavior).
    #[default]
    All = ffi::KittyPlacementLayer::ALL,
    /// Match placements positioned below the cell background (z < [`i32::MIN`] / 2).
    BelowBg = ffi::KittyPlacementLayer::BELOW_BG,
    /// Match placements positioned above the cell background and below text
    /// ([`i32::MIN`] / 2 <= z < 0).
    BelowText = ffi::KittyPlacementLayer::BELOW_TEXT,
    /// Match placements positioned above text (z >= 0).
    AboveText = ffi::KittyPlacementLayer::ABOVE_TEXT,
}

/// Pixel format of a Kitty graphics image.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, int_enum::IntEnum)]
#[non_exhaustive]
#[repr(i32)]
#[allow(missing_docs, reason = "missing upstream docs")]
pub enum ImageFormat {
    #[default]
    Rgb = ffi::KittyImageFormat::RGB,
    Rgba = ffi::KittyImageFormat::RGBA,
    Png = ffi::KittyImageFormat::PNG,
    GrayAlpha = ffi::KittyImageFormat::GRAY_ALPHA,
    Gray = ffi::KittyImageFormat::GRAY,
}

/// Compression of a Kitty graphics image.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, int_enum::IntEnum)]
#[non_exhaustive]
#[repr(i32)]
#[allow(missing_docs, reason = "missing upstream docs")]
pub enum Compression {
    #[default]
    None = ffi::KittyImageCompression::NONE,
    ZlibDeflate = ffi::KittyImageCompression::ZLIB_DEFLATE,
}

thread_local! {
    static DECODE_PNG: RefCell<Option<Box<dyn DecodePng>>> = const { RefCell::new(None) };
}

/// Set the PNG decoder.
///
/// When set, the terminal can accept PNG images via the Kitty Graphics Protocol.
/// When cleared (`None` value), PNG decoding is unsupported and PNG image data
/// will be rejected.
///
/// # Thread safety
///
/// The decoder is stored per thread, and libghostty calls it on the thread
/// that writes to the terminal. Call this on the terminal's thread.
pub fn set_png_decoder(f: Option<Box<dyn DecodePng>>) -> Result<()> {
    unsafe extern "C" fn callback(
        _userdata: *mut std::ffi::c_void,
        allocator: *const ffi::Allocator,
        data: *const u8,
        data_len: usize,
        out: *mut ffi::SysImage,
    ) -> bool {
        DECODE_PNG.with_borrow_mut(|decoder| {
            let Some(decoder) = decoder else {
                return false;
            };
            // SAFETY: We trust libghostty to return valid values.
            let alloc = unsafe { Allocator::from_raw(allocator) };
            let data = unsafe { std::slice::from_raw_parts(data, data_len) };

            match decoder.decode_png(&alloc, data) {
                Some(result) => {
                    // IMPORTANT: Do NOT run the Rust destructor here
                    // to avoid double-freeing the byte buffer.
                    let mut result = ManuallyDrop::new(result);
                    unsafe {
                        *out = ffi::SysImage {
                            width: result.width,
                            height: result.height,
                            data: result.data.as_mut_ptr(),
                            data_len: result.data.len(),
                        }
                    };
                    true
                }
                None => false,
            }
        })
    }

    // Write out the matches here to coerce function items into function
    // pointers, and trait impls into boxed trait objects.
    let ptr: ffi::SysDecodePngFn = match f {
        None => None,
        Some(_) => Some(callback),
    };
    DECODE_PNG.replace(f);

    crate::sys_set(
        ffi::SysOption::DECODE_PNG,
        ptr.map_or(std::ptr::null(), |p| p as *const std::ffi::c_void),
    )
}

/// A PNG decoder that can be used by the Kitty graphics protocol
/// to decode PNG images into 8-bit RGBA pixels.
///
/// See [`set_png_decoder`] for more details.
pub trait DecodePng: 'static {
    /// Decode a PNG into 8-bit RGBA pixels.
    ///
    /// The returned image's byte buffer *must* be allocated by
    /// the provided allocator.
    fn decode_png(&mut self, alloc: &Allocator, data: &[u8]) -> Option<DecodedImage>;
}

/// A PNG decoder for [`set_png_decoder`] using the [`png`] crate.
#[cfg(feature = "png")]
#[cfg_attr(docsrs, doc(cfg(feature = "png")))]
#[derive(Clone, Debug, Default)]
pub struct RustPngDecoder {
    buf: Vec<u8>,
}

#[cfg(feature = "png")]
impl RustPngDecoder {
    /// Create a decoder with an empty scratch buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(feature = "png")]
impl DecodePng for RustPngDecoder {
    fn decode_png(&mut self, alloc: &Allocator, data: &[u8]) -> Option<DecodedImage> {
        use png::{Decoder, Transformations};
        use std::io::Cursor;

        let mut decoder = Decoder::new(Cursor::new(data));

        // libghostty only accepts RGBA8 data, so we have to apply some
        // transformations to accept images in other formats, namely
        // expanding palette and grayscale colors to RGBA8 and stripping
        // 16-bit color depth information back down into 8-bit.
        decoder.set_transformations(Transformations::ALPHA | Transformations::STRIP_16);

        let mut frame = decoder.read_info().ok()?;
        let buf_size = frame.output_buffer_size()?;
        self.buf.clear();
        self.buf.resize(buf_size, 0);

        let info = frame.next_frame(&mut self.buf).ok()?;

        // EXPAND/ALPHA preserves grayscale's channel layout. The C callback
        // requires RGBA8, not the decoder's one- or two-channel output.
        if info.bit_depth != png::BitDepth::Eight {
            return None;
        }
        let size = (info.width as usize)
            .checked_mul(info.height as usize)?
            .checked_mul(4)?;
        let mut bytes = Bytes::new_with_alloc(alloc, size).ok()?;
        let source = &self.buf[..info.buffer_size()];
        let channels = match info.color_type {
            png::ColorType::Rgba => 4,
            png::ColorType::Rgb => 3,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Grayscale => 1,
            png::ColorType::Indexed => return None,
        };
        for (pixel, out) in source.chunks_exact(channels).zip(bytes.chunks_exact_mut(4)) {
            match channels {
                4 => out.copy_from_slice(pixel),
                3 => out.copy_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
                2 => out.copy_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]),
                _ => out.copy_from_slice(&[pixel[0], pixel[0], pixel[0], 255]),
            }
        }
        frame.finish().ok()?;

        Some(DecodedImage {
            width: info.width,
            height: info.height,
            data: bytes,
        })
    }
}

/// Result of decoding an image.
///
/// The `data` buffer must be allocated through the allocator provided to the
/// decode callback. The library takes ownership and will free it with the
/// same allocator.
#[derive(Debug)]
pub struct DecodedImage {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Byte buffer containing the decoded RGBA pixel data.
    pub data: Bytes,
}
