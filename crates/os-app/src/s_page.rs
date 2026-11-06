// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The S sounds page of a sample's This clip and Source audio tabs: the audio's spectrum over time as a picture (low
//! frequencies at the bottom, the hiss of an S high up, louder brighter), the S sounds found in it marked over it, and
//! what to do with a part chosen: make it an S, an SH, or softer (`os_dsp::sibilant`, done by the app in
//! `crate::clip_fx`). The page draws and takes input as `crate::clip_page` does, which hands it both while it shows.

use std::sync::Arc;

use os_dsp::sibilant::{Change, Kind};
use os_theme::Palette;
use os_ui::{Canvas, Font, Rect};

use crate::clip_page::{self as cp, Button, Drag, Event, Hit, PageUi, PageVals, Pos, Press, Side, Tab, PAGE_W};

/// One S sound found, on the sample's clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoundS {
    pub from_s: f64,
    pub to_s: f64,
    pub kind: Kind,
    pub voiced: bool,
    /// How sure the S or SH call is, 0.5 to 1.
    pub sure: f32,
}

impl FoundS {
    pub fn name(&self) -> &'static str {
        self.kind.name(self.voiced)
    }
}

/// The picture of a part of the audio: `w` x `h` pixels (`0x00RRGGBB`, the top row the highest frequency, `top_hz`) over
/// `span`, seconds of the sample.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Picture {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
    pub span: (f64, f64),
    pub top_hz: f32,
}

/// What the S sounds page shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Sounds {
    pub found: Arc<Vec<FoundS>>,
    /// Still looking through the audio: how far, 0..1.
    pub looking: Option<f32>,
    /// What the line under the title says: how many were found, where this voice's S and SH sit.
    pub status: String,
    /// Where this voice's S and SH sit, Hz: marked beside the picture.
    pub centres: Option<(f32, f32)>,
    /// The part chosen, seconds of the sample.
    pub sel: Option<(f64, f64)>,
    /// What the part chosen sounds like ("an SH, sure"), from the app.
    pub heard: Option<String>,
    pub picture: Option<Arc<Picture>>,
    /// How far a change goes, 0..1.
    pub strength: f32,
    /// The part chosen plays, looped.
    pub playing: bool,
    /// Why a change cannot be made now, if so (the audio still being read, a change being worked out).
    pub busy: Option<String>,
}

impl Default for Sounds {
    fn default() -> Self {
        Sounds {
            found: Arc::default(),
            looking: None,
            status: String::new(),
            centres: None,
            sel: None,
            heard: None,
            picture: None,
            strength: 0.9,
            playing: false,
            busy: None,
        }
    }
}

/// The highest frequency the picture shows, Hz (or half the rate, if lower).
pub const TOP_HZ: f32 = 16000.0;

/// The mel scale: a picture row's place by its frequency, as the ear spaces them.
pub fn mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

pub fn from_mel(m: f32) -> f32 {
    700.0 * (10f32.powf(m / 2595.0) - 1.0)
}

// Page geometry (page coordinates).
const X0: f32 = 52.0;
const X1: f32 = PAGE_W - 8.0;
const RULER_Y: f32 = 88.0;
const STRIP_Y: f32 = RULER_Y + 16.0;
const STRIP_H: f32 = 16.0;
const PIC_Y: f32 = STRIP_Y + STRIP_H;
const PIC_H: f32 = 206.0;
const WAVE_Y: f32 = PIC_Y + PIC_H + 2.0;
const WAVE_H: f32 = 40.0;
const BAR_Y: f32 = WAVE_Y + WAVE_H + 5.0;
const ROW1_Y: f32 = BAR_Y + 22.0;
const ROW2_Y: f32 = ROW1_Y + 30.0;
const NOTE_Y: f32 = ROW2_Y + 32.0;
const BTN_H: f32 = 24.0;

/// The picture: the spectrum over time.
pub fn pic_rect() -> Rect {
    Rect::span(X0, X1, PIC_Y, PIC_Y + PIC_H)
}

pub fn ruler_rect() -> Rect {
    Rect::new(X0, RULER_Y, X1 - X0, 16.0)
}

/// The strip over the picture where the S sounds found are marked.
pub fn strip_rect() -> Rect {
    Rect::new(X0, STRIP_Y, X1 - X0, STRIP_H)
}

fn wave_rect() -> Rect {
    Rect::new(X0, WAVE_Y, X1 - X0, WAVE_H)
}

fn bar() -> Rect {
    cp::bar_rect(pic_rect(), BAR_Y)
}

/// The "Make it" buttons: an S, an SH, softer.
pub fn make_rect(i: usize) -> Rect {
    Rect::new(72.0 + i as f32 * 96.0, ROW1_Y, 90.0, BTN_H)
}

/// The strength bar.
pub fn strength_rect() -> Rect {
    Rect::new(440.0, ROW1_Y + 6.0, 150.0, BTN_H - 12.0)
}

pub fn play_rect() -> Rect {
    Rect::new(8.0, ROW2_Y, 156.0, BTN_H)
}

pub fn prev_rect() -> Rect {
    Rect::new(172.0, ROW2_Y, 148.0, BTN_H)
}

pub fn next_rect() -> Rect {
    Rect::new(326.0, ROW2_Y, 148.0, BTN_H)
}

pub fn look_rect() -> Rect {
    Rect::new(X1 - 120.0, ROW2_Y, 120.0, BTN_H)
}

const MAKES: [Change; 3] = [Change::S, Change::Sh, Change::Softer];

fn make_label(c: Change) -> &'static str {
    match c {
        Change::S => "an S",
        Change::Sh => "an SH",
        Change::Softer => "softer",
    }
}

/// A frequency's row on the picture (page y), the picture reaching `top` Hz.
fn hz_to_y(hz: f32, top: f32) -> f32 {
    PIC_Y + (1.0 - mel(hz.clamp(0.0, top)) / mel(top).max(1.0)) * PIC_H
}

fn top_of(side: &Side) -> f32 {
    side.sounds.picture.as_ref().map_or(TOP_HZ, |p| p.top_hz)
}

/// The S sound found at second `s`, if one is.
fn found_at(side: &Side, s: f64, slack: f64) -> Option<usize> {
    side.sounds.found.iter().position(|f| f.from_s - slack <= s && s <= f.to_s + slack)
}

/// What is at page point (x, y) of the S sounds page.
pub fn hit(_ui: &PageUi, vals: &PageVals, tab: Tab, x: f32, y: f32) -> Option<Hit> {
    let side = vals.side(tab);
    if !side.available {
        return None;
    }
    if cp::loop_rect().contains(x, y) {
        return Some(Hit::Loop);
    }
    for (i, &m) in MAKES.iter().enumerate() {
        if make_rect(i).contains(x, y) {
            return Some(Hit::Make(m));
        }
    }
    let st = strength_rect();
    if Rect::new(st.x - 4.0, st.y - 6.0, st.w + 8.0, st.h + 12.0).contains(x, y) {
        return Some(Hit::Strength);
    }
    for (r, h) in [(play_rect(), Hit::PlayPart), (prev_rect(), Hit::Prev), (next_rect(), Hit::Next), (look_rect(), Hit::Look)] {
        if r.contains(x, y) {
            return Some(h);
        }
    }
    for i in 0..3u8 {
        if cp::zoom_rect(pic_rect(), BAR_Y, i).contains(x, y) {
            return Some(Hit::Zoom(i));
        }
    }
    let b = bar();
    if Rect::new(b.x, b.y - 2.0, b.w, b.h + 4.0).contains(x, y) {
        return Some(Hit::ViewBar);
    }
    if ruler_rect().contains(x, y) {
        return Some(Hit::Ruler);
    }
    let plot = pic_rect();
    if strip_rect().contains(x, y) {
        let slack = cp::x_to_s(side, plot, x + 3.0) - cp::x_to_s(side, plot, x);
        return Some(found_at(side, cp::x_to_s(side, plot, x), slack).map_or(Hit::Picture, Hit::Found));
    }
    if Rect::span(X0, X1, PIC_Y, WAVE_Y + WAVE_H).contains(x, y) {
        return Some(Hit::Picture);
    }
    None
}

/// The part chosen, if any, for a change or playing.
fn sel(side: &Side) -> Option<(f64, f64)> {
    side.sounds.sel.filter(|(a, b)| b > a)
}

/// Choose found sound `i`, bringing it into view.
fn choose(side: &Side, tab: Tab, i: usize, out: &mut Vec<Event>) {
    let Some(f) = side.sounds.found.get(i) else { return };
    let pad = ((f.to_s - f.from_s) * 0.15).max(0.005);
    out.push(Event::Select { tab, sel: Some((f.from_s - pad, f.to_s + pad)) });
    let (a, b) = cp::view_of(side);
    if f.from_s < a || f.to_s > b {
        let w = b - a;
        let mid = (f.from_s + f.to_s) / 2.0;
        let x0 = (mid - w / 2.0).clamp(side.from_s, side.to_s - w);
        out.push(Event::View { tab, view: side.view.map(|_| (x0, x0 + w)) });
    }
}

/// The found sound before (`back`) or after the part chosen (or the middle of the view).
fn step(side: &Side, back: bool) -> Option<usize> {
    let at = match sel(side) {
        Some((a, b)) => (a + b) / 2.0,
        None => {
            let (a, b) = cp::view_of(side);
            if back { b } else { a }
        }
    };
    let f = &side.sounds.found;
    if back {
        f.iter().rposition(|x| (x.from_s + x.to_s) / 2.0 < at - 1e-6)
    } else {
        f.iter().position(|x| (x.from_s + x.to_s) / 2.0 > at + 1e-6)
    }
}

fn strength_at(x: f32) -> f32 {
    let r = strength_rect();
    ((x - r.x) / r.w).clamp(0.0, 1.0)
}

/// A press at page point (x, y).
pub fn press(ui: &mut PageUi, vals: &PageVals, tab: Tab, x: f32, y: f32, b: Button, out: &mut Vec<Event>) -> Press {
    let side = vals.side(tab);
    let Some(h) = hit(ui, vals, tab, x, y) else { return Press::Nothing };
    let plot = pic_rect();
    match (b, h) {
        (Button::Left, Hit::Loop) => out.push(Event::Loop { tab, on: !vals.looping }),
        (Button::Left, Hit::Make(into)) => match sel(side) {
            Some((from, to)) if side.sounds.busy.is_none() => out.push(Event::ChangeS { tab, from, to, into }),
            _ => return Press::Nothing,
        },
        (Button::Left, Hit::Strength) => {
            ui.drag = Some(Drag::Strength { k: strength_at(x) });
            return Press::Capture;
        }
        (Button::Left, Hit::PlayPart) => {
            if sel(side).is_none() && !side.sounds.playing {
                return Press::Nothing;
            }
            out.push(Event::PlayPart { tab });
        }
        (Button::Left, Hit::Prev | Hit::Next) => match step(side, h == Hit::Prev) {
            Some(i) => choose(side, tab, i, out),
            None => return Press::Nothing,
        },
        (Button::Left, Hit::Look) => {
            if side.sounds.looking.is_some() {
                return Press::Nothing;
            }
            out.push(Event::FindS { tab });
        }
        (Button::Left, Hit::Zoom(i)) => return cp::zoom_press(side, tab, i, out),
        (Button::Left, Hit::ViewBar) => return cp::bar_press(ui, side, tab, bar(), x, out),
        (Button::Left, Hit::Ruler) => {
            let at = cp::x_to_s(side, plot, x);
            ui.drag = Some(Drag::Pos { at });
            out.push(Event::SetPos { tab, at });
            return Press::Capture;
        }
        (Button::Left, Hit::Found(i)) => choose(side, tab, i, out),
        (Button::Left, Hit::Picture) => {
            let at = cp::x_to_s(side, plot, x);
            ui.drag = Some(Drag::Sel { anchor: at, at, moved: false });
            return Press::Capture;
        }
        (Button::Right, Hit::Picture | Hit::Found(_)) => {
            if side.sounds.sel.is_none() {
                return Press::Nothing;
            }
            out.push(Event::Select { tab, sel: None });
        }
        _ => return Press::Nothing,
    }
    Press::Repaint
}

/// The mouse moved: a drag goes on, or what is under it is named.
pub fn motion(ui: &mut PageUi, vals: &PageVals, tab: Tab, x: f32, y: f32, out: &mut Vec<Event>) -> bool {
    let side = vals.side(tab);
    let plot = pic_rect();
    match ui.drag.clone() {
        Some(Drag::Sel { anchor, moved, .. }) => {
            let at = cp::x_to_s(side, plot, x);
            let moved = moved || (cp::s_to_x(side, plot, at) - cp::s_to_x(side, plot, anchor)).abs() > 3.0;
            ui.drag = Some(Drag::Sel { anchor, at, moved });
            let (a, b) = (anchor.min(at), anchor.max(at));
            ui.hint = format!("Choosing {} to {} ({:.0} ms)", cp::time_text(a), cp::time_text(b), (b - a) * 1000.0);
            true
        }
        Some(Drag::Strength { .. }) => {
            let k = strength_at(x);
            ui.drag = Some(Drag::Strength { k });
            ui.hint = format!("How far a change goes: {:.0}%", k * 100.0);
            true
        }
        Some(Drag::Pos { .. }) => {
            let at = cp::x_to_s(side, plot, x);
            ui.drag = Some(Drag::Pos { at });
            ui.hint = format!("Song position: {}", cp::time_text(at));
            true
        }
        Some(Drag::View { grab }) => {
            cp::bar_drag(side, tab, bar(), grab, x, out);
            ui.hint = cp::view_hint(side, Hit::ViewBar);
            true
        }
        Some(_) => false,
        None => {
            let h = hit(ui, vals, tab, x, y);
            let changed = h != ui.hover;
            ui.hover = h;
            let text = hint_of(vals, tab, h, x, y);
            let new_text = text != ui.hint;
            ui.hint = text;
            changed || new_text
        }
    }
}

/// The button was let go.
pub fn release(ui: &mut PageUi, tab: Tab, out: &mut Vec<Event>) {
    match ui.drag.take() {
        Some(Drag::Sel { anchor, at, moved: true }) => out.push(Event::Select { tab, sel: Some((anchor.min(at), anchor.max(at))) }),
        // A click without a drag: on a sound found it chooses it (the app knows which); elsewhere the choice goes.
        Some(Drag::Sel { at, moved: false, .. }) => out.push(Event::Select { tab, sel: Some((at, at)) }),
        Some(Drag::Strength { k }) => out.push(Event::Strength { tab, k }),
        Some(Drag::Pos { at }) => out.push(Event::SetPos { tab, at }),
        _ => {}
    }
}

/// The wheel at (x, y): over the picture, its ruler and its strips Ctrl zooms at the mouse, else it scrolls.
pub fn wheel(vals: &PageVals, tab: Tab, x: f32, y: f32, notches: f32, ctrl: bool, out: &mut Vec<Event>) -> bool {
    if x < X0 || !(RULER_Y..BAR_Y + 14.0).contains(&y) {
        return false;
    }
    cp::view_wheel(vals.side(tab), tab, pic_rect(), x, notches, ctrl, out)
}

fn hint_of(vals: &PageVals, tab: Tab, h: Option<Hit>, x: f32, y: f32) -> String {
    let side = vals.side(tab);
    let s = &side.sounds;
    let Some(h) = h else { return String::new() };
    let chosen = sel(side).is_some();
    match h {
        Hit::Loop => {
            if vals.looping {
                "Play loops just this audio while this page shows: click to play the whole song instead".into()
            } else {
                "Play plays the whole song: click to loop just this audio while this page shows".into()
            }
        }
        Hit::Make(c) if !chosen => format!("Make a part {}: choose the part first (drag across the picture, or click an S sound found)", make_label(c)),
        Hit::Make(_) if s.busy.is_some() => s.busy.clone().unwrap_or_default(),
        Hit::Make(Change::S) => "Turn the part chosen into an S: its hiss moves up to where this voice's S sits".into(),
        Hit::Make(Change::Sh) => "Turn the part chosen into an SH: its hiss moves down to where this voice's SH sits".into(),
        Hit::Make(Change::Softer) => "Make the part chosen softer: less of its hiss, the same sound".into(),
        Hit::Strength => format!("How far a change goes: {:.0}% (drag)", s.strength * 100.0),
        Hit::PlayPart if s.playing => "Stop playing the part".into(),
        Hit::PlayPart if !chosen => "Play the part chosen, looped: choose a part first".into(),
        Hit::PlayPart => "Play the part chosen, looped (Space plays it too while it is chosen)".into(),
        Hit::Prev => "Choose the S sound found before".into(),
        Hit::Next => "Choose the S sound found after".into(),
        Hit::Look => "Look through the audio for its S sounds again".into(),
        Hit::Zoom(_) | Hit::ViewBar => cp::view_hint(side, h),
        Hit::Ruler => "Click or drag to place the song position: playback starts here".into(),
        Hit::Found(i) => s.found.get(i).map_or(String::new(), |f| {
            let sure = if f.sure >= 0.8 { "sure" } else if f.sure >= 0.65 { "fairly sure" } else { "not sure" };
            format!("{} ({sure}) at {}, {:.0} ms: click to choose it", f.name(), cp::time_text(f.from_s), (f.to_s - f.from_s) * 1000.0)
        }),
        Hit::Picture => {
            let t = cp::time_text(cp::x_to_s(side, pic_rect(), x));
            if y < PIC_Y + PIC_H {
                let m = (1.0 - (y - PIC_Y) / PIC_H) * mel(top_of(side));
                format!("{t}, {:.1} kHz: drag to choose a part, right-click to let it go", from_mel(m) / 1000.0)
            } else {
                format!("{t}: drag to choose a part")
            }
        }
        _ => String::new(),
    }
}

/// A button of the page: greyed while it has nothing to do.
fn button(c: &mut Canvas, p: &Palette, r: Rect, label: &str, on: bool, hov: bool, lit: bool) {
    let face = if lit { p.a(0x49f1b1).with_alpha(if hov { 80 } else { 55 }) } else if on && hov { p.s(0x3a4043) } else { p.s(0x2b2f31) };
    c.fill_rrect(r, 3.0, face);
    c.text_center(Font::new(13), r, if on { p.tx(0xd4dade) } else { p.tx(0x6a7074) }, label);
}

/// The colour a sound found is marked in.
fn colour(p: &Palette, f: &FoundS) -> os_ui::Color {
    match (f.kind, f.voiced) {
        (Kind::S, false) => p.a(0x49f1b1),
        (Kind::S, true) => p.a(0x8ff7cf),
        (Kind::Sh, false) => p.a(0xffb454),
        (Kind::Sh, true) => p.a(0xffd394),
    }
}

/// Draws the page with its top-left at (ox, oy).
pub fn paint(c: &mut Canvas, p: &Palette, ox: f32, oy: f32, vals: &PageVals, ui: &PageUi, tab: Tab) {
    let side = vals.side(tab);
    let s = &side.sounds;
    let at = |r: Rect| Rect::new(r.x + ox, r.y + oy, r.w, r.h);
    let (text, dim) = (p.tx(0xd4dade), p.tx(0x8c9296));
    let title = format!("S sounds: {}", tab.name());
    c.text(Font::new(15).bold(), ox + 8.0, oy + 4.0, text, &title);
    cp::loop_switch(c, p, at(cp::loop_rect()), vals.looping, tab, ui.hover == Some(Hit::Loop));
    // What was found, what is chosen, how to read the picture.
    let f = Font::new(12);
    let status = match s.looking {
        Some(k) => format!("Looking for S sounds in this audio... {:.0}%", k * 100.0),
        None => s.status.clone(),
    };
    c.text_clip(f, ox + 8.0, oy + 26.0, PAGE_W - 16.0, text, &status);
    let chosen = match (sel(side), &s.heard) {
        (Some((a, b)), Some(h)) => format!("Chosen: {} to {} ({:.0} ms). {h}", cp::time_text(a), cp::time_text(b), (b - a) * 1000.0),
        (Some((a, b)), None) => format!("Chosen: {} to {} ({:.0} ms).", cp::time_text(a), cp::time_text(b), (b - a) * 1000.0),
        (None, _) => "Nothing chosen: drag across the picture to choose a part, or click an S sound found (the marks over it).".into(),
    };
    c.text_clip(f, ox + 8.0, oy + 44.0, PAGE_W - 16.0, if sel(side).is_some() { p.a(0x49f1b1) } else { dim }, &chosen);
    c.text_clip(
        f,
        ox + 8.0,
        oy + 62.0,
        PAGE_W - 16.0,
        dim,
        "The picture is the sound's frequencies over time, louder brighter: an S is hiss high up, an SH the same lower down.",
    );
    c.fill_rect(at(Rect::new(8.0, RULER_Y - 6.0, PAGE_W - 16.0, 1.0)), p.s(0x13191d));

    let plot = pic_rect();
    let pos = match &ui.drag {
        Some(Drag::Pos { at }) => Pos::At(*at),
        _ => side.pos,
    };
    cp::ruler_in(c, p, ox, oy, side, tab, pos, plot, ruler_rect());
    let pic = at(plot);
    let top = top_of(side);
    // The picture, or why there is none yet.
    c.fill_rect(pic, p.s(0x0d1012));
    c.push_clip(pic);
    match &s.picture {
        Some(img) if img.w > 0 => {
            let (x0, x1) = (cp::s_to_x(side, plot, img.span.0) + ox, cp::s_to_x(side, plot, img.span.1) + ox);
            c.image(Rect::new(x0, pic.y, (x1 - x0).max(1.0), pic.h), img.w, img.h, &img.px);
        }
        _ => {
            let why = if s.looking.is_some() { "Reading the audio..." } else { "No picture of this audio yet" };
            c.text_center(Font::new(13), pic, dim, why);
        }
    }
    // Where this voice's S and SH sit: a faint line each, named at the right.
    if let Some((hs, hsh)) = s.centres {
        for (hz, name) in [(hs, "S"), (hsh, "SH")] {
            let y = hz_to_y(hz, top) + oy;
            let mut x = pic.x;
            while x < pic.right() {
                c.fill_rect(Rect::new(x, y.round(), 4.0, 1.0), p.tx(0xffffff).with_alpha(60));
                x += 9.0;
            }
            let label = format!("{name} here, {:.1} kHz", hz / 1000.0);
            let w = c.text_width(Font::new(10), &label);
            c.text(Font::new(10), pic.right() - w - 4.0, y - 13.0, p.tx(0xffffff).with_alpha(150), &label);
        }
    }
    c.pop_clip();
    // The frequency scale left of the picture.
    let sf = Font::new(10);
    for hz in [500.0, 1000.0, 2000.0, 4000.0, 8000.0, 12000.0, 16000.0] {
        if hz > top {
            continue;
        }
        let y = hz_to_y(hz, top) + oy;
        let label = if hz < 1000.0 { format!("{hz:.0} Hz") } else { format!("{:.0} kHz", hz / 1000.0) };
        let w = c.text_width(sf, &label);
        c.text(sf, ox + X0 - w - 5.0, y - 6.0, dim, &label);
        c.fill_rect(Rect::new(ox + X0 - 3.0, y.round(), 3.0, 1.0), dim);
    }
    // The waveform under it.
    let wr = at(wave_rect());
    c.fill_rect(wr, p.s(0x171a1c));
    c.push_clip(wr);
    cp::wave_shape(c, side, plot, wr, p.s(0x58636a));
    c.pop_clip();
    // The sounds found, along the strip over the picture.
    let sr = at(strip_rect());
    c.fill_rect(sr, p.s(0x171a1c));
    c.push_clip(sr);
    let (va, vb) = cp::view_of(side);
    let chosen_found = sel(side).and_then(|(a, b)| s.found.iter().position(|f| f.from_s >= a - 1e-6 && f.to_s <= b + 1e-6));
    for (i, fd) in s.found.iter().enumerate() {
        if fd.to_s < va || fd.from_s > vb {
            continue;
        }
        let (a, b) = (cp::s_to_x(side, plot, fd.from_s) + ox, cp::s_to_x(side, plot, fd.to_s) + ox);
        let col = colour(p, fd);
        let hov = ui.hover == Some(Hit::Found(i));
        let alpha = if Some(i) == chosen_found || hov { 255 } else if fd.sure >= 0.8 { 200 } else { 120 };
        let r = Rect::new(a, sr.y + 2.0, (b - a).max(2.0), sr.h - 4.0);
        c.fill_rrect(r, 2.0, col.with_alpha(alpha));
        if r.w >= c.text_width(Font::condensed(10, 3).bold(), fd.name()) + 4.0 {
            c.text_center(Font::condensed(10, 3).bold(), r, p.s(0x0d1012), fd.name());
        }
        // A faint band down the picture too.
        c.fill_rect(Rect::new(a, pic.y, (b - a).max(1.0), 2.0), col.with_alpha(alpha));
    }
    c.pop_clip();
    // The part chosen, across the picture and the waveform.
    if let Some((a, b)) = sel(side).or(match &ui.drag {
        Some(Drag::Sel { anchor, at, moved: true }) => Some((anchor.min(*at), anchor.max(*at))),
        _ => None,
    }) {
        let span = Rect::span(pic.x, pic.right(), pic.y, wr.bottom());
        c.push_clip(span);
        let (xa, xb) = (cp::s_to_x(side, plot, a) + ox, cp::s_to_x(side, plot, b) + ox);
        let r = Rect::new(xa, span.y, (xb - xa).max(1.0), span.h);
        c.fill_rect(r, p.tx(0xffffff).with_alpha(30));
        c.fill_rect(Rect::new(xa, span.y, 1.0, span.h), p.a(0x49f1b1));
        c.fill_rect(Rect::new(xb, span.y, 1.0, span.h), p.a(0x49f1b1));
        c.pop_clip();
    }
    // The song position down the picture.
    if let Pos::At(t) = pos {
        if (va..=vb).contains(&t) {
            let x = cp::s_to_x(side, plot, t) + ox;
            c.fill_rect(Rect::new(x, sr.y, 1.5, wr.bottom() - sr.y), p.playhead);
        }
    }
    cp::paint_bar(c, p, ox, oy, side, bar(), plot, ui.hover);

    // What to do with the part chosen.
    let chosen = sel(side).is_some();
    c.text(Font::new(13), ox + 8.0, oy + ROW1_Y + 4.0, text, "Make it");
    for (i, &m) in MAKES.iter().enumerate() {
        let on = chosen && s.busy.is_none();
        button(c, p, at(make_rect(i)), make_label(m), on, ui.hover == Some(Hit::Make(m)), false);
    }
    let st = at(strength_rect());
    c.text(Font::new(12), st.x - 64.0, st.y - 1.0, dim, "How far");
    let k = match &ui.drag {
        Some(Drag::Strength { k }) => *k,
        _ => s.strength,
    };
    c.fill_rrect(st, 3.0, p.s(0x2b2f31));
    c.fill_rrect(Rect::new(st.x, st.y, st.w * k, st.h), 3.0, p.a(0x49f1b1).with_alpha(170));
    c.text(Font::new(12), st.right() + 8.0, st.y - 1.0, text, &format!("{:.0}%", k * 100.0));
    let play = if s.playing { "Stop playing" } else { "Play the part" };
    button(c, p, at(play_rect()), play, chosen || s.playing, ui.hover == Some(Hit::PlayPart), s.playing);
    let any = !s.found.is_empty();
    button(c, p, at(prev_rect()), "S sound before", any, ui.hover == Some(Hit::Prev), false);
    button(c, p, at(next_rect()), "S sound after", any, ui.hover == Some(Hit::Next), false);
    button(c, p, at(look_rect()), "Look again", s.looking.is_none(), ui.hover == Some(Hit::Look), false);
    let note = match (&s.busy, tab) {
        (Some(b), _) => b.clone(),
        (None, Tab::Cut) => "A change makes a new copy of the audio for this channel (Undo puts the old one back). Other clips that play the same part change with it.".into(),
        (None, Tab::Whole) => "A change makes a new copy of the audio for this channel, heard on every clip of it (Undo puts the old one back).".into(),
    };
    c.text_clip(Font::new(11), ox + 8.0, oy + NOTE_Y, PAGE_W - 16.0, dim, &note);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip_page::{Page, PageVals};

    fn vals() -> PageVals {
        let found = vec![
            FoundS { from_s: 11.0, to_s: 11.1, kind: Kind::S, voiced: false, sure: 0.9 },
            FoundS { from_s: 14.0, to_s: 14.2, kind: Kind::Sh, voiced: false, sure: 0.95 },
            FoundS { from_s: 18.0, to_s: 18.1, kind: Kind::S, voiced: true, sure: 0.7 },
        ];
        let side = Side {
            available: true,
            from_s: 10.0,
            to_s: 20.0,
            sounds: Sounds { found: Arc::new(found), ..Sounds::default() },
            ..Side::default()
        };
        PageVals { cut: side.clone(), whole: side, looping: true }
    }

    fn ui() -> PageUi {
        let mut ui = PageUi::default();
        ui.set_page(Tab::Cut, Page::Sounds);
        ui
    }

    /// The page x of second `s` (the view all of 10..20 s).
    fn x_of(s: f64) -> f32 {
        let p = pic_rect();
        p.x + ((s - 10.0) / 10.0) as f32 * p.w
    }

    #[test]
    fn dragging_across_the_picture_chooses_a_part_and_a_click_on_a_mark_chooses_that_sound() {
        let v = vals();
        let mut ui = ui();
        let mut out = Vec::new();
        let y = PIC_Y + 50.0;
        assert_eq!(cp::press(&mut ui, &v, Tab::Cut, x_of(12.0), y, Button::Left, &|x, y| (x as i32, y as i32), &mut out), Press::Capture);
        assert!(cp::motion(&mut ui, &v, Tab::Cut, x_of(13.0), y, &mut out));
        assert!(ui.hint.starts_with("Choosing"), "{}", ui.hint);
        cp::release(&mut ui, Tab::Cut, &mut out);
        let [Event::Select { tab: Tab::Cut, sel: Some((a, b)) }] = out[..] else { panic!("{out:?}") };
        assert!((a - 12.0).abs() < 0.02 && (b - 13.0).abs() < 0.02, "{a} {b}");
        // A mark on the strip: that sound, a little either side.
        out.clear();
        let sy = STRIP_Y + STRIP_H / 2.0;
        assert_eq!(hit(&ui, &v, Tab::Cut, x_of(14.1), sy), Some(Hit::Found(1)));
        cp::press(&mut ui, &v, Tab::Cut, x_of(14.1), sy, Button::Left, &|x, y| (x as i32, y as i32), &mut out);
        let [Event::Select { sel: Some((a, b)), .. }] = out[..] else { panic!("{out:?}") };
        assert!(a < 14.0 && a > 13.9 && b > 14.2 && b < 14.3, "{a} {b}");
    }

    #[test]
    fn the_buttons_change_play_and_step_through_what_was_found() {
        let mut v = vals();
        let mut ui = ui();
        let mut out = Vec::new();
        let screen = |x: f32, y: f32| (x as i32, y as i32);
        // Nothing chosen: no change to make.
        let m = make_rect(1);
        assert_eq!(cp::press(&mut ui, &v, Tab::Cut, m.cx(), m.cy(), Button::Left, &screen, &mut out), Press::Nothing);
        assert!(cp::motion(&mut ui, &v, Tab::Cut, m.cx(), m.cy(), &mut out));
        assert!(ui.hint.contains("choose the part first"), "{}", ui.hint);
        // The sound after the start of the view, then a change into an SH.
        let n = next_rect();
        cp::press(&mut ui, &v, Tab::Cut, n.cx(), n.cy(), Button::Left, &screen, &mut out);
        v.apply(&out[0]);
        assert!(matches!(v.cut.sounds.sel, Some((a, _)) if a < 11.0 && a > 10.9), "{:?}", v.cut.sounds.sel);
        out.clear();
        cp::press(&mut ui, &v, Tab::Cut, m.cx(), m.cy(), Button::Left, &screen, &mut out);
        assert!(matches!(out[..], [Event::ChangeS { tab: Tab::Cut, into: Change::Sh, .. }]), "{out:?}");
        // The next one after it, and back.
        out.clear();
        cp::press(&mut ui, &v, Tab::Cut, n.cx(), n.cy(), Button::Left, &screen, &mut out);
        v.apply(&out[0]);
        assert!(matches!(v.cut.sounds.sel, Some((a, _)) if a < 14.0 && a > 13.9));
        out.clear();
        let pr = prev_rect();
        cp::press(&mut ui, &v, Tab::Cut, pr.cx(), pr.cy(), Button::Left, &screen, &mut out);
        v.apply(&out[0]);
        assert!(matches!(v.cut.sounds.sel, Some((a, _)) if a < 11.0));
        // Play the part; the strength bar sends one value when let go.
        out.clear();
        let pl = play_rect();
        cp::press(&mut ui, &v, Tab::Cut, pl.cx(), pl.cy(), Button::Left, &screen, &mut out);
        assert_eq!(out, vec![Event::PlayPart { tab: Tab::Cut }]);
        out.clear();
        let st = strength_rect();
        assert_eq!(cp::press(&mut ui, &v, Tab::Cut, st.x + st.w * 0.25, st.cy(), Button::Left, &screen, &mut out), Press::Capture);
        cp::motion(&mut ui, &v, Tab::Cut, st.x + st.w * 0.5, st.cy(), &mut out);
        cp::release(&mut ui, Tab::Cut, &mut out);
        assert!(matches!(out[..], [Event::Strength { k, .. }] if (k - 0.5).abs() < 0.01), "{out:?}");
        // While a change is being worked out, no other.
        v.cut.sounds.busy = Some("Changing...".into());
        out.clear();
        assert_eq!(cp::press(&mut ui, &v, Tab::Cut, m.cx(), m.cy(), Button::Left, &screen, &mut out), Press::Nothing);
    }

    #[test]
    fn the_picture_zooms_and_scrolls_with_the_wheel() {
        let mut v = vals();
        let mut out = Vec::new();
        let y = PIC_Y + 10.0;
        assert!(!cp::wheel(&mut ui(), &v, Tab::Cut, x_of(15.0), y, -1.0, false, &mut out), "all in view: nothing to scroll");
        assert!(cp::wheel(&mut ui(), &v, Tab::Cut, x_of(15.0), y, 1.0, true, &mut out));
        let Event::View { view: Some((a, b)), .. } = out[0] else { panic!("{out:?}") };
        assert!(a > 10.0 && b < 20.0 && (b - a - 8.0).abs() < 0.01, "{a} {b}");
        assert!(((15.0 - a) / (b - a) - 0.5).abs() < 0.01, "the second under the mouse stays put");
        v.apply(&out[0]);
        out.clear();
        assert!(cp::wheel(&mut ui(), &v, Tab::Cut, x_of(15.0), y, -1.0, false, &mut out), "zoomed: the wheel scrolls");
        let Event::View { view: Some((c, _)), .. } = out[0] else { panic!() };
        assert!(c > a);
        // The picture's rows follow the ear's scale.
        assert!(hz_to_y(8000.0, TOP_HZ) < hz_to_y(4000.0, TOP_HZ));
        assert!((from_mel(mel(5000.0)) - 5000.0).abs() < 0.5);
    }

    #[test]
    fn the_page_fits() {
        assert!(NOTE_Y + 14.0 <= crate::clip_page::PAGE_H);
        assert!(look_rect().x > next_rect().right());
        assert!(strength_rect().x - 64.0 > make_rect(2).right());
    }
}
