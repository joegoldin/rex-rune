//! Handling various protocols pioneered by Kitty,
//! including the [Kitty graphics protocol](graphics).
//!
//! Adapted from libghostty-vt 0.2.2 (MIT OR Apache-2.0).

pub mod graphics;

#[cfg(feature = "kitty-graphics")]
pub use graphics::Graphics;
