//! Keyboard: workspace shortcuts first, then every other key to the focused pane's PTY.
//! Terminals never take Rune focus. With nothing focused Rune offers a key to the window's
//! default button, then to `on_key`, then to Tab traversal, Space/Return presses and
//! scrolling. So no default button may exist outside an open modal (the main view has none),
//! and `on_key` takes every key it forwards.
use super::{
    keys::{self, Action},
    panel::{Intent, PanelKind},
    workspace::Workspace,
};
use rex_linux::connection::Command;
use rune::{Ime, Key, KeyEvent, PhysicalKeyCode as Code, Tx};
use serde_json::json;

fn ctrl(ev: &KeyEvent) -> bool {
    ev.mods.ctrl || ev.mods.cmd
}

fn letter(ev: &KeyEvent, tx: &Tx, c: char) -> bool {
    let code = match c {
        'b' => Code::KeyB,
        'c' => Code::KeyC,
        'd' => Code::KeyD,
        'k' => Code::KeyK,
        'n' => Code::KeyN,
        'p' => Code::KeyP,
        'r' => Code::KeyR,
        't' => Code::KeyT,
        'v' => Code::KeyV,
        'w' => Code::KeyW,
        _ => return false,
    };
    // By the character the key types (Dvorak's Ctrl T is where QWERTY's K is), falling back
    // to its position on layouts without Latin letters.
    match tx.key_base() {
        Some(base) if base.is_ascii_alphabetic() => base.eq_ignore_ascii_case(&c),
        Some(_) => tx.physical_key() == Some(code),
        None => {
            ev.is_command(c)
                || matches!(&ev.key, Key::Text(t) if t.eq_ignore_ascii_case(&c.to_string()))
                || tx.physical_key() == Some(code)
        }
    }
}

/// Ctrl , by the character the key types, falling back to its position.
fn comma(ev: &KeyEvent, tx: &Tx) -> bool {
    let typed = match &ev.key {
        Key::Command(c) => *c == ',',
        Key::Text(t) => t == ",",
        _ => false,
    };
    typed
        || (tx.key_base().is_none_or(|b| !b.is_ascii_punctuation())
            && tx.physical_key() == Some(Code::Comma))
}

fn space(ev: &KeyEvent, tx: &Tx) -> bool {
    tx.physical_key() == Some(Code::Space)
        || ev.is_command(' ')
        || matches!(&ev.key, Key::Text(t) if t == " ")
}

/// Global, including focused form/query fields. Logical punctuation respects the keyboard
/// layout; numpad operators and unshifted Digit0 also work without AltGr interception.
pub fn zoom_key(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !ctrl(ev) || ev.mods.alt {
        return false;
    }
    let ch = match &ev.key {
        Key::Command(c) => Some(*c),
        Key::Text(t) if t.chars().count() == 1 => t.chars().next(),
        _ => None,
    };
    let step = match (ch, tx.physical_key()) {
        (Some('+' | '='), _) | (_, Some(Code::NumpadAdd)) => 1,
        (Some('-'), _) | (_, Some(Code::NumpadSubtract)) => -1,
        (Some('0'), _) | (_, Some(Code::Numpad0)) => 0,
        (_, Some(Code::Digit0)) if !ev.mods.shift => 0,
        _ => return false,
    };
    s.change_ui_scale(step, tx);
    true
}

/// Workspace shortcuts with no popup open. True when one ran.
fn shortcut(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let m = ev.mods;
    if ctrl(ev) && !m.alt && ev.key == Key::Tab {
        s.cycle_tab(if m.shift { -1 } else { 1 }, tx);
        return true;
    }
    if ctrl(ev) && m.shift && !m.alt {
        if letter(ev, tx, 'b') {
            s.perform(Intent::Sidebar, tx);
        } else if letter(ev, tx, 'p') {
            s.picker(false);
        } else if letter(ev, tx, 'n') {
            s.perform(Intent::NewSession, tx);
        } else if letter(ev, tx, 't') {
            s.perform(Intent::NewWindow, tx);
        } else if letter(ev, tx, 'w') {
            s.close_focused(tx);
        } else if letter(ev, tx, 'd') {
            s.perform(Intent::Split("horizontal"), tx);
        } else if ev.key == Key::Enter {
            s.perform(Intent::Zoom, tx);
        } else if space(ev, tx) {
            if let Some(block) = s.focused_block() {
                s.pane_menu(block, None);
            }
        } else if letter(ev, tx, 'c') {
            if let Some(text) = s.active_terminal().and_then(|t| t.copy_text()) {
                tx.copy(text);
            }
        } else if letter(ev, tx, 'v') {
            // The read answers later: it pastes only into the pane that asked, if that pane
            // is still focused by then.
            if let Some(block) = s.focused_block()
                && s.active_terminal().is_some_and(|t| t.status().live)
            {
                let asked = s.term_key(&block);
                tx.read_clipboard(move |s: &mut Workspace, content, _| {
                    let text = content.text().to_owned();
                    let focused = s.focused_block().map(|block| s.term_key(&block));
                    if !text.is_empty()
                        && focused.as_ref() == Some(&asked)
                        && let Some(term) = s.terminals.get_mut(&asked)
                    {
                        term.send(Command::Paste(text));
                    }
                });
            }
        } else if letter(ev, tx, 'r') {
            if let Some(term) = s.active_terminal_mut() {
                term.send(Command::Reconnect);
            }
        } else {
            return false;
        }
        return true;
    }
    if ctrl(ev) && !m.shift && !m.alt && letter(ev, tx, 'k') {
        s.picker(true);
    } else if ctrl(ev) && !m.shift && !m.alt && comma(ev, tx) {
        s.perform(Intent::Settings, tx);
    } else if ctrl(ev) && m.alt && !m.shift && letter(ev, tx, 'd') {
        s.perform(Intent::Split("vertical"), tx);
    } else if m.alt
        && ctrl(ev)
        && !m.shift
        && matches!(ev.key, Key::Left | Key::Right | Key::Up | Key::Down)
    {
        let direction = match ev.key {
            Key::Left => "left",
            Key::Right => "right",
            Key::Up => "up",
            _ => "down",
        };
        s.request("session.focus_direction", json!({"direction":direction}));
    } else if ctrl(ev) && !m.shift && !m.alt && matches!(ev.key, Key::PageUp | Key::PageDown) {
        s.cycle_session_tab(if ev.key == Key::PageUp { -1 } else { 1 });
    } else if m.shift && !ctrl(ev) && matches!(ev.key, Key::PageUp | Key::PageDown) {
        let lines = if ev.key == Key::PageUp { -20 } else { 20 };
        if let Some(term) = s.active_terminal_mut() {
            term.send(Command::Scroll(lines));
        }
    } else {
        return false;
    }
    true
}

/// Keys a list popup (switcher, Commands, host step, pane menu) takes, from its field or root.
pub fn list_key(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if zoom_key(s, ev, tx) {
        return true;
    }
    if s.panel.as_ref().is_some_and(|p| p.rename.is_some()) {
        return rename_key(s, ev, tx);
    }
    match ev.key {
        Key::Up | Key::Down => {
            s.navigate_panel(if ev.key == Key::Up { -1 } else { 1 });
            tx.instant();
        }
        Key::Enter => s.submit(tx),
        Key::Escape => s.cancel_panel(tx),
        Key::Function(2) => {
            let selected = s
                .panel
                .as_ref()
                .filter(|p| matches!(p.kind, PanelKind::Picker(true)));
            if let Some(Intent::Select(host, session)) =
                selected.and_then(|p| s.entries().get(p.selected).map(|e| e.action.clone()))
            {
                s.start_rename(host, session);
            }
        }
        // Lists have one control; Tab must not walk focus out to the window behind them.
        Key::Tab => {}
        _ if ctrl(ev)
            && ev.mods.shift
            && space(ev, tx)
            && s.panel
                .as_ref()
                .is_some_and(|p| matches!(p.kind, PanelKind::Pane)) =>
        {
            s.close_panel(tx)
        }
        _ => return false,
    }
    true
}

/// Keys the switcher row being renamed takes: Enter saves, Esc goes back to browsing.
pub fn rename_key(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match ev.key {
        Key::Enter => s.finish_rename(true),
        Key::Escape => s.finish_rename(false),
        Key::Tab | Key::Up | Key::Down => {}
        _ => return zoom_key(s, ev, tx),
    }
    true
}

pub fn on_key(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if zoom_key(s, ev, tx) {
        return true;
    }
    let code = tx.physical_key();
    if let Some(panel) = &s.panel {
        if panel.is_list() && tx.focused().is_none() {
            return list_key(s, ev, tx);
        }
        if ev.key == Key::Escape && tx.focused().is_none() {
            s.cancel_panel(tx);
            return true;
        }
        return false;
    }
    if shortcut(s, ev, tx) {
        return true;
    }
    if tx.focused().is_some() {
        // A control the pointer focused (rare: controls blur after their press) keeps its key.
        tx.blur();
    }
    let action = if tx.key_repeat() {
        Action::Repeat
    } else {
        Action::Press
    };
    let base = tx.key_base();
    let Some(term) = s.active_terminal_mut() else {
        return true;
    };
    if !term.status().live {
        return true;
    }
    term.clear_selection();
    if let Some(payload) = keys::payload(ev, code, base, action) {
        term.send(Command::Key(payload));
        if let Some(code) = code {
            s.forwarded.insert(code);
        }
    }
    true
}

/// A release reaches the PTY only when its press did: keys a shortcut, popup or field took
/// keep their releases local, even when the popup closed in between.
pub fn on_key_up(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(code) = tx.physical_key() else {
        return false;
    };
    if !s.forwarded.remove(&code) {
        return s.panel.is_none();
    }
    let code = Some(code);
    let base = tx.key_base();
    if let Some(term) = s.active_terminal_mut()
        && term.status().live
        && let Some(payload) = keys::payload(ev, code, base, Action::Release)
    {
        term.send(Command::Key(payload));
    }
    true
}

pub fn on_ime(s: &mut Workspace, ime: &Ime, _: &mut Tx) -> bool {
    let Some(term) = s.active_terminal_mut() else {
        return false;
    };
    match ime {
        Ime::Preedit(text, _) => term.preedit = text.clone(),
        Ime::Commit(text) => {
            term.preedit.clear();
            if term.status().live && !text.is_empty() {
                term.send(Command::Paste(text.clone()));
            }
        }
        _ => {}
    }
    true
}
