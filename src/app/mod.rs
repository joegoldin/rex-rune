//! The Rune front end.
mod activity;
mod dialogs;
mod fonts;
mod input;
mod integrations;
mod keys;
mod notify;
mod panel;
mod panes;
mod popups;
mod settings_view;
mod sidebar;
mod stats;
mod term;
mod theme;
mod toolbar;
mod widgets;
mod workspace;

use rex_linux::{connection::Options, settings::Settings};
use rune::{AppSpec, Cursor, Cx, El, Tx, WindowEdge, col, el, row};
use std::time::Duration;
use theme::*;
use workspace::Workspace;

const APP_ICON: &[u8] = include_bytes!("../../resources/icons/256x256/apps/dev.shuv.rex-linux.png");

fn resize_band(e: WindowEdge) -> El {
    let b = 6.;
    let corner = 12.;
    let el = el()
        .key(format!("resize:{e:?}"))
        .window_resize(e)
        .cursor(Cursor::ResizeEdge(e));
    match e {
        WindowEdge::North => el.pin(Some(corner), Some(0.), Some(corner), None).h(b),
        WindowEdge::South => el.pin(Some(corner), None, Some(corner), Some(0.)).h(b),
        WindowEdge::West => el.pin(Some(0.), Some(corner), None, Some(corner)).w(b),
        WindowEdge::East => el.pin(None, Some(corner), Some(0.), Some(corner)).w(b),
        WindowEdge::NorthWest => el.pin(Some(0.), Some(0.), None, None).size(corner, corner),
        WindowEdge::NorthEast => el.pin(None, Some(0.), Some(0.), None).size(corner, corner),
        WindowEdge::SouthWest => el.pin(Some(0.), None, None, Some(0.)).size(corner, corner),
        WindowEdge::SouthEast => el.pin(None, None, Some(0.), Some(0.)).size(corner, corner),
    }
}

pub(crate) fn view(s: &Workspace, cx: &mut Cx<Workspace>) -> El {
    let scale = s.ui_scale();
    let viewport = s.viewport.map(|v| v / scale);
    let mut body = row().key("body").grow(1.).w_full().min_h(0.);
    if s.sidebar.is_open() {
        body = body
            .child(sidebar::sidebar(s, cx))
            .child(sidebar::sidebar_edge(s));
    }
    body = body.child(panes::area(s));
    let mut root = el()
        .key("root")
        .absolute(0., 0.)
        .size(viewport[0], viewport[1])
        .origin(0., 0.)
        .scale(scale)
        .scale_motion(STILL)
        .fill(rgb(GROUND))
        .on_window_focus(|s: &mut Workspace, focused, _| s.window_focus(focused))
        .child(
            col()
                .absolute(0., 0.)
                .size(viewport[0], viewport[1])
                .pt(TOOLBAR)
                .child(body),
        )
        .child(toolbar::toolbar(s).absolute(0., 0.));
    if let Some(toasts) = dialogs::toasts(s) {
        root = root.child(toasts);
    }
    let open = match &s.panel {
        Some(panel) if panel.is_list() => Some(popups::list_popup(s, viewport)),
        Some(_) => dialogs::dialog(s, viewport),
        None => None,
    };
    root = root.child(open.unwrap_or_else(popups::overlay));
    let mut window = el()
        .key("window")
        // Without a focused control, Rune delivers releases only to the outermost node.
        .on_key_up(input::on_key_up)
        .w_full()
        .h_full()
        .fill(rgb(GROUND))
        .on_layout(|s: &mut Workspace, r| s.viewport = [r[2], r[3]])
        .child(root);
    // Native resize targets stay in window points at every content zoom. The corner above
    // Close belongs to Close rather than the resize band.
    if !cx.window_maximized() {
        for edge in [
            WindowEdge::South,
            WindowEdge::West,
            WindowEdge::East,
            WindowEdge::SouthWest,
            WindowEdge::SouthEast,
            WindowEdge::North,
            WindowEdge::NorthWest,
        ] {
            window = window.child(resize_band(edge));
        }
    }
    window
}

fn poll(s: &mut Workspace, _: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    if let Some(panel) = &mut s.panel
        && std::mem::take(&mut panel.reveal)
    {
        tx.scroll_into_view(format!("entry:{}", panel.selected));
        changed = true;
    }
    // Reveal the active tab after selection, resize and sidebar changes, and again on the
    // next frame once the strip has its new width.
    let tab = s.model().view().and_then(|v| v.active_window_id.clone());
    let reveal = (tab.clone(), s.area[2].round() as i32, s.sidebar.is_open());
    let again = std::mem::take(&mut s.reveal_tab_again);
    if reveal != s.last_tab || again {
        if let Some(id) = &tab {
            tx.scroll_into_view(format!("tab:{id}"));
        }
        s.reveal_tab_again = reveal != s.last_tab;
        s.last_tab = reveal;
        changed = true;
    }
    if s.expire_notices() {
        changed = true;
    }
    let failed: Vec<String> = s
        .terminals
        .values_mut()
        .filter_map(|t| t.notice.take())
        .collect();
    for notice in failed {
        s.notify(notice);
        changed = true;
    }
    if s.notices.iter().any(|n| !n.sticky) {
        tx.wake_after(Duration::from_secs(1));
    }
    changed
}

#[cfg(test)]
mod tests;

pub(crate) fn placeholder() -> Workspace {
    unreachable!("App::init builds the workspace")
}

pub fn run(mut options: Options) -> anyhow::Result<()> {
    options.font_size = options.font_size.clamp(8., 48.);
    let (settings, path) = match &options.config {
        Some(path) => (Settings::load(path)?, path.clone()),
        None => Settings::load_default()?,
    };
    let size = options
        .window_size
        .split_once('x')
        .and_then(|(w, h)| {
            Some((
                w.parse::<f32>().ok()?.max(640.),
                h.parse::<f32>().ok()?.max(480.),
            ))
        })
        .unwrap_or((1440., 900.));
    let fonts = fonts::load(&options.font);
    rune::set_app_id("dev.shuv.rex-linux");
    if let Err(error) = rune::set_app_icon_png(APP_ICON) {
        eprintln!("Could not load the Rex window icon: {error}");
    }
    let rune_args = std::mem::take(&mut options.rune);
    let app = rune::App::new(AppSpec {
        min_size: Some((640., 480.)),
        fonts: fonts.files,
        families: fonts.families,
        setup: Some(|s: &mut Workspace, wake| {
            let images = rune::ImageCache::new(wake);
            s.logo = Some(images.load(APP_ICON));
        }),
        on_key: Some(input::on_key),
        poll: Some(poll),
        ..AppSpec::new("Rex", size, placeholder, view)
    });
    let handle = app.handle();
    let terminal = fonts.terminal;
    let mut app = app
        .init(move || Workspace::new(options, settings, path, handle, terminal))
        .args(rune_args)
        .window_options(rune::WindowOptions {
            borderless: true,
            min_size: Some((640., 480.)),
            ..Default::default()
        })
        .on_ime(input::on_ime)
        .srgb_blend(true);
    if stats::FrameStats::enabled() {
        let mut frames = stats::FrameStats::default();
        app = app.on_frame_encoded(move |f| frames.record(f));
    }
    app.run();
    Ok(())
}
