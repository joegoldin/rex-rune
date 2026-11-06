//! Program activity per tab: one state folded from every record of every block in a Rex
//! window, the program its pill shows, the word trouble carries, and which changes become
//! desktop notifications.
use super::{
    notify::{Note, NoteTarget, Notifier},
    theme::*,
    workspace::{HostId, Workspace},
};
use rex_linux::connection::{
    DEFAULT_TAB_LABEL, HostModel,
    activity::{BlockedKind, ProgramState},
};
use rexrs::api::session;
use rune::Tx;
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabActivity {
    pub state: ProgramState,
    pub kind: Option<BlockedKind>,
    /// The deciding record's message, for the tooltip; plain text from the program.
    pub msg: Option<String>,
    /// The deciding record arrived live rather than from a query.
    pub live: bool,
    /// The program behind the deciding record.
    pub source: Option<String>,
    /// The focused block's program: the one reporting, else the foreground process.
    pub program: Option<String>,
}

fn blocks(window: &session::Window) -> impl Iterator<Item = &str> {
    window
        .layers
        .iter()
        .flat_map(|l| &l.blocks)
        .filter_map(|b| b.block_id.as_deref())
}

fn focused_block(window: &session::Window) -> Option<&str> {
    window
        .focused_block_id
        .as_deref()
        .filter(|f| blocks(window).any(|b| b == *f))
        .or_else(|| blocks(window).next())
}

/// What a tab is called: the name it was given, or for one that still has the label every
/// new tab gets, its focused program's terminal title when there is one.
pub fn tab_name(model: &HostModel, window: &session::Window) -> String {
    let label = window.label.as_deref().filter(|l| !l.is_empty());
    let title = label
        .is_none_or(|l| l == DEFAULT_TAB_LABEL)
        .then(|| focused_block(window).and_then(|b| model.title(b)))
        .flatten();
    title.or(label).unwrap_or("Terminal").to_owned()
}

/// Fold a tab's records with Blocked > Error > Working > Done > Idle.
pub fn tab_activity(model: &HostModel, window: &session::Window) -> TabActivity {
    let top = blocks(window)
        .filter_map(|b| model.activity.get(b))
        .flat_map(|records| records.values())
        .max_by_key(|r| r.state);
    let program = focused_block(window).and_then(|b| {
        model
            .activity
            .get(b)
            .and_then(|records| records.values().max_by_key(|r| r.state))
            .and_then(|r| r.app.clone())
            .or_else(|| model.programs.get(b).cloned())
    });
    TabActivity {
        state: top.map(|r| r.state).unwrap_or_default(),
        kind: top.and_then(|r| r.kind),
        msg: top.and_then(|r| r.msg.clone().or_else(|| r.title.clone())),
        live: top.is_some_and(|r| r.live),
        source: top.and_then(|r| r.app.clone()),
        program,
    }
}

/// The icon for a program; shells and anything unknown keep the terminal glyph.
pub fn program_icon(program: Option<&str>) -> &'static rune::Icon {
    match program.map(str::to_ascii_lowercase).as_deref() {
        Some("claude") => &CLAUDE,
        Some("codex") => &CODEX,
        Some("opencode") => &OPENCODE,
        Some("pi") => &PI,
        Some("nvim" | "vim" | "vi") => &VIM,
        Some("git" | "lazygit" | "tig") => &GIT,
        Some("cargo" | "rustc" | "rustup") => &CRATE,
        Some("ssh" | "mosh") => &KEY,
        _ => &TERMINAL,
    }
}

/// The word a pill shows after its label, with its colour: trouble always, and Done until
/// the tab is seen. Idle and Working show nothing.
pub fn tab_word(activity: &TabActivity, unseen_done: bool) -> Option<(&'static str, u32)> {
    match activity.state {
        ProgramState::Blocked => Some((blocked_word(activity.kind), warning_color())),
        ProgramState::Error => Some(("Error", danger_color())),
        _ if unseen_done => Some(("Done", accent_color())),
        _ => None,
    }
}

fn blocked_word(kind: Option<BlockedKind>) -> &'static str {
    match kind {
        Some(BlockedKind::Permission) => "Needs approval",
        Some(BlockedKind::Question) => "Question",
        Some(BlockedKind::Auth) => "Needs sign-in",
        None => "Needs input",
    }
}

fn finished(before: ProgramState, now: ProgramState) -> bool {
    matches!(before, ProgramState::Working | ProgramState::Blocked)
        && matches!(now, ProgramState::Done | ProgramState::Idle)
}

impl Workspace {
    /// The tab is in front of the user: this window has focus and shows that tab.
    pub fn tab_seen(&self, index: usize, session: &str, window: &str) -> bool {
        let model = &self.hosts[index].model;
        self.window_focused
            && index == self.active
            && model.selected.as_deref() == Some(session)
            && model
                .views
                .get(session)
                .is_some_and(|v| v.active_window_id.as_deref() == Some(window))
    }

    /// Compare each tab of host `index` with its last state: latch unseen Done, clear what
    /// the user now sees, and notify about live changes in tabs they can't see.
    pub fn track_activity(&mut self, index: usize) {
        let id = self.hosts[index].id;
        let epoch = self.hosts[index].model.activity_epoch;
        if self.activity_epochs.insert(id, epoch) != Some(epoch) {
            self.tab_states.retain(|(host, _), _| *host != id);
        }
        let mut now = HashMap::new();
        let mut notes = Vec::new();
        let model = &self.hosts[index].model;
        for (session, view) in &model.views {
            for window in &view.windows {
                let Some(window_id) = window.window_id.clone() else {
                    continue;
                };
                let activity = tab_activity(model, window);
                let key = (id, window_id.clone());
                let seen = self.tab_seen(index, session, &window_id);
                let before = self.tab_states.get(&key).copied();
                let state = activity.state;
                let done = before.is_some_and(|b| finished(b, state));
                // Queries only restore what was already true; only live reports are news.
                let news = before != Some(state)
                    && (done || (activity.live && state != ProgramState::Idle));
                let shown = if done { ProgramState::Done } else { state };
                let latch = news && !seen && shown == ProgramState::Done;
                now.insert(key.clone(), (state, seen, latch));
                if news && !seen {
                    notes.push((shown, session.clone(), window_id, activity));
                }
            }
        }
        for (key, (state, seen, latch)) in &now {
            if *latch {
                self.unseen_done.insert(key.clone());
            }
            if *seen || matches!(state, ProgramState::Working | ProgramState::Blocked) {
                self.unseen_done.remove(key);
            }
        }
        self.unseen_done
            .retain(|key| key.0 != id || now.contains_key(key));
        self.tab_states.retain(|(host, _), _| *host != id);
        self.tab_states
            .extend(now.into_iter().map(|(k, (state, ..))| (k, state)));
        for (state, session, window, activity) in notes {
            self.notify_state(index, state, session, window, &activity);
        }
    }

    fn notify_state(
        &mut self,
        index: usize,
        state: ProgramState,
        session: String,
        window: String,
        activity: &TabActivity,
    ) {
        let config = &self.settings.notifications;
        let wanted = match state {
            ProgramState::Working => config.working,
            ProgramState::Blocked => config.blocked,
            ProgramState::Error => config.error,
            ProgramState::Done => config.done,
            ProgramState::Idle => false,
        };
        if !config.system_notifications || !wanted {
            return;
        }
        let word = match state {
            ProgramState::Blocked => blocked_word(activity.kind),
            ProgramState::Error => "Error",
            ProgramState::Done => "Done",
            _ => "Working",
        };
        let (tab, place) = self.describe_tab(index, &session, Some(&window));
        let summary = match activity.source.as_ref().or(activity.program.as_ref()) {
            Some(program) => format!("{program} · {word}"),
            None => format!("{tab} · {word}"),
        };
        let body = match &activity.msg {
            Some(msg) => format!("{msg}\n{place}"),
            None => place,
        };
        let host = self.hosts[index].id;
        self.deliver(Note {
            summary,
            body,
            target: NoteTarget {
                host,
                session,
                window: Some(window),
            },
        });
    }

    /// Pass on what programs asked to tell the user, unless their tab is in front.
    pub fn deliver_program_notices(
        &mut self,
        index: usize,
        notices: Vec<rex_linux::connection::activity::DesktopNotice>,
    ) {
        let config = &self.settings.notifications;
        if !config.system_notifications || !config.program_messages {
            return;
        }
        for notice in notices {
            let window = self.hosts[index]
                .model
                .views
                .get(&notice.session)
                .and_then(|v| {
                    v.windows
                        .iter()
                        .find(|w| blocks(w).any(|b| b == notice.block))
                        .and_then(|w| w.window_id.clone())
                });
            if window
                .as_deref()
                .is_some_and(|w| self.tab_seen(index, &notice.session, w))
            {
                continue;
            }
            let (tab, place) = self.describe_tab(index, &notice.session, window.as_deref());
            let summary = if notice.title.is_empty() {
                tab
            } else {
                notice.title
            };
            let body = if notice.body.is_empty() {
                place
            } else {
                format!("{}\n{place}", notice.body)
            };
            let host = self.hosts[index].id;
            self.deliver(Note {
                summary,
                body,
                target: NoteTarget {
                    host,
                    session: notice.session,
                    window,
                },
            });
        }
    }

    /// A tab's label, and "session · host" for a notification body.
    fn describe_tab(&self, index: usize, session: &str, window: Option<&str>) -> (String, String) {
        let model = &self.hosts[index].model;
        let view = model.views.get(session);
        let tab = view
            .and_then(|v| v.windows.iter().find(|w| w.window_id.as_deref() == window))
            .map_or_else(|| "Terminal".into(), |w| tab_name(model, w));
        let session = model
            .sessions
            .iter()
            .find(|s| s.session_id == session)
            .map(|s| s.label.clone())
            .or_else(|| view.and_then(|v| v.label.clone()))
            .unwrap_or_else(|| "session".into());
        (
            tab,
            format!("{session} · {}", self.settings.hosts[index].label),
        )
    }

    fn deliver(&mut self, note: Note) {
        if cfg!(test) {
            self.outbox.push(note);
            return;
        }
        let handle = self.handle.clone();
        self.notifier
            .get_or_insert_with(|| Notifier::spawn(handle))
            .send(note);
    }

    /// A notification was clicked: raise the window and show its session and tab.
    pub fn open_note(&mut self, target: NoteTarget, token: Option<String>, tx: &mut Tx) {
        self.handle.activate(token.unwrap_or_default());
        let Some(index) = self.host_index(target.host) else {
            return;
        };
        if self.hosts[index].model.selected.as_deref() != Some(&target.session) {
            self.select(index, target.session.clone(), tx);
        } else if index != self.active {
            self.activate_host(index, tx);
        }
        if let Some(window) = target.window {
            self.send(
                index,
                rex_linux::connection::HostCommand::Request {
                    method: "session.focus_window".into(),
                    payload: serde_json::json!({"session_id": target.session, "window_id": window}),
                },
            );
        }
    }

    /// Whether `window` on the active host shows "Done" until seen.
    pub fn unseen(&self, window: &str) -> bool {
        self.unseen_done
            .contains(&(self.hosts[self.active].id, window.to_owned()))
    }
}

pub type TabStates = HashMap<(HostId, String), ProgramState>;
