//! Owned placement geometry from libghostty's render-state image iterator.
use super::Snapshot;
use crate::{
    alloc::Object,
    error::{Error, Result, from_result},
    ffi,
};
use std::mem::MaybeUninit;

/// An image's position, crop and paint order in the captured viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImagePlacement {
    /// Kitty image id, scoped to this terminal's active screen.
    pub image_id: u32,
    /// Changes whenever the image's pixels change.
    pub generation: u64,
    /// 0: below backgrounds; 1: below text; 2: above text.
    pub layer: i32,
    /// Draw lower z values first within a layer.
    pub z: i32,
    /// Top-left column and row, possibly outside the viewport.
    pub cell: [i32; 2],
    /// Offset from the top-left cell in server cell pixels.
    pub offset: [u32; 2],
    /// Destination width and height in server cell pixels.
    pub size: [u32; 2],
    /// Source x, y, width and height in image pixels.
    pub source: [u32; 4],
}

/// Reusable iterator. Only owned values escape an update, never borrowed FFI data.
#[derive(Debug)]
pub struct ImageIterator(Object<ffi::RenderStateImageIteratorImpl>);
impl ImageIterator {
    /// Create an empty image iterator.
    pub fn new() -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        let result =
            unsafe { ffi::ghostty_render_state_image_iterator_new(std::ptr::null(), &mut raw) };
        from_result(result)?;
        Ok(Self(Object::new(raw)?))
    }

    // All callers pair each C key with its documented scalar output type.
    fn get<T>(&self, key: ffi::RenderStateImageData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::uninit();
        let result = unsafe {
            ffi::ghostty_render_state_image_get(self.0.as_raw(), key, value.as_mut_ptr().cast())
        };
        from_result(result)?;
        // SAFETY: a successful scalar get initializes the documented output type.
        Ok(unsafe { value.assume_init() })
    }

    /// Capture drawable Kitty placements, including virtual Unicode placeholders and
    /// relative placements. Unknown future image protocols are skipped.
    pub fn placements(&mut self, snapshot: &Snapshot<'_>) -> Result<Vec<ImagePlacement>> {
        let result = unsafe {
            ffi::ghostty_render_state_get(
                snapshot.0.0.as_raw(),
                ffi::RenderStateData::IMAGE_ITERATOR,
                std::ptr::from_mut(&mut self.0.ptr).cast(),
            )
        };
        from_result(result)?;
        let mut result = Vec::new();
        while unsafe { ffi::ghostty_render_state_image_iterator_next(self.0.as_raw()) } {
            use ffi::RenderStateImageData as D;
            if self.get::<i32>(D::PROTOCOL)? != ffi::ImageProtocol::KITTY {
                continue;
            }
            let layer = self.get(D::LAYER)?;
            if !(0..=2).contains(&layer) {
                return Err(Error::InvalidValue);
            }
            result.push(ImagePlacement {
                image_id: self.get(D::IMAGE_ID)?,
                generation: self.get(D::IMAGE_GENERATION)?,
                layer,
                z: self.get(D::Z)?,
                cell: [self.get(D::VIEWPORT_COL)?, self.get(D::VIEWPORT_ROW)?],
                offset: [self.get(D::CELL_OFFSET_X)?, self.get(D::CELL_OFFSET_Y)?],
                size: [self.get(D::PIXEL_WIDTH)?, self.get(D::PIXEL_HEIGHT)?],
                source: [
                    self.get(D::SOURCE_X)?,
                    self.get(D::SOURCE_Y)?,
                    self.get(D::SOURCE_WIDTH)?,
                    self.get(D::SOURCE_HEIGHT)?,
                ],
            });
        }
        result.sort_by_key(|p| (p.layer, p.z));
        Ok(result)
    }
}
impl Drop for ImageIterator {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_render_state_image_iterator_free(self.0.as_raw()) }
    }
}
