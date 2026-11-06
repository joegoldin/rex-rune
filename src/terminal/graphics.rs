//! Ghostty owns Kitty parsing, storage and placement. Frames own bounded pixel
//! snapshots so the UI never borrows data that terminal writes can invalidate.
use anyhow::{Result, ensure};
use ghostty_vt::{
    Terminal,
    alloc::Allocator,
    kitty::graphics::{self, DecodePng, DecodedImage, ImageFormat, RustPngDecoder},
    render::{ImageIterator, ImagePlacement, Snapshot},
};
use std::{collections::HashMap, sync::Arc};

pub const IMAGE_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ImagePixels {
    pub screen: i32,
    pub id: u32,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}
#[derive(Clone, Debug)]
pub struct Image {
    pub placement: ImagePlacement,
    pub pixels: Arc<ImagePixels>,
}

#[derive(Default)]
struct BoundedPng(RustPngDecoder);
impl DecodePng for BoundedPng {
    fn decode_png(&mut self, alloc: &Allocator, data: &[u8]) -> Option<DecodedImage> {
        // Bound allocation before the decoder creates its output buffer, not after it.
        png_allocation_size(data)?;
        self.0.decode_png(alloc, data)
    }
}

fn png_allocation_size(data: &[u8]) -> Option<usize> {
    if data.len() < 24 || &data[..8] != b"\x89PNG\r\n\x1a\n" || &data[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(data[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(data[20..24].try_into().ok()?);
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))?
        .checked_mul(4)?;
    (width > 0 && height > 0 && bytes <= IMAGE_LIMIT).then_some(bytes as usize)
}

pub fn install_decoder() -> Result<()> {
    graphics::set_png_decoder(Some(Box::<BoundedPng>::default()))?;
    Ok(())
}

pub fn configure(terminal: &mut Terminal) -> Result<()> {
    terminal.set_kitty_image_storage_limit(IMAGE_LIMIT)?;
    // A server's file paths are not this client's paths. Never let remote terminal
    // output read or unlink local files or shared memory through Kitty's file media.
    terminal.set_kitty_image_from_file_allowed(false)?;
    terminal.set_kitty_image_temp_directory(None)?;
    terminal.set_kitty_image_from_shared_mem_allowed(false)?;
    Ok(())
}

pub struct Images {
    iterator: ImageIterator,
    cache: HashMap<(i32, u32, u64), Arc<ImagePixels>>,
}
impl Images {
    pub fn new() -> Result<Self> {
        Ok(Self {
            iterator: ImageIterator::new()?,
            cache: HashMap::new(),
        })
    }
    pub fn frame(&mut self, terminal: &Terminal, snapshot: &Snapshot<'_>) -> Result<Vec<Image>> {
        let placements = self.iterator.placements(snapshot)?;
        if placements.is_empty() {
            self.cache.clear();
            return Ok(Vec::new());
        }
        let screen = terminal.active_screen()? as i32;
        let storage = terminal.kitty_graphics()?;
        let mut next = HashMap::new();
        let mut result = Vec::new();
        for placement in placements {
            if placement.size.contains(&0) || placement.source[2..].contains(&0) {
                continue;
            }
            let Some(image) = storage.image(placement.image_id) else {
                continue;
            };
            let key = (screen, placement.image_id, placement.generation);
            let pixels = if let Some(pixels) = next.get(&key).or_else(|| self.cache.get(&key)) {
                Arc::clone(pixels)
            } else {
                let (width, height) = (image.width()?, image.height()?);
                let size = u64::from(width) * u64::from(height);
                ensure!(
                    size > 0 && size <= IMAGE_LIMIT / 4,
                    "Kitty image exceeds the pixel limit"
                );
                let data = image.data()?;
                let channels = match image.format()? {
                    ImageFormat::Rgba => 4,
                    ImageFormat::Rgb => 3,
                    ImageFormat::GrayAlpha => 2,
                    ImageFormat::Gray => 1,
                    _ => continue,
                };
                ensure!(
                    data.len() as u64 == size * channels,
                    "Invalid Kitty image pixel length"
                );
                let rgba: Arc<[u8]> = match channels {
                    4 => Arc::from(data),
                    _ => {
                        let mut rgba = Vec::with_capacity(size as usize * 4);
                        for pixel in data.chunks_exact(channels as usize) {
                            match channels {
                                3 => rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
                                2 => rgba
                                    .extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]),
                                _ => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]),
                            }
                        }
                        rgba.into()
                    }
                };
                Arc::new(ImagePixels {
                    screen,
                    id: key.1,
                    generation: key.2,
                    width,
                    height,
                    rgba,
                })
            };
            next.insert(key, Arc::clone(&pixels));
            result.push(Image { placement, pixels });
        }
        self.cache = next;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{Renderer, TerminalStream};
    use base64::{Engine, engine::general_purpose::STANDARD};

    fn terminal() -> Terminal {
        assert!(ghostty_vt::build_info::supports_kitty_graphics().unwrap());
        let mut terminal = Terminal::new(ghostty_vt::TerminalOptions {
            cols: 20,
            rows: 10,
            ..Default::default()
        })
        .unwrap();
        terminal.resize(20, 10, 8, 16).unwrap();
        configure(&mut terminal).unwrap();
        terminal
    }
    fn red() -> Vec<u8> {
        format!(
            "\x1b_Ga=T,f=32,s=2,v=2,c=4,r=2,i=42,q=2;{}\x1b\\",
            STANDARD.encode([255, 0, 0, 255].repeat(4))
        )
        .into_bytes()
    }
    #[test]
    fn live_rgba_and_snapshot_images_retain_pixels_placement_and_deletion() {
        let terminal = terminal();
        let initial = ghostty_vt::snapshot::encode(&terminal).unwrap();
        let mut stream = TerminalStream::new(1);
        stream.feed(&initial).unwrap();
        for byte in red() {
            stream.feed(&[byte]).unwrap();
        }
        let mut renderer = Renderer::new().unwrap();
        let frame = renderer.frame(stream.terminal.as_ref().unwrap()).unwrap();
        assert_eq!(frame.images.len(), 1);
        let image = &frame.images[0];
        assert_eq!(image.pixels.rgba.as_ref(), [255, 0, 0, 255].repeat(4));
        assert_eq!(image.placement.cell, [0, 0]);
        assert_eq!(image.placement.size, [32, 32]);
        assert_eq!(image.placement.source, [0, 0, 2, 2]);
        let second = renderer.frame(stream.terminal.as_ref().unwrap()).unwrap();
        assert!(Arc::ptr_eq(&image.pixels, &second.images[0].pixels));
        let snapshot = ghostty_vt::snapshot::encode(stream.terminal.as_ref().unwrap()).unwrap();
        let mut restored = TerminalStream::new(2);
        restored.feed(&snapshot).unwrap();
        // A fresh snapshot creates a new terminal instance. Match the data worker:
        // a render state caches one terminal's rows and must not cross instances.
        let mut restored_renderer = Renderer::new().unwrap();
        let frame = restored_renderer
            .frame(restored.terminal.as_ref().unwrap())
            .unwrap();
        assert_eq!(frame.images.len(), 1);
        assert_eq!(
            frame.images[0].pixels.rgba.as_ref(),
            [255, 0, 0, 255].repeat(4)
        );
        restored.feed(b"\x1b_Ga=d,d=A,q=2\x1b\\").unwrap();
        assert!(
            restored_renderer
                .frame(restored.terminal.as_ref().unwrap())
                .unwrap()
                .images
                .is_empty()
        );
        let terminal = restored.terminal.as_ref().unwrap();
        assert!(!terminal.is_kitty_image_from_file_allowed().unwrap());
        assert!(!terminal.is_kitty_image_from_temp_file_allowed().unwrap());
        assert!(!terminal.is_kitty_image_from_shared_mem_allowed().unwrap());
    }

    #[test]
    fn grayscale_and_rgb_png_decode_as_rgba() {
        for (encoded, expected) in [
            (
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGNoAAAAggCBd81ytgAAAABJRU5ErkJggg==",
                [128, 128, 128, 255],
            ),
            (
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR4nGNocAAAAUMAwV/UlQ0AAAAASUVORK5CYII=",
                [128, 128, 128, 64],
            ),
            (
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC",
                [255, 0, 0, 255],
            ),
        ] {
            let mut terminal = terminal();
            install_decoder().unwrap();
            terminal
                .vt_write(format!("\x1b_Ga=T,f=100,c=1,r=1,i=42,q=2;{encoded}\x1b\\").as_bytes());
            let frame = Renderer::new().unwrap().frame(&terminal).unwrap();
            assert_eq!(frame.images.len(), 1, "PNG format was rejected");
            assert_eq!(frame.images[0].pixels.rgba.as_ref(), expected);
        }
    }

    #[test]
    fn png_dimensions_are_bounded_before_decoder_allocation() {
        let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        header.extend_from_slice(&u32::MAX.to_be_bytes());
        header.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(png_allocation_size(&header).is_none());
    }
}
