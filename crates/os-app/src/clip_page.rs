// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin window's This clip and Source audio tabs on a sample's channel (`os_model::clip_fx`): what the piece or the
//! whole sample is, its plugins, and its automation lanes over its waveform. The frame draws the page in place of the
//! plugin's editor, as it does the Misc tab; values come from the app (`crate::clip_fx::page_vals`), edits go back to it as
//! [`Event`]s. A lane being dragged is kept here until the button is let go: one edit, one undo step.

use std::sync::Arc;

use os_model::clip_fx::{self as fx, Lane};
use os_model::AutoPoint;
use os_theme::Palette;
use os_ui::{Canvas, Color, Font, Rect};

pub const PAGE_W: f32 = 780.0;
pub const PAGE_H: f32 = 500.0;
pub const SLOTS: usize = 10;

/// Which of the two tabs: this clip, or the source audio all its clips come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Cut,
    Whole,
}

impl Tab {
    /// The tab's name.
    pub fn name(self) -> &'static str {
        match self {
            Tab::Cut => "This clip",
            Tab::Whole => "Source audio",
        }
    }

    /// What the tab is for, in its tooltip.
    pub fn about(self) -> &'static str {
        match self {
            Tab::Cut => "This clip: plugins and automation for just this clip in the Arranger",
            Tab::Whole => "Source audio: the sampler, Misc, plugins and automation of the whole file, heard on every clip of it",
        }
    }
}

/// Where the song position stands on a tab.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Pos {
    /// At this second of the sample: playback starts or is here.
    At(f64),
    /// In the song before this clip, or after it.
    Before,
    After,
    /// On no clip of this audio.
    #[default]
    Away,
}

/// One plugin slot as shown.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SlotVals {
    pub name: Option<String>,
    pub on: bool,
    /// 0..1.
    pub mix: f32,
}

/// One automation lane as shown.
#[derive(Clone, Debug, PartialEq)]
pub struct LaneVals {
    pub lane: Lane,
    pub name: String,
    /// The points this tab edits.
    pub own: Vec<AutoPoint>,
    /// What moves it while `own` is empty: on the Cut tab, the whole audio's lane.
    pub inherited: Vec<AutoPoint>,
}

/// A cut on the sample's clock (the Source audio tab).
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub from_s: f64,
    pub to_s: f64,
    /// The lanes it has of its own: the whole audio's do not move it there.
    pub own: Vec<Lane>,
}

/// One tab's contents.
#[derive(Clone, Debug, PartialEq)]
pub struct Side {
    /// `false`: the Cut tab with no piece to show (the window was opened from the rack, or the piece is gone).
    pub available: bool,
    /// Name and value rows of the info panel.
    pub rows: Vec<(String, String)>,
    pub slots: Vec<SlotVals>,
    /// Volume and Pan first, always; then the parameters that have lanes.
    pub lanes: Vec<LaneVals>,
    /// The part of the sample the lane editor spans, in seconds.
    pub from_s: f64,
    pub to_s: f64,
    /// Peaks (min, max) of that part, one per column of the lane editor.
    pub wave: Arc<Vec<(f32, f32)>>,
    pub marks: Vec<Mark>,
    /// The lane the app wants shown (a lane just added): a newer `focus_serial` takes over from the user's last choice.
    pub focus: Lane,
    pub focus_serial: u32,
    /// Where the song position is here.
    pub pos: Pos,
}

impl Default for Side {
    fn default() -> Self {
        Side {
            available: false,
            rows: Vec::new(),
            slots: vec![SlotVals::default(); SLOTS],
            lanes: Vec::new(),
            from_s: 0.0,
            to_s: 1.0,
            wave: Arc::default(),
            marks: Vec::new(),
            focus: Lane::Volume,
            focus_serial: 0,
            pos: Pos::Away,
        }
    }
}

/// What the page shows.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PageVals {
    pub cut: Side,
    pub whole: Side,
    /// Play loops just this audio while the page shows (the loop switch).
    pub looping: bool,
}

impl PageVals {
    pub fn side(&self, t: Tab) -> &Side {
        match t {
            Tab::Cut => &self.cut,
            Tab::Whole => &self.whole,
        }
    }

    fn side_mut(&mut self, t: Tab) -> &mut Side {
        match t {
            Tab::Cut => &mut self.cut,
            Tab::Whole => &mut self.whole,
        }
    }

    /// What an edit does to what the page shows, until the app's own values follow.
    pub fn apply(&mut self, e: &Event) {
        match e {
            Event::SetLane { tab, lane, points } => {
                if let Some(l) = self.side_mut(*tab).lanes.iter_mut().find(|l| l.lane == *lane) {
                    l.own.clone_from(points);
                }
            }
            Event::SetMix { tab, slot, mix } => {
                if let Some(v) = self.side_mut(*tab).slots.get_mut(usize::from(*slot)) {
                    v.mix = *mix;
                }
            }
            Event::ToggleSlot { tab, slot } => {
                if let Some(v) = self.side_mut(*tab).slots.get_mut(usize::from(*slot)) {
                    v.on = !v.on;
                }
            }
            Event::SetPos { tab, at } => self.side_mut(*tab).pos = Pos::At(*at),
            Event::ResetLanes { tab, lanes } => {
                for l in self.side_mut(*tab).lanes.iter_mut().filter(|l| lanes.contains(&l.lane)) {
                    l.own.clear();
                }
            }
            _ => {}
        }
    }
}

/// What the user did on the page, for the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// An empty slot was clicked: the plugin menu for it, at this screen point.
    Pick { tab: Tab, slot: u8, x: i32, y: i32 },
    /// A filled slot's menu, at this screen point.
    SlotMenu { tab: Tab, slot: u8, x: i32, y: i32 },
    /// A filled slot's name was clicked: its plugin's window.
    OpenSlot { tab: Tab, slot: u8 },
    /// A slot's LED.
    ToggleSlot { tab: Tab, slot: u8 },
    /// A slot's mix bar was let go here.
    SetMix { tab: Tab, slot: u8, mix: f32 },
    /// "Add a plugin parameter": the list of parameters, at this screen point.
    AddLane { tab: Tab, x: i32, y: i32 },
    /// A lane's points after an edit (empty: it has none any more).
    SetLane { tab: Tab, lane: Lane, points: Vec<AutoPoint> },
    /// The time ruler was clicked or dragged: the song position goes to this second of the sample.
    SetPos { tab: Tab, at: f64 },
    /// These lanes back to their baseline (their own points gone), in one edit: "Reset volume & pan".
    ResetLanes { tab: Tab, lanes: Vec<Lane> },
    /// The loop switch: whether Play plays just this audio, looped, while the page shows.
    Loop { tab: Tab, on: bool },
}

/// Mouse buttons the page takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// What a press asks of the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// Nothing happened there.
    Nothing,
    /// Repaint.
    Repaint,
    /// Repaint and keep the mouse until the button is let go.
    Capture,
}

/// A drag in progress.
#[derive(Clone, Debug, PartialEq)]
enum Drag {
    Point { lane: Lane, i: usize, moved: bool },
    Mix { slot: u8, mix: f32 },
    /// The time ruler: where the song position is being put.
    Pos { at: f64 },
}

/// The page's own state: per tab the lane shown, what is being dragged, the hint.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageUi {
    /// The lane the user chose per tab, with the `focus_serial` it was chosen under.
    chosen: [Option<(Lane, u32)>; 2],
    /// Rows of the lane list scrolled away, per tab.
    scroll: [usize; 2],
    drag: Option<Drag>,
    /// The lane being edited, while the drag lasts.
    preview: Option<Vec<AutoPoint>>,
    /// Under the mouse.
    pub hover: Option<Hit>,
    /// What the title bar says about it.
    pub hint: String,
}

impl PageUi {
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

/// What a point of the page is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    SlotName(u8),
    SlotLed(u8),
    SlotMix(u8),
    /// A row of the lane list: its place in `Side::lanes`.
    LaneRow(usize),
    AddLane,
    /// The button that puts the lane shown back to its baseline.
    Clear,
    /// "Reset volume & pan", on the automation heading.
    ResetBoth,
    /// The loop switch, right of the page's name.
    Loop,
    /// A point of the lane shown.
    Point(usize),
    /// The lane editor, away from any point.
    Plot,
    /// The time ruler over the lane editor: it places the song position.
    Ruler,
}

fn tab_at(t: Tab) -> usize {
    match t {
        Tab::Cut => 0,
        Tab::Whole => 1,
    }
}

// Page geometry (page coordinates).
const LEFT_X: f32 = 8.0;
const LEFT_W: f32 = 212.0;
const RIGHT_X: f32 = 230.0;
const INFO_H: f32 = 84.0;
const SLOTS_Y: f32 = INFO_H + 26.0;
const ROW_H: f32 = 20.0;
const LANES_Y: f32 = SLOTS_Y + SLOTS as f32 * ROW_H + 26.0;
const BOTTOM: f32 = PAGE_H - 8.0;
const AXIS_H: f32 = 16.0;
const RULER_H: f32 = 16.0;
const PLOT_Y: f32 = INFO_H + 32.0;

fn rows_shown() -> usize {
    ((BOTTOM - LANES_Y) / ROW_H) as usize
}

pub fn slot_rect(k: usize) -> Rect {
    Rect::new(LEFT_X, SLOTS_Y + k as f32 * ROW_H, LEFT_W, ROW_H - 1.0)
}

pub fn led_rect(k: usize) -> Rect {
    let r = slot_rect(k);
    Rect::new(r.x, r.y, 20.0, r.h)
}

fn mix_rect(k: usize) -> Rect {
    let r = slot_rect(k);
    Rect::new(r.right() - 46.0, r.y + 5.0, 42.0, r.h - 10.0)
}

pub fn lane_row_rect(row: usize) -> Rect {
    Rect::new(LEFT_X, LANES_Y + row as f32 * ROW_H, LEFT_W, ROW_H - 1.0)
}

fn clear_rect() -> Rect {
    Rect::new(PAGE_W - 8.0 - 116.0, INFO_H + 6.0, 116.0, 20.0)
}

/// "Reset volume & pan", at the right of the automation heading.
fn reset_both_rect() -> Rect {
    Rect::new(LEFT_X + LEFT_W - 124.0, LANES_Y - 22.0, 124.0, 18.0)
}

/// The loop switch, right of the page's name at the top.
pub fn loop_rect() -> Rect {
    Rect::new(PAGE_W - 8.0 - 196.0, 2.0, 196.0, 20.0)
}

/// What the lane shown's reset button says: the baseline it goes back to.
fn reset_label(l: &LaneVals, tab: Tab) -> &'static str {
    match (tab, l.inherited.is_empty(), l.lane) {
        (Tab::Cut, false, _) => "Follow source audio",
        (_, _, Lane::Volume) => "Reset to 0 dB",
        (_, _, Lane::Pan) => "Reset to centre",
        _ => "Remove line",
    }
}

/// Whether Volume or Pan has points of its own on this tab: "Reset volume & pan" has something to do.
fn vol_pan_own(side: &Side) -> Vec<Lane> {
    side.lanes.iter().filter(|l| matches!(l.lane, Lane::Volume | Lane::Pan) && !l.own.is_empty()).map(|l| l.lane).collect()
}

/// The time ruler over the lane editor's plot.
pub fn ruler_rect() -> Rect {
    Rect::new(RIGHT_X, PLOT_Y, plot_rect().w, RULER_H)
}

/// The lane editor's plot.
pub fn plot_rect() -> Rect {
    Rect::span(RIGHT_X, PAGE_W - 8.0, PLOT_Y + RULER_H, BOTTOM - AXIS_H)
}

/// The lane the tab shows: the app's when it asked last, else the user's; Volume when that one is gone.
pub fn shown_lane(ui: &PageUi, side: &Side, tab: Tab) -> Lane {
    let want = match ui.chosen[tab_at(tab)] {
        Some((l, serial)) if serial >= side.focus_serial => l,
        _ if side.focus_serial > 0 => side.focus,
        _ => Lane::Volume,
    };
    if side.lanes.iter().any(|l| l.lane == want) { want } else { Lane::Volume }
}

fn shown<'a>(ui: &PageUi, side: &'a Side, tab: Tab) -> Option<&'a LaneVals> {
    let want = shown_lane(ui, side, tab);
    side.lanes.iter().find(|l| l.lane == want)
}

/// The points the editor edits: the drag's while it lasts.
fn points<'a>(ui: &'a PageUi, lane: &'a LaneVals) -> &'a [AutoPoint] {
    match (&ui.drag, &ui.preview) {
        (Some(Drag::Point { lane: l, .. }), Some(p)) if *l == lane.lane => p,
        _ => &lane.own,
    }
}

/// Seconds of the sample at plot x, and the x of a time.
fn x_to_s(side: &Side, plot: Rect, x: f32) -> f64 {
    let f = f64::from(((x - plot.x) / plot.w.max(1.0)).clamp(0.0, 1.0));
    side.from_s + f * (side.to_s - side.from_s)
}

fn s_to_x(side: &Side, plot: Rect, s: f64) -> f32 {
    let span = (side.to_s - side.from_s).max(1e-9);
    plot.x + ((s - side.from_s) / span) as f32 * plot.w
}

/// A lane's value at plot y (the top is 1), and the y of a value.
fn y_to_v(plot: Rect, y: f32) -> f64 {
    f64::from(1.0 - (y - plot.y) / plot.h.max(1.0)).clamp(0.0, 1.0)
}

fn v_to_y(plot: Rect, v: f64) -> f32 {
    plot.y + (1.0 - v.clamp(0.0, 1.0) as f32) * plot.h
}

/// A point of the lane within reach of (x, y).
fn point_at(side: &Side, plot: Rect, pts: &[AutoPoint], x: f32, y: f32) -> Option<usize> {
    let mut best = None;
    let mut near = 8.0f32 * 8.0;
    for (i, p) in pts.iter().enumerate() {
        let (dx, dy) = (s_to_x(side, plot, p.beat) - x, v_to_y(plot, p.value) - y);
        let d = dx * dx + dy * dy;
        if d <= near {
            near = d;
            best = Some(i);
        }
    }
    best
}

/// What is at page point (x, y).
pub fn hit(ui: &PageUi, vals: &PageVals, tab: Tab, x: f32, y: f32) -> Option<Hit> {
    let side = vals.side(tab);
    if !side.available {
        return None;
    }
    if loop_rect().contains(x, y) {
        return Some(Hit::Loop);
    }
    if reset_both_rect().contains(x, y) {
        return Some(Hit::ResetBoth);
    }
    for k in 0..SLOTS {
        if slot_rect(k).contains(x, y) {
            let filled = side.slots.get(k).is_some_and(|s| s.name.is_some());
            if filled && led_rect(k).contains(x, y) {
                return Some(Hit::SlotLed(k as u8));
            }
            if filled && mix_rect(k).contains(x, y) {
                return Some(Hit::SlotMix(k as u8));
            }
            return Some(Hit::SlotName(k as u8));
        }
    }
    let scroll = ui.scroll[tab_at(tab)];
    for row in 0..rows_shown() {
        if lane_row_rect(row).contains(x, y) {
            let at = row + scroll;
            return match at.cmp(&side.lanes.len()) {
                std::cmp::Ordering::Less => Some(Hit::LaneRow(at)),
                std::cmp::Ordering::Equal => Some(Hit::AddLane),
                std::cmp::Ordering::Greater => None,
            };
        }
    }
    if ruler_rect().contains(x, y) {
        return Some(Hit::Ruler);
    }
    let lane = shown(ui, side, tab)?;
    if clear_rect().contains(x, y) {
        return Some(Hit::Clear);
    }
    let plot = plot_rect();
    if plot.contains(x, y) {
        return Some(point_at(side, plot, points(ui, lane), x, y).map_or(Hit::Plot, Hit::Point));
    }
    None
}

/// A press at page point (x, y). `screen` gives the screen point of a page point, for the app's menus.
pub fn press(ui: &mut PageUi, vals: &PageVals, tab: Tab, x: f32, y: f32, b: Button, screen: &dyn Fn(f32, f32) -> (i32, i32), out: &mut Vec<Event>) -> Press {
    let side = vals.side(tab);
    let Some(h) = hit(ui, vals, tab, x, y) else { return Press::Nothing };
    let (sx, sy) = screen(x, y);
    match (b, h) {
        (Button::Left, Hit::SlotName(k)) => {
            let filled = side.slots.get(usize::from(k)).is_some_and(|s| s.name.is_some());
            out.push(if filled { Event::OpenSlot { tab, slot: k } } else { Event::Pick { tab, slot: k, x: sx, y: sy } });
            Press::Repaint
        }
        (Button::Right, Hit::SlotName(k) | Hit::SlotLed(k) | Hit::SlotMix(k)) => {
            let filled = side.slots.get(usize::from(k)).is_some_and(|s| s.name.is_some());
            out.push(if filled { Event::SlotMenu { tab, slot: k, x: sx, y: sy } } else { Event::Pick { tab, slot: k, x: sx, y: sy } });
            Press::Repaint
        }
        (Button::Left, Hit::SlotLed(k)) => {
            out.push(Event::ToggleSlot { tab, slot: k });
            Press::Repaint
        }
        (Button::Left, Hit::SlotMix(k)) => {
            ui.drag = Some(Drag::Mix { slot: k, mix: mix_at(k, x) });
            Press::Capture
        }
        (Button::Left, Hit::LaneRow(i)) => {
            ui.chosen[tab_at(tab)] = side.lanes.get(i).map(|l| (l.lane, side.focus_serial));
            Press::Repaint
        }
        (Button::Left, Hit::AddLane) => {
            out.push(Event::AddLane { tab, x: sx, y: sy });
            Press::Repaint
        }
        (Button::Left, Hit::Clear) => {
            match shown(ui, side, tab) {
                Some(l) if !l.own.is_empty() => out.push(Event::SetLane { tab, lane: l.lane, points: Vec::new() }),
                _ => return Press::Nothing,
            }
            Press::Repaint
        }
        (Button::Left, Hit::ResetBoth) => {
            let lanes = vol_pan_own(side);
            if lanes.is_empty() {
                return Press::Nothing;
            }
            out.push(Event::ResetLanes { tab, lanes });
            Press::Repaint
        }
        (Button::Left, Hit::Loop) => {
            out.push(Event::Loop { tab, on: !vals.looping });
            Press::Repaint
        }
        (Button::Left, Hit::Ruler) => {
            let at = x_to_s(side, plot_rect(), x);
            ui.drag = Some(Drag::Pos { at });
            out.push(Event::SetPos { tab, at });
            Press::Capture
        }
        (Button::Left, Hit::Point(i)) => {
            let Some(l) = shown(ui, side, tab) else { return Press::Nothing };
            ui.preview = Some(l.own.clone());
            ui.drag = Some(Drag::Point { lane: l.lane, i, moved: false });
            Press::Capture
        }
        (Button::Left, Hit::Plot) => {
            // A new point on the lane, held by the mouse: a cut's first point starts its own lane.
            let Some(l) = shown(ui, side, tab) else { return Press::Nothing };
            let plot = plot_rect();
            let mut pts = l.own.clone();
            let i = fx::lane_add(&mut pts, x_to_s(side, plot, x), y_to_v(plot, y));
            ui.preview = Some(pts);
            ui.drag = Some(Drag::Point { lane: l.lane, i, moved: true });
            Press::Capture
        }
        (Button::Right, Hit::Point(i)) => {
            // A right click takes a point away; without points the lane is gone and what it hid moves the cut again.
            let Some(l) = shown(ui, side, tab) else { return Press::Nothing };
            let mut pts = l.own.clone();
            if i < pts.len() {
                pts.remove(i);
                out.push(Event::SetLane { tab, lane: l.lane, points: pts });
            }
            Press::Repaint
        }
        _ => Press::Nothing,
    }
}

fn mix_at(k: u8, x: f32) -> f32 {
    let r = mix_rect(usize::from(k));
    ((x - r.x) / r.w).clamp(0.0, 1.0)
}

/// The mouse moved while a drag lasts (or not): whether the page changed.
pub fn motion(ui: &mut PageUi, vals: &PageVals, tab: Tab, x: f32, y: f32) -> bool {
    let side = vals.side(tab);
    match ui.drag.clone() {
        Some(Drag::Point { lane, i, .. }) => {
            let plot = plot_rect();
            if let Some(p) = ui.preview.as_mut() {
                fx::lane_move(p, i, x_to_s(side, plot, x), y_to_v(plot, y));
            }
            ui.drag = Some(Drag::Point { lane, i, moved: true });
            ui.hint = point_text(side, ui.preview.as_ref().and_then(|p| p.get(i)), lane);
            true
        }
        Some(Drag::Mix { slot, .. }) => {
            let mix = mix_at(slot, x);
            ui.drag = Some(Drag::Mix { slot, mix });
            ui.hint = format!("Mix: {:.0}%", mix * 100.0);
            true
        }
        Some(Drag::Pos { .. }) => {
            let at = x_to_s(side, plot_rect(), x);
            ui.drag = Some(Drag::Pos { at });
            ui.hint = format!("Song position: {}", time_text(at));
            true
        }
        None => {
            let h = hit(ui, vals, tab, x, y);
            let changed = h != ui.hover;
            ui.hover = h;
            let text = hint_of(ui, vals, tab, h, x, y);
            let new_text = text != ui.hint;
            ui.hint = text;
            changed || new_text
        }
    }
}

/// The button was let go: a drag's result goes to the app.
pub fn release(ui: &mut PageUi, tab: Tab, out: &mut Vec<Event>) {
    match ui.drag.take() {
        Some(Drag::Point { lane, moved: true, .. }) => {
            if let Some(points) = ui.preview.take() {
                out.push(Event::SetLane { tab, lane, points });
            }
        }
        Some(Drag::Mix { slot, mix }) => out.push(Event::SetMix { tab, slot, mix }),
        Some(Drag::Pos { at }) => out.push(Event::SetPos { tab, at }),
        _ => {}
    }
    ui.preview = None;
}

/// The wheel over the page: it scrolls the lane list. Whether anything moved.
pub fn wheel(ui: &mut PageUi, vals: &PageVals, tab: Tab, y: f32, notches: f32) -> bool {
    if y < LANES_Y || notches == 0.0 {
        return false;
    }
    let rows = vals.side(tab).lanes.len() + 1;
    let most = rows.saturating_sub(rows_shown());
    let at = &mut ui.scroll[tab_at(tab)];
    let was = *at;
    *at = if notches > 0.0 { at.saturating_sub(1) } else { (*at + 1).min(most) };
    *at != was
}

/// A lane's value as the user reads it.
pub fn value_text(lane: Lane, v: f64) -> String {
    match lane {
        Lane::Volume => {
            let g = os_engine::lane_gain(v);
            if g > 0.0 { format!("{:+.1} dB", 20.0 * g.log10()) } else { "-inf dB".into() }
        }
        Lane::Pan => {
            let p = (2.0 * v - 1.0) * 100.0;
            if p.abs() < 0.5 { "Centre".into() } else if p < 0.0 { format!("{:.0}% left", -p) } else { format!("{p:.0}% right") }
        }
        Lane::Param { .. } => format!("{:.0}%", v * 100.0),
    }
}

/// A time on the sample's clock.
pub fn time_text(s: f64) -> String {
    let s = s.max(0.0);
    let m = (s / 60.0).floor();
    format!("{:02.0}:{:06.3}", m, s - m * 60.0)
}

fn point_text(side: &Side, p: Option<&AutoPoint>, lane: Lane) -> String {
    let Some(p) = p else { return String::new() };
    let name = side.lanes.iter().find(|l| l.lane == lane).map_or("", |l| l.name.as_str());
    format!("{name}: {}  at {}", value_text(lane, p.value), time_text(p.beat))
}

fn hint_of(ui: &PageUi, vals: &PageVals, tab: Tab, h: Option<Hit>, x: f32, y: f32) -> String {
    let side = vals.side(tab);
    let Some(h) = h else { return String::new() };
    match h {
        Hit::SlotName(k) => match side.slots.get(usize::from(k)).and_then(|s| s.name.as_ref()) {
            Some(n) => format!("{n}: click to open, right-click for its menu"),
            None => match tab {
                Tab::Cut => "Add a plugin to this clip only".into(),
                Tab::Whole => "Add a plugin to the source audio, heard on every clip of it".into(),
            },
        },
        Hit::SlotLed(_) => "Plugin on / off".into(),
        Hit::SlotMix(k) => format!("Mix: {:.0}%", side.slots.get(usize::from(k)).map_or(100.0, |s| s.mix * 100.0)),
        Hit::LaneRow(i) => side.lanes.get(i).map_or(String::new(), |l| format!("Show the {} line", l.name)),
        Hit::AddLane => "Automate a parameter of a plugin here".into(),
        Hit::Ruler => "Click or drag to place the song position: playback starts here".into(),
        Hit::Clear => match shown(ui, side, tab) {
            Some(l) if l.own.is_empty() => format!("{} is at its baseline already", l.name),
            Some(l) if tab == Tab::Cut && !l.inherited.is_empty() => format!("Remove this clip's own {} line: it follows the source audio's again", l.name),
            Some(l) => match l.lane {
                Lane::Volume => "Remove this volume line: back to the baseline, 0 dB (as recorded)".into(),
                Lane::Pan => "Remove this pan line: back to the baseline, the centre".into(),
                Lane::Param { .. } => "Remove this line: the parameter keeps the value its plugin has".into(),
            },
            None => String::new(),
        },
        Hit::ResetBoth => {
            if vol_pan_own(side).is_empty() {
                "Volume and pan are at their baselines already".into()
            } else {
                "Reset volume and pan: both back to their baselines (0 dB, centre) in one step".into()
            }
        }
        Hit::Loop => {
            if vals.looping {
                "Play loops just this audio while this page shows: click to play the whole song instead".into()
            } else {
                "Play plays the whole song: click to loop just this audio while this page shows".into()
            }
        }
        Hit::Point(i) => {
            let Some(l) = shown(ui, side, tab) else { return String::new() };
            format!("{}  (drag to move, right-click to remove)", point_text(side, points(ui, l).get(i), l.lane))
        }
        Hit::Plot => {
            let plot = plot_rect();
            let Some(l) = shown(ui, side, tab) else { return String::new() };
            format!("Click to add a point: {}  at {}", value_text(l.lane, y_to_v(plot, y)), time_text(x_to_s(side, plot, x)))
        }
    }
}

/// Status of a lane for a row's dot: its own points, following another's, or plain.
fn lane_state(l: &LaneVals) -> u8 {
    if !l.own.is_empty() {
        2
    } else if !l.inherited.is_empty() {
        1
    } else {
        0
    }
}

/// What the editor's top line says about the lane shown.
fn status_text(l: &LaneVals, tab: Tab, side: &Side) -> String {
    match (tab, l.own.is_empty(), l.inherited.is_empty()) {
        (Tab::Cut, false, false) => "This clip's own line: the source audio's does not touch it here.".into(),
        (Tab::Cut, false, true) => "This clip's own line.".into(),
        (Tab::Cut, true, false) => "Following the source audio's line. Click to draw this clip's own.".into(),
        (Tab::Cut, true, true) => "No automation. Click to draw this clip's own line.".into(),
        (Tab::Whole, _, _) => {
            let n = side.marks.iter().filter(|m| m.own.contains(&l.lane)).count();
            match (l.own.is_empty(), n) {
                (true, 0) => "No automation. Click to draw a line for every clip of this audio.".into(),
                (true, n) => format!("No line here. {n} clip(s) have their own."),
                (false, 0) => "Moves every clip of this audio.".into(),
                (false, 1) => "Moves every clip of this audio but the 1 with its own line.".into(),
                (false, n) => format!("Moves every clip of this audio but the {n} with their own line."),
            }
        }
    }
}

/// Draws the page with its top-left at (ox, oy).
pub fn paint(c: &mut Canvas, p: &Palette, ox: f32, oy: f32, vals: &PageVals, ui: &PageUi, tab: Tab) {
    let side = vals.side(tab);
    let at = |r: Rect| Rect::new(r.x + ox, r.y + oy, r.w, r.h);
    let (text, dim, line) = (p.tx(0xd4dade), p.tx(0x8c9296), p.s(0x13191d));
    if !side.available {
        let msg = "Double-click a clip of this audio in the Arranger to see it here.";
        c.text_center(Font::new(15), at(Rect::new(0.0, 0.0, PAGE_W, PAGE_H)), dim, msg);
        return;
    }
    info(c, p, ox, oy, side, tab);
    loop_switch(c, p, at(loop_rect()), vals.looping, tab, ui.hover == Some(Hit::Loop));
    c.fill_rect(at(Rect::new(LEFT_X, INFO_H, PAGE_W - 2.0 * LEFT_X, 1.0)), line);
    let heading = |c: &mut Canvas, y: f32, s: &str| c.text(Font::condensed(11, 4).bold(), ox + LEFT_X, oy + y, dim, s);

    heading(c, SLOTS_Y - 17.0, match tab {
        Tab::Cut => "PLUGINS ON THIS CLIP",
        Tab::Whole => "PLUGINS ON THE SOURCE AUDIO",
    });
    for (k, s) in side.slots.iter().enumerate().take(SLOTS) {
        slot_row(c, p, at(slot_rect(k)), k, s, ui, ox, oy);
    }

    heading(c, LANES_Y - 17.0, "AUTOMATION");
    {
        let b = at(reset_both_rect());
        let on = !vol_pan_own(side).is_empty();
        button(c, p, b, "Reset volume & pan", on, ui.hover == Some(Hit::ResetBoth));
    }
    let selected = shown_lane(ui, side, tab);
    let scroll = ui.scroll[tab_at(tab)];
    for row in 0..rows_shown() {
        let r = at(lane_row_rect(row));
        let i = row + scroll;
        if let Some(l) = side.lanes.get(i) {
            lane_row(c, p, r, l, l.lane == selected);
        } else if i == side.lanes.len() {
            c.icon(&os_ui::tabler::PLUS, Rect::new(r.x + 4.0, r.y + 3.0, 13.0, 13.0), dim);
            c.text_clip(Font::new(13), r.x + 22.0, r.y + 1.0, r.w - 24.0, dim, "Plugin parameter...");
        }
    }

    if let Some(l) = shown(ui, side, tab) {
        let f = Font::new(14);
        let title = format!("{}", l.name);
        c.text_clip(f.bold(), ox + RIGHT_X, oy + INFO_H + 6.0, 180.0, text, &title);
        let tw = c.text_width(f.bold(), &title).min(180.0);
        c.text_clip(Font::new(12), ox + RIGHT_X + tw + 12.0, oy + INFO_H + 8.0, PAGE_W - RIGHT_X - tw - 148.0, dim, &status_text(l, tab, side));
        let pts = points(ui, l);
        button(c, p, at(clear_rect()), reset_label(l, tab), !l.own.is_empty(), ui.hover == Some(Hit::Clear));
        let pos = match &ui.drag {
            Some(Drag::Pos { at }) => Pos::At(*at),
            _ => side.pos,
        };
        plot(c, p, ox, oy, side, l, pts, tab, pos);
        ruler(c, p, ox, oy, side, tab, pos);
    }
}

/// The time ruler: times along it and the song position's flag, as on the Arranger's. Where the song position is not on
/// this audio it says so, so the missing line explains itself.
#[allow(clippy::too_many_arguments)]
fn ruler(c: &mut Canvas, p: &Palette, ox: f32, oy: f32, side: &Side, tab: Tab, pos: Pos) {
    let plot = plot_rect();
    let r0 = ruler_rect();
    let r = Rect::new(r0.x + ox, r0.y + oy, r0.w, r0.h);
    c.fill_rect(r, p.ruler);
    c.push_clip(r);
    let f = Font::new(11);
    let span = (side.to_s - side.from_s).max(1e-6);
    // A label about every 90 px.
    let want = span * 90.0 / f64::from(r.w.max(1.0));
    let step = [0.01, 0.02, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0].into_iter().find(|&k| k >= want).unwrap_or(1200.0);
    let mut k = (side.from_s / step).ceil();
    while k * step <= side.to_s {
        let s = k * step;
        let x = s_to_x(side, plot, s) + ox;
        c.fill_rect(Rect::new(x.round(), r.bottom() - 4.0, 1.0, 4.0), p.ruler_text.with_alpha(150));
        c.text(f, x.round() + 3.0, r.y + 1.0, p.ruler_text, &short_time(s, step));
        k += 1.0;
    }
    match pos {
        Pos::At(s) => {
            let x = s_to_x(side, plot, s.clamp(side.from_s, side.to_s)) + ox;
            let (y0, k) = (r.y + 0.5, 9.0);
            c.fill_polygon(&[(x - 0.5, y0 - 0.5), (x + k + 1.0, y0 - 0.5), (x - 0.5, y0 + k + 1.0)], p.s(0x0b1114));
            c.fill_polygon(&[(x, y0), (x + k, y0), (x, y0 + k)], p.playhead);
        }
        other => {
            let msg = match (other, tab) {
                (Pos::Before, _) => "Song position is before this clip: click to place it here",
                (Pos::After, _) => "Song position is after this clip: click to place it here",
                _ => "Song position is not on this audio: click to place it here",
            };
            let w = c.text_width(f, msg) + 12.0;
            let b = Rect::new(r.cx() - w / 2.0, r.y, w, r.h);
            c.fill_rect(b, p.ruler);
            c.text_center(f, b, p.tx(0xd4dade), msg);
        }
    }
    c.pop_clip();
}

/// A ruler label: minutes and seconds, with as many decimals as the spacing `step` needs.
fn short_time(s: f64, step: f64) -> String {
    let m = (s / 60.0).floor();
    let rest = s - m * 60.0;
    if step >= 1.0 {
        format!("{m:.0}:{rest:02.0}")
    } else if step >= 0.1 {
        format!("{m:.0}:{rest:04.1}")
    } else {
        format!("{m:.0}:{rest:05.2}")
    }
}

fn info(c: &mut Canvas, p: &Palette, ox: f32, oy: f32, side: &Side, tab: Tab) {
    let (text, dim) = (p.tx(0xd4dade), p.tx(0x8c9296));
    c.text(Font::new(15).bold(), ox + LEFT_X, oy + 4.0, text, tab.name());
    // Two columns of rows.
    let per = side.rows.len().div_ceil(2).max(1);
    let (w, rh) = ((PAGE_W - 2.0 * LEFT_X) / 2.0, 14.0);
    for (i, (k, v)) in side.rows.iter().enumerate() {
        let (col, row) = (i / per, i % per);
        let x = ox + LEFT_X + col as f32 * w;
        let y = oy + 24.0 + row as f32 * rh;
        let f = Font::new(12);
        c.text_clip(f, x, y, 76.0, dim, k);
        c.text_clip(f, x + 78.0, y, w - 84.0, text, v);
    }
}

#[allow(clippy::too_many_arguments)]
fn slot_row(c: &mut Canvas, p: &Palette, r: Rect, k: usize, s: &SlotVals, ui: &PageUi, ox: f32, oy: f32) {
    let (text, dim) = (p.tx(0xd4dade), p.tx(0x8c9296));
    c.fill_rrect(r, 3.0, p.s(0x1b1d1e));
    let f = Font::new(13);
    match &s.name {
        Some(name) => {
            let led = if s.on { p.a(0x47f49a) } else { p.s(0x3a3f42) };
            c.fill_circle(r.x + 10.0, r.y + r.h / 2.0, 4.0, led);
            c.text_clip(f, r.x + 22.0, r.y + 1.0, r.w - 74.0, if s.on { text } else { dim }, name);
            let m = mix_rect(k);
            let m = Rect::new(m.x + ox, m.y + oy, m.w, m.h);
            c.fill_rect(m, p.s(0x2b2f31));
            let mix = match &ui.drag {
                Some(Drag::Mix { slot, mix }) if usize::from(*slot) == k => *mix,
                _ => s.mix,
            };
            c.fill_rect(Rect::new(m.x, m.y, m.w * mix, m.h), if s.on { p.a(0x47f49a).with_alpha(170) } else { p.s(0x4a4f52) });
        }
        None => {
            c.text_clip(f, r.x + 22.0, r.y + 1.0, r.w - 26.0, dim, &format!("Slot {} - add a plugin", k + 1));
        }
    }
}

fn lane_row(c: &mut Canvas, p: &Palette, r: Rect, l: &LaneVals, selected: bool) {
    if selected {
        c.fill_rrect(r, 3.0, p.s(0x2b2f31));
    }
    let dot = match lane_state(l) {
        2 => p.a(0x49f1b1),
        1 => p.a(0x49f1b1).with_alpha(90),
        _ => p.s(0x3a3f42),
    };
    c.fill_circle(r.x + 10.0, r.y + r.h / 2.0, 4.0, dot);
    c.text_clip(Font::new(13), r.x + 22.0, r.y + 1.0, r.w - 24.0, p.tx(0xd4dade), &l.name);
}

/// A polyline of a lane across the plot, a point every 2 px.
fn lane_line(side: &Side, r: Rect, pts: &[AutoPoint], plot: Rect) -> Vec<(f32, f32)> {
    let mut v = Vec::with_capacity((r.w / 2.0) as usize + 2);
    let mut x = 0.0;
    while x <= plot.w {
        let s = x_to_s(side, plot, plot.x + x);
        if let Some(l) = fx::lane_value(pts, s) {
            v.push((r.x + x, r.y + (1.0 - l as f32) * r.h));
        }
        x += 2.0;
    }
    v
}

#[allow(clippy::too_many_arguments)]
fn plot(c: &mut Canvas, p: &Palette, ox: f32, oy: f32, side: &Side, l: &LaneVals, pts: &[AutoPoint], tab: Tab, pos: Pos) {
    let plot = plot_rect();
    let r = Rect::new(plot.x + ox, plot.y + oy, plot.w, plot.h);
    c.fill_rect(r, p.s(0x171a1c));
    c.push_clip(r);
    // The waveform behind everything.
    if !side.wave.is_empty() {
        let (mid, amp) = (r.y + r.h / 2.0, r.h / 2.0 - 4.0);
        let n = side.wave.len() as f32;
        let mut top = Vec::with_capacity(side.wave.len());
        let mut bottom = Vec::with_capacity(side.wave.len());
        for (i, &(lo, hi)) in side.wave.iter().enumerate() {
            let x = r.x + r.w * (i as f32 + 0.5) / n;
            top.push((x, mid - hi.clamp(-1.0, 1.0) * amp));
            bottom.push((x, mid - lo.clamp(-1.0, 1.0) * amp));
        }
        top.extend(bottom.into_iter().rev());
        c.fill_polygon(&top, p.s(0x33393d));
    }
    // The scale's lines (dB for volume, left and right for pan), then the baseline: where the lane stands with no line of
    // its own, so a line drawn starts from a value that can be read.
    let marks = scale_marks(l.lane);
    let neutral = r.y + (1.0 - l.lane.neutral() as f32) * r.h;
    for &(v, _) in &marks {
        c.fill_rect(Rect::new(r.x, (r.y + (1.0 - v as f32) * r.h).round(), r.w, 1.0), p.s(0x262b2e));
    }
    let base = !matches!(l.lane, Lane::Param { .. });
    if base {
        let col = if pts.is_empty() && l.inherited.is_empty() { p.tx(0xc4ccd0) } else { p.tx(0x8c9296).with_alpha(200) };
        let mut x = r.x;
        while x < r.right() {
            c.fill_rect(Rect::new(x, neutral.round() - 0.5, (r.right() - x).min(7.0), 1.5), col);
            x += 11.0;
        }
    }

    // The whole audio's tab shows the cuts along the top, those with a line of their own marked.
    if tab == Tab::Whole {
        for m in &side.marks {
            let (a, b) = (s_to_x(side, plot, m.from_s) + ox, s_to_x(side, plot, m.to_s) + ox);
            let own = m.own.contains(&l.lane);
            if own {
                c.fill_rect(Rect::new(a, r.y, (b - a).max(1.0), r.h), p.a(0xffb454).with_alpha(26));
            }
            let col = if own { p.a(0xffb454) } else { p.a(0x49f1b1).with_alpha(120) };
            c.fill_rect(Rect::new(a, r.y, (b - a).max(1.0), 4.0), col);
        }
    }
    // What the cut follows, faint, under its own line.
    if !l.inherited.is_empty() {
        let line = lane_line(side, r, &l.inherited, plot);
        c.polyline(&line, 1.0, p.a(0x49f1b1).with_alpha(if pts.is_empty() { 170 } else { 70 }));
    }
    if !pts.is_empty() {
        let line = lane_line(side, r, pts, plot);
        c.polyline(&line, 1.8, p.a(0x49f1b1));
        for q in pts {
            let (x, y) = (s_to_x(side, plot, q.beat) + ox, v_to_y(plot, q.value) + oy);
            c.fill_circle(x, y, 4.0, p.s(0x171a1c));
            c.stroke_circle(x, y, 4.0, 1.5, p.a(0x49f1b1));
        }
    }
    // The song position, playing or not: the line playback runs along.
    if let Pos::At(s) = pos {
        c.fill_rect(Rect::new(s_to_x(side, plot, s.clamp(side.from_s, side.to_s)) + ox, r.y, 1.5, r.h), p.playhead);
    }
    c.pop_clip();
    // The time axis under the plot: its two ends. The value scale inside it: its top, its plain value and its bottom.
    let dim: Color = p.tx(0x8c9296);
    let f = Font::new(11);
    let y = r.bottom() + 1.0;
    c.text(f, r.x, y, dim, &time_text(side.from_s));
    let end = time_text(side.to_s);
    let end_w = c.text_width(f, &end);
    c.text(f, r.right() - end_w, y, dim, &end);
    let scale_x = r.x + 4.0;
    let lh = c.line_height(f);
    // The scale's values beside their lines (clear of the marks along the top of the Source audio tab).
    for &(v, ref label) in &marks {
        let y = r.y + (1.0 - v as f32) * r.h;
        let ty = (y - lh / 2.0).clamp(r.y + 6.0, r.bottom() - lh - 2.0);
        if (y - neutral).abs() > 3.0 || !base {
            c.text(f, scale_x, ty, dim, label);
        }
    }
    // The baseline's own label, at its right end: what the lane is with no line.
    if base {
        let label = match l.lane {
            Lane::Volume => "Baseline: 0 dB (as recorded)".to_string(),
            _ => "Baseline: centre".to_string(),
        };
        let label = if tab == Tab::Cut && !l.inherited.is_empty() { format!("{label}, under the source audio's line") } else { label };
        let lf = Font::new(11);
        let w = c.text_width(lf, &label) + 10.0;
        let b = Rect::new(r.right() - w - 4.0, (neutral - lh / 2.0 - 1.0).clamp(r.y + 2.0, r.bottom() - lh - 4.0), w, lh + 2.0);
        c.fill_rrect(b, 3.0, p.s(0x0f1214).with_alpha(220));
        c.text(lf, b.x + 5.0, b.y + 1.0, p.tx(0xc4ccd0), &label);
        let at = if l.lane == Lane::Volume { "0 dB" } else { "Centre" };
        c.text(f, scale_x, (neutral - lh / 2.0).clamp(r.y + 6.0, r.bottom() - lh - 2.0), p.tx(0xc4ccd0), at);
    }
}

/// The lines of a lane's scale and what they read: dB steps for volume, left and right for pan, quarters for a parameter.
fn scale_marks(lane: Lane) -> Vec<(f64, String)> {
    match lane {
        Lane::Volume => [6.0, 3.0, 0.0, -6.0, -12.0, -24.0]
            .into_iter()
            .filter_map(|db: f64| {
                let want = 10f64.powf(db / 20.0);
                let gain = |v: f64| f64::from(os_engine::lane_gain(v));
                if gain(1.0) < want || gain(0.0) > want {
                    return None;
                }
                // The value with that gain: the curve only goes up.
                let (mut lo, mut hi) = (0.0, 1.0);
                for _ in 0..40 {
                    let mid = (lo + hi) / 2.0;
                    if gain(mid) < want {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                Some((hi, if db == 0.0 { "0 dB".to_string() } else { format!("{db:+.0} dB") }))
            })
            .chain(std::iter::once((0.0, "-inf dB".to_string())))
            .collect(),
        Lane::Pan => vec![(1.0, "100% R".into()), (0.75, "50% R".into()), (0.5, "Centre".into()), (0.25, "50% L".into()), (0.0, "100% L".into())],
        Lane::Param { .. } => vec![(1.0, "100%".into()), (0.75, "75%".into()), (0.5, "50%".into()), (0.25, "25%".into()), (0.0, "0%".into())],
    }
}

/// A small button of the page: greyed while it has nothing to do.
fn button(c: &mut Canvas, p: &Palette, b: Rect, label: &str, on: bool, hov: bool) {
    let face = if on && hov { p.s(0x3a4043) } else { p.s(0x2b2f31) };
    c.fill_rrect(b, 3.0, face);
    c.text_center(Font::new(12), b, if on { p.tx(0xd4dade) } else { p.tx(0x6a7074) }, label);
}

/// The loop switch: lit while Play loops just this audio.
fn loop_switch(c: &mut Canvas, p: &Palette, b: Rect, on: bool, tab: Tab, hov: bool) {
    let face = if on { p.a(0x49f1b1).with_alpha(if hov { 70 } else { 45 }) } else if hov { p.s(0x3a4043) } else { p.s(0x2b2f31) };
    c.fill_rrect(b, 4.0, face);
    let ink = if on { p.a(0x49f1b1) } else { p.tx(0x9aa2a6) };
    c.icon(&os_ui::tabler::REPEAT, Rect::new(b.x + 6.0, b.y + 3.0, 14.0, 14.0), ink);
    let text = match (on, tab) {
        (true, Tab::Cut) => "Play loops this clip",
        (true, Tab::Whole) => "Play loops the source audio",
        (false, _) => "Play plays the whole song",
    };
    c.text_clip(Font::new(12), b.x + 24.0, b.y + 3.0, b.w - 28.0, if on { p.tx(0xd4dade) } else { ink }, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(l: Lane, name: &str, own: &[(f64, f64)], inherited: &[(f64, f64)]) -> LaneVals {
        LaneVals {
            lane: l,
            name: name.into(),
            own: own.iter().map(|&(s, v)| fx::point(s, v)).collect(),
            inherited: inherited.iter().map(|&(s, v)| fx::point(s, v)).collect(),
        }
    }

    fn side(lanes: Vec<LaneVals>) -> Side {
        Side { available: true, lanes, from_s: 10.0, to_s: 20.0, ..Side::default() }
    }

    fn vals(lanes: Vec<LaneVals>) -> PageVals {
        PageVals { cut: side(lanes.clone()), whole: side(lanes), looping: true }
    }

    fn plain() -> PageVals {
        vals(vec![lane(Lane::Volume, "Volume", &[], &[]), lane(Lane::Pan, "Pan", &[], &[])])
    }

    const NO_SCREEN: &dyn Fn(f32, f32) -> (i32, i32) = &|x, y| (x as i32, y as i32);

    /// The page point of a time and value on the plot, for a side spanning 10..20 s.
    fn at(s: f64, v: f64) -> (f32, f32) {
        let p = plot_rect();
        (p.x + ((s - 10.0) / 10.0) as f32 * p.w, p.y + (1.0 - v as f32) * p.h)
    }

    #[test]
    fn a_click_on_the_plot_adds_a_point_and_the_release_sends_it() {
        let v = plain();
        let mut ui = PageUi::default();
        let mut out = Vec::new();
        let (x, y) = at(15.0, 0.5);
        assert_eq!(press(&mut ui, &v, Tab::Cut, x, y, Button::Left, NO_SCREEN, &mut out), Press::Capture);
        assert!(out.is_empty(), "nothing until the button is let go");
        release(&mut ui, Tab::Cut, &mut out);
        let [Event::SetLane { tab: Tab::Cut, lane: Lane::Volume, points }] = &out[..] else { panic!("{out:?}") };
        assert_eq!(points.len(), 1);
        assert!((points[0].beat - 15.0).abs() < 0.02 && (points[0].value - 0.5).abs() < 0.01, "{points:?}");
        assert!(!ui.dragging());
    }

    #[test]
    fn dragging_a_point_moves_it_between_its_neighbours_and_is_one_edit() {
        let v = vals(vec![lane(Lane::Volume, "Volume", &[(12.0, 0.2), (15.0, 0.5), (18.0, 0.8)], &[]), lane(Lane::Pan, "Pan", &[], &[])]);
        let mut ui = PageUi::default();
        let mut out = Vec::new();
        let (x, y) = at(15.0, 0.5);
        assert_eq!(hit(&ui, &v, Tab::Cut, x, y), Some(Hit::Point(1)));
        assert_eq!(press(&mut ui, &v, Tab::Cut, x, y, Button::Left, NO_SCREEN, &mut out), Press::Capture);
        let (mx, my) = at(16.0, 0.7);
        assert!(motion(&mut ui, &v, Tab::Cut, mx, my));
        assert!(ui.hint.contains("Volume"), "{}", ui.hint);
        // Past its neighbour it stops there.
        let (mx, my) = at(19.5, 0.9);
        motion(&mut ui, &v, Tab::Cut, mx, my);
        assert!(out.is_empty());
        release(&mut ui, Tab::Cut, &mut out);
        let [Event::SetLane { points, .. }] = &out[..] else { panic!("{out:?}") };
        assert_eq!(points.len(), 3);
        assert_eq!(points[1].beat, 18.0, "it stopped at its neighbour");
        assert!((points[1].value - 0.9).abs() < 0.01);
        assert!(points.windows(2).all(|w| w[0].beat <= w[1].beat));
    }

    #[test]
    fn a_press_on_a_point_without_moving_changes_nothing() {
        let v = vals(vec![lane(Lane::Volume, "Volume", &[(15.0, 0.5)], &[])]);
        let mut ui = PageUi::default();
        let mut out = Vec::new();
        let (x, y) = at(15.0, 0.5);
        press(&mut ui, &v, Tab::Cut, x, y, Button::Left, NO_SCREEN, &mut out);
        release(&mut ui, Tab::Cut, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_right_click_takes_a_point_away_and_the_last_one_leaves_no_lane() {
        let v = vals(vec![lane(Lane::Volume, "Volume", &[(15.0, 0.5)], &[(10.0, 0.1)])]);
        let mut ui = PageUi::default();
        let mut out = Vec::new();
        let (x, y) = at(15.0, 0.5);
        press(&mut ui, &v, Tab::Cut, x, y, Button::Right, NO_SCREEN, &mut out);
        let [Event::SetLane { points, .. }] = &out[..] else { panic!("{out:?}") };
        assert!(points.is_empty());
        // A right click away from a point does nothing.
        let mut out = Vec::new();
        let (x, y) = at(11.0, 0.9);
        assert_eq!(press(&mut ui, &v, Tab::Cut, x, y, Button::Right, NO_SCREEN, &mut out), Press::Nothing);
        assert!(out.is_empty());
    }

    // A cut that follows the whole audio starts its own lane with its first point; the whole audio's stays out of the
    // points it sends.
    #[test]
    fn a_cuts_first_point_starts_its_own_lane_without_the_inherited_points() {
        let v = vals(vec![lane(Lane::Volume, "Volume", &[], &[(10.0, 0.1), (20.0, 0.9)])]);
        let mut ui = PageUi::default();
        let mut out = Vec::new();
        let (x, y) = at(12.0, 0.3);
        press(&mut ui, &v, Tab::Cut, x, y, Button::Left, NO_SCREEN, &mut out);
        release(&mut ui, Tab::Cut, &mut out);
        let [Event::SetLane { points, .. }] = &out[..] else { panic!("{out:?}") };
        assert_eq!(points.len(), 1);
        // The inherited line is not a point to grab: pressing on it makes a new point.
        let (x, y) = at(10.0, 0.1);
        assert_eq!(hit(&ui, &v, Tab::Cut, x + 1.0, y), Some(Hit::Plot));
    }

    /// The reset button is always there, named after the baseline it goes back to; with nothing to reset it does nothing
    /// and says so; with points it clears the lane shown.
    #[test]
    fn the_reset_button_puts_the_lane_shown_back_to_its_baseline() {
        let c = clear_rect();
        let (x, y) = (c.cx(), c.cy());
        let v = plain();
        let mut ui = PageUi::default();
        assert_eq!(hit(&ui, &v, Tab::Cut, x, y), Some(Hit::Clear));
        let mut out = Vec::new();
        assert_eq!(press(&mut ui, &v, Tab::Cut, x, y, Button::Left, NO_SCREEN, &mut out), Press::Nothing);
        assert!(out.is_empty());
        assert!(hint_of(&ui, &v, Tab::Cut, Some(Hit::Clear), x, y).contains("baseline already"));
        assert_eq!(reset_label(&v.cut.lanes[0], Tab::Cut), "Reset to 0 dB");
        assert_eq!(reset_label(&v.cut.lanes[1], Tab::Cut), "Reset to centre");
        let v = vals(vec![lane(Lane::Volume, "Volume", &[(15.0, 0.5)], &[]), lane(Lane::Pan, "Pan", &[], &[])]);
        press(&mut ui, &v, Tab::Cut, x, y, Button::Left, NO_SCREEN, &mut out);
        assert_eq!(out, vec![Event::SetLane { tab: Tab::Cut, lane: Lane::Volume, points: Vec::new() }]);
        // A clip that follows the source audio's line goes back to following it.
        let follows = lane(Lane::Volume, "Volume", &[(15.0, 0.5)], &[(10.0, 0.2)]);
        assert_eq!(reset_label(&follows, Tab::Cut), "Follow source audio");
    }

    /// "Reset volume & pan" on the automation heading: both lanes back to their baselines in one edit; with neither
    /// having a line, nothing.
    #[test]
    fn reset_volume_and_pan_clears_both_at_once() {
        let r = reset_both_rect();
        let (x, y) = (r.cx(), r.cy());
        let mut ui = PageUi::default();
        let mut out = Vec::new();
        assert_eq!(hit(&ui, &plain(), Tab::Whole, x, y), Some(Hit::ResetBoth));
        assert_eq!(press(&mut ui, &plain(), Tab::Whole, x, y, Button::Left, NO_SCREEN, &mut out), Press::Nothing);
        let mut v = vals(vec![lane(Lane::Volume, "Volume", &[(15.0, 0.5)], &[]), lane(Lane::Pan, "Pan", &[(12.0, 0.1)], &[])]);
        press(&mut ui, &v, Tab::Whole, x, y, Button::Left, NO_SCREEN, &mut out);
        assert_eq!(out, vec![Event::ResetLanes { tab: Tab::Whole, lanes: vec![Lane::Volume, Lane::Pan] }]);
        v.apply(&out[0]);
        assert!(v.whole.lanes.iter().all(|l| l.own.is_empty()), "shown at once");
        // The volume scale reads in dB around its baseline.
        let marks = scale_marks(Lane::Volume);
        let zero = marks.iter().find(|m| m.1 == "0 dB").expect("0 dB line");
        assert!((zero.0 - Lane::Volume.neutral()).abs() < 1e-6, "the baseline is 0 dB");
        assert!(marks.iter().any(|m| m.1 == "-6 dB") && marks.iter().any(|m| m.1 == "+3 dB"), "{marks:?}");
    }

    /// The loop switch says what Play plays and flips it.
    #[test]
    fn the_loop_switch_flips_what_play_plays() {
        let r = loop_rect();
        let mut ui = PageUi::default();
        let mut out = Vec::new();
        let v = plain();
        assert!(v.looping);
        press(&mut ui, &v, Tab::Cut, r.cx(), r.cy(), Button::Left, NO_SCREEN, &mut out);
        assert_eq!(out, vec![Event::Loop { tab: Tab::Cut, on: false }]);
        assert!(hint_of(&ui, &v, Tab::Cut, Some(Hit::Loop), r.cx(), r.cy()).starts_with("Play loops just this audio"));
    }

    #[test]
    fn the_lane_list_chooses_what_the_editor_shows_per_tab() {
        let v = plain();
        let mut ui = PageUi::default();
        let mut out = Vec::new();
        assert_eq!(shown_lane(&ui, &v.cut, Tab::Cut), Lane::Volume);
        let r = lane_row_rect(1);
        assert_eq!(hit(&ui, &v, Tab::Cut, r.cx(), r.cy()), Some(Hit::LaneRow(1)));
        press(&mut ui, &v, Tab::Cut, r.cx(), r.cy(), Button::Left, NO_SCREEN, &mut out);
        assert_eq!(shown_lane(&ui, &v.cut, Tab::Cut), Lane::Pan);
        assert_eq!(shown_lane(&ui, &v.whole, Tab::Whole), Lane::Volume, "the other tab has its own");
        // A point now goes on Pan.
        let (x, y) = at(15.0, 0.5);
        press(&mut ui, &v, Tab::Cut, x, y, Button::Left, NO_SCREEN, &mut out);
        release(&mut ui, Tab::Cut, &mut out);
        assert!(matches!(&out[..], [Event::SetLane { lane: Lane::Pan, .. }]), "{out:?}");
    }

    #[test]
    fn a_lane_that_is_gone_falls_back_to_volume_and_a_new_one_takes_the_focus() {
        let param = Lane::Param { whole: false, slot: 0, param: 2 };
        let mut v = vals(vec![lane(Lane::Volume, "Volume", &[], &[]), lane(Lane::Pan, "Pan", &[], &[]), lane(param, "Reverb: Mix", &[(10.0, 0.5)], &[])]);
        let mut ui = PageUi::default();
        v.cut.focus = param;
        v.cut.focus_serial = 1;
        assert_eq!(shown_lane(&ui, &v.cut, Tab::Cut), param, "the app asks to show it");
        let r = lane_row_rect(0);
        press(&mut ui, &v, Tab::Cut, r.cx(), r.cy(), Button::Left, NO_SCREEN, &mut Vec::new());
        assert_eq!(shown_lane(&ui, &v.cut, Tab::Cut), Lane::Volume, "the user's choice stays");
        v.cut.focus_serial = 2;
        assert_eq!(shown_lane(&ui, &v.cut, Tab::Cut), param, "until the app asks again");
        v.cut.lanes.pop();
        assert_eq!(shown_lane(&ui, &v.cut, Tab::Cut), Lane::Volume, "gone");
    }

    #[test]
    fn a_slot_is_a_menu_when_empty_and_a_plugin_when_filled() {
        let mut v = plain();
        v.cut.slots[1] = SlotVals { name: Some("Reverb".into()), on: true, mix: 0.5 };
        let mut ui = PageUi::default();
        let (mut out, r0, r1) = (Vec::new(), slot_rect(0), slot_rect(1));
        press(&mut ui, &v, Tab::Cut, r0.cx(), r0.cy(), Button::Left, NO_SCREEN, &mut out);
        assert!(matches!(out[..], [Event::Pick { tab: Tab::Cut, slot: 0, .. }]), "{out:?}");
        out.clear();
        press(&mut ui, &v, Tab::Cut, r1.x + 40.0, r1.cy(), Button::Left, NO_SCREEN, &mut out);
        assert_eq!(out, vec![Event::OpenSlot { tab: Tab::Cut, slot: 1 }]);
        out.clear();
        let led = led_rect(1);
        press(&mut ui, &v, Tab::Cut, led.cx(), led.cy(), Button::Left, NO_SCREEN, &mut out);
        assert_eq!(out, vec![Event::ToggleSlot { tab: Tab::Cut, slot: 1 }]);
        out.clear();
        press(&mut ui, &v, Tab::Cut, r1.x + 40.0, r1.cy(), Button::Right, NO_SCREEN, &mut out);
        assert!(matches!(out[..], [Event::SlotMenu { tab: Tab::Cut, slot: 1, .. }]), "{out:?}");
        // An empty slot's LED and mix bar are not there.
        let led = led_rect(0);
        assert_eq!(hit(&ui, &v, Tab::Cut, led.cx(), led.cy()), Some(Hit::SlotName(0)));
        let m = mix_rect(0);
        assert_eq!(hit(&ui, &v, Tab::Cut, m.cx(), m.cy()), Some(Hit::SlotName(0)));
    }

    #[test]
    fn a_slots_mix_bar_drags_and_sends_one_value() {
        let mut v = plain();
        v.whole.slots[2] = SlotVals { name: Some("Eq".into()), on: true, mix: 1.0 };
        let mut ui = PageUi::default();
        let mut out = Vec::new();
        let m = mix_rect(2);
        assert_eq!(press(&mut ui, &v, Tab::Whole, m.x + m.w * 0.25, m.cy(), Button::Left, NO_SCREEN, &mut out), Press::Capture);
        assert!(motion(&mut ui, &v, Tab::Whole, m.x + m.w * 0.5, m.cy()));
        motion(&mut ui, &v, Tab::Whole, m.x + m.w * 9.0, m.cy());
        assert!(out.is_empty());
        release(&mut ui, Tab::Whole, &mut out);
        assert_eq!(out, vec![Event::SetMix { tab: Tab::Whole, slot: 2, mix: 1.0 }]);
    }

    #[test]
    fn a_cut_tab_with_no_piece_takes_no_input() {
        let mut v = plain();
        v.cut.available = false;
        let mut ui = PageUi::default();
        let mut out = Vec::new();
        let (x, y) = at(15.0, 0.5);
        assert_eq!(press(&mut ui, &v, Tab::Cut, x, y, Button::Left, NO_SCREEN, &mut out), Press::Nothing);
        let r = slot_rect(0);
        assert_eq!(press(&mut ui, &v, Tab::Cut, r.cx(), r.cy(), Button::Left, NO_SCREEN, &mut out), Press::Nothing);
        assert!(out.is_empty());
        // The other tab still does.
        assert_eq!(press(&mut ui, &v, Tab::Whole, r.cx(), r.cy(), Button::Left, NO_SCREEN, &mut out), Press::Repaint);
    }

    #[test]
    fn many_lanes_scroll_and_the_add_row_stays_reachable() {
        let mut lanes = vec![lane(Lane::Volume, "Volume", &[], &[]), lane(Lane::Pan, "Pan", &[], &[])];
        for k in 0..20 {
            lanes.push(lane(Lane::Param { whole: false, slot: 0, param: k }, "Param", &[(10.0, 0.5)], &[]));
        }
        let v = vals(lanes);
        let mut ui = PageUi::default();
        let r0 = lane_row_rect(0);
        assert_eq!(hit(&ui, &v, Tab::Cut, r0.cx(), r0.cy()), Some(Hit::LaneRow(0)));
        assert!(!wheel(&mut ui, &v, Tab::Cut, r0.cy(), 1.0), "already at the top");
        assert!(wheel(&mut ui, &v, Tab::Cut, r0.cy(), -1.0));
        assert_eq!(hit(&ui, &v, Tab::Cut, r0.cx(), r0.cy()), Some(Hit::LaneRow(1)));
        for _ in 0..100 {
            wheel(&mut ui, &v, Tab::Cut, r0.cy(), -1.0);
        }
        let last = lane_row_rect(rows_shown() - 1);
        assert_eq!(hit(&ui, &v, Tab::Cut, last.cx(), last.cy()), Some(Hit::AddLane));
        assert!(!wheel(&mut ui, &v, Tab::Cut, r0.cy(), -1.0), "at the bottom");
        // Above the list the wheel does not scroll it.
        assert!(!wheel(&mut ui, &v, Tab::Cut, 10.0, 1.0));
    }

    #[test]
    fn values_read_as_the_user_expects() {
        assert_eq!(value_text(Lane::Volume, 0.8), "+0.0 dB");
        assert_eq!(value_text(Lane::Volume, 0.0), "-inf dB");
        assert!(value_text(Lane::Volume, 1.0).starts_with("+5.6"), "{}", value_text(Lane::Volume, 1.0));
        assert_eq!(value_text(Lane::Pan, 0.5), "Centre");
        assert_eq!(value_text(Lane::Pan, 0.0), "100% left");
        assert_eq!(value_text(Lane::Pan, 1.0), "100% right");
        assert_eq!(value_text(Lane::Param { whole: true, slot: 0, param: 0 }, 0.25), "25%");
        assert_eq!(time_text(0.0), "00:00.000");
        assert_eq!(time_text(75.5), "01:15.500");
    }

    #[test]
    fn the_status_line_says_what_moves_the_sound() {
        let s = side(vec![]);
        let own = lane(Lane::Volume, "Volume", &[(1.0, 0.5)], &[(1.0, 0.1)]);
        let follows = lane(Lane::Volume, "Volume", &[], &[(1.0, 0.1)]);
        let none = lane(Lane::Volume, "Volume", &[], &[]);
        assert!(status_text(&own, Tab::Cut, &s).contains("own line"));
        assert!(status_text(&follows, Tab::Cut, &s).contains("Following"));
        assert!(status_text(&none, Tab::Cut, &s).contains("No automation"));
        assert!(status_text(&own, Tab::Whole, &s).contains("every clip"));
        let mut marked = side(vec![]);
        marked.marks = vec![Mark { from_s: 0.0, to_s: 1.0, own: vec![Lane::Volume] }, Mark { from_s: 1.0, to_s: 2.0, own: vec![] }];
        assert!(status_text(&own, Tab::Whole, &marked).contains("1 with its own"), "{}", status_text(&own, Tab::Whole, &marked));
        assert_eq!(lane_state(&own), 2);
        assert_eq!(lane_state(&follows), 1);
        assert_eq!(lane_state(&none), 0);
    }

    /// The ruler places the song position: at once on the press, again where the drag ends, and the line goes along.
    #[test]
    fn the_ruler_places_the_song_position() {
        let v = plain();
        let mut ui = PageUi::default();
        let mut out = Vec::new();
        let (r, plot) = (ruler_rect(), plot_rect());
        assert_eq!(hit(&ui, &v, Tab::Cut, r.cx(), r.cy()), Some(Hit::Ruler));
        assert_eq!(press(&mut ui, &v, Tab::Cut, r.cx(), r.cy(), Button::Left, NO_SCREEN, &mut out), Press::Capture);
        assert!(matches!(out[..], [Event::SetPos { tab: Tab::Cut, at }] if (at - 15.0).abs() < 0.01), "{out:?}");
        out.clear();
        assert!(motion(&mut ui, &v, Tab::Cut, plot.x + plot.w * 0.25, r.y - 200.0));
        assert!(ui.hint.starts_with("Song position"), "{}", ui.hint);
        // Past the end it stops at the end.
        motion(&mut ui, &v, Tab::Cut, plot.right() + 500.0, r.cy());
        assert!(out.is_empty(), "one position per press and release");
        release(&mut ui, Tab::Cut, &mut out);
        assert_eq!(out, vec![Event::SetPos { tab: Tab::Cut, at: 20.0 }]);
        assert!(!ui.dragging());
        // What the page shows until the app's values follow.
        let mut w = v.clone();
        w.apply(&out[0]);
        assert_eq!(w.cut.pos, Pos::At(20.0));
        assert_eq!(w.whole.pos, Pos::Away, "the other tab's is its own");
        // A right click on it does nothing, and a tab with no clip takes nothing.
        assert_eq!(press(&mut ui, &v, Tab::Cut, r.cx(), r.cy(), Button::Right, NO_SCREEN, &mut out), Press::Nothing);
        let mut none = plain();
        none.cut.available = false;
        assert_eq!(hit(&ui, &none, Tab::Cut, r.cx(), r.cy()), None);
    }

    #[test]
    fn the_page_fits_its_parts() {
        assert!(lane_row_rect(rows_shown() - 1).bottom() <= PAGE_H);
        assert!(slot_rect(SLOTS - 1).bottom() < LANES_Y - 16.0);
        assert!(plot_rect().bottom() + AXIS_H <= PAGE_H);
        assert_eq!(ruler_rect().bottom(), plot_rect().y);
        assert!(plot_rect().right() <= PAGE_W);
        assert!(rows_shown() >= 5);
        assert!(LEFT_X + LEFT_W < RIGHT_X);
    }
}
