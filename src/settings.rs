use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Host {
    pub label: String,
    pub endpoint: String,
    #[serde(default)]
    pub collapsed: bool,
    /// The shell new terminals on this host start (`zsh`, `/bin/zsh`); none uses the server
    /// user's login shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// SSH alias or user@host on the same machine/account as this Rex endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ssh_target: Option<String>,
}

/// The window corner in-app notices stack in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticePosition {
    #[default]
    TopRight,
    BottomRight,
}

/// What program activity shows in tabs and which changes become desktop notifications.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationsConfig {
    pub show_in_tabs: bool,
    pub system_notifications: bool,
    /// Notify when a background tab's programs move into each state.
    pub working: bool,
    pub blocked: bool,
    pub error: bool,
    pub done: bool,
    /// Pass on OSC 9 and OSC 777 notifications that programs send themselves.
    pub program_messages: bool,
}
impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            show_in_tabs: true,
            system_notifications: true,
            working: false,
            blocked: true,
            error: true,
            done: true,
            program_messages: true,
        }
    }
}

/// The palette the chrome is drawn in. A value this version doesn't know reads as Midnight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChromeTheme {
    RosePine,
    RosePineMoon,
    GruvboxDark,
    #[default]
    #[serde(other)]
    Midnight,
}
impl ChromeTheme {
    pub const ALL: [ChromeTheme; 4] = [
        ChromeTheme::Midnight,
        ChromeTheme::RosePine,
        ChromeTheme::RosePineMoon,
        ChromeTheme::GruvboxDark,
    ];
    pub fn name(self) -> &'static str {
        match self {
            ChromeTheme::Midnight => "Midnight",
            ChromeTheme::RosePine => "Rosé Pine",
            ChromeTheme::RosePineMoon => "Rosé Pine Moon",
            ChromeTheme::GruvboxDark => "Gruvbox Dark",
        }
    }
}

/// The terminal theme this client declares to the server, saved as `follow_server`,
/// `builtin_default`, `follow_chrome` or `named:<id>`. An id is a Ghostty theme's file name,
/// or a built-in theme's after `rex/`. A value this version doesn't know reads as Follow
/// server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum TerminalThemeChoice {
    /// Declare nothing: panes show whatever colours the server gives them.
    #[default]
    FollowServer,
    /// Declare Rex's built-in colours.
    RexDefault,
    /// Declare the built-in theme paired with the chrome palette, whose background is the
    /// chrome's ground.
    FollowChrome,
    Named(String),
}
impl From<String> for TerminalThemeChoice {
    fn from(value: String) -> Self {
        match value.as_str() {
            "builtin_default" => Self::RexDefault,
            "follow_chrome" => Self::FollowChrome,
            _ => match value.strip_prefix("named:") {
                Some(id) if !id.is_empty() => Self::Named(id.to_owned()),
                _ => Self::FollowServer,
            },
        }
    }
}
impl From<TerminalThemeChoice> for String {
    fn from(choice: TerminalThemeChoice) -> Self {
        match choice {
            TerminalThemeChoice::FollowServer => "follow_server".into(),
            TerminalThemeChoice::RexDefault => "builtin_default".into(),
            TerminalThemeChoice::FollowChrome => "follow_chrome".into(),
            TerminalThemeChoice::Named(id) => format!("named:{id}"),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub hosts: Vec<Host>,
    pub last_host: Option<String>,
    pub sessions: HashMap<String, String>,
    pub sidebar_width: f32,
    /// The sidebar was open when the app last saved, so it opens again at launch.
    pub sidebar_open: bool,
    pub vertical_tabs: bool,
    pub font_size: f32,
    pub font_family: String,
    pub font_fallbacks: Vec<String>,
    pub ui_scale: f32,
    pub notice_position: NoticePosition,
    pub notifications: NotificationsConfig,
    /// Passed to `rex skill setup` as `--session-naming`, always explicitly.
    pub agent_session_naming: bool,
    pub chrome_theme: ChromeTheme,
    pub terminal_theme: TerminalThemeChoice,
    /// The user picked a terminal theme themselves at least once, so choosing a chrome
    /// palette leaves it alone. Until then a palette brings its paired theme (Follow chrome).
    pub terminal_theme_chosen: bool,
    /// This client has declared a terminal theme to some server at least once. Rex keeps a
    /// client's last declaration per block across launches, so Follow server then resets
    /// eligible blocks to Rex's colours. Never cleared automatically.
    pub terminal_theme_declared: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            hosts: vec![Host {
                label: std::fs::read_to_string("/etc/hostname")
                    .unwrap_or_else(|_| "Local".into())
                    .trim()
                    .to_owned(),
                endpoint: crate::transport::local_endpoint(),
                collapsed: false,
                shell: None,
                image_ssh_target: None,
            }],
            last_host: None,
            sessions: HashMap::new(),
            sidebar_width: 236.,
            sidebar_open: false,
            vertical_tabs: false,
            font_size: 15.,
            font_family: "JetBrainsMono Nerd Font".into(),
            font_fallbacks: Vec::new(),
            ui_scale: 1.,
            notice_position: NoticePosition::default(),
            notifications: NotificationsConfig::default(),
            agent_session_naming: false,
            chrome_theme: ChromeTheme::default(),
            terminal_theme: TerminalThemeChoice::default(),
            terminal_theme_chosen: false,
            terminal_theme_declared: false,
        }
    }
}
impl Settings {
    fn config_dir() -> PathBuf {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            })
    }
    pub fn path() -> PathBuf {
        Self::config_dir().join("rex-linux/workspace.json")
    }
    pub fn load_default() -> Result<(Self, PathBuf)> {
        let path = Self::path();
        let settings = Self::load(&path)?;
        Ok((settings, path))
    }
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let mut settings: Self = serde_json::from_slice(&std::fs::read(path)?)
            .with_context(|| format!("Unable to read {}", path.display()))?;
        for host in &settings.hosts {
            validate_host(&host.label, &host.endpoint)?;
        }
        ensure!(
            !settings.hosts.is_empty(),
            "workspace needs at least one host"
        );
        (settings.font_family, settings.font_fallbacks) =
            normalize_font_preferences(&settings.font_family, &settings.font_fallbacks)?;
        settings.font_size = settings.font_size.clamp(8., 48.);
        settings.sidebar_width = settings.sidebar_width.clamp(200., 360.);
        settings.ui_scale = normalize_ui_scale(settings.ui_scale);
        Ok(settings)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }
}

pub fn parse_font_family(family: &str) -> std::result::Result<String, String> {
    let family = family.trim();
    if family.is_empty() || family.chars().any(|c| c.is_control() || c == ',') {
        return Err("Enter one font family name, without commas or control characters.".into());
    }
    Ok(family.to_owned())
}

pub fn normalize_font_preferences(
    family: &str,
    fallbacks: &[String],
) -> Result<(String, Vec<String>)> {
    let family = parse_font_family(family).map_err(anyhow::Error::msg)?;
    let mut names: Vec<String> = Vec::new();
    for fallback in fallbacks {
        let fallback = parse_font_family(fallback).map_err(anyhow::Error::msg)?;
        if !fallback.eq_ignore_ascii_case(&family)
            && !names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&fallback))
        {
            names.push(fallback);
        }
    }
    Ok((family, names))
}

pub fn normalize_ui_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        (scale * 10.).round().clamp(7., 15.) / 10.
    } else {
        1.
    }
}
pub fn validate_host(label: &str, endpoint: &str) -> Result<()> {
    ensure!(!label.trim().is_empty(), "Enter a host name");
    if let Some(path) = endpoint.strip_prefix("unix://") {
        ensure!(path.starts_with('/'), "Unix socket paths must be absolute");
    } else {
        let url =
            url::Url::parse(endpoint).context("Use an https://, http:// or unix:// endpoint")?;
        ensure!(
            matches!(url.scheme(), "https" | "http" | "ws" | "wss") && url.host_str().is_some(),
            "Use an https://, http:// or unix:// endpoint"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_preferences_survive_a_save_and_reload() {
        let json = serde_json::json!({
            "font_family": "TX02 Nerd Font Mono",
            "font_fallbacks": ["Noto Sans CJK JP", "Noto Color Emoji"]
        });
        let settings: Settings = serde_json::from_value(json.clone()).unwrap();
        let path =
            std::env::temp_dir().join(format!("rex-font-settings-{}.json", std::process::id()));
        settings.save(&path).unwrap();
        let saved = serde_json::to_value(Settings::load(&path).unwrap()).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(saved["font_family"], json["font_family"]);
        assert_eq!(saved["font_fallbacks"], json["font_fallbacks"]);
    }

    #[test]
    fn font_preferences_default_normalize_and_reject_invalid_names() {
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.font_family, "JetBrainsMono Nerd Font");
        assert!(old.font_fallbacks.is_empty());
        let (family, fallbacks) = normalize_font_preferences(
            "  IBM Plex Mono  ",
            &[
                "Noto Sans CJK JP".into(),
                " noto sans cjk jp ".into(),
                "IBM Plex Mono".into(),
            ],
        )
        .unwrap();
        assert_eq!(family, "IBM Plex Mono");
        assert_eq!(fallbacks, ["Noto Sans CJK JP"]);
        for invalid in ["", "   ", "bad\nname", "two,families"] {
            assert!(
                normalize_font_preferences(invalid, &[]).is_err(),
                "{invalid:?}"
            );
            assert!(normalize_font_preferences("IBM Plex Mono", &[invalid.into()]).is_err());
        }
        assert!(normalize_font_preferences("IBM Plex Mono", &[]).is_ok());
    }

    #[test]
    fn older_files_load_and_the_notice_corner_round_trips() {
        let old: Settings = serde_json::from_str(
            r#"{"hosts":[{"label":"a","endpoint":"unix:///run/rex.sock"}],"font_size":14}"#,
        )
        .unwrap();
        assert_eq!(old.notice_position, NoticePosition::TopRight);
        assert_eq!(old.notifications, NotificationsConfig::default());
        let partial: Settings =
            serde_json::from_str(r#"{"notifications":{"done":false}}"#).unwrap();
        assert!(!partial.notifications.done && partial.notifications.blocked);
        let mut s = old;
        s.notice_position = NoticePosition::BottomRight;
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["notice_position"], "bottom_right");
        let back: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(back.notice_position, NoticePosition::BottomRight);
    }

    #[test]
    fn the_terminal_theme_round_trips_and_unknown_values_follow_the_server() {
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.terminal_theme, TerminalThemeChoice::FollowServer);
        assert!(!old.terminal_theme_declared && !old.terminal_theme_chosen);
        for (choice, saved) in [
            (TerminalThemeChoice::FollowServer, "follow_server"),
            (TerminalThemeChoice::RexDefault, "builtin_default"),
            (TerminalThemeChoice::FollowChrome, "follow_chrome"),
            (
                TerminalThemeChoice::Named("rex/midnight".into()),
                "named:rex/midnight",
            ),
            (
                TerminalThemeChoice::Named("Catppuccin Mocha".into()),
                "named:Catppuccin Mocha",
            ),
        ] {
            let s = Settings {
                terminal_theme: choice.clone(),
                ..Settings::default()
            };
            let json = serde_json::to_value(&s).unwrap();
            assert_eq!(json["terminal_theme"], saved);
            let back: Settings = serde_json::from_value(json).unwrap();
            assert_eq!(back.terminal_theme, choice);
        }
        for unknown in ["follow_wallpaper", "named:", "Catppuccin Mocha"] {
            let s: Settings =
                serde_json::from_value(serde_json::json!({ "terminal_theme": unknown })).unwrap();
            assert_eq!(
                s.terminal_theme,
                TerminalThemeChoice::FollowServer,
                "{unknown}"
            );
        }
    }

    #[test]
    fn the_chrome_theme_round_trips_and_unknown_values_are_midnight() {
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.chrome_theme, ChromeTheme::Midnight);
        for (chrome, saved) in [
            (ChromeTheme::Midnight, "midnight"),
            (ChromeTheme::RosePine, "rose_pine"),
            (ChromeTheme::RosePineMoon, "rose_pine_moon"),
            (ChromeTheme::GruvboxDark, "gruvbox_dark"),
        ] {
            let s = Settings {
                chrome_theme: chrome,
                terminal_theme_chosen: true,
                ..Settings::default()
            };
            let json = serde_json::to_value(&s).unwrap();
            assert_eq!(json["chrome_theme"], saved);
            let back: Settings = serde_json::from_value(json).unwrap();
            assert_eq!(back.chrome_theme, chrome);
            assert!(back.terminal_theme_chosen);
        }
        let s: Settings =
            serde_json::from_value(serde_json::json!({ "chrome_theme": "solarized_light" }))
                .unwrap();
        assert_eq!(s.chrome_theme, ChromeTheme::Midnight);
    }
}
