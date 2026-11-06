//! Memory owned by libghostty.
//!
//! Every handle in this crate uses libghostty's default allocator. The only
//! place an allocator is visible is the [PNG decoder](crate::kitty::graphics)
//! hook, where libghostty hands the decoder the allocator it wants the pixel
//! buffer allocated with.
//!
//! Adapted from libghostty-vt 0.2.2 (MIT OR Apache-2.0).
use std::{
    borrow::Borrow,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

use crate::{
    error::{Error, Result},
    ffi,
};

/// An allocator handed to a callback by libghostty.
///
/// It is only valid for the duration of that callback.
#[derive(Debug, Clone, Copy)]
pub struct Allocator {
    inner: ffi::Allocator,
}

impl Allocator {
    pub(crate) fn as_raw(&self) -> *const ffi::Allocator {
        std::ptr::from_ref(&self.inner)
    }

    /// # Safety
    ///
    /// `raw` must point to a valid allocator for as long as the returned
    /// value is used.
    pub(crate) unsafe fn from_raw(raw: *const ffi::Allocator) -> Self {
        Self {
            inner: unsafe { *raw },
        }
    }
}

/// An owned opaque libghostty object.
#[derive(Debug)]
pub(crate) struct Object<T> {
    pub(crate) ptr: NonNull<T>,
}

impl<T> Object<T> {
    pub(crate) fn new(raw: *mut T) -> Result<Self> {
        let ptr = NonNull::new(raw).ok_or(Error::OutOfMemory)?;
        Ok(Self { ptr })
    }
    pub(crate) fn as_raw(&self) -> *mut T {
        self.ptr.as_ptr()
    }
}

/// A borrowed opaque libghostty object.
#[derive(Debug)]
pub(crate) struct Ref<'a, T> {
    pub(crate) ptr: NonNull<T>,
    _phan: PhantomData<&'a ()>,
}

impl<T> Ref<'_, T> {
    pub(crate) fn new(raw: *mut T) -> Result<Self> {
        let ptr = NonNull::new(raw).ok_or(Error::OutOfMemory)?;
        Ok(Self {
            ptr,
            _phan: PhantomData,
        })
    }
    pub(crate) fn as_raw(&self) -> *mut T {
        self.ptr.as_ptr()
    }
}

/// Bytes allocated by libghostty.
///
/// The bytes are freed through libghostty when dropped.
#[derive(Debug)]
pub struct Bytes {
    ptr: NonNull<u8>,
    len: usize,
    alloc: *const ffi::Allocator,
}

impl Bytes {
    /// Allocate `len` bytes with libghostty's default allocator.
    pub fn new(len: usize) -> Result<Self> {
        // SAFETY: A NULL allocator is always valid
        unsafe { Self::new_inner(std::ptr::null(), len) }
    }

    /// Allocate `len` bytes with the allocator a callback received.
    pub fn new_with_alloc(alloc: &Allocator, len: usize) -> Result<Self> {
        // SAFETY: The allocator is valid for the duration of the callback.
        unsafe { Self::new_inner(alloc.as_raw(), len) }
    }

    unsafe fn new_inner(alloc: *const ffi::Allocator, len: usize) -> Result<Self> {
        let raw = unsafe { ffi::ghostty_alloc(alloc, len) };
        let ptr = NonNull::new(raw).ok_or(Error::OutOfMemory)?;
        Ok(unsafe { Self::from_raw_parts(ptr, len, alloc) })
    }

    /// # Safety
    ///
    /// `ptr` must be `len` bytes allocated by libghostty with `alloc`.
    pub(crate) unsafe fn from_raw_parts(
        ptr: NonNull<u8>,
        len: usize,
        alloc: *const ffi::Allocator,
    ) -> Self {
        Self { ptr, len, alloc }
    }

    /// Copy the bytes into a `Vec`.
    #[must_use]
    pub fn to_vec(&self) -> Vec<u8> {
        self.deref().to_vec()
    }
}

impl Drop for Bytes {
    fn drop(&mut self) {
        // SAFETY: We own the bytes and they were allocated with `alloc`.
        unsafe { ffi::ghostty_free(self.alloc, self.ptr.as_ptr(), self.len) };
    }
}

impl Deref for Bytes {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: See Drop
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl DerefMut for Bytes {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: See Drop
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl AsMut<[u8]> for Bytes {
    fn as_mut(&mut self) -> &mut [u8] {
        self
    }
}

impl Borrow<[u8]> for Bytes {
    fn borrow(&self) -> &[u8] {
        self
    }
}

impl<'a> IntoIterator for &'a Bytes {
    type Item = &'a u8;
    type IntoIter = std::slice::Iter<'a, u8>;

    fn into_iter(self) -> Self::IntoIter {
        self.deref().iter()
    }
}
