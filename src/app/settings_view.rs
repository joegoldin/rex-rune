//! Settings: a section list on the left and the chosen section's controls on the right.
use super::{
    dialogs::modal_frame,
    panel::{Intent, SettingsSection},
    theme::*,
    widgets::{Pill, glyph, pill},
    workspace::Workspace,
};
use rex_linux::settings::{NoticePosition, NotificationsConfig};
use rune::{Cursor, El, ScrollSpec, Tx, col, el, row, text};
use rune_kit::{IconButton, Keybind, SegmentedControl, Switch};

/// A chord's keys and what it does.
type Shortcut = (&'static [&'static str], &'static str);

/// Every workspace shortcut, grouped as Settings → Shortcuts lists them.
const SHORTCUTS: &[(&str, &[Shortcut])] = &[
    (
        "Navigate",
        &[
            (&["Ctrl", "K"], "Switch session"),
            (&["Ctrl", "Shift", "P"], "Commands"),
            (&["Ctrl", ","], "Settings"),
            (&["Ctrl", "PageUp"], "Previous tab"),
            (&["Ctrl", "PageDown"], "Next tab"),
            (&["Alt", "Arrows"], "Move pane focus"),
            (&["Ctrl", "Shift", "B"], "Toggle sidebar"),
        ],
    ),
    (
        "Create and close",
        &[
            (&["Ctrl", "Shift", "N"], "New session"),
            (&["Ctrl", "Shift", "T"], "New tab"),
            (&["Ctrl", "Shift", "D"], "Split right"),
            (&["Ctrl", "Alt", "D"], "Split down"),
            (&["Ctrl", "Shift", "W"], "Close pane or tab"),
        ],
    ),
    (
        "Panes",
        &[
            (&["Ctrl", "Shift", "Enter"], "Zoom pane"),
            (&["Ctrl", "Shift", "Space"], "Pane actions"),
            (&["Ctrl", "Shift", "R"], "Resume terminal"),
        ],
    ),
    (
        "Text and view",
        &[
            (&["Ctrl", "Shift", "C"], "Copy"),
            (&["Ctrl", "Shift", "V"], "Paste"),
            (&["Shift", "PageUp"], "Scroll history up"),
            (&["Shift", "PageDown"], "Scroll history down"),
            (&["Ctrl", "+"], "Enlarge interface"),
            (&["Ctrl", "−"], "Shrink interface"),
            (&["Ctrl", "0"], "Reset interface zoom"),
        ],
    ),
];

/// A labelled setting: name and optional explanation on the left, its control on the right.
pub(super) fn setting(label: &str, detail: &str, control: El) -> El {
    let mut words = col()
        .grow(1.)
        .min_w(0.)
        .gap(2.)
        .child(text(label.to_owned()).font(ui(13.)).color(rgb(INK)));
    if !detail.is_empty() {
        words = words.child(
            text(detail.to_owned())
                .font(ui(11.5))
                .color(rgb(MUTED))
                .wrap(),
        );
    }
    row()
        .w_full()
        .min_h(44.)
        .gap(16.)
        .center_y()
        .child(words)
        .child(control)
}

pub(super) fn group(name: &str) -> El {
    text(name.to_owned())
        .font(label_caps())
        .text_case(rune::TextCase::Uppercase)
        .color(rgb(MUTED))
        .mt(10.)
}

pub(super) fn divider() -> El {
    el().w_full().h(1.).fill(white(0.06))
}

/// A stepper: − value +, with Return acting on the focused button as Space does.
fn stepper(
    key: &str,
    value: String,
    down: impl Fn(&mut Workspace, &mut Tx) + Clone + 'static,
    up: impl Fn(&mut Workspace, &mut Tx) + Clone + 'static,
) -> El {
    let k = kit();
    let (down2, up2) = (down.clone(), up.clone());
    row()
        .gap(8.)
        .center_y()
        .child(
            IconButton::new(&MINUS)
                .label("Smaller")
                .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| down(s, tx))
                .build(k)
                .key(format!("{key}-smaller"))
                .on_key(move |s: &mut Workspace, ev: &rune::KeyEvent, tx: &mut Tx| {
                    (ev.key == rune::Key::Enter).then(|| down2(s, tx)).is_some()
                }),
        )
        .child(
            row()
                .w(52.)
                .center()
                .child(text(value).font(medium(13.)).color(rgb(INK))),
        )
        .child(
            IconButton::new(&PLUS)
                .label("Larger")
                .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| up(s, tx))
                .build(k)
                .key(format!("{key}-larger"))
                .on_key(move |s: &mut Workspace, ev: &rune::KeyEvent, tx: &mut Tx| {
                    (ev.key == rune::Key::Enter).then(|| up2(s, tx)).is_some()
                }),
        )
}

fn interface(s: &Workspace) -> El {
    let k = kit();
    let percent = (s.settings.ui_scale * 100.).round() as i32;
    let corner = match s.settings.notice_position {
        NoticePosition::TopRight => 0,
        NoticePosition::BottomRight => 1,
    };
    col()
        .gap(4.)
        .child(setting(
            "Interface zoom",
            "Chrome and terminals together, 70–150%. Ctrl + and Ctrl − also work.",
            row()
                .gap(8.)
                .center_y()
                .child(stepper(
                    "zoom",
                    format!("{percent}%"),
                    |s, tx| s.change_ui_scale(-1, tx),
                    |s, tx| s.change_ui_scale(1, tx),
                ))
                .child(pill("zoom-reset", "Reset", Pill::Neutral, 26., |s, tx| {
                    s.change_ui_scale(0, tx)
                })),
        ))
        .child(divider())
        .child(setting(
            "Session sidebar",
            "Hosts and sessions beside the terminals. Remembered for new windows. Ctrl Shift B.",
            Switch::new(s.sidebar.is_open())
                .label("Show session sidebar")
                .hide_label()
                .on_toggle_tx(|s: &mut Workspace, tx: &mut Tx| s.perform(Intent::Sidebar, tx))
                .build(k)
                .key("sidebar-switch"),
        ))
        .child(setting(
            "Tab position",
            "Top bar, or tabs grouped under each host and session in the sidebar.",
            SegmentedControl::new(
                &["Top", "Sidebar"],
                Some(usize::from(s.settings.vertical_tabs)),
            )
            .fit()
            .on_pick(|s: &mut Workspace, i, _| s.set_vertical_tabs(i == 1))
            .build(k)
            .key("tab-position"),
        ))
        .child(divider())
        .child(setting(
            "Notices",
            "Where in-app notices appear.",
            SegmentedControl::new(&["Top right", "Bottom right"], Some(corner))
                .fit()
                .on_pick(|s: &mut Workspace, i, _| {
                    s.set_notice_position(if i == 0 {
                        NoticePosition::TopRight
                    } else {
                        NoticePosition::BottomRight
                    })
                })
                .build(k)
                .key("notice-position"),
        ))
}

/// Where a notification switch keeps its value.
type Field = fn(&mut NotificationsConfig) -> &mut bool;

/// A switch for one notification preference, saved as soon as it flips.
fn notify_switch(key: &str, label: &str, on: bool, disabled: bool, field: Field) -> El {
    Switch::new(on)
        .label(label.to_owned())
        .hide_label()
        .disabled(disabled)
        .on_toggle(move |s: &mut Workspace| {
            let value = field(&mut s.settings.notifications);
            *value = !*value;
            s.save();
        })
        .build(kit())
        .key(key.to_owned())
}

fn notifications(s: &Workspace) -> El {
    let n = &s.settings.notifications;
    let off = !n.system_notifications;
    let mut body = col()
        .gap(4.)
        .child(setting(
            "Activity in tabs",
            "A word on a tab whose programs need you or failed, and Done on one that finished while you were away.",
            notify_switch("notify-tabs", "Show activity in tabs", n.show_in_tabs, false, |n| &mut n.show_in_tabs),
        ))
        .child(divider())
        .child(setting(
            "Desktop notifications",
            "For tabs you aren't looking at, through your desktop's notification service. Clicking one shows its tab.",
            notify_switch("notify-system", "Desktop notifications", n.system_notifications, false, |n| &mut n.system_notifications),
        ))
        .child(group("Notify when a program"));
    let states: [(&str, &str, &str, bool, Field); 4] = [
        (
            "notify-blocked",
            "Needs you",
            "Waiting for approval, an answer or a sign-in.",
            n.blocked,
            |n| &mut n.blocked,
        ),
        ("notify-error", "Fails", "Reports an error.", n.error, |n| {
            &mut n.error
        }),
        (
            "notify-done",
            "Finishes",
            "Stops working, or reports that it's done.",
            n.done,
            |n| &mut n.done,
        ),
        ("notify-working", "Starts working", "", n.working, |n| {
            &mut n.working
        }),
    ];
    for (i, (key, label, detail, on, field)) in states.into_iter().enumerate() {
        if i > 0 {
            body = body.child(divider());
        }
        body = body.child(setting(
            label,
            detail,
            notify_switch(key, label, on, off, field),
        ));
    }
    body.child(divider()).child(setting(
        "Messages from programs",
        "Notifications programs send themselves with OSC 9 or OSC 777.",
        notify_switch(
            "notify-messages",
            "Messages from programs",
            n.program_messages,
            off,
            |n| &mut n.program_messages,
        ),
    ))
}

fn terminal(s: &Workspace) -> El {
    let font = s.terminal_family.unwrap_or("IBM Plex Mono (bundled)");
    let mut body = col()
        .gap(4.)
        .child(setting(
            "Font",
            "Choose another with --font when starting Rex.",
            text(font.to_owned())
                .font(medium(13.))
                .color(rgb(INK))
                .truncate()
                .max_w(220.),
        ))
        .child(divider())
        .child(setting(
            "Font size",
            "8–48 pt, for every terminal.",
            stepper(
                "font",
                format!("{} pt", s.settings.font_size),
                |s, _| s.change_font(-1.),
                |s, _| s.change_font(1.),
            ),
        ))
        .child(group("Default shell"));
    for (i, host) in s.settings.hosts.iter().enumerate() {
        let detail = match &host.shell {
            Some(shell) => format!("New sessions, tabs and splits start {shell}"),
            None => "New sessions, tabs and splits use the server's default shell".into(),
        };
        body = body.child(setting(
            &host.label,
            &detail,
            pill(
                &format!("host-shell:{i}"),
                "Change…",
                Pill::Neutral,
                26.,
                move |s, tx| s.perform(Intent::HostShell(i), tx),
            ),
        ));
    }
    body.child(
        text(format!("Saved in {}", s.settings_path.display()))
            .font(ui(11.))
            .color(rgb(MUTED))
            .wrap()
            .selectable(true)
            .mt(12.),
    )
}

fn hosts(s: &Workspace) -> El {
    let last = s.settings.hosts.len() == 1;
    let mut body = col().gap(4.);
    for (i, host) in s.settings.hosts.iter().enumerate() {
        let connected = s.hosts.get(i).is_some_and(|h| h.model.connected);
        let mut actions = row().gap(6.).center_y();
        // Quiet when healthy: only trouble gets words, beside the action that fixes it.
        if !connected {
            actions = actions
                .child(text("Offline").font(medium(12.)).color(rgb(WARNING)))
                .child(pill(
                    &format!("host-retry:{i}"),
                    "Retry",
                    Pill::Warning,
                    26.,
                    move |s, tx| s.perform(Intent::RetryHost(i), tx),
                ));
        }
        actions = actions.child(pill(
            &format!("host-edit:{i}"),
            "Edit…",
            Pill::Neutral,
            26.,
            move |s, tx| s.perform(Intent::EditHost(i), tx),
        ));
        let remove = if last {
            pill(
                &format!("host-remove:{i}"),
                "Remove…",
                Pill::Neutral,
                26.,
                |_, _| {},
            )
            .opacity(0.4)
            .focusable(false)
            .help("The workspace needs at least one host")
        } else {
            pill(
                &format!("host-remove:{i}"),
                "Remove…",
                Pill::Danger,
                26.,
                move |s, tx| s.perform(Intent::RemoveHost(i), tx),
            )
        };
        let shell = host
            .shell
            .as_deref()
            .map(|sh| format!(" · {sh}"))
            .unwrap_or_default();
        body = body
            .child(
                row()
                    .key(format!("host-row:{i}"))
                    .w_full()
                    .min_h(52.)
                    .gap(12.)
                    .center_y()
                    .child(glyph(s.host_icon(i), 16., rgb(MUTED)))
                    .child(
                        col()
                            .grow(1.)
                            .min_w(0.)
                            .gap(2.)
                            .child(
                                text(host.label.clone())
                                    .font(medium(13.))
                                    .color(rgb(INK))
                                    .truncate(),
                            )
                            .child(
                                text(format!("{}{shell}", host.endpoint))
                                    .font(ui(11.5))
                                    .color(rgb(MUTED))
                                    .truncate()
                                    .selectable(true),
                            ),
                    )
                    .child(actions.child(remove)),
            )
            .child(divider());
    }
    body.child(row().mt(8.).child(pill(
        "settings-add-host",
        "Add host…",
        Pill::Neutral,
        30.,
        |s, tx| s.perform(Intent::AddHost, tx),
    )))
    .child(
        text("Removing a host only stops this app connecting to it; its sessions keep running.")
            .font(ui(11.5))
            .color(rgb(MUTED))
            .wrap()
            .mt(8.),
    )
}

fn shortcuts() -> El {
    let k = kit();
    let mut body = col().gap(4.);
    for (name, list) in SHORTCUTS {
        body = body.child(group(name));
        for (keys, label) in *list {
            body = body.child(
                row()
                    .h(30.)
                    .gap(12.)
                    .center_y()
                    .child(el().w(190.).child(Keybind::new(keys).build(k)))
                    .child(text(*label).font(ui(12.5)).color(rgb(INK))),
            );
        }
    }
    body.child(
        text("These are reserved by Rex; every other key goes to the focused terminal.")
            .font(ui(11.5))
            .color(rgb(MUTED))
            .wrap()
            .mt(12.),
    )
}

fn nav(s: &Workspace) -> El {
    let mut list = col()
        .key("settings-sections")
        .role("tablist")
        .focus_group()
        .gap(2.);
    for section in SettingsSection::ALL {
        let selected = s.settings_section == section;
        let mut item = row()
            .key(format!("settings-section:{}", section.key()))
            .role("tab")
            .label(section.title())
            .w_full()
            .h(32.)
            .px(10.)
            .center_y()
            .radius(8.)
            .continuous_corners()
            .cursor(Cursor::Pointer)
            .focusable(true)
            .focus_selected(selected)
            .focus_ring(2., 0., rgb(ACCENT))
            .on_press(move |s: &mut Workspace| s.open_section(section))
            .child(
                text(section.title())
                    .font(if selected { medium(13.) } else { ui(13.) })
                    .color(rgb(if selected { INK } else { MUTED })),
            );
        item = if selected {
            item.fill(white(0.09))
        } else {
            item.hover(white(0.05))
        };
        list = list.child(item);
    }
    col()
        .w(190.)
        .h_full()
        .padding(14.)
        .gap(12.)
        .child(
            text("Settings")
                .font(strong(15.))
                .color(rgb(INK))
                .px(10.)
                .pt(6.),
        )
        .child(list)
}

pub fn settings(s: &Workspace, viewport: [f32; 2]) -> El {
    let k = kit();
    let section = s.settings_section;
    let body = match section {
        SettingsSection::Interface => interface(s),
        SettingsSection::Terminal => terminal(s),
        SettingsSection::Notifications => notifications(s),
        SettingsSection::Integrations => super::integrations::section(s),
        SettingsSection::Hosts => hosts(s),
        SettingsSection::Shortcuts => shortcuts(),
    };
    let content = col()
        .grow(1.)
        .min_w(0.)
        .h_full()
        .child(
            row()
                .h(56.)
                .px(24.)
                .center_y()
                .justify_between()
                .child(text(section.title()).font(strong(17.)).color(rgb(INK)))
                .child(
                    IconButton::new(&CLOSE)
                        .label("Close")
                        .on_press_tx(|s: &mut Workspace, tx: &mut Tx| s.close_panel(tx))
                        .build(k)
                        .key("close-settings"),
                ),
        )
        .child(
            col()
                .key(format!("settings-scroll:{}", section.key()))
                .grow(1.)
                .min_h(0.)
                .w_full()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(body.px(24.).pb(20.)),
        );
    modal_frame(
        s,
        row()
            .w(760f32.min(viewport[0] * 0.92))
            .h(520f32.min(viewport[1] * 0.88))
            .child(nav(s))
            .child(el().w(1.).h_full().fill(white(0.06)))
            .child(content),
    )
}
