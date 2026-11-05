// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Arranger's window through the mouse and the keyboard: its two tabs, carrying and copying from one to the other,
//! the channel controls on the picker's rows, what a selected clip lights there, and the number boxes' Input a number.

use os_model::{ChannelKind, ClipSource, Note, Pattern};
use os_ui::{Canvas, Rect};
use winit::keyboard::{Key, NamedKey};

use crate::app::{test_project, Action, App, Win, WinState};
use crate::knobs::KnobId;
use crate::playlist::{self as pl, PickKind};
use crate::remote::RAct;
use crate::typein::{self, Target, INPUT_LABEL};
use crate::views::midi_page::MAct;
use crate::views::settings::SAct;

fn canvas() -> Option<Canvas> {
    Canvas::new(1920, 1080, 1.0).ok()
}

/// Five channels, the last an audio clip's.
fn app() -> App {
    let mut a = App::new(None);
    a.state.project = test_project();
    a.state.project.channels[4].kind = ChannelKind::AudioClip;
    let s = &mut a.state;
    s.control.settle(&mut s.project);
    a
}

fn hit(app: &App, a: &Action) -> Rect {
    app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
}

fn has(app: &App, a: &Action) -> bool {
    app.hits.iter().any(|h| &h.action == a)
}

fn hint(app: &App, a: &Action) -> String {
    app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.hint.to_string()).unwrap_or_default()
}

fn click(app: &mut App, c: &mut Canvas, r: Rect) {
    app.mouse_move(r.cx(), r.cy());
    app.paint(c, None);
    app.mouse_down();
    app.mouse_up();
    app.paint(c, None);
}

fn right_click(app: &mut App, c: &mut Canvas, r: Rect) {
    app.mouse_move(r.cx(), r.cy());
    app.paint(c, None);
    app.mouse_down_right();
    app.mouse_up();
    app.paint(c, None);
}

fn press(app: &mut App, key: Key, ctrl: bool) {
    app.key(&key, ctrl, false);
}

fn letter(app: &mut App, l: &str, ctrl: bool) {
    press(app, Key::Character(l.into()), ctrl);
}

fn typed(app: &mut App, text: &str) {
    for ch in text.chars() {
        press(app, Key::Character(ch.to_string().as_str().into()), false);
    }
    press(app, Key::Named(NamedKey::Enter), false);
}

fn same(a: &Canvas, b: &Canvas, what: &str) {
    let n = a.pixmap.data().iter().zip(b.pixmap.data()).filter(|(x, y)| x.abs_diff(**y) > 3).count();
    assert_eq!(n, 0, "{what}: the partial repaint differs from a full one");
}

/// The pixels of `r`, to compare.
fn pixels(c: &Canvas, r: Rect) -> Vec<u8> {
    let w = c.pixmap.width() as usize;
    let data = c.pixmap.data();
    let mut out = Vec::new();
    for y in r.y as usize..r.bottom() as usize {
        out.extend_from_slice(&data[(y * w + r.x as usize) * 4..(y * w + r.right() as usize) * 4]);
    }
    out
}

fn float(app: &mut App, r: Rect) {
    app.state.wins[0] = WinState { rect: r, maxed: false, mini: false };
}

fn tab(w: Win) -> Action {
    Action::PanelTab(w)
}

/// One window, two tabs: a click swaps the pages in one place; the rack is never over the Arranger.
#[test]
fn the_two_tabs_share_one_window() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    assert_eq!(app.state.z, vec![Win::Playlist], "it starts on the Arranger, the rack not over it");
    app.paint(&mut c, None);
    let outer = app.state.win_rect(Win::Playlist);
    assert_eq!(outer, app.state.win_rect(Win::ChannelRack), "the same window");
    for w in [Win::Playlist, Win::ChannelRack] {
        assert!(has(&app, &tab(w)), "both tabs show on the Arranger: {w:?}");
    }
    assert!(has(&app, &Action::PlaylistArea) && !has(&app, &Action::RackArea));

    let r = hit(&app, &tab(Win::ChannelRack));

    click(&mut app, &mut c, r);
    assert_eq!(app.state.z, vec![Win::ChannelRack]);
    assert!(has(&app, &Action::RackArea) && !has(&app, &Action::PlaylistArea));
    assert!(app.pl_grid().is_none(), "no grid to drop on while the rack shows");
    for w in [Win::Playlist, Win::ChannelRack] {
        assert!(has(&app, &tab(w)), "both tabs show on the rack: {w:?}");
    }
    // The rack fills the page: under the strip, all of the window's width and height.
    let page = crate::views::rack::rect(&app.state);
    assert_eq!((page.x, page.right(), page.bottom()), (outer.x, outer.right(), outer.bottom()));
    assert_eq!(page.y, outer.y + crate::views::TAB_H);
    assert!(crate::views::rack::visible_steps(&app.state) > 60, "it takes the space: many more steps than the old 16");

    // The tabs and the window's buttons stay put on both pages.
    let at_rack = (hit(&app, &tab(Win::Playlist)), hit(&app, &Action::CloseWin(Win::ChannelRack)));
    click(&mut app, &mut c, at_rack.0);
    assert_eq!(app.state.z, vec![Win::Playlist]);
    assert_eq!((hit(&app, &tab(Win::Playlist)), hit(&app, &Action::CloseWin(Win::Playlist))), at_rack);
    // A tab press moved off it before the release does nothing.
    let t = hit(&app, &tab(Win::ChannelRack));
    app.mouse_move(t.cx(), t.cy());
    app.paint(&mut c, None);
    app.mouse_down();
    app.mouse_move(t.cx(), t.bottom() + 200.0);
    app.mouse_up();
    assert_eq!(app.state.z, vec![Win::Playlist]);
}

/// F5 and F6 go to a tab, a second press flips to the other, Ctrl+Tab flips; each also fetches a covered or closed window.
#[test]
fn keys_switch_the_tabs() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.paint(&mut c, None);
    let f5 = || Key::Named(NamedKey::F5);
    let f6 = || Key::Named(NamedKey::F6);
    let tab_key = || Key::Named(NamedKey::Tab);
    press(&mut app, f6(), false);
    assert_eq!(app.state.z, vec![Win::ChannelRack]);
    press(&mut app, f5(), false);
    assert_eq!(app.state.z, vec![Win::Playlist]);
    press(&mut app, f5(), false);
    assert_eq!(app.state.z, vec![Win::ChannelRack], "a second press flips: either key alone goes back and forth");
    press(&mut app, f6(), false);
    assert_eq!(app.state.z, vec![Win::Playlist]);
    press(&mut app, tab_key(), true);
    assert_eq!(app.state.z, vec![Win::ChannelRack]);
    press(&mut app, tab_key(), true);
    assert_eq!(app.state.z, vec![Win::Playlist]);

    // Esc on the rack goes back to the Arranger, not to nothing.
    press(&mut app, f6(), false);
    press(&mut app, Key::Named(NamedKey::Escape), false);
    assert_eq!(app.state.z, vec![Win::Playlist]);

    // Covered by the piano roll: the keys bring the window forward first, on the tab asked for.
    app.state.raise(Win::PianoRoll);
    press(&mut app, f6(), false);
    assert_eq!(app.state.z, vec![Win::PianoRoll, Win::ChannelRack]);
    app.state.raise(Win::PianoRoll);
    press(&mut app, tab_key(), true);
    assert_eq!(app.state.z, vec![Win::PianoRoll, Win::ChannelRack], "Ctrl+Tab brings it forward on the tab it was on");
    press(&mut app, tab_key(), true);
    assert_eq!(app.state.z, vec![Win::PianoRoll, Win::Playlist]);

    // Closed: it comes back, Ctrl+Tab on the tab it was on.
    press(&mut app, f6(), false);
    app.act(Action::CloseWin(Win::ChannelRack));
    assert!(!app.state.shown(Win::ChannelRack) && !app.state.shown(Win::Playlist));
    press(&mut app, tab_key(), true);
    assert_eq!(app.state.z.last(), Some(&Win::ChannelRack));
    app.act(Action::CloseWin(Win::ChannelRack));
    press(&mut app, f5(), false);
    assert_eq!(app.state.z.last(), Some(&Win::Playlist));
}

/// A key held down does not flicker between the tabs.
#[test]
fn a_held_key_does_not_flicker() {
    let mut app = app();
    press(&mut app, Key::Named(NamedKey::F6), false);
    app.repeat = true;
    for _ in 0..5 {
        press(&mut app, Key::Named(NamedKey::F6), false);
        press(&mut app, Key::Named(NamedKey::Tab), true);
    }
    app.repeat = false;
    assert_eq!(app.state.z, vec![Win::ChannelRack]);
}

/// Hovering a tab shows its shortcuts, on the hint panel and in a bubble that goes with the mouse, and damage repaints
/// match full ones all along.
#[test]
fn hovering_a_tab_shows_its_shortcuts() {
    let Some(mut a) = canvas() else { return };
    let mut b = canvas().unwrap();
    let mut app = app();
    app.paint(&mut a, None);
    assert!(hint(&app, &tab(Win::Playlist)).contains("F5"));
    assert!(hint(&app, &tab(Win::ChannelRack)).contains("F6"));
    for w in [Win::Playlist, Win::ChannelRack] {
        assert!(hint(&app, &tab(w)).contains("Ctrl+Tab"), "{w:?}");
    }
    let t = hit(&app, &tab(Win::ChannelRack));
    let under = Rect::new(t.x, t.bottom() + 4.0, 150.0, 36.0);
    let before = pixels(&a, under);
    let _ = app.take_damage();

    // Onto the tab: the bubble shows under it.
    app.mouse_move(t.cx(), t.cy());
    let d = app.take_damage();
    match d {
        Some(Some(rects)) => app.paint_region(&mut a, Some(&rects)),
        Some(None) => app.paint(&mut a, None),
        None => {}
    }
    assert_ne!(pixels(&a, under), before, "the bubble");
    assert!(app.hint.contains("F6"), "the hint panel: {}", app.hint);
    app.paint(&mut b, None);
    same(&a, &b, "the bubble shown");

    // Off it again: gone, nothing left behind.
    let _ = app.take_damage();
    app.mouse_move(t.cx() + 400.0, t.cy() + 300.0);
    let d = app.take_damage();
    match d {
        Some(Some(rects)) => app.paint_region(&mut a, Some(&rects)),
        Some(None) => app.paint(&mut a, None),
        None => {}
    }
    assert_eq!(pixels(&a, under), before, "the bubble is gone");
    app.paint(&mut b, None);
    same(&a, &b, "the bubble gone");
}

/// Collapsed to its title bar the window is just its strip, and its tabs still work.
#[test]
fn a_collapsed_window_is_its_strip() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    float(&mut app, Rect::new(320.0, 150.0, 900.0, 500.0));
    app.state.wins[0].mini = true;
    app.paint(&mut c, None);
    let r = app.state.win_rect(Win::Playlist);
    assert_eq!(r.h, crate::views::TAB_H);
    assert!(!has(&app, &Action::PlaylistArea) && !has(&app, &Action::RackArea), "no page");
    assert!(!has(&app, &Action::PlOptions), "nor its title bar");
    assert!(app.hits.iter().filter(|h| h.win == Some(Win::Playlist)).all(|h| h.rect.y >= r.y - 4.0 && h.rect.bottom() <= r.bottom() + 4.0), "nothing past the strip");
    let r = hit(&app, &tab(Win::ChannelRack));
    click(&mut app, &mut c, r);
    assert_eq!(app.state.z, vec![Win::ChannelRack]);
    assert!(!has(&app, &Action::RackArea));
    let r = hit(&app, &Action::MinWin(Win::ChannelRack));
    click(&mut app, &mut c, r);
    assert!(!app.state.wins[0].mini);
    assert!(has(&app, &Action::RackArea), "open again, on the rack");
}

/// Floating, the window moves and resizes by its strip and edges on either tab, and both tabs follow; maximizing from one
/// is maximizing both.
#[test]
fn a_floating_window_moves_and_resizes_on_either_tab() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    float(&mut app, Rect::new(320.0, 150.0, 900.0, 500.0));
    for w in [Win::Playlist, Win::ChannelRack] {
        app.state.raise(w);
        app.paint(&mut c, None);
        let r0 = app.state.wins[0].rect;
        // By the strip, right of the tabs.
        let strip = crate::views::tabs::strip_rect(&app.state, app.state.win_rect(w));
        let (x, y) = (strip.right() - 200.0, strip.cy());
        assert_eq!(app.top_hit(x, y).map(|h| h.action.clone()), Some(Action::WinDrag(w)));
        app.mouse_move(x, y);
        app.paint(&mut c, None);
        app.mouse_down();
        app.mouse_move(x + 40.0, y + 30.0);
        app.mouse_up();
        let r1 = app.state.wins[0].rect;
        assert_eq!((r1.x, r1.y, r1.w, r1.h), (r0.x + 40.0, r0.y + 30.0, r0.w, r0.h), "{w:?} moves");
        // By its right edge.
        app.paint(&mut c, None);
        let (x, y) = (r1.right() + 1.0, r1.cy());
        assert_eq!(app.top_hit(x, y).map(|h| h.action.clone()), Some(Action::WinResize(w, crate::app::EDGE_R)), "{w:?}");
        app.mouse_move(x, y);
        app.paint(&mut c, None);
        app.mouse_down();
        app.mouse_move(x + 64.0, y);
        app.mouse_up();
        assert_eq!(app.state.wins[0].rect.w, r1.w + 64.0, "{w:?} resizes");
        app.state.wins[0].rect = r0;
    }
    // The rack's page follows the window.
    app.state.raise(Win::ChannelRack);
    let (w0, steps0) = (app.state.win_rect(Win::ChannelRack).w, crate::views::rack::visible_steps(&app.state));
    app.state.wins[0].rect.w = w0 + 160.0;
    assert_eq!(crate::views::rack::visible_steps(&app.state), steps0 + 10);
    // From the rack's buttons: the same window maximized.
    app.paint(&mut c, None);
    let r = hit(&app, &Action::MaxWin(Win::ChannelRack));
    click(&mut app, &mut c, r);
    assert!(app.state.is_max(Win::Playlist) && app.state.is_max(Win::ChannelRack));
    assert_eq!(app.state.docked(), Some(Win::ChannelRack));
}

/// A long rack keeps its add button under the last row while there is room, and the graph editor stays on the page.
#[test]
fn the_rack_lays_out_on_a_tall_page() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.state.raise(Win::ChannelRack);
    app.paint(&mut c, None);
    let add = hit(&app, &Action::RackAdd);
    let last = hit(&app, &Action::ChannelSelect(4));
    assert!(add.y >= last.bottom() && add.y < last.bottom() + 12.0, "under the last row: {add:?} / {last:?}");
    // The graph editor under a row near the page's bottom stays on it.
    float(&mut app, Rect::new(320.0, 150.0, 900.0, 330.0));
    app.state.selected_channel = 4;
    app.state.rack.graph_open = true;
    app.paint(&mut c, None);
    let (g, page) = (crate::views::rack::graph_rect(&app.state).unwrap(), crate::views::rack::rect(&app.state));
    assert!(g.bottom() <= page.bottom() && g.y >= page.y, "{g:?} in {page:?}");
    assert!(has(&app, &Action::RackGraphArea));
}

/// What a channel carried from the rack makes on the Arranger: its own clip, the pattern that plays it, or an audio clip of
/// its sample. The tab opens under the carried channel; the press does not click the name button; undo takes it back.
#[test]
fn a_channel_carried_from_the_rack_lands_on_the_arranger() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    let bar = u32::from(app.state.project.ppq) * 4;
    // Channel 1 plays in pattern 3 (not the current one).
    app.state.project.patterns.push(Pattern { number: 3, notes: vec![Note::step(1, 0)], ..Pattern::default() });
    app.state.pattern = 1;
    app.state.raise(Win::ChannelRack);
    app.paint(&mut c, None);

    let carry = |app: &mut App, c: &mut Canvas, row: usize, bars: f32, track_y: f32| {
        app.state.raise(Win::ChannelRack);
        app.paint(c, None);
        let name = hit(app, &Action::ChannelSelect(row));
        app.mouse_move(name.cx(), name.cy());
        app.paint(c, None);
        app.mouse_down();
        assert!(app.state.browser_drag.is_none(), "no drag yet");
        app.mouse_move(name.cx() + 10.0, name.cy() + 10.0);
        assert!(app.state.browser_drag.is_some(), "the label follows the mouse");
        // Over the Arranger's tab: it opens.
        let t = hit(app, &tab(Win::Playlist));
        app.mouse_move(t.cx(), t.cy());
        assert_eq!(app.state.z, vec![Win::Playlist], "the tab opened under the channel");
        app.paint(c, None);
        // Onto the grid.
        let g = app.pl_grid().expect("the Arranger's grid");
        app.mouse_move(g.x + bars * app.state.playlist_bar_w + 2.0, g.y + track_y);
        app.mouse_up();
        assert!(app.state.browser_drag.is_none(), "the label is gone");
        app.paint(c, None);
    };

    // An audio channel: its own clip, where it was dropped, selected and current.
    let n0 = pl::clips(&app.state).len();
    carry(&mut app, &mut c, 4, 3.0, 4.0);
    let clip = *pl::clips(&app.state).last().unwrap();
    assert_eq!(pl::clips(&app.state).len(), n0 + 1);
    assert_eq!((clip.source, clip.track), (ClipSource::Channel(4), 0));
    assert_eq!(clip.position / bar, 3);
    assert_eq!(app.state.playlist.sel, vec![n0]);
    assert_eq!(app.state.playlist.source, Some(ClipSource::Channel(4)));
    assert_eq!(app.state.playlist.picker, PickKind::Audio);
    assert!(app.state.pending_open.is_none() && !app.state.shown(Win::Plugin), "the press did not click the name button");
    // One undo step.
    letter(&mut app, "z", true);
    assert_eq!(pl::clips(&app.state).len(), n0, "undone");

    // A channel with notes: the pattern that plays it.
    carry(&mut app, &mut c, 1, 1.0, 4.0);
    let clip = *pl::clips(&app.state).last().unwrap();
    assert_eq!(clip.source, ClipSource::Pattern(3));
    assert_eq!(app.state.pattern, 3, "that pattern is current");
    assert_eq!(app.state.playlist.picker, PickKind::Pattern);

    // The current pattern when it plays the channel too.
    app.state.project.patterns[0].notes.push(Note::step(1, 0));
    app.state.pattern = 1;
    carry(&mut app, &mut c, 1, 5.0, 4.0);
    assert_eq!(pl::clips(&app.state).last().unwrap().source, ClipSource::Pattern(1));

    // A channel with nothing yet: the current pattern, and it says so.
    carry(&mut app, &mut c, 0, 7.0, 4.0);
    assert_eq!(pl::clips(&app.state).last().unwrap().source, ClipSource::Pattern(1));
    assert!(app.state.message.as_deref().is_some_and(|m| m.contains("no notes")), "{:?}", app.state.message);

    // Released on the rack itself: it just selects the channel, no clip.
    let n = pl::clips(&app.state).len();
    app.state.raise(Win::ChannelRack);
    app.paint(&mut c, None);
    let name = hit(&app, &Action::ChannelSelect(2));
    app.mouse_move(name.cx(), name.cy());
    app.paint(&mut c, None);
    app.mouse_down();
    app.mouse_move(name.cx() + 12.0, name.cy() + 30.0);
    app.mouse_up();
    assert_eq!(pl::clips(&app.state).len(), n);
    assert_eq!(app.state.selected_channel, 2);
    assert!(app.state.pending_open.is_none());

    // A plain click still opens the channel: it is not a drag.
    app.paint(&mut c, None);
    let r = hit(&app, &Action::ChannelSelect(3));
    click(&mut app, &mut c, r);
    assert_eq!(app.state.pending_open, Some(3));
    // Under 4 px is still a click.
    app.state.pending_open = None;
    let name = hit(&app, &Action::ChannelSelect(2));
    app.mouse_move(name.cx(), name.cy());
    app.paint(&mut c, None);
    app.mouse_down();
    app.mouse_move(name.cx() + 2.0, name.cy() + 1.0);
    app.mouse_up();
    assert_eq!(app.state.pending_open, Some(2));
}

/// A sampler with a sample and no notes carried over: an audio clip of its sample, on its own new channel; carried again,
/// another clip of that channel, not another channel. Undo takes the channel back with the clip.
#[test]
fn a_sampler_without_notes_lands_as_an_audio_clip() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    // A short silent WAV: 16-bit mono at 44.1 kHz.
    let frames = 4410u32;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + frames * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&44100u32.to_le_bytes());
    wav.extend_from_slice(&88200u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(frames * 2).to_le_bytes());
    wav.resize(wav.len() + frames as usize * 2, 0);
    let path = std::env::temp_dir().join(format!("lemon_tab_test_{}.wav", std::process::id()));
    std::fs::write(&path, &wav).unwrap();
    let path_text = path.to_string_lossy().into_owned();
    assert!(app.state.project.channels[0].kind.is_sampler());
    app.state.project.channels[0].sample_path = Some(path_text.clone());
    let channels = app.state.project.channels.len();
    let n0 = pl::clips(&app.state).len();

    pl::snapshot(&mut app.state);
    app.state.playlist.sel = vec![];
    let said = pl::place_channel(&mut app.state, 0, 3, 0);
    assert!(said.as_deref().is_some_and(|m| m.starts_with("Added audio clip")), "{said:?}");
    assert_eq!(app.state.project.channels.len(), channels + 1, "a channel for the clip");
    let made = app.state.project.channels.last().unwrap();
    assert_eq!((made.kind, made.sample_path.as_deref()), (ChannelKind::AudioClip, Some(path_text.as_str())));
    let index = made.index;
    let clip = *pl::clips(&app.state).last().unwrap();
    assert_eq!((clip.source, clip.track), (ClipSource::Channel(index), 3));
    assert_eq!(app.state.playlist.sel, vec![n0]);
    assert_eq!(app.state.playlist.picker, PickKind::Audio);

    // Again: the same channel.
    pl::snapshot(&mut app.state);
    let said = pl::place_channel(&mut app.state, 0, 4, 0);
    assert_eq!(said, None);
    assert_eq!(app.state.project.channels.len(), channels + 1, "no second channel");
    assert_eq!(pl::clips(&app.state).last().unwrap().source, ClipSource::Channel(index));
    assert_eq!(app.state.playlist.sel, vec![n0, n0 + 1], "both are selected");

    // A file that cannot be read says so and places nothing.
    app.state.project.channels[1].sample_path = Some("no such file.wav".into());
    let n = pl::clips(&app.state).len();
    pl::snapshot(&mut app.state);
    let said = pl::place_channel(&mut app.state, 1, 5, 0);
    assert!(said.as_deref().is_some_and(|m| m.starts_with("Could not read")), "{said:?}");
    assert_eq!(pl::clips(&app.state).len(), n);
    app.paint(&mut c, None);
    let _ = std::fs::remove_file(&path);
}

/// What is carried from the browser opens a tab the same way, and drops on its page.
#[test]
fn a_browser_item_carried_over_a_tab_opens_it() {
    use crate::app::Drag;
    use crate::views::browser::Item;
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.state.raise(Win::ChannelRack);
    app.paint(&mut c, None);
    app.state.project.patterns[0].notes.push(Note::step(1, 0));
    app.mouse = (100.0, 300.0);
    app.drag = Some(Drag::Browser { item: Item::Pattern(1), label: "Pattern 1".into(), m0: (100.0, 300.0), started: false });
    // Far enough to start, not over a tab: nothing opens.
    app.mouse_move(120.0, 320.0);
    assert_eq!(app.state.z, vec![Win::ChannelRack]);
    let t = hit(&app, &tab(Win::Playlist));
    app.mouse_move(t.cx(), t.cy());
    assert_eq!(app.state.z, vec![Win::Playlist], "the tab opened");
    app.paint(&mut c, None);
    let g = app.pl_grid().expect("the grid");
    let n0 = pl::clips(&app.state).len();
    app.mouse_move(g.x + 40.0, g.y + 4.0);
    app.mouse_up();
    assert_eq!(pl::clips(&app.state).len(), n0 + 1, "dropped on the Arranger");
    assert_eq!(pl::clips(&app.state).last().unwrap().source, ClipSource::Pattern(1));
}

/// Carrying with Shift or Ctrl held keeps those clicks' meanings: no drag starts.
#[test]
fn modified_name_clicks_do_not_carry() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.state.raise(Win::ChannelRack);
    app.paint(&mut c, None);
    let name = hit(&app, &Action::ChannelSelect(1));
    app.shift = true;
    app.mouse_move(name.cx(), name.cy());
    app.paint(&mut c, None);
    app.mouse_down();
    assert!(app.drag.is_none());
    app.mouse_up();
    app.shift = false;
    assert!(app.state.pat_ui.name_box.is_some(), "Shift+click renames");
}

/// Ctrl+C on the rack, Ctrl+V on the Arranger: clips of the copied channels at the song position on the selected track
/// and under it. A copy of clips takes over again; the rack's own paste still pastes steps.
#[test]
fn copied_channels_paste_on_the_arranger() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    let bar = u32::from(app.state.project.ppq) * 4;
    app.state.project.patterns[0].notes.push(Note::step(3, 0));
    app.state.raise(Win::ChannelRack);
    app.paint(&mut c, None);
    app.state.rack.sel = [3usize, 4].into_iter().collect();
    letter(&mut app, "c", true);
    assert_eq!(app.state.rack.copied, vec![3, 4]);

    press(&mut app, Key::Named(NamedKey::F5), false);
    app.paint(&mut c, None);
    app.state.playlist.track = 2;
    app.state.playlist.pos = 4 * bar;
    let n0 = pl::clips(&app.state).len();
    letter(&mut app, "v", true);
    let all = pl::clips(&app.state);
    assert_eq!(all.len(), n0 + 2);
    assert_eq!((all[n0].source, all[n0].track, all[n0].position), (ClipSource::Pattern(1), 2, 4 * bar));
    assert_eq!((all[n0 + 1].source, all[n0 + 1].track, all[n0 + 1].position), (ClipSource::Channel(4), 3, 4 * bar));
    assert_eq!(app.state.playlist.sel, vec![n0, n0 + 1], "what was pasted is selected");
    // One undo step.
    letter(&mut app, "z", true);
    assert_eq!(pl::clips(&app.state).len(), n0);

    // Copying clips takes over.
    letter(&mut app, "v", true);
    assert_eq!(pl::clips(&app.state).len(), n0 + 2, "pastes again");
    app.state.playlist.sel = vec![n0];
    letter(&mut app, "c", true);
    assert!(app.state.rack.copied.is_empty());
    app.state.playlist.pos = 8 * bar;
    letter(&mut app, "v", true);
    let all = pl::clips(&app.state);
    assert_eq!(all.len(), n0 + 3);
    assert_eq!((all[n0 + 2].source, all[n0 + 2].track, all[n0 + 2].position), (ClipSource::Pattern(1), 2, 8 * bar), "the copied clip");

    // On the rack, Ctrl+V pastes the rack's steps, as it always has.
    press(&mut app, Key::Named(NamedKey::F6), false);
    app.state.rack.sel = [3usize].into_iter().collect();
    letter(&mut app, "c", true);
    app.state.rack.sel = [0usize].into_iter().collect();
    letter(&mut app, "v", true);
    assert!(app.state.project.patterns[0].notes.iter().any(|n| n.channel == 0), "row 3's steps on row 0");
    assert_eq!(pl::clips(&app.state).len(), n0 + 3, "no clip");
}

/// Copied rows follow a renumbering of the rack, and a new project starts clean.
#[test]
fn copied_rows_follow_the_rack() {
    let mut app = app();
    app.state.rack.copied = vec![1, 3];
    app.state.rack.sel = [1usize].into_iter().collect();
    crate::rack::run(&mut app, crate::rack::RCmd::MoveSel(1));
    assert_eq!(app.state.rack.copied, vec![2, 3], "row 1 went down a row");
    app.state.rack.reset();
    assert!(app.state.rack.copied.is_empty());
}

/// Pictures of the window for a look: `OS_SHOT_DIR=<dir> cargo test tab_shots -- --ignored`.
#[test]
#[ignore]
fn tab_shots() {
    let dir = std::path::PathBuf::from(std::env::var("OS_SHOT_DIR").expect("OS_SHOT_DIR"));
    for design in os_theme::Design::ALL {
        let name = format!("{design:?}").to_lowercase();
        let Some(mut c) = canvas() else { return };
        let mut app = app();
        app.theme = crate::app::design_theme(design, false);
        let s = &mut app.state;
        for (i, n) in ["Kick", "Clap", "Hat", "Snare", "Vocal take 3 long name"].iter().enumerate() {
            s.project.channels[i].name = Some((*n).into());
        }
        s.project.channels[1].enabled = false;
        s.project.patterns = vec![
            Pattern { number: 1, name: Some("Drums".into()), notes: vec![Note::step(0, 0), Note::step(0, 96), Note::step(2, 48), Note::step(3, 24)], ..Pattern::default() },
            Pattern { number: 2, name: Some("Claps".into()), notes: vec![Note::step(1, 24), Note::step(1, 72)], ..Pattern::default() },
            Pattern { number: 3, ..Pattern::default() },
        ];
        s.playlist.source = Some(ClipSource::Pattern(1));
        let a = pl::place(s, 0, 0);
        s.playlist.source = Some(ClipSource::Pattern(2));
        pl::place(s, 1, 384);
        s.playlist.source = Some(ClipSource::Channel(4));
        pl::place(s, 2, 768);
        s.playlist.source = Some(ClipSource::Pattern(1));
        s.playlist.picker = PickKind::Pattern;
        app.paint(&mut c, None);
        let shot = |c: &Canvas, what: &str| {
            c.pixmap.save_png(dir.join(format!("{name}_{what}.png"))).unwrap();
        };
        shot(&c, "arranger");
        let g = pl::PGeom::new(&app.state, app.pl_grid().unwrap());
        let r = g.clip_rect(&pl::clips(&app.state)[a + 2]);
        click(&mut app, &mut c, Rect::new(r.cx(), r.cy(), 2.0, 2.0));
        shot(&c, "audio_selected");
        let t = hit(&app, &tab(Win::ChannelRack));
        app.mouse_move(t.cx(), t.cy());
        app.paint(&mut c, None);
        shot(&c, "hover_tab");
        let t = hit(&app, &Action::RackMixer(4));
        right_click(&mut app, &mut c, t);
        shot(&c, "picker_box_menu");
        press(&mut app, Key::Named(NamedKey::Escape), false);
        app.state.raise(Win::ChannelRack);
        app.mouse_move(900.0, 900.0);
        app.paint(&mut c, None);
        shot(&c, "rack");
        float(&mut app, Rect::new(340.0, 130.0, 1000.0, 520.0));
        app.paint(&mut c, None);
        shot(&c, "rack_float");
        app.state.raise(Win::Playlist);
        app.paint(&mut c, None);
        shot(&c, "arranger_float");
        app.state.wins[0].maxed = true;
        let t = hit(&app, &Action::Tempo);
        right_click(&mut app, &mut c, t);
        shot(&c, "tempo_menu");
        let i = app.state.playlist.popup.as_ref().unwrap().items.iter().position(|it| it.label == INPUT_LABEL).unwrap();
        let r = hit(&app, &Action::PlPopup(i));
        click(&mut app, &mut c, r);
        shot(&c, "tempo_box");
    }
}

/// Each picker row has its channel's tiny controls, which are the rack's: the drag, the wheel and the menus work, and the
/// rack shows what they set.
#[test]
fn picker_rows_have_the_channel_controls() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.state.playlist.picker = PickKind::Audio;
    app.paint(&mut c, None);
    let row = hit(&app, &Action::PlPick(ClipSource::Channel(4)));
    let (led, pan, vol, track) = (
        hit(&app, &Action::ChannelEnable(4)),
        hit(&app, &Action::Knob(KnobId::ChanPan(4))),
        hit(&app, &Action::Knob(KnobId::ChanVol(4))),
        hit(&app, &Action::RackMixer(4)),
    );
    // Tiny, in the row, side by side.
    for r in [led, pan, vol, track] {
        assert!(r.x >= row.x && r.right() <= row.right() && r.y >= row.y && r.bottom() <= row.bottom(), "{r:?} in {row:?}");
        assert!(r.w <= 22.0 && r.h <= 18.0, "tiny: {r:?}");
    }
    assert!(led.right() <= pan.x && pan.right() <= vol.x && vol.right() <= track.x, "{led:?} {pan:?} {vol:?} {track:?}");

    // On / off.
    let on = app.state.project.channels[4].enabled;
    click(&mut app, &mut c, led);
    assert_eq!(app.state.project.channels[4].enabled, !on);
    // Volume by a drag, and it is an undo step.
    let v0 = app.state.knob(KnobId::ChanVol(4));
    app.mouse_move(vol.cx(), vol.cy());
    app.paint(&mut c, None);
    app.mouse_down();
    app.mouse_move(vol.cx(), vol.cy() - 40.0);
    app.mouse_up();
    assert!(app.state.knob(KnobId::ChanVol(4)) > v0 || v0 == 1.0, "the knob moved");
    let v1 = app.state.knob(KnobId::ChanVol(4));
    letter(&mut app, "z", true);
    assert_eq!(app.state.knob(KnobId::ChanVol(4)), v0, "undone");
    assert_ne!(v1, v0);
    // Pan by the wheel.
    app.paint(&mut c, None);
    let p0 = app.state.knob(KnobId::ChanPan(4));
    app.mouse_move(pan.cx(), pan.cy());
    app.paint(&mut c, None);
    app.wheel(0.0, 1.0, false, false);
    assert!(app.state.knob(KnobId::ChanPan(4)) > p0);
    // The mixer track by the wheel and by a drag.
    let t0 = app.state.project.channels[4].mixer_track;
    app.mouse_move(track.cx(), track.cy());
    app.paint(&mut c, None);
    app.wheel(0.0, 1.0, false, false);
    assert_eq!(app.state.project.channels[4].mixer_track, t0 + 1);
    app.paint(&mut c, None);
    app.mouse_down();
    app.mouse_move(track.cx(), track.cy() - 20.0);
    app.mouse_up();
    assert_eq!(app.state.project.channels[4].mixer_track, t0 + 3);

    // The wheel over the row's name still scrolls the list, and a click on it still picks the source.
    app.paint(&mut c, None);
    click(&mut app, &mut c, Rect::new(row.x + 30.0, row.cy() - 1.0, 2.0, 2.0));
    assert_eq!(app.state.playlist.source, Some(ClipSource::Channel(4)));

    // The rack's menus on them.
    right_click(&mut app, &mut c, track);
    let labels = |app: &App| app.state.playlist.popup.as_ref().map(|p| p.items.iter().map(|i| i.label.to_string()).collect::<Vec<_>>()).unwrap_or_default();
    assert_eq!(labels(&app)[0], INPUT_LABEL, "first on the menu");
    assert_eq!(labels(&app).iter().filter(|l| *l == INPUT_LABEL || *l == "Enter value...").count(), 1, "and only there");
    assert!(labels(&app).iter().any(|l| l == "Assign to free mixer track"), "the rack's menu");
    press(&mut app, Key::Named(NamedKey::Escape), false);
    right_click(&mut app, &mut c, vol);
    assert!(labels(&app).iter().any(|l| l == "Enter value..."), "{:?}", labels(&app));
    press(&mut app, Key::Named(NamedKey::Escape), false);
    right_click(&mut app, &mut c, led);
    assert!(labels(&app).iter().any(|l| l == "Solo"), "{:?}", labels(&app));
    press(&mut app, Key::Named(NamedKey::Escape), false);

    // A number typed in the row's box shows in the rack, where the same box holds it.
    app.paint(&mut c, None);
    right_click(&mut app, &mut c, track);
    let i = app.state.playlist.popup.as_ref().unwrap().items.iter().position(|it| it.label == INPUT_LABEL).unwrap();
    let r = hit(&app, &Action::PlPopup(i));
    click(&mut app, &mut c, r);
    assert!(typein::is_open(&app.state));
    typed(&mut app, "9");
    assert_eq!(app.state.project.channels[4].mixer_track, 9);
    press(&mut app, Key::Named(NamedKey::F6), false);
    app.paint(&mut c, None);
    assert!(hint(&app, &Action::RackMixer(4)).contains("Bus 9"), "{}", hint(&app, &Action::RackMixer(4)));
    assert_eq!(app.state.knob(KnobId::ChanPan(4)), app.state.knob(KnobId::ChanPan(4)));
}

/// A pattern's row shows the channel it plays: the selected one if it plays several and that is one, else the first; an
/// empty pattern's row has no controls. The hints name the channel.
#[test]
fn a_pattern_row_shows_the_channel_it_plays() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.state.project.patterns = vec![
        Pattern { number: 1, notes: vec![Note::step(1, 0), Note::step(3, 24)], ..Pattern::default() },
        Pattern { number: 2, notes: vec![Note::step(2, 0)], name: Some("Hats".into()), ..Pattern::default() },
        Pattern { number: 3, ..Pattern::default() },
    ];
    app.state.playlist.picker = PickKind::Pattern;
    app.state.selected_channel = 0;
    app.paint(&mut c, None);
    assert_eq!(crate::views::playlist::pick_channel(&app.state, ClipSource::Pattern(1)), Some(1), "the first it plays");
    assert_eq!(crate::views::playlist::pick_channel(&app.state, ClipSource::Pattern(2)), Some(2));
    assert_eq!(crate::views::playlist::pick_channel(&app.state, ClipSource::Pattern(3)), None, "empty");
    assert!(has(&app, &Action::RackMixer(1)) && has(&app, &Action::RackMixer(2)));
    assert!(hint(&app, &Action::RackMixer(1)).starts_with("Clap:"), "{}", hint(&app, &Action::RackMixer(1)));
    assert!(!has(&app, &Action::RackMixer(0)) && !has(&app, &Action::RackMixer(3)));
    // The selected channel, when the pattern plays it.
    app.state.selected_channel = 3;
    app.paint(&mut c, None);
    assert_eq!(crate::views::playlist::pick_channel(&app.state, ClipSource::Pattern(1)), Some(3));
    assert!(has(&app, &Action::RackMixer(3)) && !has(&app, &Action::RackMixer(1)));
    // Its knob is that channel's.
    let k = hit(&app, &Action::Knob(KnobId::ChanVol(3)));
    let v0 = app.state.project.channels[3].volume;
    app.mouse_move(k.cx(), k.cy());
    app.paint(&mut c, None);
    app.wheel(0.0, -1.0, false, false);
    assert_ne!(app.state.project.channels[3].volume, v0);
    assert_eq!(app.state.project.channels[1].volume, app.state.project.channels[2].volume, "the others stay");
    // An automation clip's channel too.
    let auto = pl::create_automation(&mut app.state, os_model::target::TEMPO, "Tempo", 0.5);
    app.state.playlist.picker = PickKind::Automation;
    app.paint(&mut c, None);
    let row = app.state.project.channels.iter().position(|c| c.index == auto).unwrap();
    assert!(has(&app, &Action::RackMixer(row)));
}

/// Taller picker rows keep the controls in their title strip.
#[test]
fn tall_rows_keep_the_controls_in_their_title() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.state.playlist.picker = PickKind::Audio;
    let view = 400.0;
    pl::set_pick_row(&mut app.state, view, 80.0);
    app.paint(&mut c, None);
    let row = hit(&app, &Action::PlPick(ClipSource::Channel(4)));
    assert!(row.h > 60.0);
    let k = hit(&app, &Action::Knob(KnobId::ChanVol(4)));
    assert!(k.bottom() <= row.y + 30.0, "{k:?} in the title of {row:?}");
}

/// Selecting a clip lights its source in the picker, on its list, scrolled into view; a marquee does too.
#[test]
fn a_selected_clip_lights_its_source() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    // Many patterns, so the list scrolls.
    app.state.project.patterns = (1..=60).map(|n| Pattern { number: n, notes: vec![Note::step(0, 0)], ..Pattern::default() }).collect();
    let bar = u32::from(app.state.project.ppq) * 4;
    // The audio clip on track 0, the pattern 50 clip on track 1.
    app.state.playlist.source = Some(ClipSource::Channel(4));
    let audio = pl::place(&mut app.state, 0, 0);
    app.state.playlist.source = Some(ClipSource::Pattern(50));
    let pat = pl::place(&mut app.state, 1, 2 * bar);
    app.state.playlist.source = Some(ClipSource::Pattern(1));
    app.state.pattern = 1;
    app.state.playlist.picker = PickKind::Pattern;
    app.state.playlist.pick_scroll = 0.0;
    app.paint(&mut c, None);
    assert!(!has(&app, &Action::PlPick(ClipSource::Pattern(50))), "off the list's view for now");

    let clip_at = |app: &App, i: usize| {
        let g = pl::PGeom::new(&app.state, app.pl_grid().unwrap());
        g.clip_rect(&pl::clips(&app.state)[i])
    };
    // A click on the pattern clip: it is current and in view.
    let r = clip_at(&app, pat);
    click(&mut app, &mut c, Rect::new(r.cx(), r.cy(), 2.0, 2.0));
    assert_eq!(app.state.playlist.source, Some(ClipSource::Pattern(50)));
    assert_eq!(app.state.pattern, 50);
    assert!(has(&app, &Action::PlPick(ClipSource::Pattern(50))), "scrolled into view");
    let row = hit(&app, &Action::PlPick(ClipSource::Pattern(50)));
    let list = hit(&app, &Action::PlPickList);
    assert!(row.y >= list.y && row.bottom() <= list.bottom(), "whole in the list: {row:?} in {list:?}");
    assert!(app.state.playlist.pick_scroll > 0.0);

    // A click on the audio clip: its list opens and its row is lit, unlike when another is.
    let lit_rows = |c: &Canvas, app: &App| pixels(c, hit(app, &Action::PlPick(ClipSource::Channel(4))));
    app.state.playlist.picker = PickKind::Audio;
    app.state.playlist.source = Some(ClipSource::Pattern(1));
    app.state.playlist.sel.clear();
    app.state.playlist.pick_scroll = 0.0;
    app.paint(&mut c, None);
    let unlit = lit_rows(&c, &app);
    app.state.playlist.picker = PickKind::Pattern;
    app.paint(&mut c, None);
    let r = clip_at(&app, audio);
    click(&mut app, &mut c, Rect::new(r.cx(), r.cy(), 2.0, 2.0));
    assert_eq!(app.state.playlist.picker, PickKind::Audio, "its list");
    assert_eq!(app.state.playlist.source, Some(ClipSource::Channel(4)));
    assert_ne!(lit_rows(&c, &app), unlit, "lit");
    // Its channel's controls are right there to change.
    assert!(has(&app, &Action::RackMixer(4)));

    // A marquee over the pattern clip: its list and row, though nothing was clicked; the source stays.
    app.state.playlist.tool = pl::PlTool::Select;
    app.state.playlist.sel.clear();
    app.state.playlist.pick_scroll = 0.0;
    app.paint(&mut c, None);
    let r = clip_at(&app, pat);
    app.mouse_move(r.x - 20.0, r.y - 2.0);
    app.paint(&mut c, None);
    app.mouse_down();
    app.mouse_move(r.right() + 20.0, r.bottom() - 2.0);
    app.paint(&mut c, None);
    app.mouse_up();
    app.paint(&mut c, None);
    assert_eq!(app.state.playlist.sel, vec![pat], "the marquee selected it");
    assert_eq!(app.state.playlist.picker, PickKind::Pattern, "its list");
    assert!(has(&app, &Action::PlPick(ClipSource::Pattern(50))), "in view");
    assert_eq!(app.state.playlist.source, Some(ClipSource::Channel(4)), "what the pencil places did not change");
}

/// Right-clicks `box_rect`, picks Input a number..., types `text` and accepts.
fn input(app: &mut App, c: &mut Canvas, box_rect: Rect, text: &str) {
    right_click(app, c, box_rect);
    let p = app.state.playlist.popup.as_ref().expect("the box's menu");
    let i = p.items.iter().position(|it| it.label == INPUT_LABEL).unwrap_or_else(|| panic!("no {INPUT_LABEL} in {:?}", p.items.iter().map(|i| i.label.to_string()).collect::<Vec<_>>()));
    let r = hit(app, &Action::PlPopup(i));
    click(app, c, r);
    assert!(typein::is_open(&app.state), "the value box opened");
    assert!(app.state.playlist.popup.is_none(), "the menu closed");
    typed(app, text);
    app.paint(c, None);
    assert!(!typein::is_open(&app.state), "accepted");
}

/// The toolbar's tempo: Input a number, with its automation entries kept; an undo step; out of range clamps; nonsense and
/// Esc change nothing.
#[test]
fn input_a_number_on_the_tempo() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.paint(&mut c, None);
    let t = hit(&app, &Action::Tempo);
    let t0 = app.state.tempo;
    input(&mut app, &mut c, t, "128.5");
    assert_eq!(app.state.tempo, 128.5);
    letter(&mut app, "z", true);
    assert_eq!(app.state.tempo, t0, "one undo step");
    input(&mut app, &mut c, t, "9999");
    assert_eq!(app.state.tempo, 522.0, "in range");
    input(&mut app, &mut c, t, "1");
    assert_eq!(app.state.tempo, 10.0);
    input(&mut app, &mut c, t, "120 bpm");
    assert_eq!(app.state.tempo, 120.0);
    input(&mut app, &mut c, t, "fast");
    assert_eq!(app.state.tempo, 120.0, "not a number");
    right_click(&mut app, &mut c, t);
    let labels: Vec<String> = app.state.playlist.popup.as_ref().unwrap().items.iter().map(|i| i.label.to_string()).collect();
    assert_eq!(labels[0], INPUT_LABEL, "first on the menu: {labels:?}");
    assert!(labels.iter().any(|l| l == "Create automation clip"), "{labels:?}");
    assert!(!labels.iter().any(|l| l.contains("MIDI learn")), "{labels:?}");
    let i = 0;
    let r = hit(&app, &Action::PlPopup(i));
    click(&mut app, &mut c, r);
    for ch in "77".chars() {
        press(&mut app, Key::Character(ch.to_string().as_str().into()), false);
    }
    press(&mut app, Key::Named(NamedKey::Escape), false);
    assert_eq!(app.state.tempo, 120.0, "Esc cancels");
    assert!(!typein::is_open(&app.state));
    // The box's ✓ accepts too.
    app.paint(&mut c, None);
    right_click(&mut app, &mut c, t);
    let r = hit(&app, &Action::PlPopup(i));
    click(&mut app, &mut c, r);
    press(&mut app, Key::Character("9".into()), false);
    press(&mut app, Key::Character("0".into()), false);
    app.paint(&mut c, None);
    let r = hit(&app, &Action::TypeIn(typein::TiAct::Accept));
    click(&mut app, &mut c, r);
    assert_eq!(app.state.tempo, 90.0);
}

/// The pattern selector: its menu's Input a number selects the pattern typed.
#[test]
fn input_a_number_on_the_pattern_selector() {
    use crate::views::menus::{item_at, PATTERNS};
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.paint(&mut c, None);
    let r = hit(&app, &Action::PatternNext);
    right_click(&mut app, &mut c, r);
    assert_eq!(app.state.menu_open, Some(PATTERNS));
    let i = (0..60).find(|&i| item_at(PATTERNS, i).is_some_and(|it| it.label == INPUT_LABEL)).expect("the entry");
    let r = hit(&app, &Action::MenuItem(PATTERNS, i));
    click(&mut app, &mut c, r);
    assert!(typein::is_open(&app.state));
    typed(&mut app, "12");
    assert_eq!(app.state.pattern, 12);
    assert_eq!(app.state.playlist.source, Some(ClipSource::Pattern(12)));
    // In range.
    app.paint(&mut c, None);
    let r = hit(&app, &Action::PatternNext);
    right_click(&mut app, &mut c, r);
    let r = hit(&app, &Action::MenuItem(PATTERNS, i));
    click(&mut app, &mut c, r);
    typed(&mut app, "5000");
    assert_eq!(app.state.pattern, crate::patterns::MAX);
    // A picker row's menu does not offer it: the row is not a number box.
    assert!(!crate::views::menus::pattern_popup(0.0, 0.0).items.iter().any(|it| it.label == INPUT_LABEL));
}

/// The rack's number boxes.
#[test]
fn input_a_number_on_the_rack() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.state.raise(Win::ChannelRack);
    app.paint(&mut c, None);
    // The mixer track: an undo step, kept in range.
    let m = hit(&app, &Action::RackMixer(1));
    let t0 = app.state.project.channels[1].mixer_track;
    input(&mut app, &mut c, m, "7");
    assert_eq!(app.state.project.channels[1].mixer_track, 7);
    letter(&mut app, "z", true);
    assert_eq!(app.state.project.channels[1].mixer_track, t0);
    input(&mut app, &mut c, m, "9999");
    assert_eq!(app.state.project.channels[1].mixer_track, crate::mixer::max_insert(&app.state.project));
    input(&mut app, &mut c, m, "master");
    assert_eq!(app.state.project.channels[1].mixer_track, 0);
    // The pattern length: steps, Auto, in range.
    let l = hit(&app, &Action::RackLen);
    input(&mut app, &mut c, l, "32");
    assert_eq!(crate::rack::explicit_len(&app.state), Some(32));
    input(&mut app, &mut c, l, "auto");
    assert_eq!(crate::rack::explicit_len(&app.state), None);
    input(&mut app, &mut c, l, "48");
    assert_eq!(crate::rack::explicit_len(&app.state), Some(48));
    input(&mut app, &mut c, l, "0");
    assert_eq!(crate::rack::explicit_len(&app.state), None);
    // A loop box.
    app.state.rack.loop_mode = true;
    app.paint(&mut c, None);
    let b = hit(&app, &Action::RackLoopBox(2));
    input(&mut app, &mut c, b, "5");
    assert_eq!(crate::rack::loop_box_len(&app.state, 2), 5);
    assert_eq!(crate::rack::row_len(&app.state, 2), 5);
    // Under Advanced looping the box holds a word: no number to type.
    app.state.rack.loop_type = 2;
    app.paint(&mut c, None);
    let b = hit(&app, &Action::RackLoopBox(2));
    right_click(&mut app, &mut c, b);
    assert!(app.state.playlist.popup.is_none(), "nothing to input");
}

/// The Advanced fill dialog's numbers, over a modal dialog: the wheel, then Input a number; Enter in the box does not
/// accept the dialog, and the dialog is still there.
#[test]
fn input_a_number_on_the_fill_dialog() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.state.raise(Win::ChannelRack);
    crate::rack::open_fill(&mut app.state, 0);
    app.paint(&mut c, None);
    let len = hit(&app, &Action::RackFill(crate::rack::FillHit::Number(false)));
    let l0 = app.state.rack.fill.as_ref().unwrap().len;
    app.mouse_move(len.cx(), len.cy());
    app.paint(&mut c, None);
    app.wheel(0.0, 1.0, false, false);
    assert_eq!(app.state.rack.fill.as_ref().unwrap().len, l0 + 1);
    input(&mut app, &mut c, len, "12");
    let f = app.state.rack.fill.as_ref().expect("still open");
    assert_eq!((f.len, f.steps.len()), (12, 12));
    // Fill every: its interval. In range: not above the length.
    app.state.rack.fill.as_mut().unwrap().kind = crate::rack::FillType::Every;
    app.paint(&mut c, None);
    let n = hit(&app, &Action::RackFill(crate::rack::FillHit::Number(true)));
    input(&mut app, &mut c, n, "3");
    assert_eq!(app.state.rack.fill.as_ref().unwrap().amount, 3);
    input(&mut app, &mut c, n, "999");
    assert_eq!(app.state.rack.fill.as_ref().unwrap().amount, 12);
    assert!(app.state.rack.fill.is_some());
    // Its own Enter still accepts it.
    press(&mut app, Key::Named(NamedKey::Enter), false);
    assert!(app.state.rack.fill.is_none());
}

/// The settings window's number boxes.
#[test]
fn input_a_number_in_the_settings() {
    use crate::midi_cfg::{self as cfg, Offset};
    use crate::views::settings as st;
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    // The time settings as a time division: number boxes (as a signature they are lists).
    app.state.project.bar = Some((4, 4));
    app.state.project.bar_as_signature = Some(false);
    app.open_settings(st::PROJECT);
    app.paint(&mut c, None);
    let b = hit(&app, &Action::Settings(SAct::Number(st::BEATS_PER_BAR)));
    input(&mut app, &mut c, b, "7");
    assert_eq!(app.state.project.bar().0, 7);
    assert!(app.state.settings.open, "the window stays");
    letter(&mut app, "z", true);
    assert_ne!(app.state.project.bar().0, 7, "undone");
    input(&mut app, &mut c, b, "999");
    assert_eq!(app.state.project.bar().0, 32, "in range");

    // Settings ▸ MIDI: the offset box, and its reset stays on the menu.
    app.open_settings(st::MIDI);
    app.paint(&mut c, None);
    let off = MAct::Offset(Offset::MasterSync);
    let keep = cfg::offset(Offset::MasterSync);
    let b = hit(&app, &Action::Settings(SAct::Midi(off)));
    input(&mut app, &mut c, b, "42");
    assert_eq!(cfg::offset(Offset::MasterSync), 42);
    right_click(&mut app, &mut c, b);
    let i = app.state.playlist.popup.as_ref().unwrap().items.iter().position(|it| it.label == "Reset").expect("Reset");
    let r = hit(&app, &Action::PlPopup(i));
    click(&mut app, &mut c, r);
    assert_eq!(cfg::offset(Offset::MasterSync), 0, "reset");
    cfg::set_offset(Offset::MasterSync, keep);

    // The PPI ruler.
    let keep = crate::ppi::ppi(app.state.display_scale);
    typein::accept(&mut app.state, Target::Ppi, false, "150");
    app.paint(&mut c, None);
    assert_eq!(crate::ppi::ppi(app.state.display_scale), 150);
    typein::accept(&mut app.state, Target::Ppi, false, "9999");
    app.paint(&mut c, None);
    assert_eq!(crate::ppi::ppi(app.state.display_scale), crate::ppi::MAX);
    crate::ppi::set(keep);
}

/// The Remote control settings box's value boxes, over the modal box; its own keys come back after.
#[test]
fn input_a_number_on_the_remote_box() {
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.paint(&mut c, None);
    app.mouse = (700.0, 500.0);
    app.remote_cmd(crate::remote::RCmd::Link(Target::Knob(KnobId::MasterVol)));
    app.paint(&mut c, None);
    let b = hit(&app, &Action::Remote(RAct::Channel));
    input(&mut app, &mut c, b, "5");
    let l = app.state.link_box.as_ref().expect("still open");
    assert_eq!(l.ch, 4, "the box counts from 1");
    let b = hit(&app, &Action::Remote(RAct::Ctrl));
    input(&mut app, &mut c, b, "74");
    assert_eq!(app.state.link_box.as_ref().unwrap().cc, Some(74));
    input(&mut app, &mut c, b, "---");
    assert_eq!(app.state.link_box.as_ref().unwrap().cc, None);
    let b = hit(&app, &Action::Remote(RAct::Port));
    input(&mut app, &mut c, b, "999");
    assert_eq!(app.state.link_box.as_ref().unwrap().port, Some(255), "in range");
    // A click off the menu closes just the menu; Esc on the menu too; then Esc takes the box.
    right_click(&mut app, &mut c, b);
    assert!(app.state.playlist.popup.is_some());
    press(&mut app, Key::Named(NamedKey::Escape), false);
    assert!(app.state.playlist.popup.is_none() && app.state.link_box.is_some());
    app.paint(&mut c, None);
    right_click(&mut app, &mut c, b);
    app.mouse_move(5.0, 5.0);
    app.paint(&mut c, None);
    app.mouse_down();
    app.mouse_up();
    assert!(app.state.playlist.popup.is_none() && app.state.link_box.is_some());
    press(&mut app, Key::Named(NamedKey::Escape), false);
    assert!(app.state.link_box.is_none());
}

/// A channel's Misc page: a number box's menu offers a number past its list, in range; a knob and the chord list do not.
#[cfg(plugin_windows)]
#[test]
fn input_a_number_on_the_misc_page() {
    use crate::misc_page::{self as mp, M};
    let Some(mut c) = canvas() else { return };
    let mut app = app();
    app.paint(&mut c, None);
    let menu = crate::misc::menu(&app, 0, M::PolyMax, 0.0, 0.0);
    assert_eq!(menu.items[0].label, INPUT_LABEL);
    assert_eq!(menu.items[0].cmd, crate::playlist::Cmd::TypeIn(Target::Misc(0, M::PolyMax)));
    assert_ne!(crate::misc::menu(&app, 0, M::ArpChord, 0.0, 0.0).items[0].label, INPUT_LABEL, "the chord list is names");
    assert_ne!(crate::misc::menu(&app, 0, M::LevelPan, 0.0, 0.0).items[0].label, INPUT_LABEL, "a knob");
    typein::accept(&mut app.state, Target::Misc(0, M::PolyMax), false, "24");
    app.paint(&mut c, None);
    assert_eq!(crate::misc::get(&app.state.project.channels[0].misc, M::PolyMax), 24.0, "past its list of 16");
    let (_, hi) = mp::range_of(M::PolyMax);
    typein::accept(&mut app.state, Target::Misc(0, M::PolyMax), false, "9999");
    app.paint(&mut c, None);
    assert_eq!(crate::misc::get(&app.state.project.channels[0].misc, M::PolyMax), hi, "in range");
}

/// Switching tabs, hovering the controls and changing them repaint as a full paint does, for every design.
#[test]
fn repaints_match_full_paints() {
    for design in os_theme::Design::ALL {
        let Some(mut a) = canvas() else { return };
        let mut b = canvas().unwrap();
        let mut app = app();
        app.theme = crate::app::design_theme(design, false);
        app.state.playlist.picker = PickKind::Audio;
        app.paint(&mut a, None);
        let _ = app.take_damage();
        let step = |app: &mut App, a: &mut Canvas, b: &mut Canvas, what: &str| {
            let d = app.take_damage();
            match d {
                Some(Some(rects)) => app.paint_region(a, Some(&rects)),
                Some(None) => app.paint(a, None),
                None => {}
            }
            app.paint(b, None);
            same(a, b, what);
        };
        // A picker control hovered, dragged, left.
        let k = hit(&app, &Action::Knob(KnobId::ChanVol(4)));
        app.mouse_move(k.cx(), k.cy());
        step(&mut app, &mut a, &mut b, "knob hovered");
        app.mouse_down();
        step(&mut app, &mut a, &mut b, "knob pressed");
        app.mouse_move(k.cx(), k.cy() - 30.0);
        step(&mut app, &mut a, &mut b, "knob dragged");
        app.mouse_up();
        step(&mut app, &mut a, &mut b, "knob released");
        let t = hit(&app, &Action::RackMixer(4));
        app.mouse_move(t.cx(), t.cy());
        step(&mut app, &mut a, &mut b, "box hovered");
        app.wheel(0.0, 1.0, false, false);
        step(&mut app, &mut a, &mut b, "box wheel");
        app.mouse_down();
        step(&mut app, &mut a, &mut b, "box pressed");
        app.mouse_move(t.cx(), t.cy() - 20.0);
        step(&mut app, &mut a, &mut b, "box dragged");
        app.mouse_up();
        step(&mut app, &mut a, &mut b, "box released");
        let l = hit(&app, &Action::ChannelEnable(4));
        app.mouse_move(l.cx(), l.cy());
        step(&mut app, &mut a, &mut b, "LED hovered");
        app.mouse_down();
        app.mouse_up();
        step(&mut app, &mut a, &mut b, "LED clicked");
        // The tabs.
        let t = hit(&app, &tab(Win::ChannelRack));
        app.mouse_move(t.cx(), t.cy());
        step(&mut app, &mut a, &mut b, "tab hovered");
        app.mouse_down();
        step(&mut app, &mut a, &mut b, "tab pressed");
        app.mouse_up();
        step(&mut app, &mut a, &mut b, "rack shown");
        press(&mut app, Key::Named(NamedKey::F5), false);
        step(&mut app, &mut a, &mut b, "arranger by key");
        press(&mut app, Key::Named(NamedKey::Tab), true);
        step(&mut app, &mut a, &mut b, "rack by key");
        // The rack's own knob.
        let k = hit(&app, &Action::Knob(KnobId::ChanVol(1)));
        app.mouse_move(k.cx(), k.cy());
        step(&mut app, &mut a, &mut b, "rack knob hovered");
        app.mouse_down();
        app.mouse_move(k.cx(), k.cy() - 30.0);
        step(&mut app, &mut a, &mut b, "rack knob dragged");
        app.mouse_up();
        step(&mut app, &mut a, &mut b, "rack knob released");
    }
}

/// Every design paints both tabs.
#[test]
fn every_design_paints_both_tabs() {
    for design in os_theme::Design::ALL {
        let Some(mut c) = canvas() else { return };
        let mut app = app();
        app.theme = crate::app::design_theme(design, false);
        for w in [Win::Playlist, Win::ChannelRack] {
            app.state.raise(w);
            app.state.wins[0].maxed = true;
            app.paint(&mut c, None);
            assert!(has(&app, &tab(Win::Playlist)) && has(&app, &tab(Win::ChannelRack)));
            float(&mut app, Rect::new(320.0, 150.0, 900.0, 500.0));
            app.paint(&mut c, None);
            assert!(has(&app, &Action::WinResize(w, crate::app::EDGE_R)));
        }
    }
}
