//! One terminal pane: its stream, local view state (selection, preedit, retained-frame offset)
//! and the cell grid, drawn on a fixed column grid.
use super::{
    keys,
    theme::{CARD, TERMINAL_SELECTION, mix, rgb},
};
use rex_linux::{
    connection::{Command, Connection},
    terminal::{
        Cell, Frame,
        graphics::{Image as TermImage, ImagePixels},
    },
};
use rune::{El, Family, Mods, Span, TextStyle, el, memo, rich, text};
use serde_json::json;
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

pub const PADDING: f32 = 8.;

pub struct Term {
    pub connection: Connection,
    pub frame: Option<Arc<Frame>>,
    pub last_size: Option<(u16, u16)>,
    /// The grid's rect in window points (inside the padding).
    pub rect: [f32; 4],
    pub selection: Option<(usize, usize)>,
    pub preedit: String,
    pub notice: Option<String>,
    pub scroll: f32,
    /// A button press forwarded to the program, until its release: the gesture keeps going to
    /// the program even if the program stops tracking or Shift goes down mid-drag.
    pub mouse: Option<MouseGesture>,
    /// The last cell a motion report named, so moves within a cell send nothing.
    pub mouse_cell: Option<(u16, u16)>,
    images: HashMap<usize, (Arc<ImagePixels>, rune::Image)>,
}

#[derive(Clone, Copy)]
pub struct MouseGesture {
    pub button: &'static str,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Metrics {
    pub cell: (f32, f32),
    pub style: TextStyle,
}

const FEATURES: &str = "'liga' 0, 'calt' 0, 'clig' 0, 'kern' 0";

pub fn metrics(size: f32, family: Option<&'static str>) -> Metrics {
    let base = TextStyle::new(size, 400.)
        .lh((size * 1.4).ceil())
        .features(FEATURES);
    let style = match family {
        Some(name) => base.family(Family::Named(name)),
        None => base.mono(),
    };
    let width = rune::text::advance("M", style).max(1.);
    Metrics {
        cell: (width, (size * 1.4).ceil()),
        style,
    }
}

pub struct Status {
    pub live: bool,
    pub owner: bool,
    pub status: String,
}

impl Term {
    pub fn new(connection: Connection) -> Self {
        let frame = connection
            .model
            .lock()
            .expect("terminal model")
            .frame
            .clone();
        Self {
            connection,
            frame,
            last_size: None,
            rect: [0.; 4],
            selection: None,
            preedit: String::new(),
            notice: None,
            scroll: 0.,
            mouse: None,
            mouse_cell: None,
            images: HashMap::new(),
        }
    }

    pub fn refresh(&mut self) {
        self.frame = self
            .connection
            .model
            .lock()
            .expect("terminal model")
            .frame
            .clone();
    }

    pub fn cache_images(&mut self, cache: Option<&rune::ImageCache>) {
        let (Some(cache), Some(frame)) = (cache, &self.frame) else {
            self.images.clear();
            return;
        };
        // Keeping the pixel Arc beside the texture prevents pointer reuse from making
        // a fresh snapshot look like the previous image with the same protocol id.
        for image in &frame.images {
            let pixels = &image.pixels;
            self.images
                .entry(Arc::as_ptr(pixels) as usize)
                .or_insert_with(|| {
                    (
                        Arc::clone(pixels),
                        cache.rgba(pixels.width, pixels.height, Arc::clone(&pixels.rgba)),
                    )
                });
        }
        self.images.retain(|key, _| {
            frame
                .images
                .iter()
                .any(|image| Arc::as_ptr(&image.pixels) as usize == *key)
        });
    }

    pub fn status(&self) -> Status {
        let m = self.connection.model.lock().expect("terminal model");
        Status {
            live: m.connected && m.control_connected,
            owner: m.owner,
            status: m.status.clone(),
        }
    }

    pub fn send(&mut self, command: Command) {
        if let Err(error) = self.connection.commands.try_send(command) {
            self.notice = Some(format!("Input not sent: {error}"));
        }
    }

    pub fn visible_rows(&self, cell: (f32, f32)) -> usize {
        (self.rect[3] / cell.1).floor().max(1.) as usize
    }

    pub fn display_row(&self, cell: (f32, f32)) -> usize {
        self.frame
            .as_ref()
            .map_or(0, |f| f.display_row_start(self.visible_rows(cell)))
    }

    pub fn hidden_rows(&self, cell: (f32, f32)) -> usize {
        if self.rect[3] <= 0. {
            return 0;
        }
        self.frame.as_ref().map_or(0, |f| {
            (f.rows as usize).saturating_sub(self.visible_rows(cell))
        })
    }

    /// A non-owner never resizes; an owner sends its grid size whenever it changes.
    pub fn resize_if_owner(&mut self, font: f32, family: Option<&'static str>) {
        if self.rect[2] <= 0. || self.rect[3] <= 0. {
            return;
        }
        let Metrics { cell, .. } = metrics(font, family);
        let status = self.status();
        let cols = (self.rect[2] / cell.0).floor().clamp(1., 65535.) as u16;
        let rows = (self.rect[3] / cell.1).floor().clamp(1., 65535.) as u16;
        if !status.owner || !status.live {
            self.last_size = None;
        } else if self.last_size != Some((cols, rows)) {
            self.last_size = Some((cols, rows));
            self.send(Command::Resize(
                cols,
                rows,
                cell.0.ceil() as u16,
                cell.1.ceil() as u16,
            ));
        }
    }

    /// The cell index under a point in the grid's own coordinates.
    pub fn index(&self, x: f32, y: f32, cell: (f32, f32)) -> usize {
        let cols = self.frame.as_ref().map_or(1, |f| f.cols) as usize;
        let rows = self.frame.as_ref().map_or(1, |f| f.rows) as usize;
        let col = (x / cell.0).max(0.) as usize;
        let row = (y / cell.1).max(0.) as usize;
        (row + self.display_row(cell)).min(rows - 1) * cols + col.min(cols - 1)
    }

    /// The program has asked for mouse reports.
    pub fn tracking(&self) -> bool {
        self.frame.as_ref().is_some_and(|f| f.mouse_tracking)
    }

    /// The server's cell (column, row) under a point in the grid's own coordinates, clamped
    /// to the grid, so a drag past the edge reports the edge cell.
    pub fn cell_at(&self, x: f32, y: f32, cell: (f32, f32)) -> (u16, u16) {
        let cols = self.frame.as_ref().map_or(1, |f| f.cols).max(1);
        let rows = self.frame.as_ref().map_or(1, |f| f.rows).max(1);
        let col = (x / cell.0).floor().max(0.) as usize;
        let row = (y / cell.1).floor().max(0.) as usize + self.display_row(cell);
        (
            col.min(cols as usize - 1) as u16,
            row.min(rows as usize - 1) as u16,
        )
    }

    /// One mouse report. Shift never goes along: it is the local-selection override, and a
    /// Shift press leaves the server's button reporting stuck for the block.
    pub fn send_mouse(&mut self, action: &str, button: Option<&str>, at: (u16, u16), mods: Mods) {
        let mut payload = json!({
            "action": action,
            "x": at.0,
            "y": at.1,
            "mods": keys::mods(Mods { shift: false, ..mods }),
        });
        if let Some(button) = button {
            payload["button"] = json!(button);
        }
        self.mouse_cell = Some(at);
        self.send(Command::Mouse(payload));
    }

    /// A wheel turn of whole lines at a cell: negative scrolls up, positive down.
    pub fn send_wheel(&mut self, at: (u16, u16), lines: i32) {
        let step = lines.signum();
        for _ in 0..lines.unsigned_abs().min(8) {
            self.send(Command::Wheel(
                json!({"x": at.0, "y": at.1, "delta_x": 0, "delta_y": step}),
            ));
        }
    }

    pub fn copy_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        Some(self.frame.as_ref()?.selected_text(a, b))
    }
}

/// How the cursor draws: owning the keyboard (filled), visible but not (hollow), or not at all.
#[derive(Clone, Copy, Hash, PartialEq)]
pub enum CursorLook {
    Block,
    Hollow,
}

fn hash_cell(cell: &Cell, h: &mut DefaultHasher) {
    cell.text.hash(h);
    (cell.fg, cell.bg, cell.spacer_tail).hash(h);
    let s = &cell.style;
    (
        s.bold,
        s.italic,
        s.faint,
        s.strikethrough,
        s.underline != ghostty_vt::style::Underline::None,
    )
        .hash(h);
}

#[derive(Clone, Copy, PartialEq)]
struct RunStyle {
    fg: u32,
    bold: bool,
    italic: bool,
    faint: bool,
    underline: bool,
    strike: bool,
}
impl RunStyle {
    /// `default_bg` is the server's default background, which draws as the card colour
    /// wherever it shows (inverse video's foreground included).
    fn of(cell: &Cell, default_bg: u32) -> Self {
        let s = &cell.style;
        let tint = |c: u32| if c == default_bg { CARD } else { c };
        Self {
            // Faint text is pre-mixed toward its cell in sRGB.
            fg: if s.faint {
                mix(tint(cell.fg), tint(cell.bg), 0.65)
            } else {
                tint(cell.fg)
            },
            bold: s.bold,
            italic: s.italic,
            faint: s.faint,
            underline: s.underline != ghostty_vt::style::Underline::None,
            strike: s.strikethrough,
        }
    }
}

fn glyph_run(content: String, style: RunStyle, x: f32, y: f32, m: &Metrics) -> El {
    let font = face(m, style.bold, style.italic);
    let color = rgb(style.fg);
    let len = content.len();
    let mut e = if style.underline {
        rich(
            content,
            vec![Span {
                underline: Some(color),
                ..Span::new(0, len)
            }],
        )
    } else {
        text(content)
    };
    if style.strike {
        e = e.strike(0, len, Some(color));
    }
    e.no_wrap().font(font).color(color).absolute(x, y)
}

/// (character, bold, italic, size bits, family) → advances exactly one cell.
type FitCache = std::collections::HashMap<(char, bool, bool, u32, &'static str), bool>;

thread_local! {
    /// Whether a character, in a given face, advances exactly one cell: only those join
    /// multi-character segments, so nothing after a fallback or wider glyph drifts.
    static FITS: std::cell::RefCell<FitCache> = Default::default();
}

fn face(m: &Metrics, bold: bool, italic: bool) -> TextStyle {
    let mut font = m.style;
    if bold {
        font.weight = 700.;
    }
    if italic {
        font = font.italic();
    }
    font
}

fn fits_cell(ch: char, bold: bool, italic: bool, m: &Metrics) -> bool {
    if ch.is_ascii_graphic() || ch == ' ' {
        return true;
    }
    let family = match m.style.family {
        Family::Named(name) => name,
        _ => "",
    };
    let key = (ch, bold, italic, m.style.size.to_bits(), family);
    FITS.with_borrow_mut(|cache| {
        *cache.entry(key).or_insert_with(|| {
            let advance = rune::text::advance(&ch.to_string(), face(m, bold, italic));
            (advance - m.cell.0).abs() < 0.01
        })
    })
}

/// One text element for a run of single-cell characters starting at column `col`, with a span
/// per style change, so colours and weights never add elements.
struct Segment {
    col: usize,
    text: String,
    spans: Vec<Span>,
    strikes: Vec<(usize, usize, u32)>,
    style: Option<RunStyle>,
    start: usize,
}

impl Segment {
    fn new(col: usize) -> Self {
        Self {
            col,
            text: String::new(),
            spans: Vec::new(),
            strikes: Vec::new(),
            style: None,
            start: 0,
        }
    }
    fn close_span(&mut self) {
        if let Some(style) = self.style.take() {
            let end = self.text.len();
            if end > self.start {
                let color = rgb(style.fg);
                let mut span = Span::new(self.start, end);
                span.color = Some(color);
                if style.bold {
                    span.weight = Some(700.);
                }
                span.italic = style.italic;
                if style.underline {
                    span.underline = Some(color);
                }
                self.spans.push(span);
                if style.strike {
                    self.strikes.push((self.start, end, style.fg));
                }
            }
        }
    }
    fn push(&mut self, text: &str, style: RunStyle) {
        if self.style != Some(style) {
            self.close_span();
            self.style = Some(style);
            self.start = self.text.len();
        }
        self.text.push_str(text);
    }
    fn finish(mut self, m: &Metrics) -> Option<El> {
        self.close_span();
        // Trailing undecorated blanks draw nothing.
        while self.text.ends_with(' ')
            && self
                .spans
                .last()
                .is_some_and(|s| s.underline.is_none() && (s.end as usize) == self.text.len())
            && !self.strikes.iter().any(|&(_, e, _)| e == self.text.len())
        {
            self.text.pop();
            if let Some(last) = self.spans.last_mut() {
                last.end -= 1;
                if last.end <= last.start {
                    self.spans.pop();
                }
            }
        }
        if self.text.trim().is_empty()
            && self.strikes.is_empty()
            && !self.spans.iter().any(|s| s.underline.is_some())
        {
            return None;
        }
        let mut e = rich(self.text, self.spans)
            // The server owns line breaks. Fractional-DPI rounding must not wrap a run again.
            .no_wrap()
            .font(m.style)
            .absolute(self.col as f32 * m.cell.0, 0.);
        for (a, b, fg) in self.strikes {
            e = e.strike(a, b, Some(rgb(fg)));
        }
        Some(e)
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq)]
enum RowPass {
    All,
    Background,
    Text,
}

#[cfg(test)]
fn row_el(cells: &[Cell], y: f32, frame_bg: u32, sel: Option<(usize, usize)>, m: &Metrics) -> El {
    row_pass(cells, y, frame_bg, sel, m, RowPass::All)
}

fn row_pass(
    cells: &[Cell],
    y: f32,
    frame_bg: u32,
    sel: Option<(usize, usize)>,
    m: &Metrics,
    pass: RowPass,
) -> El {
    let (cw, ch) = m.cell;
    // Memoized rows are flattened on their own, so each says again that it never animates:
    // otherwise a glyph run that leaves a changed row fades out over its replacement.
    let mut row = el().w_full().h(ch).absolute(0., y).instant_changes();
    // Images below text belong between the background and glyph passes.
    if pass != RowPass::Text {
        let mut start = 0;
        while start < cells.len() {
            let bg = cells[start].bg;
            let mut end = start + 1;
            while end < cells.len() && cells[end].bg == bg {
                end += 1;
            }
            if bg != frame_bg {
                row = row.child(
                    el().absolute(start as f32 * cw, 0.)
                        .size((end - start) as f32 * cw, ch)
                        .fill(rgb(bg)),
                );
            }
            start = end;
        }
        if let Some((a, b)) = sel {
            row = row.child(
                el().absolute(a as f32 * cw, 0.)
                    .size((b + 1 - a) as f32 * cw, ch)
                    .fill(rgb(TERMINAL_SELECTION)),
            );
        }
    }
    if pass == RowPass::Background {
        return row;
    }
    let mut segment: Option<Segment> = None;
    for (col, cell) in cells.iter().enumerate() {
        if cell.spacer_tail || cell.text.starts_with('\u{10eeee}') {
            continue;
        }
        let style = RunStyle::of(cell, frame_bg);
        let text = if cell.text.is_empty() {
            " "
        } else {
            cell.text.as_str()
        };
        let mut chars = text.chars();
        let single = chars.next().filter(|_| chars.next().is_none());
        let wide = cells.get(col + 1).is_some_and(|c| c.spacer_tail);
        let fits = !wide && single.is_some_and(|c| fits_cell(c, style.bold, style.italic, m));
        if fits {
            if text == " " && segment.is_none() && !style.underline && !style.strike {
                continue;
            }
            segment
                .get_or_insert_with(|| Segment::new(col))
                .push(text, style);
        } else {
            if let Some(done) = segment.take().and_then(|s| s.finish(m)) {
                row = row.child(done);
            }
            row = row.child(glyph_run(text.to_owned(), style, col as f32 * cw, 0., m));
        }
    }
    if let Some(done) = segment.and_then(|s| s.finish(m)) {
        row = row.child(done);
    }
    row
}

fn image_el(
    image: &TermImage,
    texture: &rune::Image,
    frame: &Frame,
    m: &Metrics,
    start: usize,
) -> El {
    let p = &image.placement;
    let scale = [
        m.cell.0 / frame.cell_pixels[0].max(1) as f32,
        m.cell.1 / frame.cell_pixels[1].max(1) as f32,
    ];
    let width = p.size[0] as f32 * scale[0];
    let height = p.size[1] as f32 * scale[1];
    let sx = width / p.source[2] as f32;
    let sy = height / p.source[3] as f32;
    el().absolute(
        p.cell[0] as f32 * m.cell.0 + p.offset[0] as f32 * scale[0],
        (p.cell[1] as f32 - start as f32) * m.cell.1 + p.offset[1] as f32 * scale[1],
    )
    .size(width, height)
    .clip_rounded(0.)
    .instant_changes()
    .child(
        el().absolute(-(p.source[0] as f32) * sx, -(p.source[1] as f32) * sy)
            .size(
                image.pixels.width as f32 * sx,
                image.pixels.height as f32 * sy,
            )
            .pixels(texture.pixels()),
    )
}

pub struct GridLook {
    pub cursor: Option<CursorLook>,
    pub ime: bool,
}

/// The visible rows of `term`'s frame, each memoized by its content.
pub fn grid(key: &str, term: &Term, m: &Metrics, look: GridLook) -> El {
    let Some(frame) = &term.frame else {
        return el().w_full().h_full();
    };
    let (cw, ch) = m.cell;
    let visible = term.visible_rows(m.cell);
    let start = frame.display_row_start(visible);
    let cols = frame.cols as usize;
    let selection = term.selection.map(|(a, b)| (a.min(b), a.max(b)));
    let rows = |mut grid: El, pass: RowPass| {
        for (y, cells) in frame.cells.iter().enumerate().skip(start).take(visible) {
            let sel = selection.and_then(|(a, b)| {
                let (row0, row1) = (y * cols, y * cols + cols - 1);
                (a <= row1 && b >= row0).then(|| (a.max(row0) - row0, b.min(row1) - row0))
            });
            let mut h = DefaultHasher::new();
            for cell in cells {
                hash_cell(cell, &mut h);
            }
            (sel, frame.bg, cw.to_bits(), ch.to_bits(), m.style, pass).hash(&mut h);
            let top = (y - start) as f32 * ch;
            let id = format!("{key}/r{}:{pass:?}", y - start);
            grid = grid.child(memo(id, h.finish(), || {
                row_pass(cells, top, frame.bg, sel, m, pass)
            }));
        }
        grid
    };
    let images = |mut grid: El, layer: i32| {
        for (index, image) in frame
            .images
            .iter()
            .enumerate()
            .filter(|(_, image)| image.placement.layer == layer)
        {
            if let Some((_, texture)) = term.images.get(&(Arc::as_ptr(&image.pixels) as usize)) {
                grid = grid.child(
                    image_el(image, texture, frame, m, start).key(format!("{key}/image:{index}")),
                );
            }
        }
        grid
    };
    let mut grid = el().w_full().h_full().clip_rounded(0.).instant_changes();
    if frame.images.is_empty() {
        grid = rows(grid, RowPass::All);
    } else {
        grid = images(grid, 0);
        grid = rows(grid, RowPass::Background);
        grid = images(grid, 1);
        grid = rows(grid, RowPass::Text);
        grid = images(grid, 2);
    }
    if let Some((x, y)) = frame.cursor
        && let Some(cursor_look) = look.cursor
        && (y as usize) >= start
        && (y as usize) < start + visible
    {
        let (px, py) = (x as f32 * cw, (y as usize - start) as f32 * ch);
        let cell = frame.cells.get(y as usize).and_then(|r| r.get(x as usize));
        let color = rgb(frame.cursor_color);
        let mut cursor = el()
            .key(format!("{key}/cursor"))
            .absolute(px, py)
            .size(cw, ch);
        cursor = match cursor_look {
            CursorLook::Block => cursor.fill(color),
            CursorLook::Hollow => cursor.border(1., color),
        };
        if look.ime {
            cursor = cursor.ime_cursor();
        }
        grid = grid.child(cursor);
        if cursor_look == CursorLook::Block
            && let Some(cell) =
                cell.filter(|c| !c.text.is_empty() && !c.text.starts_with('\u{10eeee}'))
        {
            let style = RunStyle {
                fg: CARD,
                ..RunStyle::of(cell, frame.bg)
            };
            grid = grid.child(glyph_run(cell.text.clone(), style, px, py, m));
        }
        if !term.preedit.is_empty() {
            let len = term.preedit.len();
            grid = grid.child(
                rich(
                    term.preedit.clone(),
                    vec![Span {
                        underline: Some(rgb(frame.fg)),
                        ..Span::new(0, len)
                    }],
                )
                .no_wrap()
                .font(m.style)
                .color(rgb(frame.fg))
                .fill(rgb(CARD))
                .absolute(px, py),
            );
        }
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Runtime};

    struct RowState {
        cells: Vec<Cell>,
        family: Option<&'static str>,
    }
    fn view(s: &RowState, _: &mut Cx<RowState>) -> El {
        let m = metrics(15., s.family);
        el().w_full()
            .h_full()
            .child(row_el(&s.cells, 0., 0, None, &m))
    }

    #[test]
    fn typing_styled_terminal_runs_never_creates_a_second_ui_line() {
        let mut terminal = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
            cols: 253,
            rows: 4,
            ..Default::default()
        })
        .unwrap();
        let mut renderer = rex_linux::terminal::Renderer::new().unwrap();
        let fonts = crate::app::fonts::load("JetBrainsMono Nerd Font");
        let mut rt = Runtime::with_text(
            RowState {
                cells: vec![],
                family: fonts.terminal,
            },
            view,
            (2300., 100.),
            Env::new(true, 1.5),
            rune::text::TextSystem::new(fonts.files, fonts.families),
        );
        // Shell prompts mix fallback glyphs, styles and ASCII. Rebuild on every keystroke,
        // including runs preceded by blanks, at the user's fractional display scale.
        for n in 1..80 {
            terminal.vt_write(
                format!(
                    "\x1b[2J\x1b[H\u{f179} \u{e0b1} \x1b[36m\u{f015}~\x1b[0m   {}",
                    "a".repeat(n)
                )
                .as_bytes(),
            );
            let frame = renderer.frame(&terminal).unwrap();
            rt.update(|s, _| s.cells = frame.cells[0].clone());
            rt.frame(1. / 60.);
            for e in rt.elements().into_iter().filter(|e| e.text.is_some()) {
                assert!(
                    e.rect[3] <= 22.,
                    "keystroke {n}: {:?} wraps to {:?}",
                    e.text,
                    e.rect
                );
            }
            for q in rt.instances.iter().filter(|q| rune::compose::is_glyph(q)) {
                assert!(
                    q.rect[1] + q.rect[3] <= 33.,
                    "keystroke {n}: glyph drawn on the next line: {:?}",
                    q.rect
                );
            }
        }
    }
}
