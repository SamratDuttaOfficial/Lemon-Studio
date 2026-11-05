// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Arranger's window: one window with two tabs, the Arranger and the Instrument rack, so neither covers the other.
//! The strip at its top holds the tabs and the window's buttons; the page under it is the tab's own view.
//!
//! F5 and F6 go to a tab (a second press flips to the other one) and Ctrl+Tab flips between them. Something carried from
//! the browser or the rack over a tab opens that tab, so it can be dropped on its page.

use os_ui::kit::lemon as lk;
use os_ui::{Font, Rect};

use super::{playlist, rack, window_body, window_buttons, FrameInfo, TAB_H};
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

/// What a hovered tab says, on the hint panel and in its bubble.
pub fn tab_hint(w: Win) -> String {
    let (_, name, _, key) = TABS.iter().find(|t| t.0 == w).copied().unwrap_or(TABS[0]);
    format!("{name}: {key}  -  switch tabs: {FLIP_KEY}")
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
        // The lit tab runs into the page under it.
        ui.c.fill_rrect(Rect::new(tab.x, tab.y, tab.w, tab.h + radius), radius, face);
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
    window_buttons(ui, win, r.right() - 64.0, r.y, t.text_dim, t.glyph);
}

/// The shortcut bubble of the tab under the mouse (painted after the windows; it takes no clicks).
pub fn paint_tip(ui: &mut Ui, s: &State) {
    let Some(&Action::PanelTab(w)) = ui.hovered() else { return };
    // Something being carried: the tab opens by itself, and the bubble would be in the way.
    if s.browser_drag.is_some() {
        return;
    }
    let shown = TABS.iter().map(|t| t.0).find(|&o| s.shown(o));
    let Some(shown) = shown else { return };
    let strip = strip_rect(s, s.win_rect(shown));
    let i = TABS.iter().position(|t| t.0 == w).unwrap_or(0);
    let tab = tab_rect(strip, i);
    let (_, name, _, key) = TABS[i];
    let f = Font::new(13);
    let lines = [format!("{name}: {key}"), format!("Switch tabs: {FLIP_KEY}")];
    let width = lines.iter().map(|l| ui.c.text_width(f, l)).fold(0.0f32, f32::max).ceil() + 16.0;
    let (lh, h) = (16.0, 2.0 * 16.0 + 10.0);
    let (sw, sh) = ui.c.size();
    let b = Rect::new(tab.x.min(sw - width - 2.0).max(0.0), (tab.bottom() + 3.0).min(sh - h - 2.0), width, h);
    ui.c.fill_rrect(b.outset(1.0), 6.0, os_ui::Color::rgb(0).with_alpha(90));
    ui.c.fill_rrect(b, 5.0, ui.t.s(0x0b0d0f).with_alpha(245));
    for (k, l) in lines.iter().enumerate() {
        ui.c.text(f, b.x + 8.0, b.y + 5.0 + k as f32 * lh, ui.t.tx(0xe6e6e6), l);
    }
}
