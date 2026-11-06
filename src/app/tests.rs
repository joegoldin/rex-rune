//! Headless focus and key-routing checks on Rune's runtime (no server: the one host is a
//! socket that does not exist).
use super::{
    panel::{Form, Intent, PanelKind, SessionAction},
    placeholder, view,
    workspace::Workspace,
};
use clap::Parser;
use rex_linux::{
    connection::{HostCommand, HostConnection, Options},
    settings::{Host, Settings},
};
use rune::{AppSpec, Env, Key, KeyEvent, Mods, Runtime};

fn workspace() -> Workspace {
    workspace_hosts(1)
}

fn workspace_hosts(count: usize) -> Workspace {
    let endpoint = "unix:///nonexistent/rex-test.sock";
    let options = Options::parse_from(["rex-linux", "--endpoint", endpoint]);
    let settings = Settings {
        hosts: (0..count)
            .map(|i| Host {
                label: format!("test-{i}"),
                endpoint: if i == 0 {
                    endpoint.into()
                } else {
                    format!("unix:///nonexistent/rex-test-{i}.sock")
                },
                collapsed: false,
                shell: None,
            })
            .collect(),
        ..Settings::default()
    };
    let handle = rune::App::new(AppSpec::new("Rex", (1024., 700.), placeholder, view)).handle();
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "rex-linux-test-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    Workspace::new(options, settings, path, handle, None)
}

fn click(rt: &mut Runtime<Workspace>, key: &str) {
    let r = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing {key}"))
        .rect;
    let (x, y) = (r[0] + r[2] / 2., r[1] + r[3] / 2.);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..4 {
        rt.frame(1. / 60.);
    }
}

/// Every session a host was asked to create, as (label, shell).
fn creates(rx: &flume::Receiver<HostCommand>) -> Vec<(Option<String>, Option<String>)> {
    rx.try_iter()
        .filter_map(|c| match c {
            HostCommand::CreateSession { label, shell } => Some((label, shell)),
            _ => None,
        })
        .collect()
}

fn has(rt: &Runtime<Workspace>, key: &str) -> bool {
    rt.elements().iter().any(|e| e.key.as_deref() == Some(key))
}

#[test]
fn host_plus_creates_on_its_own_host_at_once_with_its_shell() {
    let mut ws = workspace_hosts(3);
    ws.sidebar.toggle();
    let active = capture_host(&mut ws, 0);
    let other = capture_host(&mut ws, 1);
    let offline = capture_host(&mut ws, 2);
    ws.hosts[0].model.connected = true;
    ws.hosts[0].model.sessions = vec![session("cur")];
    ws.hosts[0].model.selected = Some("cur".into());
    ws.hosts[1].model.connected = true;
    ws.hosts[1].model.sessions = vec![session("b")];
    ws.hosts[2].model.sessions = vec![session("away")];
    ws.settings.hosts[1].shell = Some("zsh".into());
    ws.settings.hosts[1].collapsed = true;
    let mut rt = runtime(ws);
    assert!(has(&rt, "host-new-session:0") && has(&rt, "host-new-session:1"));
    assert!(!has(&rt, "host-new-session:2"), "offline hosts have no +");
    assert!(has(&rt, "host-new-session-slot:2"), "but keep its slot");

    click(&mut rt, "host-new-session:1");
    assert_eq!(creates(&other), [(None, Some("zsh".to_owned()))]);
    assert!(
        creates(&active).is_empty(),
        "nothing goes to the active host"
    );
    assert_eq!(rt.state.active, 1);
    assert!(rt.state.panel.is_none());
    assert!(
        !rt.state.settings.hosts[1].collapsed,
        "a collapsed host expands"
    );
    assert_eq!(focused(&rt), None, "the next key goes to the terminal");

    let mut tx = rune::Tx::default();
    rt.state.create_session(2, None, &mut tx);
    assert!(creates(&offline).is_empty(), "offline hosts create nothing");
    assert_eq!(rt.state.active, 1);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn host_menu_rows_online_and_offline() {
    let mut ws = workspace_hosts(2);
    ws.sidebar.toggle();
    let other = capture_host(&mut ws, 0);
    ws.hosts[0].model.connected = true;
    ws.hosts[0].model.sessions = vec![session("cur")];
    ws.settings.hosts[0].shell = Some("zsh".into());
    let mut rt = runtime(ws);
    let rows = |rt: &Runtime<Workspace>| -> Vec<(String, String, bool)> {
        rt.state
            .entries()
            .into_iter()
            .map(|e| (e.label, e.detail, e.enabled))
            .collect()
    };

    // A right-click on the + opens the menu; it never creates.
    secondary_click(&mut rt, "host-new-session:0");
    let panel = rt.state.panel.as_ref().expect("host menu");
    assert!(matches!(panel.kind, PanelKind::Host(0)));
    assert_eq!(panel.title, "test-0");
    assert!(creates(&other).is_empty());
    assert_eq!(
        rows(&rt),
        [
            ("New session".to_owned(), String::new(), true),
            ("Default shell…".to_owned(), "zsh".to_owned(), true),
            ("Edit host…".to_owned(), String::new(), true),
            ("Remove host…".to_owned(), String::new(), true),
        ]
    );
    press(&mut rt, Key::Escape);

    secondary_click(&mut rt, "host:1");
    assert_eq!(rt.state.panel.as_ref().unwrap().title, "test-1 · Offline");
    assert_eq!(
        rows(&rt),
        [
            ("New session".to_owned(), String::new(), false),
            ("Default shell…".to_owned(), String::new(), true),
            ("Retry connection".to_owned(), String::new(), true),
            ("Edit host…".to_owned(), String::new(), true),
            ("Remove host…".to_owned(), String::new(), true),
        ]
    );
    assert_eq!(rt.state.panel.as_ref().unwrap().selected, 1);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_host_menu_edits_a_host_in_place() {
    let mut ws = workspace_hosts(2);
    ws.sidebar.toggle();
    let mut rt = runtime(ws);
    secondary_click(&mut rt, "host:1");
    let index = rt
        .state
        .entries()
        .iter()
        .position(|e| matches!(e.action, Intent::EditHost(1)))
        .expect("Edit host…");
    rt.state.panel.as_mut().unwrap().selected = index;
    press(&mut rt, Key::Enter);
    let panel = rt.state.panel.as_ref().expect("the edit form");
    assert!(matches!(panel.kind, PanelKind::Form(Form::EditHost(1))));
    assert_eq!(panel.inputs[0].text, "test-1");
    assert_eq!(panel.inputs[1].text, rt.state.settings.hosts[1].endpoint);

    let endpoint = "https://test-1.example.ts.net:8443".to_owned();
    rt.state.panel.as_mut().unwrap().inputs[1] = rune::TextEdit::new(&endpoint);
    click(&mut rt, "save-form");
    assert!(rt.state.panel.is_none(), "saving closes the form");
    assert_eq!(rt.state.settings.hosts[1].endpoint, endpoint);
    assert_eq!(rt.state.settings.hosts[1].label, "test-1");

    // The only host can be edited but not removed.
    rt.state.settings.hosts.truncate(1);
    rt.state.hosts.truncate(1);
    secondary_click(&mut rt, "host:0");
    let remove = rt
        .state
        .entries()
        .into_iter()
        .find(|e| matches!(e.action, Intent::RemoveHost(0)))
        .expect("Remove host…");
    assert!(!remove.enabled);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_empty_host_row_collapses_with_its_host() {
    let mut ws = workspace_hosts(2);
    ws.sidebar.toggle();
    capture_host(&mut ws, 1);
    ws.hosts[1].model.connected = true;
    let mut rt = runtime(ws);
    assert!(has(&rt, "new-session-on:1"));
    click(&mut rt, "host-disclosure:1");
    // Let the leaving row finish its exit.
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
    assert!(rt.state.settings.hosts[1].collapsed);
    assert!(!has(&rt, "new-session-on:1"), "collapsing hides the row");
    assert!(has(&rt, "host-new-session:1"), "the header + stays");
    click(&mut rt, "host-disclosure:1");
    assert!(has(&rt, "new-session-on:1"), "expanding brings it back");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn empty_host_row_creates_in_one_step() {
    let mut ws = workspace_hosts(2);
    ws.sidebar.toggle();
    let other = capture_host(&mut ws, 1);
    ws.hosts[1].model.connected = true;
    let mut rt = runtime(ws);
    assert!(!has(&rt, "new-session-on:0"), "offline hosts keep retry");
    click(&mut rt, "new-session-on:1");
    assert_eq!(creates(&other), [(None, None)]);
    assert_eq!(rt.state.active, 1);
    assert!(rt.state.panel.is_none());
    assert_eq!(focused(&rt), None);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn only_a_switcher_query_names_the_new_session() {
    let mut ws = workspace_hosts(2);
    let active = capture_host(&mut ws, 0);
    let other = capture_host(&mut ws, 1);
    ws.hosts[0].model.connected = true;
    ws.hosts[1].model.connected = true;
    let mut tx = rune::Tx::default();

    ws.picker(true);
    ws.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("api");
    let footer: Vec<_> = ws.entries().into_iter().filter(|e| e.footer).collect();
    assert_eq!(footer[0].label, "New session \"api\" on test-0");
    assert_eq!(footer[0].detail, "", "no shortcut hint for a named create");
    assert_eq!(footer[1].label, "Create \"api\" on another host…");
    ws.perform(footer[0].action.clone(), &mut tx);
    assert_eq!(creates(&active), [(Some("api".to_owned()), None)]);
    assert!(ws.panel.is_none());

    // Another host via the chooser keeps the name and says so.
    ws.picker(true);
    ws.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("api");
    ws.perform(Intent::ChooseHost("api".into()), &mut tx);
    assert_eq!(ws.panel.as_ref().unwrap().title, "Create \"api\" on…");
    let host = ws.entries().remove(1);
    ws.perform(host.action, &mut tx);
    assert_eq!(creates(&other), [(Some("api".to_owned()), None)]);

    // Commands' query is a search, never a name.
    ws.picker(false);
    ws.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("new ses");
    let entry = ws
        .entries()
        .into_iter()
        .find(|e| e.label.starts_with("New session on"))
        .expect("Commands create");
    assert_eq!(entry.label, "New session on test-1");
    ws.perform(entry.action, &mut tx);
    assert_eq!(creates(&other), [(None, None)]);
    assert!(creates(&active).is_empty());
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn every_connected_host_offers_a_new_session_in_the_switcher() {
    let mut ws = workspace_hosts(2);
    capture_host(&mut ws, 0);
    let other = capture_host(&mut ws, 1);
    ws.hosts[0].model.connected = true;
    ws.hosts[1].model.connected = true;
    let mut tx = rune::Tx::default();

    ws.picker(true);
    let rows: Vec<_> = ws.entries().into_iter().filter(|e| !e.footer).collect();
    for host in 0..2 {
        let row = rows
            .iter()
            .find(|e| matches!(e.action, Intent::CreateOn(h, None) if h == host))
            .expect("an empty host has a New session row");
        assert_eq!(row.label, "New session");
        assert!(row.enabled);
        assert!(
            matches!(&row.group_create, Some((Intent::CreateOn(h, None), help))
                if *h == host && help == &format!("New session on test-{host}")),
            "the host header carries a create button"
        );
    }
    let row = rows
        .into_iter()
        .find(|e| matches!(e.action, Intent::CreateOn(1, _)))
        .unwrap();
    ws.perform(row.action, &mut tx);
    assert_eq!(creates(&other), [(None, None)]);

    // A typed name carries into both the row and the header button.
    ws.picker(true);
    ws.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("test-1");
    let row = ws
        .entries()
        .into_iter()
        .find(|e| matches!(e.action, Intent::CreateOn(1, _)) && !e.footer)
        .unwrap();
    assert_eq!(row.label, "New session \"test-1\"");
    assert!(matches!(
        &row.group_create,
        Some((Intent::CreateOn(1, Some(name)), _)) if name == "test-1"
    ));
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn default_shell_form_trims_clears_and_rejects_arguments() {
    let mut ws = workspace_hosts(2);
    let mut tx = rune::Tx::default();
    let set = |ws: &mut Workspace, text: &str| {
        ws.perform(Intent::HostShell(1), &mut rune::Tx::default());
        let panel = ws.panel.as_mut().unwrap();
        assert!(matches!(panel.kind, PanelKind::Form(Form::HostShell(1))));
        panel.inputs[0] = rune::TextEdit::new(text);
        ws.submit(&mut rune::Tx::default());
    };
    set(&mut ws, "  zsh ");
    assert_eq!(ws.settings.hosts[1].shell.as_deref(), Some("zsh"));
    assert!(ws.settings.hosts[0].shell.is_none(), "only that host");
    for bad in ["zsh -l", "zsh\u{7}"] {
        set(&mut ws, bad);
        assert_eq!(
            ws.panel.as_ref().unwrap().error.as_deref(),
            Some("Enter one shell name or path, without arguments.")
        );
        ws.cancel_panel(&mut tx);
        assert_eq!(ws.settings.hosts[1].shell.as_deref(), Some("zsh"));
    }
    ws.perform(Intent::HostShell(1), &mut tx);
    assert_eq!(
        ws.panel.as_ref().unwrap().inputs[0].text,
        "zsh",
        "prefilled"
    );
    ws.cancel_panel(&mut tx);
    set(&mut ws, "");
    assert!(ws.settings.hosts[1].shell.is_none(), "empty clears it");
    let saved = std::fs::read_to_string(&ws.settings_path).unwrap();
    assert!(!saved.contains("\"shell\""), "no shell key once cleared");
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn tabs_and_splits_start_the_right_hosts_shell() {
    let mut ws = workspace_hosts(2);
    let active = capture_host(&mut ws, 0);
    let other = capture_host(&mut ws, 1);
    ws.hosts[0].model.connected = true;
    ws.hosts[0].model.selected = Some("cur".into());
    ws.settings.hosts[0].shell = Some("zsh".into());
    ws.settings.hosts[1].shell = Some("fish".into());
    let mut tx = rune::Tx::default();
    ws.perform(Intent::NewWindow, &mut tx);
    let sent = requests(&active);
    assert_eq!(sent[0].0, "session.new_window");
    assert_eq!(sent[0].2["layout"]["block"]["options"]["shell"], "zsh");
    let target = super::panel::PaneTarget {
        host: 1,
        session: "b".into(),
        block: "block:1".into(),
        label: "shell".into(),
        tab: "Shell".into(),
    };
    ws.hosts[1].model.views.insert(
        "b".into(),
        serde_json::from_value(serde_json::json!({
            "revision": 1,
            "session_id": "b",
            "windows": [{"window_id": "w", "layers": [{"kind": "tiled", "blocks": [{"block_id": "block:1"}]}]}]
        }))
        .expect("a view"),
    );
    ws.perform(
        Intent::Pane(target, super::panel::PaneAction::Split("horizontal")),
        &mut tx,
    );
    let sent = requests(&other);
    assert_eq!(sent[0].0, "session.new_split");
    assert_eq!(sent[0].2["layout"]["block"]["options"]["shell"], "fish");
    std::fs::remove_file(&ws.settings_path).ok();
}

/// Host 0 connected on session "s" with a one-pane tab "single" and a split tab "split".
fn tabbed_workspace(active: &str) -> (Workspace, flume::Receiver<HostCommand>) {
    let mut ws = workspace();
    let rx = capture_host(&mut ws, 0);
    let model = &mut ws.hosts[0].model;
    model.connected = true;
    model.sessions = vec![session("s")];
    model.selected = Some("s".into());
    model.views.insert(
        "s".into(),
        serde_json::from_value(serde_json::json!({
            "revision": 1,
            "session_id": "s",
            "active_window_id": active,
            "windows": [
                {"window_id": "single", "label": "server", "focused_block_id": "block:a",
                 "layers": [{"kind": "tiled", "layout": {"block_id": "block:a"},
                    "blocks": [{"block_id": "block:a"}]}]},
                {"window_id": "split", "label": "dev", "focused_block_id": "block:b",
                 "layers": [{"kind": "tiled", "layout": {"split": {"direction": "horizontal", "ratio": 0.5,
                    "before": {"block_id": "block:b"}, "after": {"block_id": "block:c"}}},
                    "blocks": [{"block_id": "block:b"}, {"block_id": "block:c"}]}]}
            ]
        }))
        .expect("a view"),
    );
    (ws, rx)
}

fn ctrl_shift(rt: &mut Runtime<Workspace>, c: char) {
    rt.key(KeyEvent {
        key: Key::Text(c.to_ascii_uppercase().to_string()),
        mods: Mods {
            ctrl: true,
            shift: true,
            ..Mods::default()
        },
    });
    for _ in 0..3 {
        rt.frame(1. / 60.);
    }
}

#[test]
fn a_single_pane_is_full_bleed_and_a_split_keeps_its_cards() {
    let (ws, _rx) = tabbed_workspace("single");
    let mut rt = runtime(ws);
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
    assert!(has(&rt, "pane:block:a"));
    assert!(!has(&rt, "pane-header:block:a"), "no header on a lone pane");
    let r = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("pane:block:a"))
        .unwrap()
        .rect;
    assert_eq!((r[0], r[2]), (0., 1024.), "edge to edge: {r:?}");
    assert!(!has(&rt, "close-tab:single"), "tabs have no close button");
    std::fs::remove_file(&rt.state.settings_path).ok();

    let (ws, _rx) = tabbed_workspace("split");
    let rt = runtime(ws);
    assert!(has(&rt, "pane-header:block:b") && has(&rt, "pane-header:block:c"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_tabs_menu_and_middle_click_act_on_that_tab() {
    let (ws, rx) = tabbed_workspace("single");
    let mut rt = runtime(ws);
    secondary_click(&mut rt, "tab:split");
    let panel = rt.state.panel.as_ref().expect("a tab menu");
    assert!(matches!(&panel.kind, PanelKind::Window(id) if id == "split"));
    assert_eq!(panel.title, "dev");
    let labels: Vec<String> = rt.state.entries().into_iter().map(|e| e.label).collect();
    assert_eq!(labels, ["Rename tab…", "Close tab…", "New tab"]);
    assert!(has(&rt, "menu-surface"));
    press(&mut rt, Key::Escape);
    assert!(rt.state.panel.is_none());

    let r = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("tab:split"))
        .unwrap()
        .rect;
    let (x, y) = (r[0] + r[2] / 2., r[1] + r[3] / 2.);
    rt.button_down(x, y, rune::PointerButton::Middle);
    rt.button_up(x, y, rune::PointerButton::Middle);
    for _ in 0..4 {
        rt.frame(1. / 60.);
    }
    let panel = rt.state.panel.as_ref().expect("a close confirmation");
    assert!(
        matches!(&panel.kind, PanelKind::Confirm { method, payload, .. }
            if method == "session.close_window" && payload["window_id"] == "split")
    );
    assert!(
        requests(&rx).iter().all(|r| r.0 != "session.focus_window"),
        "a middle-click doesn't switch tabs"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn ctrl_shift_w_closes_a_lone_panes_tab_or_a_splits_focused_pane() {
    let (ws, _rx) = tabbed_workspace("single");
    let mut rt = runtime(ws);
    ctrl_shift(&mut rt, 'w');
    let panel = rt.state.panel.take().expect("a confirmation");
    assert!(
        matches!(&panel.kind, PanelKind::Confirm { method, payload, .. }
            if method == "session.close_window" && payload["window_id"] == "single")
    );
    std::fs::remove_file(&rt.state.settings_path).ok();

    let (ws, _rx) = tabbed_workspace("split");
    let mut rt = runtime(ws);
    ctrl_shift(&mut rt, 'w');
    let panel = rt.state.panel.take().expect("a confirmation");
    assert!(
        matches!(&panel.kind, PanelKind::Confirm { method, payload, .. }
            if method == "block.close" && payload["block_id"] == "block:b")
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

fn session(id: &str) -> rexrs::api::session::Session {
    rexrs::api::session::Session {
        session_id: id.into(),
        label: format!("{id}-label"),
    }
}

/// Swap host `index`'s worker for a channel the test reads.
fn capture_host(ws: &mut Workspace, index: usize) -> flume::Receiver<HostCommand> {
    let (commands, receiver) = flume::unbounded();
    let (_, wake) = flume::unbounded();
    ws.hosts[index].connection = HostConnection {
        model: Default::default(),
        commands,
        wake,
    };
    receiver
}

/// Every request a host was sent, as (method, session_id, payload).
fn requests(rx: &flume::Receiver<HostCommand>) -> Vec<(String, String, serde_json::Value)> {
    rx.try_iter()
        .filter_map(|c| match c {
            HostCommand::Request { method, payload } => {
                Some((method, payload["session_id"].as_str()?.to_owned(), payload))
            }
            _ => None,
        })
        .collect()
}

fn secondary_click(rt: &mut Runtime<Workspace>, key: &str) {
    let r = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing {key}"))
        .rect;
    let (x, y) = (r[0] + r[2] / 2., r[1] + r[3] / 2.);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    for _ in 0..4 {
        rt.frame(1. / 60.);
    }
}

#[test]
fn switcher_rows_rename_in_place_and_the_session_chip_opens_the_session_menu() {
    let mut ws = workspace_hosts(2);
    let _current = capture_host(&mut ws, 0);
    let other = capture_host(&mut ws, 1);
    ws.hosts[0].model.connected = true;
    ws.hosts[0].model.sessions = vec![session("cur")];
    ws.hosts[0].model.selected = Some("cur".into());
    ws.hosts[1].model.connected = true;
    ws.hosts[1].model.sessions = vec![session("b")];
    let mut rt = runtime(ws);

    click(&mut rt, "identity");
    let b_row = |rt: &Runtime<Workspace>| {
        rt.elements()
            .into_iter()
            .find(|e| {
                e.label.as_deref() == Some("b-label")
                    && e.key.as_deref().is_some_and(|k| k.starts_with("entry:"))
            })
            .and_then(|e| e.key)
            .expect("b's switcher row")
    };
    let row = b_row(&rt);
    let index: usize = row["entry:".len()..].parse().unwrap();

    // Right-click renames the row in place; Esc goes back to browsing, not out.
    secondary_click(&mut rt, &row);
    let panel = rt.state.panel.as_ref().expect("switcher");
    assert!(matches!(panel.kind, PanelKind::Picker(true)));
    assert_eq!(panel.rename.as_ref().unwrap().1.text, "b-label");
    assert!(
        rt.elements()
            .iter()
            .any(|e| e.key.as_deref() == Some("session-rename"))
    );
    press(&mut rt, Key::Escape);
    let panel = rt.state.panel.as_ref().expect("switcher stays open");
    assert!(panel.rename.is_none());

    // F2 on the highlighted row, then typing over the selected name and Enter saves it.
    rt.state.panel.as_mut().unwrap().selected = index;
    press(&mut rt, Key::Function(2));
    assert!(rt.state.panel.as_ref().unwrap().rename.is_some());
    for c in "renamed".chars() {
        rt.key(KeyEvent {
            key: Key::Text(c.to_string()),
            mods: Mods::default(),
        });
    }
    press(&mut rt, Key::Enter);
    assert!(rt.state.panel.as_ref().unwrap().rename.is_none());
    assert_eq!(
        requests(&other)
            .iter()
            .map(|(m, id, p)| (m.as_str(), id.as_str(), &p["label"]))
            .collect::<Vec<_>>(),
        vec![("session.set_label", "b", &serde_json::json!("renamed"))]
    );

    // The highlighted row's × asks first, and cancelling comes back to the switcher.
    rt.state.panel.as_mut().unwrap().selected = index;
    rt.frame(1. / 60.);
    click(&mut rt, &format!("close:{index}"));
    let panel = rt.state.panel.as_ref().expect("close confirmation");
    assert!(matches!(panel.kind, PanelKind::Confirm { .. }));
    press(&mut rt, Key::Escape);
    let panel = rt.state.panel.as_ref().expect("back to the switcher");
    assert!(matches!(panel.kind, PanelKind::Picker(true)));
    press(&mut rt, Key::Escape);

    secondary_click(&mut rt, "identity");
    let panel = rt.state.panel.as_ref().expect("session menu");
    assert_eq!(panel.title, "test-0 · cur-label");
    let rename = rt
        .state
        .entries()
        .iter()
        .position(|e| e.label == "Rename session…")
        .unwrap();
    click(&mut rt, &format!("entry:{rename}"));
    let panel = rt.state.panel.as_ref().expect("rename form");
    assert_eq!(panel.inputs[0].text, "cur-label");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn session_menu_acts_on_its_row_not_the_current_session() {
    let mut ws = workspace_hosts(2);
    ws.sidebar.toggle();
    let current = capture_host(&mut ws, 0);
    let other = capture_host(&mut ws, 1);
    ws.hosts[0].model.connected = true;
    ws.hosts[0].model.sessions = vec![session("cur")];
    ws.hosts[0].model.selected = Some("cur".into());
    ws.hosts[1].model.connected = true;
    ws.hosts[1].model.sessions = vec![session("a"), session("b"), session("c")];
    ws.hosts[1].model.selected = Some("a".into());
    let mut rt = runtime(ws);

    secondary_click(&mut rt, "session:1:b");
    let panel = rt.state.panel.as_ref().expect("session menu");
    assert!(matches!(panel.kind, PanelKind::Session));
    assert_eq!(panel.title, "test-1 · b-label");
    assert!(
        rt.elements()
            .iter()
            .any(|e| e.key.as_deref() == Some("session-target:1:b")),
        "the targeted row is outlined"
    );
    let labels: Vec<_> = rt
        .state
        .entries()
        .into_iter()
        .map(|e| (e.label, e.enabled))
        .collect();
    assert_eq!(
        labels,
        [
            ("Switch to session".to_owned(), true),
            ("Rename session…".to_owned(), true),
            ("Move up".to_owned(), true),
            ("Move down".to_owned(), true),
            ("Close session…".to_owned(), true),
        ]
    );

    // Close → confirm → submit destroys b on host 1, and nothing on the current host.
    let target = rt.state.panel.as_ref().unwrap().session.clone().unwrap();
    let mut tx = rune::Tx::default();
    rt.state.perform(
        Intent::Session(target.clone(), SessionAction::Close),
        &mut tx,
    );
    let panel = rt.state.panel.as_ref().expect("confirm");
    assert_eq!(panel.title, "Close b-label?");
    rt.state.submit(&mut tx);
    let sent = requests(&other);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        (sent[0].0.as_str(), sent[0].1.as_str()),
        ("session.destroy", "b")
    );
    assert!(
        requests(&current).is_empty(),
        "the current session is untouched"
    );

    // Rename names b on host 1 too.
    rt.state.perform(
        Intent::Session(target.clone(), SessionAction::Rename),
        &mut tx,
    );
    rt.state.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("renamed");
    rt.state.submit(&mut tx);
    let sent = requests(&other);
    assert_eq!(
        (sent[0].0.as_str(), sent[0].1.as_str(), &sent[0].2["label"]),
        ("session.set_label", "b", &serde_json::json!("renamed"))
    );

    // Up goes before a; down from the next-to-last row goes to the end.
    rt.state.perform(
        Intent::Session(target.clone(), SessionAction::MoveUp),
        &mut tx,
    );
    rt.state
        .perform(Intent::Session(target, SessionAction::MoveDown), &mut tx);
    let sent = requests(&other);
    assert_eq!(sent[0].2["before_session_id"], "a");
    assert!(sent[1].2.get("before_session_id").is_none());
    assert!(requests(&current).is_empty());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn session_menu_on_an_edge_row_and_an_offline_host() {
    let mut ws = workspace_hosts(2);
    ws.sidebar.toggle();
    ws.hosts[0].model.connected = true;
    ws.hosts[0].model.sessions = vec![session("cur"), session("next")];
    ws.hosts[0].model.selected = Some("cur".into());
    ws.hosts[1].model.sessions = vec![session("away")];
    let mut rt = runtime(ws);

    secondary_click(&mut rt, "session:0:cur");
    let entries: Vec<_> = rt
        .state
        .entries()
        .into_iter()
        .map(|e| (e.label, e.enabled))
        .collect();
    assert_eq!(
        entries[0],
        ("Rename session…".to_owned(), true),
        "no Switch on the current row"
    );
    assert_eq!(entries[1], ("Move up".to_owned(), false));
    assert_eq!(entries[2], ("Move down".to_owned(), true));
    press(&mut rt, Key::Escape);
    assert!(rt.state.panel.is_none());

    secondary_click(&mut rt, "session:1:away");
    let panel = rt.state.panel.as_ref().expect("offline session menu");
    assert_eq!(panel.title, "test-1 · away-label · Offline");
    let entries: Vec<_> = rt
        .state
        .entries()
        .into_iter()
        .map(|e| (e.label, e.enabled))
        .collect();
    assert_eq!(
        entries,
        [
            ("Switch to session".to_owned(), true),
            ("Rename session…".to_owned(), false),
            ("Move up".to_owned(), false),
            ("Move down".to_owned(), false),
            ("Close session…".to_owned(), false),
            ("Retry connection".to_owned(), true),
            ("Edit host…".to_owned(), true),
        ]
    );
    assert_eq!(panel.selected, 0);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// Shift+right-click goes to a mouse-tracking program instead of opening the pane menu, so
/// the grid's secondary handler must see Shift.
#[test]
fn a_secondary_press_handler_sees_held_shift() {
    struct S {
        shift: Option<bool>,
    }
    fn v(_: &S, _: &mut rune::Cx<S>) -> rune::El {
        rune::el().w_full().h_full().child(
            rune::el()
                .key("target")
                .size(100., 100.)
                .on_secondary_press(|s: &mut S, _, tx: &mut rune::Tx| {
                    s.shift = Some(tx.mods().shift)
                }),
        )
    }
    let mut rt = Runtime::new(S { shift: None }, v, (200., 200.), Env::new(true, 1.));
    rt.frame(1. / 60.);
    rt.physical_input(rune::PhysicalInput::Modifiers(Mods {
        shift: true,
        ..Mods::default()
    }));
    rt.secondary_down(50., 50.);
    rt.secondary_up(50., 50.);
    rt.frame(1. / 60.);
    assert_eq!(rt.state.shift, Some(true));
}

#[test]
fn ui_zoom_reflows_the_root_and_preserves_a_focused_forms_text() {
    let mut ws = workspace();
    ws.open_panel(
        PanelKind::Form(Form::RenameSession(None)),
        "Rename session",
        &[("Session name", "", "keep-me")],
    );
    let mut rt = runtime_at(ws, (1200., 800.));
    let focus = focused(&rt);
    let mods = Mods {
        ctrl: true,
        cmd: true,
        ..Mods::default()
    };
    for _ in 0..8 {
        rt.key(KeyEvent {
            key: Key::Command('+'),
            mods,
        });
        rt.frame(1. / 60.);
    }
    assert_eq!(rt.state.settings.ui_scale, 1.5);
    assert_eq!(focused(&rt), focus);
    assert_eq!(rt.state.panel.as_ref().unwrap().inputs[0].text, "keep-me");
    assert!((rt.state.area[2] - 1200. / 1.5).abs() < 1.);
    for _ in 0..10 {
        rt.key(KeyEvent {
            key: Key::Command('-'),
            mods,
        });
        rt.frame(1. / 60.);
    }
    assert_eq!(rt.state.settings.ui_scale, 0.7);
    rt.key(KeyEvent {
        key: Key::Command('0'),
        mods,
    });
    rt.frame(1. / 60.);
    assert_eq!(rt.state.settings.ui_scale, 1.);
    assert_eq!(focused(&rt), focus);
    assert_eq!(rt.state.panel.as_ref().unwrap().inputs[0].text, "keep-me");
    let saved = Settings::load(&rt.state.settings_path).unwrap();
    assert_eq!(saved.ui_scale, 1.);
    std::fs::remove_file(&rt.state.settings_path).unwrap();
}

fn runtime(ws: Workspace) -> Runtime<Workspace> {
    runtime_at(ws, (1024., 700.))
}

#[test]
fn scaled_pointer_hits_and_sidebar_drag_use_window_deltas_once() {
    let mut ws = workspace_hosts(2);
    ws.sidebar.toggle();
    ws.settings.ui_scale = 1.5;
    ws.hosts[1].model.connected = true;
    let mut rt = runtime_at(ws, (1200., 800.));
    click(&mut rt, "new-session-on:1");
    assert_eq!(rt.state.active, 1, "the scaled press hit the row");
    let edge = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("sidebar-edge"))
        .unwrap()
        .rect;
    let x = edge[0] + edge[2] / 2.;
    let y = edge[1] + 120.;
    let before = rt.state.sidebar.width();
    rt.pointer_down(x, y);
    rt.pointer_move(x + 30., y);
    rt.pointer_up(x + 30., y);
    for _ in 0..4 {
        rt.frame(1. / 60.);
    }
    assert!(
        (rt.state.sidebar.width() - before - 20.).abs() < 1.,
        "30 window points = 20 layout points at 150%"
    );
    std::fs::remove_file(&rt.state.settings_path).unwrap();
}

#[test]
fn zoom_in_a_query_keeps_text_and_small_windows_keep_their_layout_floor() {
    let mut ws = workspace();
    ws.picker(true);
    ws.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("keep-query");
    let mut rt = runtime_at(ws, (640., 480.));
    let focus = focused(&rt);
    rt.key(KeyEvent {
        key: Key::Command('='),
        mods: Mods {
            ctrl: true,
            cmd: true,
            ..Mods::default()
        },
    });
    rt.frame(1. / 60.);
    assert_eq!(rt.state.settings.ui_scale, 1.1);
    assert_eq!(rt.state.ui_scale(), 1.);
    assert_eq!(
        rt.state.panel.as_ref().unwrap().inputs[0].text,
        "keep-query"
    );
    assert_eq!(focused(&rt), focus);
    assert!((rt.state.area[2] - 640.).abs() < 1.);
    std::fs::remove_file(&rt.state.settings_path).unwrap();
}

fn runtime_at(ws: Workspace, size: (f32, f32)) -> Runtime<Workspace> {
    let mut rt = Runtime::new(ws, view, size, Env::new(true, 1.));
    rt.on_key = Some(super::input::on_key);
    for _ in 0..4 {
        rt.frame(1. / 60.);
    }
    rt
}

fn focused(rt: &Runtime<Workspace>) -> Option<String> {
    rt.elements()
        .into_iter()
        .find(|e| e.focused)
        .map(|e| e.path)
}

fn press(rt: &mut Runtime<Workspace>, key: Key) {
    rt.key(KeyEvent {
        key,
        mods: Mods::default(),
    });
    for _ in 0..3 {
        rt.frame(1. / 60.);
    }
}

#[test]
fn close_confirmation_opens_on_keep_running_and_return_keeps_it_running() {
    let mut ws = workspace();
    ws.panel = Some(super::panel::Panel::new(
        PanelKind::Confirm {
            method: "session.close_window".into(),
            payload: serde_json::json!({}),
            description: "Ends programs.".into(),
        },
        "Close this tab?",
        &[],
    ));
    let mut rt = runtime(ws);
    let path = focused(&rt).expect("a focused control");
    assert!(path.ends_with("cancel-close"), "focus starts on {path}");
    press(&mut rt, Key::Enter);
    assert!(
        rt.state.panel.is_none(),
        "Return pressed Keep running and closed the dialog"
    );
    assert!(rt.state.notices.is_empty(), "no request failed");
}

#[test]
fn the_toolbar_settings_button_opens_settings() {
    let mut rt = runtime(workspace());
    // The root eases from the default viewport to the window's before the right end is on screen.
    for _ in 0..60 {
        rt.frame(1. / 60.);
    }
    click(&mut rt, "settings");
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
}

#[test]
fn forms_focus_their_first_field_and_settings_start_on_its_section_list() {
    let mut ws = workspace();
    ws.open_panel(
        PanelKind::Form(Form::RenameSession(None)),
        "Rename session",
        &[("Session name", "", "work")],
    );
    let rt = runtime(ws);
    let path = focused(&rt).expect("a focused field");
    assert!(
        path.contains("field:0"),
        "rename focuses its field, not {path}"
    );

    let mut rt = runtime(workspace());
    rt.key(KeyEvent {
        key: Key::Text(",".into()),
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
    });
    for _ in 0..3 {
        rt.frame(1. / 60.);
    }
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    let path = focused(&rt).expect("a focused control");
    assert!(
        path.ends_with("settings-section:interface"),
        "settings focus {path}"
    );
    // The keyboard path scripts/x11-smoke.py takes to the font stepper.
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Enter);
    assert_eq!(
        rt.state.settings_section,
        super::panel::SettingsSection::Terminal
    );
    let font = rt.state.settings.font_size;
    for _ in 0..4 {
        press(&mut rt, Key::Tab);
    }
    let path = focused(&rt).expect("a focused control");
    assert!(path.ends_with("font-larger"), "Tab reached {path}");
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.settings.font_size, font + 1.);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn settings_sections_and_their_forms_return_to_settings() {
    let mut ws = workspace_hosts(2);
    let mut tx = rune::Tx::default();
    ws.perform(Intent::Settings, &mut tx);
    let mut rt = runtime(ws);
    click(&mut rt, "settings-section:hosts");
    assert!(has(&rt, "host-row:0") && has(&rt, "host-row:1"));
    click(&mut rt, "host-edit:1");
    let panel = rt.state.panel.as_ref().expect("the edit form");
    assert!(matches!(panel.kind, PanelKind::Form(Form::EditHost(1))));
    assert_eq!(panel.inputs[0].text, "test-1");
    press(&mut rt, Key::Escape);
    assert!(
        matches!(
            rt.state.panel.as_ref().map(|p| &p.kind),
            Some(PanelKind::Settings)
        ),
        "Escape goes back to Settings"
    );
    click(&mut rt, "host-remove:1");
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::RemoveHost(1))
    ));
    click(&mut rt, "confirm-close");
    assert_eq!(rt.state.settings.hosts.len(), 1);
    assert!(
        matches!(
            rt.state.panel.as_ref().map(|p| &p.kind),
            Some(PanelKind::Settings)
        ),
        "back in Settings after removing"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn removing_a_host_keeps_the_others_and_ignores_its_late_wakes() {
    let mut ws = workspace_hosts(3);
    let ids: Vec<_> = ws.hosts.iter().map(|h| h.id).collect();
    ws.active = 2;
    for (id, block) in ids.iter().zip(["a", "b", "c"]) {
        ws.claimed.insert((*id, block.to_owned()));
    }
    let mut tx = rune::Tx::default();
    ws.remove_host(1, &mut tx);
    let labels: Vec<_> = ws.settings.hosts.iter().map(|h| h.label.as_str()).collect();
    assert_eq!(labels, ["test-0", "test-2"]);
    assert_eq!(
        ws.hosts.iter().map(|h| h.id).collect::<Vec<_>>(),
        [ids[0], ids[2]]
    );
    assert_eq!(ws.active, 1, "the active host keeps being active");
    assert!(ws.claimed.contains(&(ids[0], "a".into())));
    assert!(ws.claimed.contains(&(ids[2], "c".into())));
    assert!(!ws.claimed.iter().any(|(id, _)| *id == ids[1]));
    // A wake the removed host's worker sent before it stopped lands nowhere.
    ws.sync_host(ids[1], &mut tx);
    assert_eq!(ws.hosts.len(), 2);

    ws.remove_host(0, &mut tx);
    ws.remove_host(0, &mut tx);
    assert_eq!(ws.settings.hosts.len(), 1, "the last host stays");
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn a_new_endpoint_reconnects_under_a_new_id_and_a_rename_does_not() {
    let mut ws = workspace_hosts(2);
    let id = ws.hosts[1].id;
    ws.claimed.insert((id, "b".into()));
    ws.edit_host(
        1,
        "renamed".into(),
        ws.settings.hosts[1].endpoint.clone(),
        Some("zsh".into()),
    );
    assert_eq!(ws.hosts[1].id, id);
    assert_eq!(ws.settings.hosts[1].label, "renamed");
    assert_eq!(ws.settings.hosts[1].shell.as_deref(), Some("zsh"));

    ws.edit_host(
        1,
        "renamed".into(),
        "unix:///nonexistent/elsewhere.sock".into(),
        None,
    );
    assert_ne!(ws.hosts[1].id, id);
    assert_eq!(
        ws.settings.hosts[1].endpoint,
        "unix:///nonexistent/elsewhere.sock"
    );
    assert!(ws.claimed.is_empty(), "the old server's claims are dropped");
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn terminal_keys_are_not_taken_by_focus_traversal_or_activation() {
    let mut rt = runtime(workspace());
    for key in [
        Key::Tab,
        Key::Text(" ".into()),
        Key::Enter,
        Key::Escape,
        Key::ContextMenu,
    ] {
        press(&mut rt, key.clone());
        assert_eq!(focused(&rt), None, "{key:?} moved focus off the terminal");
        assert!(rt.state.panel.is_none(), "{key:?} opened a panel");
    }
}

#[test]
fn escape_walks_back_from_the_host_step_to_the_switcher() {
    let mut ws = workspace();
    ws.picker(true);
    ws.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("proj");
    let mut tx = rune::Tx::default();
    ws.perform(Intent::ChooseHost("proj".into()), &mut tx);
    let mut rt = runtime(ws);
    press(&mut rt, Key::Escape);
    let panel = rt.state.panel.as_ref().expect("the switcher");
    assert!(
        matches!(panel.kind, PanelKind::Picker(true)),
        "Escape on the host step returns to the switcher"
    );
    assert_eq!(panel.inputs[0].text, "proj", "with its query intact");
    press(&mut rt, Key::Escape);
    assert!(rt.state.panel.is_none());
}

fn record(
    state: rex_linux::connection::activity::ProgramState,
    live: bool,
) -> rex_linux::connection::activity::StatusRecord {
    rex_linux::connection::activity::StatusRecord {
        state,
        app: Some("claude".into()),
        live,
        ..Default::default()
    }
}

fn set_activity(
    ws: &mut Workspace,
    block: &str,
    records: &[(&str, rex_linux::connection::activity::StatusRecord)],
) {
    let model = &mut ws.hosts[0].model;
    model.activity.remove(block);
    for (id, r) in records {
        model
            .activity
            .entry(block.into())
            .or_default()
            .insert((*id).into(), r.clone());
    }
    ws.track_activity(0);
}

#[test]
fn a_tab_folds_its_records_by_priority_and_shows_its_program() {
    use super::activity::{program_icon, tab_activity, tab_word};
    use rex_linux::connection::activity::{BlockedKind, ProgramState::*};
    let (mut ws, _rx) = tabbed_workspace("single");
    let model = &mut ws.hosts[0].model;
    model.programs.insert("block:a".into(), "nvim".into());
    model.programs.insert("block:b".into(), "bash".into());
    let mut blocked = record(Blocked, true);
    blocked.kind = Some(BlockedKind::Permission);
    blocked.msg = Some("Run cargo test?".into());
    model.activity.insert(
        "block:c".into(),
        [("".into(), record(Working, true)), ("x".into(), blocked)].into(),
    );
    model
        .activity
        .insert("block:b".into(), [("".into(), record(Error, true))].into());
    let view = model.view().unwrap().clone();
    let single = tab_activity(model, &view.windows[0]);
    assert_eq!(single.state, Idle);
    assert_eq!(single.program.as_deref(), Some("nvim"));
    assert!(std::ptr::eq(program_icon(Some("nvim")), &super::theme::VIM));
    assert_eq!(tab_word(&single, false), None);
    let split = tab_activity(model, &view.windows[1]);
    // Blocked beats Error beats Working, across both panes of the split.
    assert_eq!(split.state, Blocked);
    assert_eq!(split.msg.as_deref(), Some("Run cargo test?"));
    // The focused pane (block:b) reports through claude, which beats its foreground shell.
    assert_eq!(split.program.as_deref(), Some("claude"));
    assert_eq!(tab_word(&split, false).map(|w| w.0), Some("Needs approval"));
    assert!(std::ptr::eq(
        program_icon(Some("bash")),
        &super::theme::TERMINAL
    ));
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn background_tabs_latch_done_and_notify_only_about_live_changes() {
    use rex_linux::connection::activity::ProgramState::*;
    let (mut ws, _rx) = tabbed_workspace("single");
    ws.track_activity(0);
    // A query restoring a blocked state isn't news.
    set_activity(&mut ws, "block:b", &[("", record(Blocked, false))]);
    assert!(ws.outbox.is_empty());
    // A live report that it's working again isn't worth a notification by default.
    set_activity(&mut ws, "block:b", &[("", record(Working, true))]);
    assert!(ws.outbox.is_empty());
    // Finishing in a tab the user can't see latches Done and notifies.
    set_activity(&mut ws, "block:b", &[]);
    assert!(ws.unseen("split"));
    assert_eq!(ws.outbox.len(), 1);
    assert_eq!(ws.outbox[0].summary, "dev · Done");
    assert_eq!(ws.outbox[0].target.window.as_deref(), Some("split"));
    // A live error is news too.
    set_activity(&mut ws, "block:c", &[("", record(Error, true))]);
    assert_eq!(ws.outbox.len(), 2);
    assert_eq!(ws.outbox[1].summary, "claude · Error");
    // The same changes in the tab in front are quiet, and seeing a tab clears its Done.
    set_activity(&mut ws, "block:a", &[("", record(Working, true))]);
    set_activity(&mut ws, "block:a", &[("", record(Done, true))]);
    assert_eq!(ws.outbox.len(), 2);
    assert!(!ws.unseen("single"));
    ws.hosts[0]
        .model
        .views
        .get_mut("s")
        .unwrap()
        .active_window_id = Some("split".into());
    ws.track_activity(0);
    assert!(!ws.unseen("split"));
    // A reconnect drops activity wholesale; that gap is not programs finishing.
    set_activity(&mut ws, "block:a", &[("", record(Working, true))]);
    ws.hosts[0].model.activity.clear();
    ws.hosts[0].model.activity_epoch += 1;
    ws.window_focused = false;
    ws.track_activity(0);
    assert_eq!(ws.outbox.len(), 2);
    assert!(!ws.unseen("single"));
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn notification_settings_gate_what_is_sent() {
    use rex_linux::connection::activity::{DesktopNotice, ProgramState::*};
    let (mut ws, _rx) = tabbed_workspace("single");
    ws.track_activity(0);
    ws.settings.notifications.done = false;
    set_activity(&mut ws, "block:b", &[("", record(Working, true))]);
    set_activity(&mut ws, "block:b", &[]);
    assert!(ws.outbox.is_empty(), "Done notifications are off");
    assert!(ws.unseen("split"), "the tab still shows Done");
    let notice = |block: &str| DesktopNotice {
        session: "s".into(),
        block: block.into(),
        title: String::new(),
        body: "Build finished".into(),
    };
    ws.deliver_program_notices(0, vec![notice("block:a"), notice("block:c")]);
    assert_eq!(ws.outbox.len(), 1, "the tab in front stays quiet");
    assert_eq!(ws.outbox[0].summary, "dev");
    assert_eq!(ws.outbox[0].body, "Build finished\ns-label · test-0");
    ws.settings.notifications.system_notifications = false;
    set_activity(&mut ws, "block:c", &[("", record(Error, true))]);
    ws.deliver_program_notices(0, vec![notice("block:c")]);
    assert_eq!(ws.outbox.len(), 1);
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn tab_pills_carry_the_activity_word_and_hide_it_when_turned_off() {
    use rex_linux::connection::activity::{BlockedKind, ProgramState::*};
    let pill = |show: bool| {
        let (mut ws, _rx) = tabbed_workspace("single");
        ws.settings.notifications.show_in_tabs = show;
        let mut blocked = record(Blocked, true);
        blocked.kind = Some(BlockedKind::Question);
        ws.hosts[0]
            .model
            .activity
            .insert("block:c".into(), [("".into(), blocked)].into());
        let rt = runtime(ws);
        let elements = rt.elements();
        let word = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("tab-word:split"))
            .and_then(|e| e.text.clone());
        let label = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("tab:split"))
            .and_then(|e| e.label.clone());
        std::fs::remove_file(&rt.state.settings_path).ok();
        (word, label)
    };
    assert_eq!(
        pill(true),
        (Some("Question".into()), Some("dev · Question".into()))
    );
    assert_eq!(pill(false), (None, Some("dev".into())));
}

#[test]
fn a_default_tab_takes_its_programs_title_and_a_named_one_keeps_its_name() {
    let (mut ws, _rx) = tabbed_workspace("single");
    let model = &mut ws.hosts[0].model;
    let view = model.views.get_mut("s").unwrap();
    view.windows[1].label = Some(rex_linux::connection::DEFAULT_TAB_LABEL.into());
    for (block, title) in [
        ("block:a", "vim notes"),
        ("block:b", "OC | USB device exploration"),
    ] {
        model.titles.insert(block.into(), (title.into(), None));
    }
    let rt = runtime(ws);
    let label = |key: &str| {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .and_then(|e| e.label)
    };
    assert_eq!(label("tab:single").as_deref(), Some("server"));
    assert_eq!(
        label("tab:split").as_deref(),
        Some("OC | USB device exploration")
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

const SKILL_STATUS: &str = include_str!("fixtures/skill_status.json");
const SKILL_SETUP_CODEX: &str = include_str!("fixtures/skill_setup_codex.json");

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

#[test]
fn agent_commands_never_autostart_and_always_name_the_naming_choice() {
    use super::integrations::{AgentChange, status_args};
    assert_eq!(
        status_args(),
        strings(&["--autostart=false", "skill", "status", "--json"])
    );
    let setup = AgentChange::Setup {
        agents: vec!["codex".into()],
        force: false,
    };
    assert_eq!(
        setup.args(false, true),
        strings(&[
            "--autostart=false",
            "skill",
            "setup",
            "--agent=codex",
            "--session-naming=false",
            "--dry-run",
            "--json"
        ])
    );
    let all = AgentChange::Setup {
        agents: vec![],
        force: true,
    };
    assert_eq!(
        all.args(true, false),
        strings(&[
            "--autostart=false",
            "skill",
            "setup",
            "--session-naming=true",
            "--force",
            "--json"
        ])
    );
    let remove = AgentChange::Remove {
        agents: vec!["pi".into()],
    };
    assert_eq!(
        remove.args(true, false),
        strings(&[
            "--autostart=false",
            "skill",
            "remove",
            "--agent=pi",
            "--json"
        ])
    );
}

fn integrations_open(status: Result<String, String>) -> Runtime<Workspace> {
    let mut ws = workspace();
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(super::panel::SettingsSection::Integrations);
    assert_eq!(
        ws.integrations.ran,
        vec![super::integrations::status_args()],
        "opening the section reads the status once"
    );
    ws.open_section(super::panel::SettingsSection::Integrations);
    assert_eq!(ws.integrations.ran.len(), 1);
    ws.apply_agent_status(status);
    runtime_at(ws, (1280., 900.))
}

fn element_text(rt: &Runtime<Workspace>, key: &str) -> Vec<String> {
    let elements = rt.elements();
    let Some(index) = elements.iter().position(|e| e.key.as_deref() == Some(key)) else {
        return Vec::new();
    };
    let mut inside = vec![index];
    let mut texts = Vec::new();
    for (i, e) in elements.iter().enumerate().skip(index) {
        if i != index && !e.parent.is_some_and(|p| inside.contains(&p)) {
            continue;
        }
        inside.push(i);
        if let Some(t) = &e.text {
            texts.push(t.clone());
        }
    }
    texts
}

#[test]
fn integrations_list_agents_and_preview_changes_before_making_them() {
    use super::integrations::AgentChange;
    let mut rt = integrations_open(Ok(SKILL_STATUS.into()));
    let codex = element_text(&rt, "agent:codex").join(" | ");
    assert!(codex.contains("Not installed"), "{codex}");
    assert!(codex.contains("with OpenCode and Pi"), "{codex}");
    assert!(
        element_text(&rt, "agent:claude")
            .join(" ")
            .contains("Install…")
    );
    click(&mut rt, "agent-setup:codex");
    let preview = rt.state.integrations.ran.last().unwrap().clone();
    assert!(preview.contains(&"--dry-run".to_owned()));
    assert!(preview.contains(&"--session-naming=false".to_owned()));
    let change = AgentChange::Setup {
        agents: vec!["codex".into()],
        force: false,
    };
    rt.state
        .show_agent_preview(change.clone(), Ok(SKILL_SETUP_CODEX.into()));
    let ws = rt.state;
    let panel = ws.panel.as_ref().unwrap();
    assert_eq!(panel.title, "Set up Rex for Codex?");
    assert!(matches!(panel.kind, PanelKind::AgentChange { .. }));
    let mut rt = runtime_at(ws, (1280., 900.));
    let lines = rt
        .elements()
        .into_iter()
        .filter_map(|e| e.text)
        .collect::<Vec<_>>()
        .join("\n");
    // The shared skill names every agent it affects, not only the one asked for.
    assert!(
        lines.contains("Create skill /home/user/.agents/skills/rex · Codex, OpenCode, Pi"),
        "{lines}"
    );
    assert!(
        lines.contains("shared skill changes for every agent"),
        "{lines}"
    );
    click(&mut rt, "agents-apply");
    let applied = rt.state.integrations.ran.last().unwrap().clone();
    assert!(!applied.contains(&"--dry-run".to_owned()));
    assert_eq!(applied, change.args(false, false));
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_missing_rex_explains_itself_and_naming_reapplies_to_installed_agents() {
    let rt = integrations_open(Err(
        "The rex command isn't installed on this machine.".into()
    ));
    let texts = rt
        .elements()
        .into_iter()
        .filter_map(|e| e.text)
        .collect::<Vec<_>>();
    assert!(texts.iter().any(|t| t.contains("isn't installed")));
    std::fs::remove_file(&rt.state.settings_path).ok();

    let installed = SKILL_STATUS.replacen("\"missing\"", "\"current\"", 2);
    let mut ws = workspace();
    ws.apply_agent_status(Ok(installed));
    ws.toggle_session_naming();
    assert!(ws.settings.agent_session_naming);
    let preview = ws.integrations.ran.last().unwrap();
    assert!(
        preview.contains(&"--agent=claude".to_owned()),
        "{preview:?}"
    );
    assert!(!preview.contains(&"--agent=codex".to_owned()));
    assert!(preview.contains(&"--session-naming=true".to_owned()));
    assert!(preview.contains(&"--dry-run".to_owned()));
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn font_preferences_save_from_settings_without_server_requests() {
    let mut ws = workspace();
    let requests = capture_host(&mut ws, 0);
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    let mut rt = runtime(ws);
    click(&mut rt, "settings-section:terminal");
    click(&mut rt, "font-change");
    rt.state.panel.as_mut().unwrap().inputs[0].text = "TX02 Nerd Font Mono".into();
    rt.state.panel.as_mut().unwrap().inputs[1].text = "Noto Sans CJK JP, Noto Color Emoji".into();
    click(&mut rt, "save-form");
    assert!(matches!(
        rt.state.panel.as_ref().unwrap().kind,
        PanelKind::Settings
    ));
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&rt.state.settings_path).unwrap()).unwrap();
    assert_eq!(saved["font_family"], "TX02 Nerd Font Mono");
    assert_eq!(
        saved["font_fallbacks"],
        serde_json::json!(["Noto Sans CJK JP", "Noto Color Emoji"])
    );
    assert!(
        requests.try_iter().next().is_none(),
        "font preferences are entirely local"
    );
    std::fs::remove_file(&rt.state.settings_path).unwrap();
}

#[test]
fn font_preferences_validate_cancel_and_clear_optional_fallbacks() {
    let mut ws = workspace();
    ws.settings.font_fallbacks = vec!["Inter".into()];
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    let mut rt = runtime(ws);
    click(&mut rt, "settings-section:terminal");
    click(&mut rt, "font-change");
    rt.state.panel.as_mut().unwrap().inputs[0].text.clear();
    click(&mut rt, "save-form");
    assert!(rt.state.panel.as_ref().unwrap().error.is_some());
    assert!(!rt.state.settings_path.exists());
    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.settings.font_fallbacks, ["Inter"]);
    click(&mut rt, "font-change");
    rt.state.panel.as_mut().unwrap().inputs[1].text.clear();
    click(&mut rt, "save-form");
    assert!(rt.state.settings.font_fallbacks.is_empty());
    let restored = Settings::load(&rt.state.settings_path).unwrap();
    assert!(restored.font_fallbacks.is_empty());
    assert_eq!(restored.font_family, "JetBrainsMono Nerd Font");
    std::fs::remove_file(&rt.state.settings_path).unwrap();
}
