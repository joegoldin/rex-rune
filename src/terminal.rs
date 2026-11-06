use crate::wire::SnapshotBuffer;
use anyhow::{Result, ensure};
use ghostty_vt::{
    RenderState, Terminal,
    render::{CellIterator, RowIterator},
    style::RgbColor,
};
use serde_json::Value;

pub mod selection;

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Output(Vec<u8>),
    Metadata(Value),
}

/// Retains partial escape prefixes across frames and reconnects. Only a Rex
/// APC with this channel's nonce is interpreted as transport metadata.
pub struct ApcParser {
    prefix: Vec<u8>,
    pending: Vec<u8>,
}
impl ApcParser {
    pub fn new(nonce: u64) -> Self {
        Self {
            prefix: format!("\x1b_rex1;{nonce};").into_bytes(),
            pending: Vec::new(),
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Part>> {
        let mut parts = Vec::new();
        let mut output = Vec::new();
        for &byte in bytes {
            self.pending.push(byte);
            loop {
                if self.pending.starts_with(&self.prefix) {
                    ensure!(self.pending.len() <= 1024 * 1024, "Rex APC exceeds 1 MiB");
                    if self.pending.ends_with(b"\x1b\\") {
                        if !output.is_empty() {
                            parts.push(Part::Output(std::mem::take(&mut output)));
                        }
                        let json = &self.pending[self.prefix.len()..self.pending.len() - 2];
                        parts.push(Part::Metadata(serde_json::from_slice(json)?));
                        self.pending.clear();
                    }
                    break;
                }
                if self.prefix.starts_with(&self.pending) {
                    break;
                }
                output.push(self.pending.remove(0));
            }
        }
        if !output.is_empty() {
            parts.push(Part::Output(output));
        }
        Ok(parts)
    }
}

pub struct TerminalStream {
    pub terminal: Option<Terminal>,
    snapshot: SnapshotBuffer,
    parser: ApcParser,
    pub received: u64,
    pub snapshot_count: usize,
    pub exit_code: Option<i64>,
}
impl TerminalStream {
    pub fn new(nonce: u64) -> Self {
        Self {
            terminal: None,
            snapshot: SnapshotBuffer::default(),
            parser: ApcParser::new(nonce),
            received: 0,
            snapshot_count: 0,
            exit_code: None,
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Result<()> {
        self.received += bytes.len() as u64;
        let tail;
        let bytes = if self.terminal.is_none() {
            let Some((terminal, rest)) = self.snapshot.push(bytes)? else {
                return Ok(());
            };
            self.terminal = Some(terminal);
            self.snapshot_count += 1;
            tail = rest;
            &tail
        } else {
            bytes
        };
        let terminal = self.terminal.as_mut().expect("snapshot installed terminal");
        for part in self.parser.feed(bytes)? {
            match part {
                Part::Output(bytes) => terminal.vt_write(&bytes),
                Part::Metadata(value) => match value["type"].as_str() {
                    Some("resize") => {
                        let cols = value["cols"]
                            .as_u64()
                            .filter(|v| (1..=65535).contains(v))
                            .ok_or_else(|| anyhow::anyhow!("invalid resize columns"))?
                            as u16;
                        let rows = value["rows"]
                            .as_u64()
                            .filter(|v| (1..=65535).contains(v))
                            .ok_or_else(|| anyhow::anyhow!("invalid resize rows"))?
                            as u16;
                        terminal.resize(
                            cols,
                            rows,
                            value["cell_width_px"].as_u64().unwrap_or(0) as u32,
                            value["cell_height_px"].as_u64().unwrap_or(0) as u32,
                        )?;
                    }
                    Some("theme") => {
                        terminal.set_default_fg_color(parse_color(value["foreground"].as_str()))?;
                        terminal.set_default_bg_color(parse_color(value["background"].as_str()))?;
                        terminal.set_default_cursor_color(parse_color(value["cursor"].as_str()))?;
                        // Each theme replaces the previous palette, including omissions.
                        terminal.set_default_color_palette(None)?;
                        if let Some(entries) = value["palette"].as_array() {
                            let mut palette = terminal.color_palette()?;
                            for entry in entries.iter().filter_map(Value::as_str) {
                                if let Some((index, color)) = entry.split_once('=')
                                    && let (Ok(index), Some(color)) =
                                        (index.parse::<u8>(), parse_color(Some(color)))
                                {
                                    palette.0[index as usize] = color;
                                }
                            }
                            terminal.set_default_color_palette(Some(palette))?;
                        }
                    }
                    Some("child_exited") => self.exit_code = value["exit_code"].as_i64(),
                    _ => {}
                },
            }
        }
        Ok(())
    }
    pub fn text(&self) -> Result<String> {
        let terminal = self
            .terminal
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("snapshot not ready"))?;
        let mut formatter = ghostty_vt::fmt::Formatter::new(
            terminal,
            ghostty_vt::fmt::FormatterOptions::default(),
        )?;
        Ok(String::from_utf8_lossy(&formatter.format_alloc()?).into_owned())
    }
}

fn parse_color(value: Option<&str>) -> Option<RgbColor> {
    let value = value?.trim_start_matches('#');
    if value.len() != 6 {
        return None;
    }
    let color = u32::from_str_radix(value, 16).ok()?;
    Some(RgbColor {
        r: (color >> 16) as u8,
        g: (color >> 8) as u8,
        b: color as u8,
    })
}
pub fn rgb(color: RgbColor) -> u32 {
    (color.r as u32) << 16 | (color.g as u32) << 8 | color.b as u32
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub text: String,
    pub spacer_tail: bool,
    pub fg: u32,
    pub bg: u32,
    pub style: ghostty_vt::style::Style,
}
#[derive(Clone, Debug)]
pub struct Frame {
    pub selection_rows: Vec<Option<(usize, usize)>>,
    pub selection_text: Option<std::sync::Arc<str>>,
    pub selection_error: Option<String>,
    pub selection_revision: u64,
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Vec<Cell>>,
    pub fg: u32,
    pub bg: u32,
    pub cursor: Option<(u16, u16)>,
    pub cursor_color: u32,
    /// The program asked for mouse reports (DEC 9, 1000, 1002 or 1003).
    pub mouse_tracking: bool,
}

impl Frame {
    /// Local viewport cropping must retain the newest rows, or the cursor when
    /// a TUI has positioned it above them. This never changes the server size.
    pub fn display_row_start(&self, visible_rows: usize) -> usize {
        let bottom = (self.rows as usize).saturating_sub(visible_rows.max(1));
        self.cursor.map_or(bottom, |(_, y)| bottom.min(y as usize))
    }

    pub fn selected_text(&self, a: usize, b: usize) -> String {
        let (start, end) = (a.min(b), a.max(b));
        let cols = self.cols as usize;
        let mut lines = Vec::new();
        for row in start / cols..=end / cols {
            let Some(cells) = self.cells.get(row) else {
                break;
            };
            let mut line = String::new();
            for (col, cell) in cells.iter().enumerate() {
                let index = row * cols + col;
                if index >= start && index <= end && !cell.spacer_tail {
                    line.push_str(if cell.text.is_empty() {
                        " "
                    } else {
                        &cell.text
                    });
                }
            }
            lines.push(line.trim_end().to_owned());
        }
        lines.join("\n")
    }
}
pub struct Renderer {
    copy_buffer: Vec<u8>,
    state: RenderState,
    rows: RowIterator,
    cells: CellIterator,
}
impl Renderer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            copy_buffer: Vec::new(),
            state: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
        })
    }
    pub fn frame(&mut self, terminal: &Terminal) -> Result<Frame> {
        let (selection_text, selection_error) =
            match selection::text(terminal, &mut self.copy_buffer) {
                Ok(text) => (text, None),
                Err(error) => (None, Some(format!("Copy failed: {error}"))),
            };
        let snapshot = self.state.update(terminal)?;
        let colors = snapshot.colors()?;
        let fg = rgb(colors.foreground);
        let bg = rgb(colors.background);
        let mut frame = Frame {
            cols: snapshot.cols()?,
            rows: snapshot.rows()?,
            cells: Vec::new(),
            selection_rows: Vec::new(),
            selection_text,
            selection_error,
            selection_revision: 0,
            fg,
            bg,
            cursor: if snapshot.cursor_visible()? {
                snapshot.cursor_viewport()?.map(|p| (p.x, p.y))
            } else {
                None
            },
            cursor_color: snapshot.cursor_color()?.map(rgb).unwrap_or(fg),
            mouse_tracking: terminal.is_mouse_tracking()?,
        };
        let mut rows = self.rows.update(&snapshot)?;
        while let Some(row) = rows.next() {
            frame.selection_rows.push(
                row.selection()?
                    .map(|s| (usize::from(s.start_x), usize::from(s.end_x))),
            );
            let mut cells = self.cells.update(row)?;
            let mut line = Vec::new();
            while let Some(cell) = cells.next() {
                let mut text = String::new();
                cell.graphemes_utf8(&mut text)?;
                let style = cell.style()?;
                let mut fg = cell.fg_color()?.map(rgb).unwrap_or(fg);
                let mut bg = cell.bg_color()?.map(rgb).unwrap_or(bg);
                if style.inverse {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if style.invisible {
                    text.clear();
                }
                line.push(Cell {
                    text,
                    spacer_tail: cell.raw_cell()?.wide()?
                        == ghostty_vt::screen::CellWide::SpacerTail,
                    fg,
                    bg,
                    style,
                });
            }
            frame.cells.push(line);
        }
        snapshot.clean()?;
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cropped_snapshot_keeps_cursor_and_copy_coordinates() {
        let mut terminal = Terminal::new(ghostty_vt::TerminalOptions {
            cols: 12,
            rows: 5,
            ..Default::default()
        })
        .unwrap();
        terminal.vt_write(b"old\r\nolder\r\nrecent\r\nnewest\r\nprompt");
        let mut renderer = Renderer::new().unwrap();
        let frame = renderer.frame(&terminal).unwrap();
        let start = frame.display_row_start(3);
        assert_eq!(start, 2);
        assert_eq!(frame.selected_text(start * 12, start * 12 + 5), "recent");
        assert_eq!(frame.cursor.unwrap().1 as usize - start, 2);
        assert_eq!(frame.display_row_start(8), 0);

        // A TUI cursor on an older row takes precedence over bottom alignment.
        terminal.vt_write(b"\x1b[2;1H");
        assert_eq!(renderer.frame(&terminal).unwrap().display_row_start(3), 1);
        terminal.vt_write(b"\x1b[?25l");
        let hidden_cursor = renderer.frame(&terminal).unwrap();
        assert!(hidden_cursor.cursor.is_none());
        assert_eq!(hidden_cursor.display_row_start(3), 2);
    }
    #[test]
    fn copy_preserves_graphemes_without_wide_cell_padding() {
        let mut terminal = Terminal::new(ghostty_vt::TerminalOptions {
            cols: 20,
            rows: 3,
            ..Default::default()
        })
        .unwrap();
        terminal.vt_write("界 e\u{301}\r\nsecond".as_bytes());
        let frame = Renderer::new().unwrap().frame(&terminal).unwrap();
        assert_eq!(frame.selected_text(0, 25), "界 e\u{301}\nsecond");
        assert_eq!(frame.selected_text(25, 0), "界 e\u{301}\nsecond");
    }

    #[test]
    fn corrupted_snapshot_never_installs_a_terminal() {
        let terminal = Terminal::new(ghostty_vt::TerminalOptions::default()).unwrap();
        let mut bytes = ghostty_vt::snapshot::encode(&terminal).unwrap().to_vec();
        bytes[16] ^= 1; // First record's CRC, not a transport boundary.
        let mut stream = TerminalStream::new(42);
        assert!(stream.feed(&bytes).is_err());
        assert!(stream.terminal.is_none());
    }
    #[test]
    fn every_apc_split_and_false_nonce() {
        let message = b"a\x1b_rex1;123;{\"type\":\"resize\",\"cols\":80,\"rows\":24}\x1b\\z";
        for split in 0..=message.len() {
            let mut parser = ApcParser::new(123);
            let mut parts = parser.feed(&message[..split]).unwrap();
            parts.extend(parser.feed(&message[split..]).unwrap());
            let text: Vec<u8> = parts
                .iter()
                .filter_map(|p| {
                    if let Part::Output(b) = p {
                        Some(b.as_slice())
                    } else {
                        None
                    }
                })
                .flatten()
                .copied()
                .collect();
            assert_eq!(text, b"az");
            assert_eq!(
                parts
                    .iter()
                    .filter(|p| matches!(p, Part::Metadata(_)))
                    .count(),
                1
            );
        }
        let mut parser = ApcParser::new(456);
        assert_eq!(
            parser.feed(message).unwrap(),
            vec![Part::Output(message.to_vec())]
        );
    }
    #[test]
    fn snapshot_tail_and_parser_continuation_survive_fragmentation() {
        let mut terminal = Terminal::new(ghostty_vt::TerminalOptions {
            cols: 20,
            rows: 3,
            ..Default::default()
        })
        .unwrap();
        terminal.set_continuation_max_bytes(1024).unwrap();
        terminal.vt_write(b"before\x1b[31");
        let snapshot = ghostty_vt::snapshot::encode(&terminal).unwrap();
        let mut bytes = snapshot.to_vec();
        bytes.extend_from_slice(
            b"mAFTER\x1b_rex1;42;{\"type\":\"resize\",\"cols\":30,\"rows\":4}\x1b\\",
        );
        for size in [1, 7, 10, 64, 1024, bytes.len()] {
            let mut stream = TerminalStream::new(42);
            for chunk in bytes.chunks(size) {
                stream.feed(chunk).unwrap();
            }
            assert_eq!(stream.received, bytes.len() as u64);
            assert_eq!(stream.snapshot_count, 1);
            assert!(stream.text().unwrap().contains("beforeAFTER"));
            assert_eq!(stream.terminal.as_ref().unwrap().cols().unwrap(), 30);
        }
    }
}
