//! Per-window colour tokens, themed kits, shared surfaces and icons.
use rune::{Color, El, Icon, Shadow, Spring, SpringPair, TextStyle, Transition};
use rune_kit::{Kit, PaletteFocus, PaletteSeed};
use std::sync::OnceLock;

/// sRGB `0xRRGGBB` → linear `Color`.
pub fn rgb(value: u32) -> Color {
    let channel = |shift: u32| rune::color::srgb_decode(((value >> shift) & 0xff) as f32 / 255.);
    Color {
        r: channel(16),
        g: channel(8),
        b: channel(0),
        a: 1.,
    }
}

/// `a` over `b` at opacity `t`, mixed in sRGB.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let channel = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xff) as f32, ((b >> shift) & 0xff) as f32);
        ((x * t + y * (1. - t)).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

pub fn white(a: f32) -> Color {
    Color::WHITE.alpha(a)
}
pub fn black(a: f32) -> Color {
    Color::BLACK.alpha(a)
}
pub fn shadow(x: f32, y: f32, blur: f32, color: Color) -> Shadow {
    Shadow {
        x,
        y,
        blur,
        spread: 0.,
        color,
        inset: false,
    }
}
pub fn inset(y: f32, color: Color) -> Shadow {
    Shadow {
        x: 0.,
        y,
        blur: 0.,
        spread: 0.,
        color,
        inset: true,
    }
}

thread_local! {
    // Rune builds each window synchronously on its UI thread. Set at every root render,
    // so separate windows and parallel headless runtimes do not share a theme.
    static CURRENT: std::cell::Cell<rex_linux::theme::Theme> = Default::default();
}

pub fn set_current(theme: rex_linux::theme::Theme) {
    CURRENT.set(theme);
}

pub fn ground_color() -> u32 {
    CURRENT.get().colors().ground
}
pub fn card_color() -> u32 {
    CURRENT.get().colors().card
}
pub fn popover_color() -> u32 {
    CURRENT.get().colors().popover
}
pub fn ink_color() -> u32 {
    CURRENT.get().colors().ink
}
pub fn muted_color() -> u32 {
    CURRENT.get().colors().muted
}
pub fn accent_color() -> u32 {
    CURRENT.get().colors().accent
}
pub fn accent2_color() -> u32 {
    CURRENT.get().colors().accent2
}
pub fn live_color() -> u32 {
    CURRENT.get().colors().live
}
pub fn warning_color() -> u32 {
    CURRENT.get().colors().warning
}
pub fn danger_color() -> u32 {
    CURRENT.get().colors().danger
}
pub fn terminal_selection_color() -> u32 {
    CURRENT.get().colors().selection
}

pub const TOOLBAR: f32 = 52.;
pub const GUTTER: f32 = 8.;
pub const MARGIN: f32 = 10.;
pub const CARD_RADIUS: f32 = 12.;
pub const CARD_HEADER: f32 = 30.;

/// Springs for chrome: quick and calm.
pub const SNAPPY: Spring = Spring {
    duration: 0.28,
    bounce: 0.0,
};
pub const POP: Spring = Spring {
    duration: 0.32,
    bounce: 0.12,
};
/// For things that must not move (terminal content).
pub const STILL: Spring = Spring {
    duration: 0.001,
    bounce: 0.,
};

/// A sliding highlight's own entrance and exit: none, so one highlight shows even when its
/// matched partner isn't mounted (Reduce Motion, a row scrolled in).
pub fn no_presence() -> Transition {
    Transition::identity().springs(SpringPair::new(STILL, STILL))
}

/// A popover's entrance from its anchor: a little scale and a fade on a gentle spring.
pub fn pop_in() -> Transition {
    Transition::scale(0.96).springs(SpringPair::new(POP, SNAPPY))
}

pub fn kit() -> &'static Kit {
    static MIDNIGHT: OnceLock<Kit> = OnceLock::new();
    static GRUVBOX: OnceLock<Kit> = OnceLock::new();
    let kit = match CURRENT.get() {
        rex_linux::theme::Theme::Midnight => &MIDNIGHT,
        rex_linux::theme::Theme::GruvboxDark => &GRUVBOX,
    };
    kit.get_or_init(|| {
        let mut k = Kit::from_palette_focus(
            PaletteSeed {
                bg: rgb(ground_color()),
                fg: rgb(ink_color()),
                accent: rgb(accent_color()),
            },
            PaletteFocus::Custom(rgb(accent_color())),
        );
        let c = &mut k.c;
        (c.page, c.bg_base, c.surface_base) = (
            rgb(ground_color()),
            rgb(ground_color()),
            rgb(ground_color()),
        );
        c.bg_deep = rgb(card_color());
        for slot in [
            &mut c.layer_01,
            &mut c.layer_02,
            &mut c.surface_raised,
            &mut c.surface_raised_base,
            &mut c.surface_float,
        ] {
            *slot = rgb(popover_color());
        }
        (c.hover, c.surface_hover, c.surface_raised_hover) =
            (white(0.07), white(0.07), white(0.07));
        c.pressed = white(0.11);
        (c.border_base, c.border_muted, c.border_weak) = (white(0.12), white(0.08), white(0.06));
        (c.border_strong, c.border_hover) = (white(0.2), white(0.2));
        c.border_focus = rgb(accent_color());
        (c.text_base, c.text_strong, c.icon_base) =
            (rgb(ink_color()), rgb(ink_color()), rgb(ink_color()));
        for slot in [
            &mut c.text_muted,
            &mut c.text_faint,
            &mut c.text_weak,
            &mut c.icon_muted,
            &mut c.icon_faint,
            &mut c.icon_weaker,
        ] {
            *slot = rgb(muted_color());
        }
        (c.text_accent, c.icon_accent) = (rgb(accent_color()), rgb(accent_color()));
        c.success.fg = rgb(live_color());
        c.warning.fg = rgb(warning_color());
        c.danger.fg = rgb(danger_color());
        k.radius.sm = 8.;
        k.radius.md = 10.;
        k
    })
}

pub fn ui(size: f32) -> TextStyle {
    TextStyle::new(size, 450.)
}
pub fn medium(size: f32) -> TextStyle {
    TextStyle::new(size, 560.)
}
pub fn strong(size: f32) -> TextStyle {
    TextStyle::new(size, 620.)
}
/// Small capitals for group labels.
pub fn label_caps() -> TextStyle {
    TextStyle::new(10.5, 650.).tracking(0.6)
}

/// A glassy pill every control in the bar sits on.
pub fn chip(e: El) -> El {
    e.fill(white(0.055))
        .border_gradient(1., white(0.13), white(0.04))
        .radius(10.)
        .continuous_corners()
}

/// A popover or dialog surface: frosted navy with a lit rim and a deep shadow.
pub fn glass(e: El, radius: f32, blur: bool) -> El {
    let e = e
        .fill(rgb(popover_color()).alpha(0.95))
        .border_gradient(1., white(0.16), white(0.05))
        .radius(radius)
        .continuous_corners()
        .box_shadow(shadow(0., 24., 60., black(0.6)))
        .box_shadow(shadow(0., 2., 6., black(0.35)));
    if blur { e.backdrop_blur(22.) } else { e }
}

macro_rules! icon {
    ($name:ident, $id:literal, $($el:literal),+) => {
        pub static $name: Icon = Icon {
            name: $id,
            view: 20.0,
            body: concat!($("<", $el, r#" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#),+),
        };
    };
}
icon!(
    TERMINAL,
    "rex-terminal",
    r#"path d="m4 6 4 4-4 4""#,
    r#"path d="M10.5 14.5h5.5""#
);
icon!(CHEVRON, "rex-chevron", r#"path d="m7 8.5 3 3 3-3""#);
icon!(RIGHT, "rex-right", r#"path d="m8.5 6.5 3.5 3.5-3.5 3.5""#);
icon!(LEFT, "rex-left", r#"path d="m11.5 6.5-3.5 3.5 3.5 3.5""#);
icon!(CHECK, "rex-check", r#"path d="m5.5 10.5 3 3 6-7""#);
icon!(
    PLUS,
    "rex-plus",
    r#"path d="M10 4.5v11""#,
    r#"path d="M4.5 10h11""#
);
icon!(
    CLOSE,
    "rex-close",
    r#"path d="m6.5 6.5 7 7""#,
    r#"path d="m13.5 6.5-7 7""#
);
icon!(
    PENCIL,
    "rex-pencil",
    r#"path d="M12.5 5.5 14.5 7.5 8 14l-2.75.75L6 12z""#
);
icon!(
    SEARCH,
    "rex-search",
    r#"circle cx="9" cy="9" r="5""#,
    r#"path d="m13 13 3.5 3.5""#
);
icon!(
    COMMAND,
    "rex-command",
    r#"path d="M12.5 5v10a2.5 2.5 0 1 0 2.5-2.5H5a2.5 2.5 0 1 0 2.5 2.5V5a2.5 2.5 0 1 0-2.5 2.5h10a2.5 2.5 0 1 0-2.5-2.5""#
);
icon!(
    SETTINGS,
    "rex-settings",
    r#"path d="M4 6.5h5M13 6.5h3""#,
    r#"circle cx="11" cy="6.5" r="2""#,
    r#"path d="M4 13.5h3M11 13.5h5""#,
    r#"circle cx="9" cy="13.5" r="2""#
);
icon!(
    MONITOR,
    "rex-monitor",
    r#"rect x="3" y="4" width="14" height="9.5" rx="2""#,
    r#"path d="M10 13.5v3""#,
    r#"path d="M7 16.5h6""#
);
icon!(
    SERVER,
    "rex-server",
    r#"rect x="3.5" y="4" width="13" height="5" rx="1.5""#,
    r#"rect x="3.5" y="11" width="13" height="5" rx="1.5""#,
    r#"path d="M6.5 6.5h.01""#,
    r#"path d="M6.5 13.5h.01""#
);
icon!(
    SIDEBAR,
    "rex-sidebar",
    r#"rect x="3" y="4" width="14" height="12" rx="3""#,
    r#"path d="M8 4v12""#
);
icon!(MINUS, "rex-minus", r#"path d="M6 10h8""#);

// Programs a tab can show instead of the terminal glyph (`activity::program_icon`).
icon!(
    CLAUDE,
    "rex-claude",
    r#"path d="M10 3.5v13""#,
    r#"path d="M3.5 10h13""#,
    r#"path d="m5.4 5.4 9.2 9.2""#,
    r#"path d="m14.6 5.4-9.2 9.2""#
);
icon!(
    CODEX,
    "rex-codex",
    r#"path d="M10 2.75 16.25 6.4v7.2L10 17.25 3.75 13.6V6.4Z""#,
    r#"path d="m8 8 2 2-2 2""#,
    r#"path d="M11 12.5h1.5""#
);
icon!(
    VIM,
    "rex-vim",
    r#"path d="m5 5 5 10.5L15 5""#,
    r#"path d="M3.5 5h4""#,
    r#"path d="M12.5 5h4""#
);
icon!(
    GIT,
    "rex-git",
    r#"circle cx="6.5" cy="5" r="1.75""#,
    r#"circle cx="6.5" cy="15" r="1.75""#,
    r#"circle cx="13.5" cy="7" r="1.75""#,
    r#"path d="M6.5 6.75v6.5""#,
    r#"path d="M13.5 8.75c0 3-7 2.25-7 4.5""#
);
icon!(
    CRATE,
    "rex-crate",
    r#"path d="M10 3 16.25 6.5v7L10 17l-6.25-3.5v-7Z""#,
    r#"path d="M3.75 6.5 10 10l6.25-3.5""#,
    r#"path d="M10 10v7""#
);
icon!(
    KEY,
    "rex-key",
    r#"circle cx="7" cy="13" r="3""#,
    r#"path d="m9.2 10.8 6.3-6.3""#,
    r#"path d="m13.5 6.5 2 2""#
);

/// Solid marks drawn in `currentColor`, for logos that outlines make unrecognisable.
macro_rules! mark {
    ($name:ident, $id:literal, $body:literal) => {
        pub static $name: Icon = Icon {
            name: $id,
            view: 20.0,
            body: $body,
        };
    };
}
// Pi: a rounded tile with a block P and dot cut out of it.
mark!(
    PI,
    "rex-pi",
    r#"<path fill="currentColor" fill-rule="evenodd" d="M5.5 2.5h9a3 3 0 0 1 3 3v9a3 3 0 0 1-3 3h-9a3 3 0 0 1-3-3v-9a3 3 0 0 1 3-3ZM6 5.5h7v5.5H9v3.5H6ZM9 7.5h1.75v1.5H9ZM13 12h2v2.5h-2Z"/>"#
);
// OpenCode: a heavy block O whose lower counter is shaded.
mark!(
    OPENCODE,
    "rex-opencode",
    r#"<path fill="currentColor" fill-rule="evenodd" d="M4 2.5h12v15H4ZM7.5 6v8h5V6Z"/><path fill="currentColor" fill-opacity=".45" d="M7.5 10h5v4h-5Z"/>"#
);
