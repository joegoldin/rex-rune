//! Client-local terminal defaults. Applications retain their explicit RGB/OSC colours.
use ghostty_vt::{Terminal, style::RgbColor};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    GruvboxDark,
    #[default]
    #[serde(other)]
    Midnight,
}

#[derive(Clone, Copy)]
pub struct Colors {
    pub ground: u32,
    pub card: u32,
    pub popover: u32,
    pub ink: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent2: u32,
    pub live: u32,
    pub warning: u32,
    pub danger: u32,
    pub selection: u32,
}

impl Theme {
    pub fn colors(self) -> Colors {
        match self {
            Self::Midnight => Colors {
                ground: 0x001528,
                card: 0x001528,
                popover: 0x0d1524,
                ink: 0xe8eef6,
                muted: 0x93a3b8,
                accent: 0x7cc4ff,
                accent2: 0xa78bfa,
                live: 0x5eead4,
                warning: 0xf5d48a,
                danger: 0xff8a8a,
                selection: 0x2f4a6e,
            },
            Self::GruvboxDark => Colors {
                ground: 0x282828,
                card: 0x282828,
                popover: 0x3c3836,
                ink: 0xebdbb2,
                muted: 0xa89984,
                accent: 0x83a598,
                accent2: 0xd3869b,
                live: 0xb8bb26,
                warning: 0xfabd2f,
                danger: 0xfb4934,
                selection: 0x504945,
            },
        }
    }

    /// Defaults, not forced cell colours: OSC 4/10/11 and explicit SGR still win.
    pub fn apply(self, terminal: &mut Terminal) -> anyhow::Result<()> {
        let c = self.colors();
        let rgb = |v: u32| RgbColor {
            r: (v >> 16) as u8,
            g: (v >> 8) as u8,
            b: v as u8,
        };
        terminal.set_default_fg_color(Some(rgb(c.ink)))?;
        terminal.set_default_bg_color(Some(rgb(c.card)))?;
        terminal.set_default_cursor_color(Some(rgb(c.ink)))?;
        terminal.set_default_color_palette(None)?;
        if self == Self::GruvboxDark {
            let mut palette = terminal.default_color_palette()?;
            for (slot, value) in palette.0.iter_mut().zip([
                0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
                0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
            ]) {
                *slot = rgb(value);
            }
            terminal.set_default_color_palette(Some(palette))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Renderer;

    #[test]
    fn themes_change_defaults_but_preserve_explicit_app_colours() {
        let mut terminal = Terminal::new(Default::default()).unwrap();
        terminal.vt_write(b"plain \x1b[31mred \x1b[38;2;1;2;3;48;2;4;5;6mRGB");
        Theme::GruvboxDark.apply(&mut terminal).unwrap();
        let mut renderer = Renderer::new().unwrap();
        let frame = renderer.frame(&terminal).unwrap();
        assert_eq!((frame.fg, frame.bg), (0xebdbb2, 0x282828));
        assert_eq!(frame.cells[0][0].fg, 0xebdbb2);
        assert_eq!(frame.cells[0][6].fg, 0xcc241d);
        assert_eq!(
            (frame.cells[0][10].fg, frame.cells[0][10].bg),
            (0x010203, 0x040506)
        );
        terminal.vt_write(b"\x1b]4;1;#112233\x07\x1b]10;#abcdef\x07\x1b]11;#123456\x07");
        Theme::Midnight.apply(&mut terminal).unwrap();
        let frame = renderer.frame(&terminal).unwrap();
        assert_eq!((frame.fg, frame.bg), (0xabcdef, 0x123456));
        assert_eq!(frame.cells[0][6].fg, 0x112233);
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::terminal::{Renderer, TerminalStream};

    #[test]
    fn theme_preferences_default_and_round_trip_without_changing_other_settings() {
        let settings: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.theme, Theme::Midnight);
        for theme in [Theme::Midnight, Theme::GruvboxDark] {
            let settings = crate::settings::Settings {
                theme,
                font_size: 19.,
                ..settings.clone()
            };
            let restored: crate::settings::Settings =
                serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
            assert_eq!(restored.theme, theme);
            assert_eq!(restored.font_size, 19.);
        }
        assert_eq!(
            serde_json::from_str::<Theme>("\"future_theme\"").unwrap(),
            Theme::Midnight
        );
    }

    #[test]
    fn selected_theme_survives_snapshot_and_server_theme_metadata() {
        let terminal = Terminal::new(Default::default()).unwrap();
        let mut bytes = ghostty_vt::snapshot::encode(&terminal).unwrap().to_vec();
        bytes.extend_from_slice(b"\x1b_rex1;42;{\"type\":\"theme\",\"foreground\":\"#ffffff\",\"background\":\"#000000\"}\x1b\\plain");
        let mut stream = TerminalStream::new(42);
        stream.set_theme(Theme::GruvboxDark).unwrap();
        stream.feed(&bytes).unwrap();
        let mut renderer = Renderer::new().unwrap();
        let frame = renderer.frame(stream.terminal.as_ref().unwrap()).unwrap();
        assert_eq!((frame.fg, frame.bg), (0xebdbb2, 0x282828));
        stream.set_theme(Theme::Midnight).unwrap();
        let frame = renderer.frame(stream.terminal.as_ref().unwrap()).unwrap();
        assert_eq!((frame.fg, frame.bg), (0xe8eef6, 0x001528));
    }
}
