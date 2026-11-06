// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Arranger's window: one window with two tabs, the Arranger and the Instrument rack, so neither covers the other.
//! The strip at its top holds the tabs and the window's buttons; the page under it is the tab's own view.
//!
//! F5 and F6 go to a tab (a second press flips to the other one) and Ctrl+Tab flips between them. Something carried from
//! the browser or the rack over a tab opens that tab, so it can be dropped on its page. The window is always there: it
//! minimizes to its strip and maximizes, and has no close button.

use os_ui::kit::lemon as lk;
use os_ui::{Font, Rect};

use super::{playlist, rack, some_window_buttons, window_body, FrameInfo, TAB_H};
use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};

/// The tabs, in strip order: the window, its name, its icon, its shortcut.
const TABS: [(Win, &str, Icon, &str); 2] =
    [(Win::Playlist, "Arranger", Icon::Playlist, "F5"), (Win::ChannelRack, "Instrument rack", Icon::ChannelRack, "F6")];

const FLIP_KEY: &str = "Ctrl+Tab";
const TAB_W: f32 = 148.0;
const TAB_GAP: f32 = 2.0;

/// The strip of the window whose outer rect is `outer`.
pub fn strip_rect(s: &State, outer: Rect) -> Rect {
    let page = super::frame_rects(s, Win::Playlist, outer).0;
    Rect::new(page.x, page.y - TAB_H, page.w, TAB_H)
}

/// Tab `i` of the strip `r`.
pub fn tab_rect(r: Rect, i: usize) -> Rect {
    Rect::new(r.x + 3.0 + i as f32 * (TAB_W + TAB_GAP), r.y + 2.0, TAB_W, r.h - 2.0)
}

/// What a hovered tab says, on the hint panel and in its tooltip.
pub fn tab_hint(w: Win) -> String {
    let (_, name, _, key) = TABS.iter().find(|t| t.0 == w).copied().unwrap_or(TABS[0]);
    format!("Show the {name}: {key}  -  switch tabs: {FLIP_KEY}")
}

/// The window: the shown tab's page, then the strip over the frame's title area.
pub fn paint(ui: &mut Ui, s: &State, win: Win, outer: Rect, f: &FrameInfo) {
    // A collapsed window is just its strip.
    if s.wins[0].mini {
        window_body(ui, outer);
    } else if win == Win::ChannelRack {
        rack::paint(ui, s, f);
    } else {
        playlist::paint(ui, s, outer, f);
    }
    strip(ui, s, win, outer);
}

fn strip(ui: &mut Ui, s: &State, win: Win, outer: Rect) {
    let t = ui.t;
    let r = strip_rect(s, outer);
    let lemon = lk::on(t);
    ui.c.fill_rect(r, t.window_title);
    ui.c.fill_rect(Rect::new(r.x, r.bottom() - 1.0, r.w, 1.0), t.splitter_dark);
    ui.hit(r, Action::WinDrag(win), "Drag to move");
    for (i, (w, name, icon, _)) in TABS.into_iter().enumerate() {
        let tab = tab_rect(r, i);
        let on = w == win;
        let hov = ui.hit(tab, Action::PanelTab(w), tab_hint(w));
        let down = ui.down(tab, &Action::PanelTab(w));
        let face = if on {
            t.window_body
        } else if hov || down {
            t.button_hi
        } else {
            t.window_title
        };
        let radius = if lemon { lk::r_btn(t) } else { 2.0 };
        // Rounded at the top only: the shape runs on under the strip's edge, which cuts it, so it never covers the
        // page's icons under the strip. The lit tab runs into the page.
        ui.c.push_clip(Rect::new(tab.x, tab.y, tab.w, tab.h));
        ui.c.fill_rrect(Rect::new(tab.x, tab.y, tab.w, tab.h + radius), radius, face);
        ui.c.pop_clip();
        ui.c.fill_rect(Rect::new(tab.x, r.bottom() - 1.0, tab.w, 1.0), if on { face } else { t.splitter_dark });
        if on {
            ui.c.fill_rect(Rect::new(tab.x + 1.0, tab.y, tab.w - 2.0, 2.0), t.accent);
        }
        let ink = if on { t.glyph } else if hov { t.text } else { t.text_dim };
        icons::draw(ui.c, icon, Rect::new(tab.x + 7.0, tab.y + 3.0, 15.0, 15.0), ink);
        let f = Font::new(14);
        let lh = ui.c.line_height(f);
        ui.c.text_clip(f, tab.x + 26.0, (tab.cy() - lh / 2.0).round(), tab.w - 30.0, ink, name);
    }
    // Minimize and maximize: the window is always there, so it has no close button.
    some_window_buttons(ui, win, r.right() - 44.5, r.y, 2);
}
