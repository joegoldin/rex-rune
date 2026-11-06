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

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub hosts: Vec<Host>,
    pub last_host: Option<String>,
    pub sessions: HashMap<String, String>,
    pub sidebar_width: f32,
    pub theme: crate::theme::Theme,
    pub font_size: f32,
    pub ui_scale: f32,
    pub notice_position: NoticePosition,
    pub notifications: NotificationsConfig,
    /// Passed to `rex skill setup` as `--session-naming`, always explicitly.
    pub agent_session_naming: bool,
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
            }],
            last_host: None,
            sessions: HashMap::new(),
            sidebar_width: 236.,
            theme: crate::theme::Theme::default(),
            font_size: 15.,
            ui_scale: 1.,
            notice_position: NoticePosition::default(),
            notifications: NotificationsConfig::default(),
            agent_session_naming: false,
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
}
