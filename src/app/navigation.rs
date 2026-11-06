//! Window-local back/forward history. Stable host ids prevent a deleted host's visit
//! from landing on a different server when the saved-host indices shift.
use super::workspace::{HostId, Workspace};
use rex_linux::connection::HostCommand;
use rune::Tx;
use serde_json::json;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visit {
    pub host: HostId,
    pub session: String,
    pub window: Option<String>,
}

#[derive(Default)]
pub struct History {
    entries: Vec<Visit>,
    cursor: usize,
    pub pending: Option<Visit>,
}
impl History {
    fn observe(&mut self, visit: Visit) {
        if let Some(target) = &self.pending
            && (target.host != visit.host
                || target.session != visit.session
                || target.window.is_some() && target.window != visit.window)
        {
            return;
        }
        self.pending = None;
        if self.entries.get(self.cursor) == Some(&visit) {
            return;
        }
        self.entries.truncate(self.cursor + 1);
        self.entries.push(visit);
        if self.entries.len() > 128 {
            self.entries.remove(0);
        }
        self.cursor = self.entries.len() - 1;
    }

    fn retain(&mut self, valid: impl Fn(&Visit) -> bool) {
        let old = self.cursor;
        let current_valid = self.entries.get(old).is_some_and(&valid);
        let mut index = 0;
        let mut before = 0;
        self.entries.retain(|entry| {
            let keep = valid(entry);
            if index < old && keep {
                before += 1;
            }
            index += 1;
            keep
        });
        self.cursor = if current_valid {
            before
        } else {
            before.saturating_sub(1)
        };
        if self.pending.as_ref().is_some_and(|v| !valid(v)) {
            self.pending = None;
        }
    }

    fn step(&mut self, direction: isize) -> Option<Visit> {
        let next = self.cursor.checked_add_signed(direction)?;
        let visit = self.entries.get(next)?.clone();
        self.cursor = next;
        Some(visit)
    }
}

impl Workspace {
    fn current_visit(&self) -> Option<Visit> {
        let host = self.hosts.get(self.active)?;
        let session = host.model.selected.clone()?;
        host.model.view()?;
        Some(Visit {
            host: host.id,
            session,
            window: host.model.window().and_then(|w| w.window_id.clone()),
        })
    }

    pub fn observe_navigation(&mut self) {
        let hosts = &self.hosts;
        self.navigation.retain(|v| {
            hosts.iter().find(|h| h.id == v.host).is_some_and(|h| {
                h.model.sessions.iter().any(|s| s.session_id == v.session)
                    && v.window.as_ref().is_none_or(|window| {
                        h.model.views.get(&v.session).is_some_and(|view| {
                            view.windows
                                .iter()
                                .any(|w| w.window_id.as_ref() == Some(window))
                        })
                    })
            })
        });
        if self.navigation.pending.as_ref().is_some_and(|v| {
            hosts
                .iter()
                .find(|h| h.id == v.host)
                .is_none_or(|h| !h.model.connected || h.model.notice.is_some())
        }) {
            self.navigation.pending = None;
        }
        if let Some(visit) = self.current_visit() {
            self.navigation.observe(visit);
        }
    }

    pub fn select_visit(&mut self, host: usize, session: String, tx: &mut Tx) {
        self.observe_navigation();
        let window = self.hosts[host].model.views.get(&session).and_then(|v| {
            v.active_window_id
                .clone()
                .or_else(|| v.windows.first()?.window_id.clone())
        });
        self.navigate_to(
            Visit {
                host: self.hosts[host].id,
                session,
                window,
            },
            tx,
        );
    }

    pub(super) fn navigate_to(&mut self, visit: Visit, tx: &mut Tx) {
        let Some(host) = self.host_index(visit.host) else {
            return;
        };
        self.active = host;
        self.drag = None;
        self.settings.hosts[host].collapsed = false;
        self.update_visibility();
        self.navigation.pending = Some(visit.clone());
        if self.hosts[host].model.selected.as_ref() != Some(&visit.session) {
            self.send(host, HostCommand::Select(visit.session.clone()));
        }
        if let Some(window) = visit.window {
            self.send(
                host,
                HostCommand::Request {
                    method: "session.focus_window".into(),
                    payload: json!({"session_id": visit.session, "window_id": window}),
                },
            );
        }
        self.save();
        self.update_title(tx);
    }

    pub fn navigate_history(&mut self, direction: isize, tx: &mut Tx) {
        if self.panel.is_some() {
            return;
        }
        self.observe_navigation();
        if let Some(visit) = self.navigation.step(direction) {
            self.navigate_to(visit, tx);
        }
    }

    /// Traverse the same host/session/window order as the sidebar, even when
    /// the previous key's target has not yet been acknowledged by the server.
    pub fn cycle_tab(&mut self, direction: isize, tx: &mut Tx) {
        self.observe_navigation();
        let Some(mut current) = self
            .navigation
            .pending
            .clone()
            .or_else(|| self.current_visit())
        else {
            return;
        };
        if current.window.is_none() {
            current.window = self
                .hosts
                .iter()
                .find(|h| h.id == current.host)
                .and_then(|h| h.model.views.get(&current.session))
                .and_then(|v| {
                    v.active_window_id
                        .clone()
                        .or_else(|| v.windows.first()?.window_id.clone())
                });
        }
        let mut tabs = Vec::new();
        for host in self.hosts.iter().filter(|h| h.model.connected) {
            for session in &host.model.sessions {
                if let Some(view) = host.model.views.get(&session.session_id) {
                    tabs.extend(view.windows.iter().filter_map(|window| {
                        Some(Visit {
                            host: host.id,
                            session: session.session_id.clone(),
                            window: Some(window.window_id.clone()?),
                        })
                    }));
                } else {
                    // An initial metadata fetch may still be in flight. Selecting
                    // the session loads its view instead of silently skipping it.
                    tabs.push(Visit {
                        host: host.id,
                        session: session.session_id.clone(),
                        window: None,
                    });
                }
            }
        }
        if tabs.is_empty() {
            return;
        }
        let index = tabs
            .iter()
            .position(|v| {
                v == &current
                    || v.host == current.host && v.session == current.session && v.window.is_none()
            })
            .unwrap_or(0);
        let next = (index as isize + direction).rem_euclid(tabs.len() as isize) as usize;
        self.navigate_to(tabs[next].clone(), tx);
    }

    pub fn cycle_session_tab(&mut self, direction: isize) {
        let Some(visit) = self
            .navigation
            .pending
            .clone()
            .or_else(|| self.current_visit())
        else {
            return;
        };
        let Some(host) = self.host_index(visit.host) else {
            return;
        };
        let Some(view) = self.hosts[host].model.views.get(&visit.session) else {
            return;
        };
        let ids: Vec<_> = view
            .windows
            .iter()
            .filter_map(|w| w.window_id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        let current = ids
            .iter()
            .position(|id| Some(id) == visit.window.as_ref())
            .unwrap_or(0);
        let next = (current as isize + direction).rem_euclid(ids.len() as isize) as usize;
        let window = ids[next].clone();
        self.observe_navigation();
        self.navigation.pending = Some(Visit {
            window: Some(window.clone()),
            ..visit.clone()
        });
        self.send(
            host,
            HostCommand::Request {
                method: "session.focus_window".into(),
                payload: json!({"session_id": visit.session, "window_id": window}),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn visit(id: &str) -> Visit {
        Visit {
            host: HostId(0),
            session: "s".into(),
            window: Some(id.into()),
        }
    }
    #[test]
    fn history_branches_without_readding_replayed_visits() {
        let mut h = History::default();
        for id in ["a", "a", "b", "c"] {
            h.observe(visit(id));
        }
        assert_eq!(h.entries.len(), 3);
        assert_eq!(h.step(-1), Some(visit("b")));
        h.pending = Some(visit("b"));
        h.observe(visit("c"));
        assert!(h.pending.is_some());
        h.observe(visit("b"));
        assert_eq!(h.entries.len(), 3);
        assert_eq!(h.step(1), Some(visit("c")));
        h.step(-1);
        h.observe(visit("d"));
        assert!(h.step(1).is_none());
        assert_eq!(h.step(-1), Some(visit("b")));
        assert_eq!(h.step(-1), Some(visit("a")));
        assert!(h.step(-1).is_none());
    }
    #[test]
    fn removed_targets_are_skipped_and_history_is_bounded() {
        let mut h = History::default();
        for id in ["a", "closed", "b"] {
            h.observe(visit(id));
        }
        h.retain(|v| v.window.as_deref() != Some("closed"));
        assert_eq!(h.step(-1), Some(visit("a")));
        assert_eq!(h.step(1), Some(visit("b")));
        for i in 0..200 {
            h.observe(visit(&i.to_string()));
        }
        assert_eq!(h.entries.len(), 128);
        assert_eq!(h.cursor, 127);
    }
}
