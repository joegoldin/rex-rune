//! Clipboard reads and image transfers stay bound to the pane that requested them.
use super::workspace::{HostId, Workspace};
use anyhow::{Context, Result, bail};
use rex_linux::{connection::Command, image_paste};
use rune::Tx;

#[derive(Clone)]
struct Target {
    host: HostId,
    session: String,
    block: String,
    generation: u64,
    endpoint: String,
    ssh: Option<String>,
}
impl Target {
    fn capture(s: &Workspace) -> Option<Self> {
        if !s.window_focused || s.panel.is_some() {
            return None;
        }
        let term = s.active_terminal()?;
        if !term.status().live {
            return None;
        }
        let entry = &s.settings.hosts[s.active];
        Some(Self {
            host: s.hosts[s.active].id,
            session: s.model().selected.clone()?,
            block: s.focused_block()?,
            generation: term.generation(),
            endpoint: entry.endpoint.clone(),
            ssh: entry.image_ssh_target.clone(),
        })
    }
    fn current(&self, s: &Workspace) -> bool {
        Self::capture(s).is_some_and(|now| {
            now.host == self.host
                && now.session == self.session
                && now.block == self.block
                && now.generation == self.generation
                && now.endpoint == self.endpoint
                && now.ssh == self.ssh
        })
    }
}

fn image_file(bytes: &[u8], endpoint: &str, ssh: Option<&str>) -> Result<std::path::PathBuf> {
    if let Some(ssh) = ssh {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Could not start image transfer")?;
        runtime.block_on(image_paste::upload_ssh(bytes, ssh))
    } else if endpoint.starts_with("unix://") {
        image_paste::write_local(bytes, &std::env::temp_dir())
    } else {
        bail!(
            "Configure this host's SSH image upload target in Remote Hosts; a local file is not accessible remotely"
        )
    }
}

impl Workspace {
    pub fn paste_clipboard(&mut self, tx: &mut Tx) {
        if self.paste_pending {
            self.inform("An image paste is already in progress.");
            return;
        }
        let Some(target) = Target::capture(self) else {
            return;
        };
        self.paste_pending = true;
        tx.read_clipboard(move |s: &mut Workspace, content, _| {
            if !target.current(s) {
                s.paste_pending = false;
                return;
            }
            let Some(image) = content.image().cloned() else {
                s.paste_pending = false;
                if !content.text().is_empty()
                    && let Some(term) = s.active_terminal_mut()
                {
                    term.clear_selection();
                    term.send(Command::Paste(content.text().to_owned()));
                }
                return;
            };
            let handle = s.handle.clone();
            let result = std::thread::Builder::new()
                .name("rex-image-paste".into())
                .spawn(move || {
                    let result = image_file(image.png(), &target.endpoint, target.ssh.as_deref());
                    handle.update(move |s, _| {
                        s.paste_pending = false;
                        if !target.current(s) {
                            s.inform("Image paste cancelled because the target changed.");
                            return;
                        }
                        let text = result.and_then(|path| {
                            image_paste::quote_path(
                                path.to_str().context("Image path is not UTF-8")?,
                            )
                        });
                        match text {
                            Ok(text) => {
                                if let Some(term) = s.active_terminal_mut() {
                                    term.clear_selection();
                                    term.send(Command::Paste(text));
                                }
                            }
                            Err(error) => s.inform(format!("Image paste failed: {error}")),
                        }
                    });
                });
            if let Err(error) = result {
                s.paste_pending = false;
                s.inform(format!("Image paste failed: {error}"));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn remote_images_without_a_target_never_use_client_local_files() {
        for endpoint in ["http://host", "https://host", "ws://host", "wss://host"] {
            let error = super::image_file(b"unused", endpoint, None).unwrap_err();
            assert!(
                error.to_string().contains("SSH image upload target"),
                "{error}"
            );
        }
    }
}
