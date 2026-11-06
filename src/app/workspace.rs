//! Workspace state and behavior: hosts, sessions, tabs, panes, popups and preferences.
//! Rendering lives in the sibling view modules; nothing here builds elements.
use super::{
    activity::TabStates,
    notify::{Note, Notifier},
    panel::*,
    term::Term,
    widgets::ScrollFx,
};
use rex_linux::{
    connection::{Command, DEFAULT_TAB_LABEL, HostCommand, HostConnection, HostModel, Options},
    layout::{Divider, shell_layout},
    settings::{Host, NoticePosition, Settings, validate_host},
};
use rune::{AppHandle, PhysicalKeyCode, TextEdit, Tx};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

/// A host's identity for this run. Indices shift when a host is removed; ids never repeat,
/// so a late wake or a stored terminal can't land on another host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HostId(pub u64);

pub struct HostState {
    pub id: HostId,
    pub connection: HostConnection,
    pub model: HostModel,
}

pub struct DividerDrag {
    pub host: usize,
    pub session: String,
    pub divider: Divider,
    pub start: f64,
    pub ratio: f64,
}

pub struct Notice {
    pub id: u64,
    pub text: String,
    /// Errors stay until closed; information leaves after six seconds.
    pub sticky: bool,
    pub at: std::time::Instant,
}

pub struct Workspace {
    pub logo: Option<rune::Image>,
    pub options: Options,
    pub settings: Settings,
    pub settings_path: PathBuf,
    pub hosts: Vec<HostState>,
    pub active: usize,
    pub terminals: HashMap<(HostId, String), Term>,
    pub panel: Option<Panel>,
    pub notices: Vec<Notice>,
    next_notice: u64,
    zoom_notice: Option<u64>,
    pub drag: Option<DividerDrag>,
    /// Physical keys whose press went to a terminal; only their releases follow.
    pub forwarded: HashSet<PhysicalKeyCode>,
    /// Panes this window has taken size ownership of since it last came forward.
    pub claimed: HashSet<(HostId, String)>,
    pub handle: AppHandle<Workspace>,
    pub window_focused: bool,
    /// The pane area in window points, for divider drags and target outlines.
    pub area: [f32; 4],
    pub viewport: [f32; 2],
    pub sidebar: rune_kit::PaneState,
    pub terminal_family: Option<&'static str>,
    pub title: String,
    /// (active tab, pane width, sidebar open) when the active tab was last revealed.
    pub last_tab: (Option<String>, i32, bool),
    pub reveal_tab_again: bool,
    pub tab_strip: TabStrip,
    pub tab_pos: rune::Live<[f32; 2]>,
    pub tab_fade: [rune::Live<f32>; 2],
    pub sidebar_fx: ScrollFx,
    pub switcher_fx: ScrollFx,
    pub sidebar_drag_start: Option<f32>,
    scratch: TextEdit,
    next_host: u64,
    pub settings_section: SettingsSection,
    /// Each tab's folded activity state when last synced.
    pub tab_states: TabStates,
    /// Tabs whose programs finished while the user wasn't looking; they show "Done".
    pub unseen_done: HashSet<(HostId, String)>,
    pub activity_epochs: HashMap<HostId, u64>,
    pub notifier: Option<Notifier>,
    /// Notifications headless tests would have sent.
    pub outbox: Vec<Note>,
    pub integrations: super::integrations::Integrations,
}

#[derive(Default)]
pub struct TabStrip {
    pub viewport: f32,
    pub content: f32,
}
impl TabStrip {
    pub fn overflows(&self) -> bool {
        self.content > self.viewport + 1.
    }
}

impl Workspace {
    pub fn new(
        options: Options,
        mut settings: Settings,
        path: PathBuf,
        handle: AppHandle<Workspace>,
        terminal_family: Option<&'static str>,
    ) -> Self {
        let explicit = options.session.is_some()
            || options.new.is_some()
            || options.endpoint != rex_linux::transport::local_endpoint();
        if !settings
            .hosts
            .iter()
            .any(|h| h.endpoint == options.endpoint)
        {
            settings.hosts.push(Host {
                label: options.endpoint.trim_start_matches("https://").into(),
                endpoint: options.endpoint.clone(),
                collapsed: false,
                shell: None,
            });
        }
        let wanted = if explicit {
            Some(&options.endpoint)
        } else {
            settings.last_host.as_ref()
        };
        let active = wanted
            .and_then(|e| settings.hosts.iter().position(|h| &h.endpoint == e))
            .unwrap_or(0);
        if options.font_size != 15. {
            settings.font_size = options.font_size;
        }
        // Every launch starts with the sidebar collapsed; only its width is remembered.
        let sidebar = rune_kit::PaneState::new(236.)
            .range(200., 360.)
            .with_width(settings.sidebar_width)
            .collapsed();
        let mut this = Self {
            logo: None,
            options,
            settings,
            settings_path: path,
            hosts: Vec::new(),
            active,
            terminals: HashMap::new(),
            panel: None,
            notices: Vec::new(),
            next_notice: 0,
            zoom_notice: None,
            drag: None,
            forwarded: HashSet::new(),
            claimed: HashSet::new(),
            handle,
            window_focused: true,
            area: [0.; 4],
            viewport: [1440., 900.],
            sidebar,
            terminal_family,
            title: "Rex".into(),
            last_tab: (None, 0, false),
            reveal_tab_again: false,
            tab_strip: TabStrip::default(),
            tab_pos: rune::Live::new([0.; 2]),
            tab_fade: [rune::Live::new(0.4), rune::Live::new(1.)],
            sidebar_fx: ScrollFx::default(),
            switcher_fx: ScrollFx::default(),
            sidebar_drag_start: None,
            scratch: TextEdit::new(""),
            next_host: 0,
            settings_section: SettingsSection::default(),
            tab_states: HashMap::new(),
            unseen_done: HashSet::new(),
            activity_epochs: HashMap::new(),
            notifier: None,
            outbox: Vec::new(),
            integrations: Default::default(),
        };
        for index in 0..this.settings.hosts.len() {
            this.connect_host(index, explicit);
        }
        this
    }

    fn connect_host(&mut self, index: usize, explicit: bool) {
        let state = self.spawn_host(index, explicit);
        self.hosts.push(state);
    }

    /// A connection to saved host `index` under a fresh id, with its wake thread.
    fn spawn_host(&mut self, index: usize, explicit: bool) -> HostState {
        let host = &self.settings.hosts[index];
        let cli = explicit && index == self.active;
        let preferred = if cli {
            self.options.session.clone()
        } else {
            self.settings.sessions.get(&host.endpoint).cloned()
        };
        let connection = HostConnection::spawn(
            host.endpoint.clone(),
            preferred,
            cli.then(|| self.options.new.clone())
                .flatten()
                .map(|label| (label, host.shell.clone())),
            cli.then(|| self.options.block.clone()).flatten(),
            index == self.active,
        );
        let id = HostId(self.next_host);
        self.next_host += 1;
        let receiver = connection.wake.clone();
        let handle = self.handle.clone();
        std::thread::Builder::new()
            .name(format!("rex-host-{}", id.0))
            .spawn(move || {
                while receiver.recv().is_ok() {
                    handle.update(move |s, tx| s.sync_host(id, tx));
                }
            })
            .expect("host wake thread");
        let model = connection.model.lock().expect("host model").clone();
        HostState {
            id,
            connection,
            model,
        }
    }

    /// Forget a host's terminals and claims; dropping its `HostState` shuts its worker down.
    fn forget_host(&mut self, state: HostState) {
        let id = state.id;
        self.terminals.retain(|(host, _), _| *host != id);
        self.claimed.retain(|(host, _)| *host != id);
        self.tab_states.retain(|(host, _), _| *host != id);
        self.unseen_done.retain(|(host, _)| *host != id);
        self.activity_epochs.remove(&id);
    }

    /// Stop connecting to saved host `index` and drop it from the workspace. Its server, its
    /// sessions and their programs are untouched. The last host can't be removed.
    pub fn remove_host(&mut self, index: usize, tx: &mut Tx) {
        if self.settings.hosts.len() <= 1 || index >= self.hosts.len() {
            return;
        }
        let state = self.hosts.remove(index);
        self.forget_host(state);
        let host = self.settings.hosts.remove(index);
        self.settings.sessions.remove(&host.endpoint);
        // Menus and drags name hosts by position, which just shifted.
        self.panel = None;
        self.drag = None;
        if self.active == index {
            self.active = 0;
        } else if self.active > index {
            self.active -= 1;
        }
        self.update_visibility();
        self.save();
        self.update_title(tx);
        self.inform(format!(
            "Removed {}. Its sessions keep running there.",
            host.label
        ));
    }

    /// Rename a host or change its default shell in place. A new endpoint is another server:
    /// the old connection is dropped and the host reconnects under a new id.
    pub fn edit_host(
        &mut self,
        index: usize,
        label: String,
        endpoint: String,
        shell: Option<String>,
    ) {
        let host = &mut self.settings.hosts[index];
        host.label = label;
        host.shell = shell;
        if host.endpoint != endpoint {
            let old = std::mem::replace(&mut host.endpoint, endpoint);
            self.settings.sessions.remove(&old);
            let state = self.spawn_host(index, false);
            let old = std::mem::replace(&mut self.hosts[index], state);
            self.forget_host(old);
            if self.active == index {
                self.drag = None;
            }
            self.update_visibility();
        }
        self.save();
    }

    pub fn host_index(&self, id: HostId) -> Option<usize> {
        self.hosts.iter().position(|h| h.id == id)
    }

    /// The key of `block`'s terminal on the active host.
    pub fn term_key(&self, block: &str) -> (HostId, String) {
        (self.hosts[self.active].id, block.to_owned())
    }

    /// A host worker's wake: nothing when its host has been removed since.
    pub fn sync_host(&mut self, id: HostId, tx: &mut Tx) {
        if let Some(index) = self.host_index(id) {
            self.sync(index, tx);
        }
    }

    pub fn sync(&mut self, index: usize, tx: &mut Tx) {
        let id = self.hosts[index].id;
        let old_session = self.hosts[index].model.selected.clone();
        let (model, notices) = {
            let mut shared = self.hosts[index]
                .connection
                .model
                .lock()
                .expect("host model");
            let notices = std::mem::take(&mut shared.notices);
            (shared.clone(), notices)
        };
        for (block, connection) in &model.terminals {
            let key = (id, block.clone());
            if !self.terminals.contains_key(&key) {
                let receiver = connection.wake.clone();
                let handle = self.handle.clone();
                let wake_key = key.clone();
                std::thread::Builder::new()
                    .name("rex-terminal".into())
                    .spawn(move || {
                        while receiver.recv().is_ok() {
                            let key = wake_key.clone();
                            handle.update(move |s, _| s.refresh_terminal(&key));
                        }
                    })
                    .expect("terminal wake thread");
                self.terminals.insert(key, Term::new(connection.clone()));
            }
        }
        self.terminals
            .retain(|(host, block), _| *host != id || model.terminals.contains_key(block));
        let changed_session = self
            .settings
            .sessions
            .get(&self.settings.hosts[index].endpoint)
            != model.selected.as_ref();
        if let Some(selected) = &model.selected {
            self.settings.sessions.insert(
                self.settings.hosts[index].endpoint.clone(),
                selected.clone(),
            );
        }
        self.hosts[index].model = model;
        self.track_activity(index);
        self.deliver_program_notices(index, notices);
        if self
            .panel
            .as_ref()
            .and_then(|p| p.target.as_ref())
            .is_some_and(|t| t.host == index && !self.target_exists(t))
        {
            self.close_panel(tx);
            self.inform("That pane is no longer available.");
        }
        if index == self.active && old_session != self.hosts[index].model.selected {
            self.drag = None;
        }
        if changed_session && self.hosts[index].model.selected.is_some() {
            self.save();
        }
        self.claim_visible();
        let (font, family) = (self.settings.font_size, self.terminal_family);
        for ((host, _), term) in self.terminals.iter_mut() {
            if *host == id {
                term.resize_if_owner(font, family);
            }
        }
        self.update_title(tx);
    }

    /// Take size ownership of the panes on screen, as the native Rex apps and multiplexers
    /// do: once per pane while this window has focus, and again each time it comes forward.
    /// Another client taking a pane back is left alone until then (or until input here).
    pub fn claim_visible(&mut self) {
        if !self.window_focused {
            return;
        }
        let model = &self.hosts[self.active].model;
        let Some(session) = model.selected.clone() else {
            return;
        };
        let Some(window) = model.window() else { return };
        let id = self.hosts[self.active].id;
        let blocks: Vec<String> = window
            .layers
            .iter()
            .flat_map(|l| &l.blocks)
            .filter_map(|b| b.block_id.clone())
            .filter(|b| !self.claimed.contains(&(id, b.clone())))
            .collect();
        if blocks.is_empty() {
            return;
        }
        for b in &blocks {
            self.claimed.insert((id, b.clone()));
        }
        self.send(self.active, HostCommand::Claim { session, blocks });
    }

    pub fn window_focus(&mut self, focused: bool) {
        self.window_focused = focused;
        if !focused {
            for term in self.terminals.values_mut() {
                term.end_selection();
            }
        }
        if focused {
            self.claimed.clear();
            self.claim_visible();
            self.track_activity(self.active);
        }
    }

    pub fn refresh_terminal(&mut self, key: &(HostId, String)) {
        let font = self.settings.font_size;
        if let Some(term) = self.terminals.get_mut(key) {
            term.refresh();
            term.resize_if_owner(font, self.terminal_family);
        }
    }

    pub fn update_title(&mut self, tx: &mut Tx) {
        let model = &self.hosts[self.active].model;
        let session = model
            .view()
            .and_then(|v| v.label.clone())
            .unwrap_or_else(|| "Choose session".into());
        let title = format!(
            "{session} — {} — Rex",
            self.settings.hosts[self.active].label
        );
        if title != self.title {
            tx.set_title(title.clone());
            self.title = title;
        }
    }

    pub fn notify(&mut self, text: impl Into<String>) {
        self.push_notice(text.into(), true);
    }

    pub fn inform(&mut self, text: impl Into<String>) {
        self.push_notice(text.into(), false);
    }

    /// Preserve the 640×480 layout floor while retaining the user's preferred zoom.
    pub fn ui_scale(&self) -> f32 {
        self.settings.ui_scale.min(
            (self.viewport[0] / 640.)
                .min(self.viewport[1] / 480.)
                .max(1.),
        )
    }

    pub fn change_ui_scale(&mut self, step: i32, tx: &mut Tx) {
        let percent = if step == 0 {
            100
        } else {
            (self.settings.ui_scale * 100.).round() as i32 + step * 10
        }
        .clamp(70, 150);
        let scale = percent as f32 / 100.;
        if scale != self.settings.ui_scale {
            self.settings.ui_scale = scale;
            self.save();
        }
        if let Some(id) = self.zoom_notice.take() {
            self.dismiss_notice(id);
        }
        let effective = (self.ui_scale() * 100.).round() as i32;
        let suffix = if effective < percent {
            format!(" · showing {effective}% to fit this window")
        } else if percent == 150 {
            " · maximum".into()
        } else if percent == 70 {
            " · minimum".into()
        } else {
            String::new()
        };
        self.inform(format!("Zoom {percent}%{suffix}"));
        self.zoom_notice = Some(self.next_notice);
        tx.instant();
    }

    fn push_notice(&mut self, text: String, sticky: bool) {
        if self.notices.iter().any(|n| n.text == text) {
            return;
        }
        self.next_notice += 1;
        self.notices.push(Notice {
            id: self.next_notice,
            text,
            sticky,
            at: std::time::Instant::now(),
        });
        if self.notices.len() > 3 {
            self.notices.remove(0);
        }
    }

    pub fn expire_notices(&mut self) -> bool {
        let before = self.notices.len();
        self.notices
            .retain(|n| n.sticky || n.at.elapsed() < std::time::Duration::from_secs(6));
        before != self.notices.len()
    }

    pub fn dismiss_notice(&mut self, id: u64) {
        self.notices.retain(|n| n.id != id);
    }

    pub fn dismiss_host_notice(&mut self, host: usize) {
        self.hosts[host]
            .connection
            .model
            .lock()
            .expect("host model")
            .notice = None;
        self.hosts[host].model.notice = None;
    }

    pub fn save(&mut self) {
        self.settings.last_host = self
            .settings
            .hosts
            .get(self.active)
            .map(|h| h.endpoint.clone());
        self.settings.sidebar_width = self.sidebar.width();
        if let Err(error) = self.settings.save(&self.settings_path) {
            self.notify(format!("Preferences not saved: {error:#}"));
        }
    }

    pub fn model(&self) -> &HostModel {
        &self.hosts[self.active].model
    }

    pub fn focused_block(&self) -> Option<String> {
        self.model().focused().map(str::to_owned)
    }

    pub fn active_terminal(&self) -> Option<&Term> {
        self.terminals.get(&self.term_key(&self.focused_block()?))
    }

    pub fn active_terminal_mut(&mut self) -> Option<&mut Term> {
        let key = self.term_key(&self.focused_block()?);
        self.terminals.get_mut(&key)
    }

    pub fn send(&mut self, index: usize, command: HostCommand) {
        if self.hosts[index].connection.commands.send(command).is_err() {
            self.notify("Host worker stopped. Restart Rex to reconnect.");
        }
    }

    pub fn request(&mut self, method: &str, mut payload: Value) {
        let Some(session) = self.model().selected.clone() else {
            return;
        };
        payload["session_id"] = json!(session);
        self.send(
            self.active,
            HostCommand::Request {
                method: method.into(),
                payload,
            },
        );
    }

    /// The shell new terminals on `host` start, if the user chose one.
    pub fn shell(&self, host: usize) -> Option<&str> {
        self.settings.hosts[host].shell.as_deref()
    }

    pub fn terminal_command(&mut self, key: &(HostId, String), command: Command) {
        if let Some(term) = self.terminals.get_mut(key) {
            term.send(command);
        }
    }

    pub fn select(&mut self, host: usize, id: String, tx: &mut Tx) {
        self.active = host;
        self.drag = None;
        self.update_visibility();
        self.settings.hosts[host].collapsed = false;
        self.send(host, HostCommand::Select(id));
        self.save();
        self.update_title(tx);
    }

    pub fn activate_host(&mut self, host: usize, tx: &mut Tx) {
        self.settings.hosts[host].collapsed = false;
        self.active = host;
        self.drag = None;
        self.update_visibility();
        self.save();
        self.update_title(tx);
    }

    pub fn update_visibility(&mut self) {
        for term in self.terminals.values_mut() {
            term.end_selection();
        }
        for index in 0..self.hosts.len() {
            self.send(index, HostCommand::Visibility(index == self.active));
        }
    }

    pub fn toggle_sidebar(&mut self) {
        self.sidebar.toggle();
        self.save();
    }

    pub fn perform(&mut self, action: Intent, tx: &mut Tx) {
        if !action.opens_followup() && self.panel.is_some() {
            self.close_panel(tx);
        }
        // Only pane actions carry a captured pane, and session actions a captured session,
        // into the panel they open.
        if let Some(panel) = &mut self.panel {
            if !matches!(action, Intent::Pane(..)) {
                panel.target = None;
            }
            if !matches!(action, Intent::Session(..)) {
                panel.session = None;
            }
        }
        match action {
            Intent::Select(host, id) => self.select(host, id, tx),
            Intent::Tab(id) => self.request("session.focus_window", json!({"window_id":id})),
            Intent::Switcher => self.picker(true),
            Intent::NewSession => self.create_session(self.active, None, tx),
            Intent::CreateOn(host, name) => self.create_session(host, name, tx),
            Intent::ChooseHost(query) => {
                let title = if query.is_empty() {
                    "Create session on…".to_owned()
                } else {
                    format!("Create \"{query}\" on…")
                };
                self.open_panel(PanelKind::Hosts(query), &title, &[])
            }
            Intent::RetryHost(host) => self.send(host, HostCommand::Reconnect),
            Intent::HostShell(host) => {
                let title = format!("Default shell on {}", self.settings.hosts[host].label);
                let shell = self.shell(host).unwrap_or_default().to_owned();
                self.open_panel(
                    PanelKind::Form(Form::HostShell(host)),
                    &title,
                    &[("Shell", "Server default — e.g. zsh or /bin/zsh", &shell)],
                );
            }
            Intent::Pane(target, action) => self.pane_action(target, action, tx),
            Intent::Session(target, action) => self.session_action(target, action, tx),
            Intent::AddHost => self.open_panel(
                PanelKind::Form(Form::AddHost),
                "Add host",
                &[("Host name", "my-host", ""), ("Endpoint", "https://host.example.ts.net or unix:///path", "")],
            ),
            Intent::NewWindow => self.request(
                "session.new_window",
                json!({"window_label":DEFAULT_TAB_LABEL,"layout":shell_layout(self.shell(self.active)),"focus":true}),
            ),
            Intent::Split(direction) => {
                if let Some(block) = self.focused_block() {
                    self.request("session.new_split", split_payload(&block, direction, self.shell(self.active)));
                }
            }
            Intent::RenameSession => {
                let label = self.model().view().and_then(|v| v.label.clone()).unwrap_or_default();
                self.open_panel(PanelKind::Form(Form::RenameSession(None)), "Rename session", &[("Session name", "", &label)]);
            }
            Intent::RenameWindow(id) => {
                let label = self
                    .model()
                    .view()
                    .and_then(|v| v.windows.iter().find(|w| w.window_id.as_ref() == Some(&id)))
                    .map(|w| super::activity::tab_name(self.model(), w))
                    .unwrap_or_default();
                self.open_panel(PanelKind::Form(Form::RenameWindow(id)), "Rename tab", &[("Tab name", "", &label)]);
            }
            Intent::CloseWindow(id) => self.confirm(
                "Close this tab?",
                "session.close_window",
                json!({"window_id":id}),
                "Ends every program in this tab on the server. Closing the app itself leaves programs running.",
            ),
            Intent::Zoom => {
                if let Some(block) = self.focused_block() {
                    self.request("session.zoom_block", json!({"block_id":block}));
                }
            }
            Intent::Sidebar => self.toggle_sidebar(),
            Intent::Settings => {
                let back = self.panel.take();
                self.panel = Some(Panel::new(PanelKind::Settings, "Settings", &[]));
                self.panel.as_mut().unwrap().target = back.and_then(|p| p.target);
            }
            Intent::EditHost(host) => {
                let h = self.settings.hosts[host].clone();
                self.open_panel(
                    PanelKind::Form(Form::EditHost(host)),
                    &format!("Edit {}", h.label),
                    &[
                        ("Host name", "my-host", &h.label),
                        ("Endpoint", "https://host.example.ts.net or unix:///path", &h.endpoint),
                        ("Default shell", "Server default — e.g. zsh or /bin/zsh", h.shell.as_deref().unwrap_or("")),
                    ],
                );
            }
            Intent::RemoveHost(host) => {
                let label = self.settings.hosts[host].label.clone();
                self.open_panel(PanelKind::RemoveHost(host), &format!("Remove {label}?"), &[]);
            }
            Intent::Retry => self.send(self.active, HostCommand::Reconnect),
        }
    }

    /// The one way to create a session: at once, on `host`, named only when a name was
    /// typed (else the server picks a unique one), starting the host's default shell.
    pub fn create_session(&mut self, host: usize, label: Option<String>, tx: &mut Tx) {
        if !self.hosts[host].model.connected {
            return;
        }
        if self.panel.is_some() {
            self.close_panel(tx);
        }
        self.settings.hosts[host].collapsed = false;
        self.active = host;
        self.drag = None;
        self.update_visibility();
        self.save();
        self.update_title(tx);
        let shell = self.settings.hosts[host].shell.clone();
        self.send(host, HostCommand::CreateSession { label, shell });
    }

    /// A sidebar host header's right-click menu, at the pointer (window points).
    pub fn host_menu(&mut self, host: usize, anchor: (f32, f32)) {
        let mut title = self.settings.hosts[host].label.clone();
        if !self.hosts[host].model.connected {
            title.push_str(" · Offline");
        }
        let scale = self.ui_scale();
        self.open_panel(PanelKind::Host(host), &title, &[]);
        let panel = self.panel.as_mut().unwrap();
        panel.target = None;
        panel.anchor = Some((anchor.0 / scale, anchor.1 / scale));
    }

    /// A tab's right-click menu, at the pointer (window points).
    pub fn tab_menu(&mut self, window: String, anchor: (f32, f32)) {
        let title = self
            .model()
            .view()
            .and_then(|v| {
                v.windows
                    .iter()
                    .find(|w| w.window_id.as_ref() == Some(&window))
            })
            .map(|w| super::activity::tab_name(self.model(), w))
            .unwrap_or_else(|| "Tab".into());
        let scale = self.ui_scale();
        self.open_panel(PanelKind::Window(window), &title, &[]);
        let panel = self.panel.as_mut().unwrap();
        panel.target = None;
        panel.anchor = Some((anchor.0 / scale, anchor.1 / scale));
    }

    /// Ctrl Shift W: closes the focused pane of a split tab, or the tab when it has one pane.
    /// Both ask first.
    pub fn close_focused(&mut self, tx: &mut Tx) {
        let Some(window) = self.model().window() else {
            return;
        };
        let panes = window.layers.iter().flat_map(|l| &l.blocks).count();
        let id = window.window_id.clone();
        if panes > 1
            && let Some(target) = self.focused_block().and_then(|b| self.target(&b))
        {
            self.perform(Intent::Pane(target, PaneAction::Close), tx);
        } else if let Some(id) = id {
            self.perform(Intent::CloseWindow(id), tx);
        }
    }

    pub fn target(&self, block: &str) -> Option<PaneTarget> {
        let model = self.model();
        Some(PaneTarget {
            host: self.active,
            session: model.selected.clone()?,
            block: block.into(),
            label: self.block_label(block),
            tab: model
                .window()?
                .label
                .clone()
                .unwrap_or_else(|| "Terminal".into()),
        })
    }

    pub fn target_exists(&self, target: &PaneTarget) -> bool {
        self.hosts[target.host]
            .model
            .views
            .get(&target.session)
            .is_some_and(|v| {
                v.windows
                    .iter()
                    .flat_map(|w| &w.layers)
                    .flat_map(|l| &l.blocks)
                    .any(|b| b.block_id.as_deref() == Some(&target.block))
            })
    }

    pub fn pane_menu(&mut self, block: String, anchor: Option<(f32, f32)>) {
        let anchor = anchor.map(|(x, y)| (x / self.ui_scale(), y / self.ui_scale()));
        let Some(target) = self.target(&block) else {
            return;
        };
        let title = format!("{} · {}", target.tab, target.label);
        self.open_panel(PanelKind::Pane, &title, &[]);
        let panel = self.panel.as_mut().unwrap();
        panel.target = Some(target);
        panel.anchor = anchor;
    }

    /// A sidebar session row's right-click menu, at the pointer (window points).
    pub fn session_menu(&mut self, host: usize, session: String, anchor: (f32, f32)) {
        let Some(label) = self.hosts[host]
            .model
            .sessions
            .iter()
            .find(|s| s.session_id == session)
            .map(|s| s.label.clone())
        else {
            return;
        };
        let mut title = format!("{} · {label}", self.settings.hosts[host].label);
        if !self.hosts[host].model.connected {
            title.push_str(" · Offline");
        }
        let scale = self.ui_scale();
        self.open_panel(PanelKind::Session, &title, &[]);
        let panel = self.panel.as_mut().unwrap();
        panel.target = None;
        panel.session = Some(SessionTarget {
            host,
            session,
            label,
        });
        panel.anchor = Some((anchor.0 / scale, anchor.1 / scale));
        if let Some(first) = self.entries().iter().position(|e| e.enabled) {
            self.panel.as_mut().unwrap().selected = first;
        }
    }

    /// Every session action names its own host and session, never the selected one.
    fn session_request(&mut self, target: &SessionTarget, method: &str, mut payload: Value) {
        payload["session_id"] = json!(target.session);
        self.send(
            target.host,
            HostCommand::Request {
                method: method.into(),
                payload,
            },
        );
    }

    fn session_action(&mut self, target: SessionTarget, action: SessionAction, tx: &mut Tx) {
        let sessions = &self.hosts[target.host].model.sessions;
        let Some(position) = sessions.iter().position(|s| s.session_id == target.session) else {
            self.close_panel(tx);
            self.inform("That session is no longer available.");
            return;
        };
        match action {
            SessionAction::Rename => {
                let label = target.label.clone();
                self.open_panel(
                    PanelKind::Form(Form::RenameSession(Some(target.clone()))),
                    "Rename session",
                    &[("Session name", "", &label)],
                );
                self.panel.as_mut().unwrap().session = Some(target);
            }
            SessionAction::MoveUp | SessionAction::MoveDown => {
                let before = match action {
                    SessionAction::MoveUp => position.checked_sub(1),
                    _ => Some(position + 2),
                };
                let mut payload = json!({});
                match before.map(|i| sessions.get(i)) {
                    Some(Some(next)) => payload["before_session_id"] = json!(next.session_id),
                    // Down from the next-to-last row: to the end.
                    Some(None) if position + 1 < sessions.len() => {}
                    _ => return,
                }
                self.session_request(&target, "session.move", payload);
            }
            SessionAction::Close => {
                let host = &self.settings.hosts[target.host].label;
                let description = match self.hosts[target.host]
                    .model
                    .views
                    .get(&target.session)
                    .map(|v| v.windows.len())
                {
                    Some(n) => format!(
                        "Ends every program in its {n} tab{} on {host}. Closing the app itself leaves programs running.",
                        if n == 1 { "" } else { "s" }
                    ),
                    None => format!(
                        "Ends every program in this session on {host}. Closing the app itself leaves programs running."
                    ),
                };
                let title = format!("Close {}?", target.label);
                self.confirm(
                    &title,
                    "session.destroy",
                    json!({"session_id": target.session}),
                    &description,
                );
                self.panel.as_mut().unwrap().session = Some(target);
            }
        }
    }

    fn pane_action(&mut self, target: PaneTarget, action: PaneAction, tx: &mut Tx) {
        if !self.target_exists(&target) {
            self.close_panel(tx);
            self.inform("That pane is no longer available.");
            return;
        }
        let request = |this: &mut Self, method: &str, mut payload: Value| {
            payload["session_id"] = target.session.clone().into();
            this.send(
                target.host,
                HostCommand::Request {
                    method: method.into(),
                    payload,
                },
            );
        };
        match action {
            PaneAction::Split(direction) => request(
                self,
                "session.new_split",
                split_payload(&target.block, direction, self.shell(target.host)),
            ),
            PaneAction::Zoom => {
                request(
                    self,
                    "session.focus_block",
                    json!({"block_id":target.block}),
                );
                request(self, "session.zoom_block", json!({"block_id":target.block}));
            }
            PaneAction::Rename => {
                let label = target.label.clone();
                self.open_panel(
                    PanelKind::Form(Form::RenamePane(target.block.clone())),
                    "Rename pane",
                    &[("Pane name", "", &label)],
                );
                self.panel.as_mut().unwrap().target = Some(target);
            }
            PaneAction::Close => {
                let title = format!("Close {} in {}?", target.label, target.tab);
                let description = format!(
                    "Ends the program in this pane on {}. Closing the app itself keeps programs running.",
                    self.settings.hosts[target.host].label
                );
                self.confirm(
                    &title,
                    "block.close",
                    json!({"session_id":target.session,"block_id":target.block}),
                    &description,
                );
                self.panel.as_mut().unwrap().target = Some(target);
            }
            PaneAction::Reconnect => {
                let key = (self.hosts[target.host].id, target.block);
                self.terminal_command(&key, Command::Reconnect);
            }
        }
    }

    pub fn block_label(&self, id: &str) -> String {
        self.model()
            .window()
            .into_iter()
            .flat_map(|w| &w.layers)
            .flat_map(|l| &l.blocks)
            .find(|b| b.block_id.as_deref() == Some(id))
            .and_then(|b| b.label.clone())
            .unwrap_or_else(|| "Terminal".into())
    }

    fn confirm(&mut self, title: &str, method: &str, payload: Value, description: &str) {
        let target = self.panel.as_ref().and_then(|p| p.target.clone());
        let mut panel = Panel::new(
            PanelKind::Confirm {
                method: method.into(),
                payload,
                description: description.into(),
            },
            title,
            &[],
        );
        panel.target = target;
        // Asked from the switcher: cancelling goes back to it.
        panel.back = self
            .panel
            .take()
            .filter(|p| matches!(p.kind, PanelKind::Picker(true)))
            .map(|mut p| {
                p.rename = None;
                Box::new(p)
            });
        self.panel = Some(panel);
    }

    pub fn picker(&mut self, sessions: bool) {
        self.open_panel(
            PanelKind::Picker(sessions),
            if sessions {
                "Switch session"
            } else {
                "Commands"
            },
            &[(
                "",
                if sessions {
                    "Filter sessions or hosts…"
                } else {
                    "Search commands, sessions, and tabs…"
                },
                "",
            )],
        );
        if sessions {
            let selected = self.entries().iter().position(|e| e.current).unwrap_or(0);
            let panel = self.panel.as_mut().unwrap();
            panel.selected = selected;
            panel.reveal = true;
        }
    }

    pub fn open_panel(&mut self, kind: PanelKind, title: &str, fields: &[(&str, &str, &str)]) {
        for term in self.terminals.values_mut() {
            term.end_selection();
        }
        let previous = self.panel.take();
        let target = previous.as_ref().and_then(|p| p.target.clone());
        // Forms and confirmations opened from Settings go back to it.
        let keep_back = matches!(kind, PanelKind::Hosts(_))
            || (matches!(
                kind,
                PanelKind::Form(_) | PanelKind::RemoveHost(_) | PanelKind::AgentChange { .. }
            ) && previous
                .as_ref()
                .is_some_and(|p| matches!(p.kind, PanelKind::Settings)));
        let mut panel = Panel::new(kind, title, fields);
        panel.target = target;
        panel.back = if keep_back {
            previous.map(Box::new)
        } else {
            None
        };
        self.panel = Some(panel);
        if let Some(first) = self.entries().iter().position(|e| e.enabled) {
            self.panel.as_mut().unwrap().selected = first;
        }
    }

    /// The Settings panel a form or confirmation was opened from, to return to after it.
    fn settings_back(&mut self) -> Option<Panel> {
        self.panel
            .as_mut()
            .and_then(|p| p.back.take())
            .filter(|b| matches!(b.kind, PanelKind::Settings))
            .map(|b| *b)
    }

    /// Show a Settings section; Integrations reads the agents' state when first shown.
    pub fn open_section(&mut self, section: SettingsSection) {
        self.settings_section = section;
        if section == SettingsSection::Integrations
            && self.integrations.status == super::integrations::AgentStatus::Unknown
        {
            self.refresh_integrations();
        }
    }

    pub fn set_notice_position(&mut self, position: NoticePosition) {
        if self.settings.notice_position != position {
            self.settings.notice_position = position;
            self.save();
        }
    }

    pub fn close_panel(&mut self, tx: &mut Tx) {
        self.panel = None;
        tx.blur();
    }

    /// Escape on a modal: back to the panel it came from, or closed.
    pub fn cancel_panel_quiet(&mut self) {
        if let Some(mut back) = self.panel.as_mut().and_then(|p| p.back.take()) {
            if let Some(input) = back.inputs.first_mut() {
                input.focused = true;
            }
            self.panel = Some(*back);
        } else {
            self.panel = None;
        }
    }

    pub fn cancel_panel(&mut self, tx: &mut Tx) {
        if let Some(mut back) = self.panel.as_mut().and_then(|p| p.back.take()) {
            if let Some(input) = back.inputs.first_mut() {
                input.focused = true;
            }
            self.panel = Some(*back);
        } else {
            self.close_panel(tx);
        }
    }

    pub fn navigate_panel(&mut self, delta: isize) {
        let entries = self.entries();
        if let Some(panel) = &mut self.panel {
            let mut selected = panel.selected as isize + delta;
            while selected >= 0 && (selected as usize) < entries.len() {
                if entries[selected as usize].enabled {
                    panel.selected = selected as usize;
                    panel.reveal = true;
                    break;
                }
                selected += delta.signum();
            }
        }
    }

    pub fn query(&self) -> String {
        self.panel
            .as_ref()
            .and_then(|p| p.inputs.first())
            .map(|i| i.text.trim().to_owned())
            .unwrap_or_default()
    }

    pub fn query_changed(&mut self) {
        let first = self.entries().iter().position(|e| e.enabled).unwrap_or(0);
        if let Some(panel) = &mut self.panel {
            panel.selected = first;
            panel.error = None;
            panel.reveal = true;
        }
    }

    pub fn submit(&mut self, tx: &mut Tx) {
        let Some(panel) = &self.panel else { return };
        let kind = panel.kind.clone();
        let values: Vec<String> = panel
            .inputs
            .iter()
            .map(|v| v.text.trim().to_owned())
            .collect();
        if panel.is_list() {
            let selected = panel.selected;
            if let Some(entry) = self
                .entries()
                .into_iter()
                .nth(selected)
                .filter(|e| e.enabled)
            {
                self.perform(entry.action, tx);
            }
            return;
        }
        match kind {
            PanelKind::Form(form) => {
                let error = if let Form::HostShell(_) = form {
                    // Empty clears it; the server takes one program name, never arguments.
                    values[0]
                        .chars()
                        .any(|c| c.is_whitespace() || c.is_control())
                        .then(|| "Enter one shell name or path, without arguments.".to_owned())
                } else if values.first().is_none_or(|s| s.is_empty()) {
                    Some("Enter a name".to_owned())
                } else if let Form::AddHost | Form::EditHost(_) = form {
                    let editing = match form {
                        Form::EditHost(i) => Some(i),
                        _ => None,
                    };
                    validate_host(&values[0], &values[1])
                        .err()
                        .map(|e| e.to_string())
                        .or_else(|| {
                            self.settings
                                .hosts
                                .iter()
                                .enumerate()
                                .any(|(i, h)| Some(i) != editing && h.endpoint == values[1])
                                .then(|| "This endpoint is already in the workspace".to_owned())
                        })
                        .or_else(|| {
                            values
                                .get(2)
                                .is_some_and(|v| {
                                    v.chars().any(|c| c.is_whitespace() || c.is_control())
                                })
                                .then(|| {
                                    "Enter one shell name or path, without arguments.".to_owned()
                                })
                        })
                } else {
                    None
                };
                if let Some(error) = error {
                    self.panel.as_mut().unwrap().error = Some(error);
                    return;
                }
                let target = self.panel.as_ref().and_then(|p| p.target.clone());
                let back = self.settings_back();
                self.close_panel(tx);
                match form {
                    Form::HostShell(host) => {
                        self.settings.hosts[host].shell =
                            (!values[0].is_empty()).then(|| values[0].clone());
                        self.save();
                    }
                    Form::RenameSession(None) => self.request("session.set_label", json!({"label":values[0]})),
                    Form::RenameSession(Some(session)) => {
                        self.session_request(&session, "session.set_label", json!({"label":values[0]}))
                    }
                    Form::RenameWindow(id) => {
                        self.request("session.set_window_label", json!({"window_id":id,"label":values[0]}))
                    }
                    Form::RenamePane(id) => match target {
                        Some(target) => self.send(
                            target.host,
                            HostCommand::Request {
                                method: "session.set_block_label".into(),
                                payload: json!({"session_id":target.session,"block_id":id,"label":values[0]}),
                            },
                        ),
                        None => self.request("session.set_block_label", json!({"block_id":id,"label":values[0]})),
                    },
                    Form::AddHost => {
                        self.settings.hosts.push(Host {
                            label: values[0].clone(),
                            endpoint: values[1].clone(),
                            collapsed: false,
                            shell: None,
                        });
                        self.active = self.settings.hosts.len() - 1;
                        self.connect_host(self.active, false);
                        self.update_visibility();
                        self.save();
                    }
                    Form::EditHost(host) => {
                        let shell = values.get(2).filter(|v| !v.is_empty()).cloned();
                        self.edit_host(host, values[0].clone(), values[1].clone(), shell);
                    }
                }
                self.update_title(tx);
                self.panel = back.or(self.panel.take());
            }
            PanelKind::RemoveHost(host) => {
                let back = self.settings_back();
                self.remove_host(host, tx);
                self.panel = back;
            }
            PanelKind::AgentChange { change, .. } => {
                let back = self.settings_back();
                self.apply_agents(change);
                self.panel = back;
            }
            PanelKind::Confirm {
                method, payload, ..
            } => {
                let target = self.panel.as_ref().and_then(|p| p.target.clone());
                let session = self.panel.as_ref().and_then(|p| p.session.clone());
                self.close_panel(tx);
                if let Some(session) = session {
                    self.session_request(&session, &method, payload);
                    return;
                }
                match target.filter(|_| method == "block.close") {
                    Some(target) => {
                        self.send(target.host, HostCommand::Request { method, payload })
                    }
                    None => self.request(&method, payload),
                }
            }
            _ => {}
        }
    }

    pub fn change_font(&mut self, delta: f32) {
        self.settings.font_size = (self.settings.font_size + delta).clamp(8., 48.);
        for term in self.terminals.values_mut() {
            term.last_size = None;
        }
        let font = self.settings.font_size;
        for term in self.terminals.values_mut() {
            term.resize_if_owner(font, self.terminal_family);
        }
        self.save();
    }

    pub fn finish_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            self.send(
                drag.host,
                HostCommand::Request {
                    method: "session.resize_pane".into(),
                    payload: json!({
                        "session_id": drag.session,
                        "direction": if drag.divider.horizontal { "horizontal" } else { "vertical" },
                        "before_block_id": drag.divider.before,
                        "after_block_id": drag.divider.after,
                        "ratio": drag.ratio,
                    }),
                },
            );
        }
    }

    /// A field's model. Rune may write back to a field that its own key just removed (Escape
    /// closing the popup), so a closed panel's writes land in a scratch edit.
    /// Turn a switcher row into a field holding its session's name, selected.
    pub fn start_rename(&mut self, host: usize, session: String) {
        let Some(label) = self.hosts[host]
            .model
            .sessions
            .iter()
            .find(|s| s.session_id == session)
            .map(|s| s.label.clone())
        else {
            return;
        };
        let Some(panel) = &mut self.panel else { return };
        let mut edit = TextEdit::new(&label);
        edit.anchor = 0;
        edit.focus = label.len();
        edit.focused = true;
        if let Some(search) = panel.inputs.first_mut() {
            search.focused = false;
        }
        panel.rename = Some((
            SessionTarget {
                host,
                session,
                label,
            },
            edit,
        ));
    }

    /// Leave the row being renamed, saving its new name when `save`; the search field gets
    /// the keyboard back.
    pub fn finish_rename(&mut self, save: bool) {
        let Some(panel) = &mut self.panel else { return };
        let Some((target, edit)) = panel.rename.take() else {
            return;
        };
        if let Some(search) = panel.inputs.first_mut() {
            search.focused = true;
        }
        let label = edit.text.trim();
        if save && !label.is_empty() && label != target.label {
            self.session_request(&target, "session.set_label", json!({"label": label}));
        }
    }

    /// Ask to close a switcher row's session; cancelling returns to the switcher.
    pub fn close_from_switcher(&mut self, host: usize, session: &str, tx: &mut Tx) {
        let Some(label) = self.hosts[host]
            .model
            .sessions
            .iter()
            .find(|s| s.session_id == session)
            .map(|s| s.label.clone())
        else {
            return;
        };
        let target = SessionTarget {
            host,
            session: session.to_owned(),
            label,
        };
        self.perform(Intent::Session(target, SessionAction::Close), tx);
    }

    pub fn rename_input(&mut self) -> &mut TextEdit {
        match self.panel.as_mut().and_then(|p| p.rename.as_mut()) {
            Some((_, edit)) => edit,
            None => &mut self.scratch,
        }
    }

    pub fn edit_input(&mut self, index: usize) -> &mut TextEdit {
        match self.panel.as_mut().and_then(|p| p.inputs.get_mut(index)) {
            Some(edit) => edit,
            None => &mut self.scratch,
        }
    }
}

fn split_payload(block: &str, direction: &str, shell: Option<&str>) -> Value {
    json!({"anchor_id":block,"direction":direction,"side":"after","layout":shell_layout(shell),"focus":true})
}
