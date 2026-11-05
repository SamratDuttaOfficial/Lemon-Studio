// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A cut's and a whole sample's plugins and automation in the app (`os_model::clip_fx`): what the plugin window's Cut part
//! and Whole audio tabs show (`clip_page`), and what their edits do. A unit's chain is a track of its own number
//! (`clip_fx::track_of`), so its plugins are hosted and opened as a mixer slot's are; the mixer itself never shows it.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use os_model::clip_fx::{self as fx, Lane};
use os_model::{AutoPoint, Clip, ClipSource};

use crate::app::{App, State};
use crate::clip_page::{Event, LaneVals, Mark, PageVals, Side, SlotVals, Tab, SLOTS};
use crate::control::command::Region;
use crate::mixer::FxReq;
use crate::playlist::{self as pl, Cmd, PItem, Popup};
use crate::plugins::fx_key;

/// Columns of a lane editor's waveform.
const WAVE_COLS: usize = 548;

/// Which unit an edit is for. A cut is named by its clip's id, as clips move in their list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Cut(u32),
    Whole(usize),
}

/// What the app's own menus on the page do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum C {
    /// Effect `index` (into `Plugins::effects`) into this slot, in place of what it holds.
    Effect { scope: Scope, slot: u8, index: usize },
    Remove { scope: Scope, slot: u8 },
    /// Moves a slot's plugin up (-1) or down (+1).
    Move { scope: Scope, slot: u8, by: i8 },
    Open { scope: Scope, slot: u8 },
    Toggle { scope: Scope, slot: u8 },
    /// A lane for this parameter, starting from the parameter's value now.
    AddLane { scope: Scope, whole: bool, slot: u8, param: u16 },
}

/// The app's state of the two tabs.
#[derive(Debug, Default)]
pub struct ClipFxUi {
    /// The piece each channel's window shows on its Cut tab: the clip double-clicked last, by its id.
    pub cut: BTreeMap<usize, u32>,
    /// Peaks of the spans shown (`WAVE_COLS` columns), by file and span.
    pub(crate) peaks: RefCell<HashMap<(String, u64, u64), Arc<Vec<(f32, f32)>>>>,
    /// Where the song is while it plays, in ticks.
    pub play_tick: Option<f64>,
    /// The lane the app last asked each row's tabs to show (`clip_page::Side::focus`).
    pub(crate) focus: BTreeMap<(usize, bool), (Lane, u32)>,
    /// The tab a window should show next: set by a double-click that asks for it.
    pub pending_tab: Option<(usize, Tab)>,
}

/// The clip with id `id` in the current arrangement: its place.
fn clip_at(s: &State, id: u32) -> Option<usize> {
    s.project.arrangement()?.clips.iter().position(|c| c.id.0 == id)
}

/// The rack row a scope belongs to.
fn row_of(s: &State, scope: Scope) -> Option<usize> {
    match scope {
        Scope::Whole(row) => Some(row),
        Scope::Cut(id) => {
            let c = s.project.arrangement()?.clips.get(clip_at(s, id)?)?;
            match c.source {
                ClipSource::Channel(ch) => s.project.channels.iter().position(|x| x.index == ch),
                ClipSource::Pattern(_) => None,
            }
        }
    }
}

/// The unit of `scope` if it has one.
fn unit_of(s: &State, scope: Scope) -> Option<u32> {
    let h = match scope {
        Scope::Whole(row) => s.project.channels.get(row)?.whole_fx,
        Scope::Cut(id) => s.project.arrangement()?.clips.get(clip_at(s, id)?)?.fx,
    };
    s.project.fx_unit(h).map(|u| u.handle)
}

/// The unit of `scope`, made when it has none.
fn make_unit(s: &mut State, scope: Scope) -> Option<u32> {
    match scope {
        Scope::Whole(row) => s.project.whole_unit(row),
        Scope::Cut(id) => {
            let i = clip_at(s, id)?;
            s.project.clip_unit(i)
        }
    }
}

fn is_whole(scope: Scope) -> bool {
    matches!(scope, Scope::Whole(_))
}

/// The scope a tab of row `row`'s window edits; `None` for a Cut tab with no piece.
fn scope_of(s: &State, row: usize, tab: Tab) -> Option<Scope> {
    match tab {
        Tab::Whole => Some(Scope::Whole(row)),
        Tab::Cut => {
            let id = *s.clip_fx.cut.get(&row)?;
            let c = s.project.arrangement()?.clips.get(clip_at(s, id)?)?;
            (c.source == ClipSource::Channel(s.project.channels.get(row)?.index)).then_some(Scope::Cut(id))
        }
    }
}

/// A plugin slot's name as the page lists it.
fn slot_name(u: &fx::FxUnit, slot: usize) -> Option<String> {
    u.track.slots.get(slot)?.as_ref().map(crate::project_plugins::slot_title)
}

fn slots_of(s: &State, handle: u32) -> Vec<SlotVals> {
    let Some(u) = s.project.fx_unit(handle) else { return vec![SlotVals { on: true, mix: 1.0, name: None }; SLOTS] };
    (0..SLOTS).map(|k| SlotVals { name: slot_name(u, k), on: u.track.slot_on[k], mix: u.track.slot_mix[k] as f32 }).collect()
}

/// A parameter's name on the page: "Plugin: Parameter".
fn param_label(s: &State, holder: u32, slot: u8, param: u16) -> String {
    let plugin = s.project.fx_unit(holder).and_then(|u| slot_name(u, usize::from(slot))).unwrap_or_else(|| format!("Slot {}", slot + 1));
    let key = fx_key(fx::track_of(holder), slot);
    let name = crate::params::param_name(s, key, usize::from(param)).unwrap_or_else(|| format!("Parameter {}", param + 1));
    format!("{plugin}: {name}")
}

/// The lanes a tab lists.
fn lanes_of(s: &State, own: u32, whole: u32, tab: Tab) -> Vec<LaneVals> {
    let pts = |h: u32, l: Lane| s.project.fx_unit(h).map_or(Vec::new(), |u| u.lane(l).to_vec());
    let inherited = |l: Lane| if tab == Tab::Cut { pts(whole, l) } else { Vec::new() };
    let mut v = vec![
        LaneVals { lane: Lane::Volume, name: "Volume".into(), own: pts(own, Lane::Volume), inherited: inherited(Lane::Volume) },
        LaneVals { lane: Lane::Pan, name: "Pan".into(), own: pts(own, Lane::Pan), inherited: inherited(Lane::Pan) },
    ];
    let mut params: Vec<Lane> = Vec::new();
    for h in [own, whole] {
        for l in s.project.fx_unit(h).map_or(Vec::new(), fx::FxUnit::lanes) {
            if matches!(l, Lane::Param { .. }) && !params.contains(&l) {
                params.push(l);
            }
        }
    }
    params.sort();
    for lane in params {
        let Lane::Param { whole: on_whole, slot, param } = lane else { continue };
        let holder = if on_whole { whole } else { own };
        // A lane of the plugin's own, while the plugin is there.
        let present = s.project.fx_unit(holder).is_some_and(|u| u.track.slots.get(usize::from(slot)).is_some_and(Option::is_some));
        if !present || (tab == Tab::Whole && !on_whole) {
            continue;
        }
        let label = param_label(s, holder, slot, param);
        let name = if tab == Tab::Cut && on_whole { format!("Whole: {label}") } else { label };
        v.push(LaneVals { lane, name, own: pts(own, lane), inherited: inherited(lane) });
    }
    v
}

/// The peaks of `from_s..to_s` of `path`'s sample, in `WAVE_COLS` columns.
fn peaks_of(s: &State, path: &str, from_s: f64, to_s: f64) -> Arc<Vec<(f32, f32)>> {
    let key = (path.to_string(), from_s.to_bits(), to_s.to_bits());
    if let Some(p) = s.clip_fx.peaks.borrow().get(&key) {
        return p.clone();
    }
    // The file is not decoded yet: nothing to show, and nothing to remember.
    let Some(w) = s.playlist.wave(path) else { return Arc::default() };
    let per_col = (to_s - from_s).max(0.0) / WAVE_COLS as f64 * f64::from(w.rate);
    let per = w.per.max(1) as f64;
    let mut out = Vec::with_capacity(WAVE_COLS);
    for i in 0..WAVE_COLS {
        let a = from_s * f64::from(w.rate) + i as f64 * per_col;
        let (fa, fb) = ((a / per) as usize, ((a + per_col) / per) as usize);
        if fa >= w.peaks.len() {
            out.push((0.0, 0.0));
            continue;
        }
        let fb = fb.max(fa + 1).min(w.peaks.len());
        let stride = ((fb - fa) / 32).max(1);
        out.push(w.peaks[fa..fb].iter().step_by(stride).fold((f32::MAX, f32::MIN), |(lo, hi), &(a, b)| (lo.min(a), hi.max(b))));
    }
    let out = Arc::new(out);
    let mut cache = s.clip_fx.peaks.borrow_mut();
    // Spans change as clips are trimmed: this only ever holds what was just looked at.
    if cache.len() > 64 {
        cache.clear();
    }
    cache.insert(key, out.clone());
    out
}

/// The part of the sample clip `c` plays, in seconds.
fn clip_span(s: &State, c: &Clip) -> (f64, f64) {
    let (w0, _) = pl::window(s, c);
    let from = w0.max(0.0) / 1000.0;
    (from, from + f64::from(c.length) * pl::to_src(s, c.source, 1.0) / 1000.0)
}

fn db(g: f64) -> String {
    if g > 0.0 { format!("{:+.1} dB", 20.0 * g.log10()) } else { "-inf dB".into() }
}

/// "Bar 5.2": a tick as the user counts it.
fn bar_text(s: &State, tick: u32) -> String {
    let (beats, steps) = s.project.bar();
    let beat = u32::from(s.project.ppq.max(1)) * u32::from(steps) / 4;
    let bar = (beat.max(1) * u32::from(beats)).max(1);
    format!("{}.{}", tick / bar + 1, (tick % bar) / beat.max(1) + 1)
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn pitch_text(ch: &os_model::Channel) -> String {
    match ch.pitch_cents {
        0 => "As recorded".into(),
        c => format!("{:+.2} semitones", f64::from(c) / 100.0),
    }
}

fn mixer_name(s: &State, ch: &os_model::Channel) -> String {
    crate::mixer::name(s, ch.mixer_track)
}

fn cut_rows(s: &State, ch: &os_model::Channel, c: &Clip, from: f64, to: f64) -> Vec<(String, String)> {
    let row = |k: &str, v: String| (k.to_string(), v);
    let (fin, fout) = (c.stored_fade_in().map_or(0.0, |f| f.0), c.stored_fade_out().map_or(0.0, |f| f.0));
    let fades = if fin > 0.0 || fout > 0.0 { format!("in {fin:.0} ms, out {fout:.0} ms") } else { "None".into() };
    vec![
        row("File", ch.sample_path.as_deref().map_or("-".into(), file_name)),
        row("Part", format!("{} to {}  ({:.3} s)", crate::clip_page::time_text(from), crate::clip_page::time_text(to), to - from)),
        row("On playlist", format!("Bar {} to {}, track {}", bar_text(s, c.position), bar_text(s, c.end()), c.track + 1)),
        row("Gain", db(f64::from(c.gain()))),
        row("Fades", fades),
        row("Pitch", pitch_text(ch)),
        row("Mixer", mixer_name(s, ch)),
    ]
}

fn whole_rows(s: &State, ch: &os_model::Channel, wave: Option<&pl::Wave>) -> Vec<(String, String)> {
    let line = |k: &str, v: String| (k.to_string(), v);
    let clips = s.project.arrangement().map_or(0, |a| a.clips.iter().filter(|c| c.source == ClipSource::Channel(ch.index)).count());
    let mut v = vec![line("File", ch.sample_path.as_deref().map_or("-".into(), file_name))];
    if let Some(w) = wave {
        v.push(line("Length", format!("{} ({:.3} s)", crate::clip_page::time_text(w.secs()), w.secs())));
        v.push(line("Rate", format!("{} Hz", w.rate)));
    }
    v.push(line("Cuts", format!("{clips} on the playlist")));
    v.push(line("Pitch", pitch_text(ch)));
    v.push(line("Volume", format!("{:.0}%", ch.volume * 100.0)));
    v.push(line("Mixer", mixer_name(s, ch)));
    v
}

/// What rack row `row`'s window shows on the two tabs; `None` for a channel that plays no sample.
pub fn page_vals(s: &State, row: usize) -> Option<PageVals> {
    let ch = s.project.channels.get(row)?;
    if !ch.kind.is_sampler() {
        return None;
    }
    let path = ch.sample_path.as_deref();
    let clips: Vec<Clip> = s.project.arrangement().map_or(Vec::new(), |a| a.clips.iter().filter(|c| c.source == ClipSource::Channel(ch.index)).copied().collect());
    // A sample with clips has its waveform on the playlist. One without (a sampler channel) may be long and kept on disk,
    // and this runs each tick its window is open: it shows what is decoded already, and decodes nothing.
    let wave = path.and_then(|p| if clips.is_empty() { s.playlist.wave_loaded(p) } else { s.playlist.wave(p) });
    let total = wave.as_ref().map_or(1.0, |w| w.secs().max(0.001));

    // The whole audio.
    let marks = clips
        .iter()
        .map(|c| {
            let (from_s, to_s) = clip_span(s, c);
            Mark { from_s, to_s, own: s.project.fx_unit(c.fx).map_or(Vec::new(), fx::FxUnit::lanes) }
        })
        .collect();
    let focus = |whole: bool| s.clip_fx.focus.get(&(row, whole)).copied().unwrap_or((Lane::Volume, 0));
    let (focus_whole, serial_whole) = focus(true);
    let whole = Side {
        available: true,
        rows: whole_rows(s, ch, wave.as_deref()),
        slots: slots_of(s, ch.whole_fx),
        lanes: lanes_of(s, ch.whole_fx, ch.whole_fx, Tab::Whole),
        from_s: 0.0,
        to_s: total,
        wave: path.filter(|_| wave.is_some()).map_or_else(Arc::default, |p| peaks_of(s, p, 0.0, total)),
        marks,
        focus: focus_whole,
        focus_serial: serial_whole,
    };

    // The piece.
    let piece = scope_of(s, row, Tab::Cut).and_then(|sc| match sc {
        Scope::Cut(id) => s.project.arrangement()?.clips.get(clip_at(s, id)?).copied(),
        Scope::Whole(_) => None,
    });
    let cut = match piece {
        Some(c) => {
            let (from_s, to_s) = clip_span(s, &c);
            let (focus_cut, serial_cut) = focus(false);
            Side {
                available: true,
                rows: cut_rows(s, ch, &c, from_s, to_s),
                slots: slots_of(s, c.fx),
                lanes: lanes_of(s, c.fx, ch.whole_fx, Tab::Cut),
                from_s,
                to_s,
                wave: path.map_or_else(Arc::default, |p| peaks_of(s, p, from_s, to_s)),
                marks: Vec::new(),
                focus: focus_cut,
                focus_serial: serial_cut,
            }
        }
        None => Side::default(),
    };

    // Where playback is on the sample's clock: in the clip of this sample playing now, the piece's first.
    let playhead = s.clip_fx.play_tick.and_then(|t| {
        let mut playing = clips.iter().filter(|c| f64::from(c.position) <= t && t < f64::from(c.end()));
        let c = piece.filter(|p| f64::from(p.position) <= t && t < f64::from(p.end())).or_else(|| playing.next().copied())?;
        let (from, _) = clip_span(s, &c);
        Some(from + (t - f64::from(c.position)) * pl::to_src(s, c.source, 1.0) / 1000.0)
    });
    Some(PageVals { cut, whole, playhead })
}

/// A double-click on clip `c`, a clip of rack row `row`: it is the piece the window shows.
pub fn show_piece(s: &mut State, row: usize, c: &Clip, tab: Option<Tab>) {
    s.clip_fx.cut.insert(row, c.id.0);
    if let Some(t) = tab {
        s.clip_fx.pending_tab = Some((row, t));
    }
}

/// Every unit that may hold a lane of a plugin in `scope`'s own chain: for a whole audio's chain, the cuts of that
/// sample as well.
fn related_units(s: &State, scope: Scope) -> Vec<u32> {
    let mut v: Vec<u32> = unit_of(s, scope).into_iter().collect();
    if let Some(ch) = match scope {
        Scope::Whole(row) => s.project.channels.get(row),
        Scope::Cut(_) => None,
    } {
        for a in &s.project.arrangements {
            v.extend(a.clips.iter().filter(|c| c.source == ClipSource::Channel(ch.index) && c.fx != 0).map(|c| c.fx));
        }
    }
    v
}

/// Runs a command of the page's menus. Returns a status message.
pub fn run(app: &mut App, c: C) -> Option<String> {
    match c {
        C::Effect { scope, slot, index } => add_effect(app, scope, slot, index),
        C::Remove { scope, slot } => remove(app, scope, slot),
        C::Move { scope, slot, by } => move_slot(app, scope, slot, by),
        C::Open { scope, slot } => {
            if let Some(h) = unit_of(&app.state, scope) {
                app.state.mixer.pending.push(FxReq::Show { track: fx::track_of(h), slot });
            }
            app.invalidate();
            None
        }
        C::Toggle { scope, slot } => toggle(app, scope, slot),
        C::AddLane { scope, whole, slot, param } => add_lane(app, scope, whole, slot, param),
    }
}

/// Effect `index` into a slot of `scope`'s chain, in place of what it holds; its window opens as a mixer slot's does. One
/// undo step.
fn add_effect(app: &mut App, scope: Scope, slot: u8, index: usize) -> Option<String> {
    let name = app.plugins.effects.get(index)?.desc.name.clone();
    crate::control::edit_app(app, "Add plugin", Region::Project, |app| {
        let s = &mut app.state;
        let h = make_unit(s, scope)?;
        let t = fx::track_of(h);
        for r in related_units(s, scope) {
            if let Some(u) = s.project.fx_unit_mut(r) {
                u.forget_slot(is_whole(scope), slot);
            }
        }
        let m = s.project.slot_track_mut(t)?;
        let k = usize::from(slot);
        let replaced = m.slots[k].is_some();
        m.slots[k] = Some(crate::plugins::new_slot(&name));
        m.slot_on[k] = true;
        m.slot_mix[k] = 1.0;
        if replaced {
            // The old instance is kept for an undo that brings it back.
            s.mixer.pending.push(FxReq::Delete { track: t, slot });
        }
        crate::mixer::forget_slot_ui(s, t, slot);
        s.mixer.pending.push(FxReq::Create { track: t, slot, index, state: None });
        s.piano.dirty = true;
        Some(())
    });
    app.invalidate();
    None
}

fn remove(app: &mut App, scope: Scope, slot: u8) -> Option<String> {
    crate::control::edit_app(app, "Remove plugin", Region::Project, |app| {
        let s = &mut app.state;
        let h = unit_of(s, scope)?;
        for r in related_units(s, scope) {
            if let Some(u) = s.project.fx_unit_mut(r) {
                u.forget_slot(is_whole(scope), slot);
            }
        }
        crate::mixer::clear_slot(s, fx::track_of(h), slot);
        s.piano.dirty = true;
        Some(())
    });
    app.invalidate();
    None
}

fn move_slot(app: &mut App, scope: Scope, slot: u8, by: i8) -> Option<String> {
    let to = i16::from(slot) + i16::from(by);
    if !(0..SLOTS as i16).contains(&to) {
        return None;
    }
    let to = to as u8;
    crate::control::edit_app(app, "Move plugin", Region::Project, |app| {
        let s = &mut app.state;
        let h = unit_of(s, scope)?;
        let t = fx::track_of(h);
        let m = s.project.slot_track_mut(t)?;
        let (a, b) = (usize::from(slot), usize::from(to));
        m.slots.swap(a, b);
        m.slot_on.swap(a, b);
        m.slot_mix.swap(a, b);
        for r in related_units(s, scope) {
            if let Some(u) = s.project.fx_unit_mut(r) {
                u.swap_slots(is_whole(scope), slot, to);
            }
        }
        s.mixer.pending.push(FxReq::Remap(vec![((t, slot), (t, to)), ((t, to), (t, slot))]));
        s.piano.dirty = true;
        Some(())
    });
    app.invalidate();
    None
}

fn toggle(app: &mut App, scope: Scope, slot: u8) -> Option<String> {
    crate::control::edit_app(app, "Plugin on / off", Region::Project, |app| {
        let s = &mut app.state;
        let h = unit_of(s, scope)?;
        let m = s.project.slot_track_mut(fx::track_of(h))?;
        let on = m.slot_on.get_mut(usize::from(slot))?;
        *on = !*on;
        s.piano.dirty = true;
        Some(())
    });
    app.invalidate();
    None
}

/// A lane for a parameter of a plugin, starting from the parameter's value now. It is shown at once.
fn add_lane(app: &mut App, scope: Scope, whole: bool, slot: u8, param: u16) -> Option<String> {
    let lane = Lane::Param { whole, slot, param };
    let row = row_of(&app.state, scope)?;
    crate::control::edit_app(app, "Add automation", Region::Project, |app| {
        let s = &mut app.state;
        let holder = if whole { s.project.channels.get(row)?.whole_fx } else { unit_of(s, scope)? };
        let value = crate::params::param_value(s, fx_key(fx::track_of(holder), slot), usize::from(param)).unwrap_or(0.5);
        let at = match scope {
            Scope::Cut(id) => {
                let c = *s.project.arrangement()?.clips.get(clip_at(s, id)?)?;
                clip_span(s, &c).0
            }
            Scope::Whole(_) => 0.0,
        };
        let h = make_unit(s, scope)?;
        let u = s.project.fx_unit_mut(h)?;
        let l = u.lane_mut(lane);
        if l.is_empty() {
            fx::lane_add(l, at, value);
        }
        s.piano.dirty = true;
        Some(())
    });
    let s = &mut app.state;
    let e = s.clip_fx.focus.entry((row, is_whole(scope))).or_insert((lane, 0));
    *e = (lane, e.1 + 1);
    app.invalidate();
    None
}

/// A lane's new points (none: it has none any more). One undo step.
fn set_lane(app: &mut App, scope: Scope, lane: Lane, points: Vec<AutoPoint>) {
    crate::control::edit_app(app, "Automation", Region::Project, |app| {
        let s = &mut app.state;
        let h = if points.is_empty() { unit_of(s, scope)? } else { make_unit(s, scope)? };
        let u = s.project.fx_unit_mut(h)?;
        *u.lane_mut(lane) = points;
        u.params.retain(|l| !l.points.is_empty());
        s.piano.dirty = true;
        Some(())
    });
    app.invalidate();
}

fn set_mix(app: &mut App, scope: Scope, slot: u8, mix: f32) {
    crate::control::edit_app(app, "Plugin mix", Region::Project, |app| {
        let s = &mut app.state;
        let h = unit_of(s, scope)?;
        let m = s.project.slot_track_mut(fx::track_of(h))?;
        *m.slot_mix.get_mut(usize::from(slot))? = f64::from(mix.clamp(0.0, 1.0));
        s.piano.dirty = true;
        Some(())
    });
    app.invalidate();
}

/// A slot's menu on the page, at canvas point (x, y).
fn slot_menu(s: &State, scope: Scope, slot: u8, x: f32, y: f32) -> Popup {
    let on = unit_of(s, scope).and_then(|h| s.project.fx_unit(h)).is_some_and(|u| u.track.slot_on[usize::from(slot)]);
    let cmd = |c: C| Cmd::ClipFx(c);
    let mut up = PItem::new("Move up", cmd(C::Move { scope, slot, by: -1 }));
    if slot == 0 {
        up = up.off();
    }
    let mut down = PItem::new("Move down", cmd(C::Move { scope, slot, by: 1 }));
    if usize::from(slot) + 1 == SLOTS {
        down = down.off();
    }
    let items = vec![
        PItem::new("Open", cmd(C::Open { scope, slot })),
        PItem::new(if on { "Turn off" } else { "Turn on" }, cmd(C::Toggle { scope, slot })),
        PItem::sep(),
        PItem::sub("Replace", crate::plugin_menu::items(s, crate::plugin_menu::Target::ClipFx(scope, slot), true)),
        up,
        down,
        PItem::sep(),
        PItem::new("Remove", cmd(C::Remove { scope, slot })),
    ];
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// The list of parameters a lane can be added for: the plugins of `scope`'s chain, and on a cut the whole audio's too (a
/// lane there moves the shared plugin during this cut alone).
fn lane_menu(s: &State, scope: Scope, row: usize, x: f32, y: f32) -> Popup {
    let mut items = Vec::new();
    let whole = s.project.channels.get(row).map_or(0, |c| c.whole_fx);
    let chains: Vec<(u32, bool, &str)> = match scope {
        Scope::Cut(_) => vec![(unit_of(s, scope).unwrap_or(0), false, ""), (whole, true, "Whole: ")],
        Scope::Whole(_) => vec![(whole, true, "")],
    };
    for (h, on_whole, prefix) in chains {
        let Some(u) = s.project.fx_unit(h) else { continue };
        for k in 0..SLOTS as u8 {
            let Some(name) = slot_name(u, usize::from(k)) else { continue };
            let key = fx_key(fx::track_of(h), k);
            let params: Vec<PItem> = s
                .params
                .visible(key)
                .filter_map(|(i, e)| {
                    let param = u16::try_from(i).ok().filter(|&p| p < 0x8000)?;
                    Some(PItem::new(e.name.clone(), Cmd::ClipFx(C::AddLane { scope, whole: on_whole, slot: k, param })))
                })
                .collect();
            let label = format!("{prefix}{name}");
            // A plugin that lists no parameter has none that can be automated.
            items.push(if params.is_empty() { PItem::new(format!("{label} (nothing to automate)"), Cmd::Nothing).off() } else { PItem::sub(label, params) });
        }
    }
    if items.is_empty() {
        items.push(PItem::new("Add a plugin first", Cmd::Nothing).off());
    }
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// What the page asked for.
pub fn event(app: &mut App, row: usize, e: Event) {
    let tab = match &e {
        Event::Pick { tab, .. } | Event::SlotMenu { tab, .. } | Event::OpenSlot { tab, .. } | Event::ToggleSlot { tab, .. } | Event::SetMix { tab, .. } | Event::AddLane { tab, .. } | Event::SetLane { tab, .. } => *tab,
    };
    let Some(scope) = scope_of(&app.state, row, tab) else { return };
    match e {
        Event::Pick { slot, x, y, .. } => {
            let at = app.popup_at(row, x, y);
            let menu = crate::plugin_menu::menu(&app.state, crate::plugin_menu::Target::ClipFx(scope, slot), at.0, at.1);
            app.state.playlist.popup = Some(menu);
            app.invalidate();
        }
        Event::SlotMenu { slot, x, y, .. } => {
            let at = app.popup_at(row, x, y);
            app.state.playlist.popup = Some(slot_menu(&app.state, scope, slot, at.0, at.1));
            app.invalidate();
        }
        Event::AddLane { x, y, .. } => {
            let at = app.popup_at(row, x, y);
            app.state.playlist.popup = Some(lane_menu(&app.state, scope, row, at.0, at.1));
            app.invalidate();
        }
        Event::OpenSlot { slot, .. } => {
            run(app, C::Open { scope, slot });
        }
        Event::ToggleSlot { slot, .. } => {
            toggle(app, scope, slot);
        }
        Event::SetMix { slot, mix, .. } => set_mix(app, scope, slot, mix),
        Event::SetLane { lane, points, .. } => set_lane(app, scope, lane, points),
    }
}

/// Each tick: a clip copied or split, a channel cloned, shares a unit with its original; the copy gets its own, with its
/// plugins hosted from the state the original's are in now.
pub fn settle(app: &mut App) {
    if app.state.project.fx_units.is_empty() {
        return;
    }
    let made = app.state.project.separate_fx_units();
    if made.is_empty() {
        return;
    }
    let mut relink = false;
    for (from, to) in made {
        for k in 0..SLOTS as u8 {
            let key = fx_key(fx::track_of(from), k);
            let (Some(state), Some(d)) = (app.plugins.state(key), app.plugins.descriptor(key)) else { continue };
            let Some(index) = app.plugins.effect_index_of_uid(&d.uid()) else { continue };
            relink |= app.plugins.create_effect(fx::track_of(to), k, index, Some(&state)).is_ok();
        }
    }
    let s = &mut app.state;
    s.control.settle(&mut s.project);
    s.piano.dirty = true;
    if relink {
        app.transport.set_effects(app.plugins.effects());
    }
    app.invalidate();
}

/// The window title's target for a chain's plugin: "Cut part" or "Whole audio".
pub fn chain_name(p: &os_model::Project, handle: u32) -> &'static str {
    if p.channels.iter().any(|c| c.whole_fx == handle) { "Whole audio" } else { "Cut part" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_model::clip_fx::point;

    /// An app with one audio channel and two clips of it, the clips' ids settled. Its row and the clips' ids.
    fn app() -> (App, usize, u32, u32) {
        let mut a = App::new(None);
        a.plugins = crate::plugins::Plugins::at_48k();
        a.state.plugdb.rebuild(&a.plugins);
        let wav = std::env::temp_dir().join(format!("os-app-clipfx-{}-{:?}.wav", std::process::id(), std::thread::current().id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 96000 * 2]).unwrap();
        crate::playlist::add_audio_clip(&mut a.state, &wav.to_string_lossy(), 0, 0).expect("an audio clip");
        let row = a.state.project.channels.len() - 1;
        let first = *pl::clips(&a.state).last().unwrap();
        let second = Clip { position: first.end() + 96, ..first };
        pl::arr_mut(&mut a.state).clips.push(second);
        let s = &mut a.state;
        s.control.settle(&mut s.project);
        let clips = pl::clips(&a.state);
        let (x, y) = (clips[clips.len() - 2].id.0, clips[clips.len() - 1].id.0);
        (a, row, x, y)
    }

    fn undo(a: &mut App) {
        crate::history::undo(&mut a.state);
    }

    fn lanes(a: &App, id: u32) -> Vec<AutoPoint> {
        let c = pl::clips(&a.state).iter().find(|c| c.id.0 == id).copied().unwrap();
        a.state.project.fx_unit(c.fx).map_or(Vec::new(), |u| u.volume.clone())
    }

    #[test]
    fn a_cuts_lane_is_saved_per_cut_and_undo_takes_it_back() {
        let (mut a, row, x, y) = app();
        a.state.clip_fx.cut.insert(row, x);
        event(&mut a, row, Event::SetLane { tab: Tab::Cut, lane: Lane::Volume, points: vec![point(0.0, 0.5)] });
        assert_eq!(lanes(&a, x).len(), 1);
        assert!(lanes(&a, y).is_empty(), "the other cut has none");
        undo(&mut a);
        assert!(lanes(&a, x).is_empty(), "one undo step");
    }

    #[test]
    fn the_page_shows_what_the_project_holds() {
        let (mut a, row, x, y) = app();
        a.state.clip_fx.cut.insert(row, x);
        // The whole audio's lane first: the piece follows it.
        event(&mut a, row, Event::SetLane { tab: Tab::Whole, lane: Lane::Volume, points: vec![point(0.0, 0.25)] });
        let v = page_vals(&a.state, row).expect("a sample's channel has the tabs");
        assert!(v.whole.available && v.cut.available);
        assert_eq!(v.whole.lanes[0].lane, Lane::Volume);
        assert_eq!(v.whole.lanes[0].own.len(), 1);
        assert!(v.cut.lanes[0].own.is_empty());
        assert_eq!(v.cut.lanes[0].inherited.len(), 1, "the cut shows the line it follows");
        assert_eq!(v.whole.marks.len(), 2, "the whole audio shows its cuts");
        assert!(v.whole.marks.iter().all(|m| m.own.is_empty()));
        // Now the piece gets its own.
        event(&mut a, row, Event::SetLane { tab: Tab::Cut, lane: Lane::Volume, points: vec![point(0.0, 0.75)] });
        let v = page_vals(&a.state, row).unwrap();
        assert_eq!(v.cut.lanes[0].own[0].value, 0.75);
        assert_eq!(v.cut.lanes[0].inherited[0].value, 0.25);
        assert_eq!(v.whole.marks.iter().filter(|m| m.own.contains(&Lane::Volume)).count(), 1);
        // The other piece, shown instead: it follows the whole audio.
        a.state.clip_fx.cut.insert(row, y);
        let v = page_vals(&a.state, row).unwrap();
        assert!(v.cut.lanes[0].own.is_empty());
        // A piece that is gone: the Cut tab has nothing to show, the Whole tab goes on.
        pl::arr_mut(&mut a.state).clips.retain(|c| c.id.0 != y);
        let v = page_vals(&a.state, row).unwrap();
        assert!(!v.cut.available && v.whole.available);
        // The rack's own channels without a sample's clips still have the Whole tab.
        a.state.clip_fx.cut.clear();
        let v = page_vals(&a.state, row).unwrap();
        assert!(!v.cut.available && v.whole.available);
    }

    #[test]
    fn the_info_says_which_part_the_piece_is() {
        let (mut a, row, x, _) = app();
        a.state.clip_fx.cut.insert(row, x);
        let v = page_vals(&a.state, row).unwrap();
        let get = |k: &str| v.cut.rows.iter().find(|r| r.0 == k).map(|r| r.1.clone()).unwrap_or_default();
        assert!(get("File").contains("os-app-clipfx"), "{:?}", v.cut.rows);
        assert!(get("Part").starts_with("00:00.000"), "{:?}", v.cut.rows);
        assert!(get("On playlist").contains("track 1"), "{:?}", v.cut.rows);
        assert_eq!(get("Fades"), "None");
        assert_eq!(get("Gain"), "+0.0 dB");
        assert!(v.whole.rows.iter().any(|r| r.0 == "Cuts" && r.1.starts_with("2 ")), "{:?}", v.whole.rows);
        assert!(v.whole.rows.iter().any(|r| r.0 == "Length" && r.1.contains("2.000")), "{:?}", v.whole.rows);
    }

    #[test]
    fn a_channel_that_plays_no_sample_has_no_tabs() {
        let (mut a, _, _, _) = app();
        let row = crate::app::add_plugin_channel(&mut a.state.project, "Something");
        assert_eq!(page_vals(&a.state, row), None);
        assert_eq!(page_vals(&a.state, 9999), None);
    }

    #[test]
    fn adding_a_plugin_fills_the_slot_and_asks_for_an_instance() {
        let (mut a, row, x, _) = app();
        a.state.clip_fx.cut.insert(row, x);
        let index = a.plugins.effect_index("Lemon Reverb").expect("the built-in effects");
        run(&mut a, C::Effect { scope: Scope::Cut(x), slot: 3, index });
        let v = page_vals(&a.state, row).unwrap();
        assert!(v.cut.slots[3].name.is_some(), "{:?}", v.cut.slots[3]);
        assert!(v.cut.slots[3].on);
        assert_eq!(v.cut.slots[3].mix, 1.0);
        let c = pl::clips(&a.state).iter().find(|c| c.id.0 == x).copied().unwrap();
        let t = fx::track_of(c.fx);
        assert!(a.state.mixer.pending.iter().any(|r| matches!(r, FxReq::Create { track, slot: 3, .. } if *track == t)));
        // Off and on, moved, removed: each one step on the page.
        run(&mut a, C::Toggle { scope: Scope::Cut(x), slot: 3 });
        assert!(!page_vals(&a.state, row).unwrap().cut.slots[3].on);
        run(&mut a, C::Move { scope: Scope::Cut(x), slot: 3, by: -1 });
        let v = page_vals(&a.state, row).unwrap();
        assert!(v.cut.slots[2].name.is_some() && v.cut.slots[3].name.is_none() && !v.cut.slots[2].on);
        run(&mut a, C::Remove { scope: Scope::Cut(x), slot: 2 });
        assert!(page_vals(&a.state, row).unwrap().cut.slots[2].name.is_none());
        undo(&mut a);
        assert!(page_vals(&a.state, row).unwrap().cut.slots[2].name.is_some(), "undo brings it back");
    }

    #[test]
    fn a_plugin_taken_away_takes_its_lanes_with_it() {
        let (mut a, row, x, y) = app();
        a.state.clip_fx.cut.insert(row, x);
        let whole = a.state.project.whole_unit(row).unwrap();
        {
            let u = a.state.project.fx_unit_mut(whole).unwrap();
            u.track.slots[1] = Some(crate::plugins::new_slot("Fx"));
            *u.lane_mut(Lane::Param { whole: true, slot: 1, param: 2 }) = vec![point(0.0, 0.5)];
        }
        // A cut holds an override of that parameter, the other cut holds a lane of its own chain at the same slot.
        event(&mut a, row, Event::SetLane { tab: Tab::Cut, lane: Lane::Param { whole: true, slot: 1, param: 2 }, points: vec![point(0.0, 0.9)] });
        a.state.clip_fx.cut.insert(row, y);
        event(&mut a, row, Event::SetLane { tab: Tab::Cut, lane: Lane::Param { whole: false, slot: 1, param: 2 }, points: vec![point(0.0, 0.1)] });
        run(&mut a, C::Remove { scope: Scope::Whole(row), slot: 1 });
        let unit = |a: &App, id: u32| {
            let c = pl::clips(&a.state).iter().find(|c| c.id.0 == id).copied().unwrap();
            a.state.project.fx_unit(c.fx).cloned().unwrap()
        };
        assert!(a.state.project.fx_unit(whole).unwrap().params.is_empty());
        assert!(unit(&a, x).params.is_empty(), "the override went with the whole audio's plugin");
        assert_eq!(unit(&a, y).params.len(), 1, "a lane of a cut's own chain did not");
    }

    #[test]
    fn a_split_or_copied_clip_gets_a_unit_of_its_own() {
        let (mut a, _, x, _) = app();
        let i = pl::clips(&a.state).iter().position(|c| c.id.0 == x).unwrap();
        let h = a.state.project.clip_unit(i).unwrap();
        a.state.project.fx_unit_mut(h).unwrap().volume = vec![point(0.0, 0.5)];
        let copy = Clip { position: 9600, id: os_model::Id::NONE, ..pl::clips(&a.state)[i] };
        pl::arr_mut(&mut a.state).clips.push(copy);
        settle(&mut a);
        let clips = pl::clips(&a.state);
        let new = clips.iter().find(|c| c.position == 9600).unwrap();
        assert!(new.fx != 0 && new.fx != h);
        assert_eq!(a.state.project.fx_unit(new.fx).unwrap().volume.len(), 1, "it keeps what the original had");
        assert!(!new.id.is_none(), "and has an id of its own");
        let new_fx = new.fx;
        a.state.project.fx_unit_mut(new_fx).unwrap().volume.clear();
        assert_eq!(a.state.project.fx_unit(h).unwrap().volume.len(), 1, "editing the copy leaves the original");
    }

    #[test]
    fn the_lane_menu_offers_only_what_can_be_automated() {
        let (mut a, row, x, _) = app();
        a.state.clip_fx.cut.insert(row, x);
        let m = lane_menu(&a.state, Scope::Cut(x), row, 0.0, 0.0);
        assert_eq!(m.items.len(), 1, "no plugins yet");
        assert!(!m.items[0].enabled);
        // A plugin without parameters says so; it adds nothing.
        let whole = a.state.project.whole_unit(row).unwrap();
        a.state.project.fx_unit_mut(whole).unwrap().track.slots[0] = Some(crate::plugins::new_slot("Fx"));
        let m = lane_menu(&a.state, Scope::Cut(x), row, 0.0, 0.0);
        assert_eq!(m.items.len(), 1);
        assert!(m.items[0].label.starts_with("Whole: Fx"), "{}", m.items[0].label);
        assert!(!m.items[0].enabled);
    }

    #[test]
    fn a_new_lane_shows_at_once_and_starts_from_the_parameter() {
        let (mut a, row, x, _) = app();
        a.state.clip_fx.cut.insert(row, x);
        let h = a.state.project.whole_unit(row).unwrap();
        a.state.project.fx_unit_mut(h).unwrap().track.slots[0] = Some(crate::plugins::new_slot("Fx"));
        run(&mut a, C::AddLane { scope: Scope::Whole(row), whole: true, slot: 0, param: 4 });
        let v = page_vals(&a.state, row).unwrap();
        let lane = Lane::Param { whole: true, slot: 0, param: 4 };
        assert_eq!(v.whole.focus, lane);
        assert!(v.whole.focus_serial > 0);
        let l = v.whole.lanes.iter().find(|l| l.lane == lane).expect("listed");
        assert_eq!(l.own.len(), 1);
        assert!(l.name.starts_with("Fx: "), "{}", l.name);
        // The cut's tab lists it as the whole audio's, to override.
        let on_cut = v.cut.lanes.iter().find(|l| l.lane == lane).expect("listed on the cut");
        assert!(on_cut.own.is_empty() && on_cut.inherited.len() == 1);
        assert!(on_cut.name.starts_with("Whole: "), "{}", on_cut.name);
    }

    #[test]
    fn the_plugin_menu_for_a_slot_adds_to_that_slot() {
        let (a, _, x, _) = app();
        let items = crate::plugin_menu::items(&a.state, crate::plugin_menu::Target::ClipFx(Scope::Cut(x), 4), false);
        let found = items.iter().find_map(|i| match i.cmd {
            Cmd::Plugin(crate::plugin_menu::PlugCmd::ClipEffect { scope, slot, .. }) => Some((scope, slot)),
            _ => None,
        });
        assert!(!a.state.plugdb.lists.fx.is_empty(), "there are effects to list");
        assert_eq!(found, Some((Scope::Cut(x), 4)));
    }

    #[test]
    fn a_double_click_names_the_piece_and_can_ask_for_its_tab() {
        let (mut a, row, x, _) = app();
        let c = *pl::clips(&a.state).iter().find(|c| c.id.0 == x).unwrap();
        show_piece(&mut a.state, row, &c, Some(Tab::Cut));
        assert_eq!(a.state.clip_fx.cut.get(&row), Some(&x));
        assert_eq!(a.state.clip_fx.pending_tab, Some((row, Tab::Cut)));
    }

    #[test]
    fn a_whole_lane_edit_leaves_the_cuts_own_alone() {
        let (mut a, row, x, y) = app();
        a.state.clip_fx.cut.insert(row, x);
        event(&mut a, row, Event::SetLane { tab: Tab::Cut, lane: Lane::Volume, points: vec![point(0.0, 0.9)] });
        event(&mut a, row, Event::SetLane { tab: Tab::Whole, lane: Lane::Volume, points: vec![point(0.0, 0.1)] });
        event(&mut a, row, Event::SetLane { tab: Tab::Whole, lane: Lane::Volume, points: vec![point(0.0, 0.2), point(1.0, 0.3)] });
        let clips = pl::clips(&a.state).to_vec();
        let (cx, cy) = (clips.iter().find(|c| c.id.0 == x).unwrap(), clips.iter().find(|c| c.id.0 == y).unwrap());
        let r = a.state.project.clip_lane(cx, Lane::Volume).unwrap();
        assert!(r.own && r.points[0].value == 0.9);
        let r = a.state.project.clip_lane(cy, Lane::Volume).unwrap();
        assert!(!r.own && r.points.len() == 2);
    }

    #[test]
    fn clearing_a_cuts_line_gives_it_back_to_the_whole_audio() {
        let (mut a, row, x, _) = app();
        a.state.clip_fx.cut.insert(row, x);
        event(&mut a, row, Event::SetLane { tab: Tab::Whole, lane: Lane::Pan, points: vec![point(0.0, 0.1)] });
        event(&mut a, row, Event::SetLane { tab: Tab::Cut, lane: Lane::Pan, points: vec![point(0.0, 0.9)] });
        event(&mut a, row, Event::SetLane { tab: Tab::Cut, lane: Lane::Pan, points: Vec::new() });
        let c = *pl::clips(&a.state).iter().find(|c| c.id.0 == x).unwrap();
        let r = a.state.project.clip_lane(&c, Lane::Pan).unwrap();
        assert!(!r.own && r.points[0].value == 0.1);
    }

    #[test]
    fn a_chains_track_never_shows_in_the_mixer_or_gets_an_automation_clip_target() {
        let (mut a, row, _, _) = app();
        let before = a.state.project.mixer.len();
        let h = a.state.project.whole_unit(row).unwrap();
        a.state.project.fx_unit_mut(h).unwrap().track.slots[0] = Some(crate::plugins::new_slot("Fx"));
        assert_eq!(a.state.project.mixer.len(), before);
        let key = fx_key(fx::track_of(h), 0);
        assert_eq!(crate::params::target_of(&a.state, key, 3), None, "FL's target numbers have no place for it");
        assert_eq!(chain_name(&a.state.project, h), "Whole audio");
        assert_eq!(chain_name(&a.state.project, h + 77), "Cut part");
    }

    // ------------------------------------------------------------------------------------------------------------
    // The real thing: Lemon Sampler through its CLAP entry, with the real effects, as a song plays and exports.

    /// A sine of `hz` at half scale, `secs` long, in a temporary WAV file; its path.
    fn sine_wav(name: &str, hz: f64, secs: f64) -> std::path::PathBuf {
        let wav = std::env::temp_dir().join(format!("os-app-clipfx-{name}-{}-{:?}.wav", std::process::id(), std::thread::current().id()));
        let n = (48000.0 * secs) as usize;
        let x: Vec<f32> = (0..n)
            .flat_map(|i| {
                let v = ((2.0 * std::f64::consts::PI * hz * i as f64 / 48000.0).sin() * 0.5) as f32;
                [v, v]
            })
            .collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &x).unwrap();
        wav
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// Hosted plugins at 48 kHz.
    fn plugins() -> crate::plugins::Plugins {
        crate::plugins::Plugins::at_48k()
    }

    /// A project at 120 BPM (a tick is 250 frames at 48 kHz) with one audio clip channel playing `wav` on insert 1, and
    /// clips of it `(position, length, window start in seconds)`.
    fn song(wav: &std::path::Path, clips: &[(u32, u32, f64)]) -> os_model::Project {
        let mut p = crate::app::test_project();
        p.ppq = 96;
        p.tempo = Some(120.0);
        let mut ch = p.channels.remove(0);
        ch.index = 0;
        ch.kind = os_model::ChannelKind::AudioClip;
        ch.sample_path = Some(wav.to_string_lossy().into_owned());
        ch.volume = 0.78125;
        ch.mixer_track = 1;
        p.channels = vec![ch];
        p.mixer = (0..3).map(os_model::MixerTrack::new).collect();
        let clips = clips.iter().map(|&(pos, len, from)| Clip { window: Some((from * 1000.0, from * 1000.0 + 60000.0)), ..Clip::new(pos, len, 0, ClipSource::Channel(0)) }).collect();
        p.arrangements = vec![os_model::Arrangement { number: 1, clips, ..Default::default() }];
        p.current_arrangement = Some(1);
        p
    }

    /// Exports `p` through the real plugins: hosts Lemon Sampler on row 0 and what its units' slots name.
    fn export(p: &os_model::Project, pl: &mut crate::plugins::Plugins) -> Vec<f32> {
        let index = pl.index_of(crate::plugins::LEMON_SAMPLER).expect("Lemon Sampler");
        pl.create_with(0, index, Some(&crate::plugins::sampler_state(&p.channels[0]))).unwrap();
        for u in &p.fx_units {
            for k in 0..SLOTS {
                let Some(s) = &u.track.slots[k] else { continue };
                let i = pl.effect_index(&s.name).unwrap_or_else(|| panic!("no effect called {}", s.name));
                pl.create_effect(u.track.index, k as u8, i, None).unwrap();
            }
        }
        pl.set_offline(true);
        let mut lib = crate::transport::host_library(None);
        let mut e = os_engine::Engine::with_instruments(p, &mut lib, 48000, os_engine::PlayMode::Song, &pl.instruments());
        crate::project_plugins::attach_effects(&mut e, p, pl.effects());
        e.render_all(256)
    }

    /// Frames of ticks `a..b` of an interleaved render, the left side.
    fn ticks(out: &[f32], a: usize, b: usize) -> Vec<f32> {
        out.chunks_exact(2).skip(a * 250).take((b - a) * 250).map(|f| f[0]).collect()
    }

    // The instrument itself: 15 buses, the voice on the one it was given and nowhere else.
    #[test]
    fn the_real_sampler_plays_a_voice_on_its_own_output() {
        let wav = sine_wav("bus", 1000.0, 2.0);
        let mut pl = plugins();
        let mut ch = crate::app::test_project().channels.remove(0);
        ch.sample_path = Some(wav.to_string_lossy().into_owned());
        let index = pl.index_of(crate::plugins::LEMON_SAMPLER).unwrap();
        pl.create_with(0, index, Some(&crate::plugins::sampler_state(&ch))).unwrap();
        let inst = pl.hosted[&0].plugin.clone() as os_engine::SharedInstrument;
        let mut i = inst.lock().unwrap();
        assert_eq!(i.voice_buses(), 15, "Lemon Sampler has its voices' outputs");
        i.voice_start_on(0, 60, 100.0 / 128.0, 0.0, os_engine::NoteLevels::default(), 1, 3);
        let (mut main_l, mut main_r) = (0.0f32, 0.0f32);
        let (mut bus_l, mut other) = (0.0f32, 0.0f32);
        for _ in 0..40 {
            let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
            i.process(48000, &mut l, &mut r);
            main_l = main_l.max(rms(&l));
            main_r = main_r.max(rms(&r));
            let (mut bl, mut br) = (vec![0.0f32; 480], vec![0.0f32; 480]);
            assert!(i.take_bus(3, &mut bl, &mut br), "bus 3 is in use");
            bus_l = bus_l.max(rms(&bl));
            assert_eq!(rms(&bl), rms(&br), "centred");
            let (mut ol, mut or) = (vec![1.0f32; 480], vec![1.0f32; 480]);
            assert!(!i.take_bus(4, &mut ol, &mut or), "no voice ever played on bus 4");
            other = other.max(rms(&ol));
        }
        assert!(bus_l > 0.1, "the voice is on its bus: {bus_l}");
        assert_eq!((main_l, main_r), (0.0, 0.0), "and not on the main output");
        assert_eq!(other, 0.0);
        // A voice on the main output is on the main output.
        i.voice_start_on(0, 60, 100.0 / 128.0, 0.0, os_engine::NoteLevels::default(), 2, 0);
        let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
        i.process(48000, &mut l, &mut r);
        assert!(rms(&l[2400..]) > 0.1, "{}", rms(&l[2400..]));
        drop(i);
        let _ = std::fs::remove_file(&wav);
    }

    // Other plugins keep their extra ports to themselves; nothing plays on a bus an instance does not hand out.
    #[test]
    fn other_instruments_have_no_voice_buses() {
        let mut pl = plugins();
        let index = pl.index_of(crate::osc3::LEMON_TRIO).unwrap();
        pl.create_with(0, index, None).unwrap();
        let inst = pl.hosted[&0].plugin.clone() as os_engine::SharedInstrument;
        assert_eq!(inst.lock().unwrap().voice_buses(), 0);
    }

    // A song through the real sampler: lanes on a whole audio and a cut move those clips, and only those; the levels are
    // what the plain song plays times the lanes' gains.
    #[test]
    fn real_sampler_lanes_follow_the_rule() {
        let wav = sine_wav("lanes", 1000.0, 4.0);
        let clips = [(0, 96, 0.0), (96, 96, 0.5), (192, 96, 1.0)];
        let plain = export(&song(&wav, &clips), &mut plugins());
        let level = rms(&ticks(&plain, 24, 72));
        assert!(level > 0.1, "{level}");

        let mut p = song(&wav, &clips);
        let whole = p.whole_unit(0).unwrap();
        p.fx_unit_mut(whole).unwrap().volume = vec![point(0.0, 0.4)];
        let cut = p.clip_unit(1).unwrap();
        p.fx_unit_mut(cut).unwrap().volume = vec![point(0.0, 0.7)];
        let out = export(&p, &mut plugins());
        let g = |v: f64| os_engine::lane_gain(v);
        let check = |a: usize, b: usize, want: f32, what: &str| {
            let got = rms(&ticks(&out, a, b));
            assert!((got - want).abs() < want * 0.02 + 1e-3, "{what}: {got} against {want}");
        };
        check(24, 72, level * g(0.4), "the first cut follows the whole audio");
        check(120, 168, level * g(0.7), "the second has its own line");
        check(216, 264, level * g(0.4), "the third follows the whole audio");
        let _ = std::fs::remove_file(&wav);
    }

    // A pan lane through the real sampler.
    #[test]
    fn a_real_samplers_pan_lane_moves_the_cut() {
        let wav = sine_wav("pan", 1000.0, 2.0);
        let mut p = song(&wav, &[(0, 192, 0.0)]);
        let cut = p.clip_unit(0).unwrap();
        p.fx_unit_mut(cut).unwrap().pan = vec![point(0.0, 0.0), point(1.0, 1.0)];
        let out = export(&p, &mut plugins());
        let side = |k: usize, a: usize, b: usize| -> f32 {
            let x: Vec<f32> = out.chunks_exact(2).skip(a * 250).take((b - a) * 250).map(|f| f[k]).collect();
            rms(&x)
        };
        assert!(side(0, 2, 12) > side(1, 2, 12) * 4.0, "it starts on the left: {} {}", side(0, 2, 12), side(1, 2, 12));
        assert!(side(1, 180, 190) > side(0, 180, 190) * 4.0, "it ends on the right: {} {}", side(0, 180, 190), side(1, 180, 190));
        assert!((side(0, 92, 100) - side(1, 92, 100)).abs() < side(0, 92, 100) * 0.1, "centred half way");
        let _ = std::fs::remove_file(&wav);
    }

    // A real reverb on one cut: that cut grows a tail that rings on after it, the other cut stays as dry as it was.
    #[test]
    fn a_real_plugin_on_a_cut_changes_that_cut_alone() {
        let wav = sine_wav("reverb", 1000.0, 4.0);
        // Two cuts, with silence after each. The first has the reverb.
        let clips = [(0, 48, 0.0), (1920, 48, 0.0)];
        let plain = export(&song(&wav, &clips), &mut plugins());
        assert!(rms(&ticks(&plain, 60, 150)) < 1e-4, "dry: silence after the cut");
        let dry_second = ticks(&plain, 1920, 1968);

        let mut p = song(&wav, &clips);
        let cut = p.clip_unit(0).unwrap();
        p.fx_unit_mut(cut).unwrap().track.slots[0] = Some(crate::plugins::new_slot("Lemon Reverb"));
        let out = export(&p, &mut plugins());
        let tail = rms(&ticks(&out, 56, 150));
        assert!(tail > 1e-3, "the first cut rings on through its reverb: {tail}");
        // The second cut, ten seconds on, is the same dry signal but for whatever is left of the first's tail by then.
        let second = ticks(&out, 1920, 1968);
        assert_eq!(second.len(), dry_second.len());
        let diff: f32 = second.iter().zip(&dry_second).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        let residual = rms(&ticks(&out, 1900, 1918));
        assert!(diff <= residual * 8.0 + 1e-3, "the other cut is unchanged but for the tail reaching it: {diff}, {residual}");
        let _ = std::fs::remove_file(&wav);
    }

    // A real plugin on the whole audio: every cut changes.
    #[test]
    fn a_real_plugin_on_the_whole_audio_changes_every_cut() {
        let wav = sine_wav("whole", 1000.0, 4.0);
        let clips = [(0, 48, 0.0), (192, 48, 0.0)];
        let mut p = song(&wav, &clips);
        let whole = p.whole_unit(0).unwrap();
        p.fx_unit_mut(whole).unwrap().track.slots[0] = Some(crate::plugins::new_slot("Lemon Reverb"));
        let out = export(&p, &mut plugins());
        assert!(rms(&ticks(&out, 56, 150)) > 1e-3, "the first cut has a tail");
        assert!(rms(&ticks(&out, 248, 340)) > 1e-3, "and so has the second");
        let _ = std::fs::remove_file(&wav);
    }

    // A real parameter lane moves the real plugin as the song plays.
    #[test]
    fn a_real_parameter_lane_moves_the_real_plugin() {
        let wav = sine_wav("param", 1000.0, 4.0);
        let mut p = song(&wav, &[(0, 192, 0.0)]);
        let cut = p.clip_unit(0).unwrap();
        p.fx_unit_mut(cut).unwrap().track.slots[0] = Some(crate::plugins::new_slot("Lemon Balance"));
        // Lane: the first parameter from 0 to 1 over the clip.
        let lane = Lane::Param { whole: false, slot: 0, param: 0 };
        *p.fx_unit_mut(cut).unwrap().lane_mut(lane) = vec![point(0.0, 0.0), point(1.0, 1.0)];
        let mut pl = plugins();
        let out = export(&p, &mut pl);
        assert!(!out.is_empty());
        let key = fx_key(fx::track_of(cut), 0);
        let plugin = pl.hosted[&key].plugin.clone();
        let g = plugin.lock().unwrap();
        let info = g.param_info(0).expect("a first parameter");
        let v = g.param_value(info.id).expect("its value");
        assert!(v > 0.9, "the lane took it to its end: {v}");
        drop(g);
        let _ = std::fs::remove_file(&wav);
    }

    // The song's mixer tracks are as they were: the chain plays into the channel's insert, whose fader decides.
    #[test]
    fn a_real_chain_plays_through_the_channels_mixer_track() {
        let wav = sine_wav("mixer", 1000.0, 2.0);
        let mut p = song(&wav, &[(0, 96, 0.0)]);
        let cut = p.clip_unit(0).unwrap();
        p.fx_unit_mut(cut).unwrap().track.slots[0] = Some(crate::plugins::new_slot("Lemon Balance"));
        let level = rms(&ticks(&export(&p, &mut plugins()), 24, 72));
        assert!(level > 0.1, "{level}");
        p.mixer[1].volume = Some(0.0);
        let down = rms(&ticks(&export(&p, &mut plugins()), 24, 72));
        assert!(down < 1e-4, "{down}");
        let _ = std::fs::remove_file(&wav);
    }

    // Plugins and lanes make the round trip through a real file: states as saved, hosted again on opening.
    #[test]
    fn a_real_plugins_state_comes_back_from_a_saved_project() {
        let wav = sine_wav("state", 1000.0, 1.0);
        let mut p = song(&wav, &[(0, 96, 0.0)]);
        let cut = p.clip_unit(0).unwrap();
        p.fx_unit_mut(cut).unwrap().track.slots[2] = Some(crate::plugins::new_slot("Lemon Balance"));
        let mut pl = plugins();
        let i = pl.effect_index("Lemon Balance").unwrap();
        pl.create_effect(fx::track_of(cut), 2, i, None).unwrap();
        // Move a parameter, as its window would. The plugin keeps it in its own steps: what it holds then is what comes
        // back.
        let key = fx_key(fx::track_of(cut), 2);
        let held = {
            let mut g = pl.hosted[&key].plugin.lock().unwrap();
            let info = g.param_info(0).unwrap();
            assert!(g.set_param(info.id, 0.123));
            // It reaches the plugin with its next block, as it does while the app runs.
            let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
            g.process_fx(48000, &mut l, &mut r);
            g.param_value(info.id).unwrap()
        };
        assert!((held - 0.123).abs() < 0.01, "{held}");
        let default = {
            let fresh = plugins();
            let mut other = fresh;
            other.create_effect(0, 0, i, None).unwrap();
            let g = other.hosted[&fx_key(0, 0)].plugin.lock().unwrap();
            g.param_value(g.param_info(0).unwrap().id).unwrap()
        };
        assert!((held - default).abs() > 0.1, "a value other than a new instance's: {held} {default}");
        let mut saved = p.clone();
        crate::project_plugins::store_hosted(&mut saved, &pl);
        let slot = saved.fx_unit(cut).unwrap().track.slots[2].as_ref().unwrap();
        let wrapped = crate::project_plugins::wrapped_slot(slot).expect("stored as a wrapped plugin, as a mixer slot is");
        assert_eq!(wrapped.name, "Lemon Balance");

        let mut file = os_flp::project::create(&saved).unwrap();
        os_flp::project::apply(&saved, &mut file).unwrap();
        let back = os_flp::project::load(&os_flp::FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        let slot = back.fx_unit(cut).unwrap().track.slots[2].as_ref().unwrap();
        let r = crate::project_plugins::wrapped_slot(slot).unwrap();
        let mut again = plugins();
        let i = crate::project_plugins::find(&again.effects, &r).expect("found again");
        again.create_effect(fx::track_of(cut), 2, i, Some(&r.state)).unwrap();
        let g = again.hosted[&key].plugin.lock().unwrap();
        let info = g.param_info(0).unwrap();
        assert!((g.param_value(info.id).unwrap() - held).abs() < 1e-6, "its value is back");
        drop(g);
        let _ = std::fs::remove_file(&wav);
    }

    // The app hosts a chain's plugin under its chain's number, and it follows its slot through an undo and a redo.
    #[test]
    fn the_app_hosts_a_chains_plugin_and_it_follows_undo_and_redo() {
        let (mut a, row, x, _) = app();
        a.state.clip_fx.cut.insert(row, x);
        let i = a.plugins.effect_index("Lemon Balance").expect("the built-in effects");
        run(&mut a, C::Effect { scope: Scope::Cut(x), slot: 0, index: i });
        a.mixer_tick();
        let c = *pl::clips(&a.state).iter().find(|c| c.id.0 == x).unwrap();
        let key = fx_key(fx::track_of(c.fx), 0);
        assert!(a.plugins.hosted.contains_key(&key), "hosted under its chain's number");
        // Ids settle, and the instance follows its slot.
        let s = &mut a.state;
        s.control.settle(&mut s.project);
        a.follow_ids(true, true);
        assert!(a.plugins.hosted.contains_key(&key), "it stays where it belongs");
        assert!(!a.plugins.hosted[&key].owner.is_none(), "it takes its slot's id");
        crate::history::undo(&mut a.state);
        a.follow_ids(true, true);
        assert!(!a.plugins.hosted.contains_key(&key), "undone, it goes");
        crate::history::redo(&mut a.state);
        a.follow_ids(true, true);
        assert!(a.plugins.hosted.contains_key(&key), "redone, it is back");
    }

    /// A project to try this on in the real program: a two-second tone cut into three clips on one playlist track, saved
    /// as `clip-fx-demo.flp` beside its sample in `OS_SHOT_DIR`
    /// (`OS_SHOT_DIR=<dir> cargo test -p os-app --bin lemonstudio clip_fx_demo_project -- --ignored`).
    #[test]
    #[ignore]
    fn clip_fx_demo_project() {
        let dir = std::path::PathBuf::from(std::env::var("OS_SHOT_DIR").expect("OS_SHOT_DIR"));
        let wav = dir.join("clip-fx-demo.wav");
        let x: Vec<f32> = (0..48000 * 16)
            .flat_map(|i| {
                let t = f64::from(i) / 48000.0;
                let beat = (t * 2.0).fract();
                let v = (2.0 * std::f64::consts::PI * 220.0 * t).sin() * (-4.0 * beat).exp() * 0.8;
                [v as f32, v as f32]
            })
            .collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &x).unwrap();
        let mut a = App::new(None);
        crate::playlist::add_audio_clip(&mut a.state, &wav.to_string_lossy(), 0, 0).expect("an audio clip");
        let first = *pl::clips(&a.state).last().unwrap();
        // Cut in three, the middle one from further on in the sample, with room between them on the playlist.
        let len = first.length / 4;
        let ms = |ticks: u32| pl::to_src(&a.state, first.source, f64::from(ticks));
        let piece = |pos: u32, from: u32| Clip { position: pos, length: len, window: Some((ms(from), ms(from + len))), ..first };
        let bar = a.state.project.bar_ticks();
        let pieces = [piece(0, 0), piece(len + bar, len), piece(2 * (len + bar), 3 * len)];
        let arr = pl::arr_mut(&mut a.state);
        arr.clips.retain(|c| c.source != first.source);
        arr.clips.extend(pieces);
        // With `OS_DEMO_FULL`: a line on the whole audio, the middle cut with its own line and a plugin, for looking at.
        if std::env::var_os("OS_DEMO_FULL").is_some() {
            a.plugins = crate::plugins::Plugins::at_48k();
            let row = a.state.project.channels.iter().position(|c| ClipSource::Channel(c.index) == first.source).unwrap();
            let whole = a.state.project.whole_unit(row).unwrap();
            let total = pl::to_src(&a.state, first.source, f64::from(first.length)) / 1000.0;
            a.state.project.fx_unit_mut(whole).unwrap().volume = vec![point(0.0, 0.8), point(total, 0.15)];
            a.state.project.fx_unit_mut(whole).unwrap().pan = vec![point(0.0, 0.2), point(total, 0.8)];
            let i = pl::clips(&a.state).iter().position(|c| c.position == len + bar).unwrap();
            let cut = a.state.project.clip_unit(i).unwrap();
            let from = pl::to_src(&a.state, first.source, f64::from(len)) / 1000.0;
            let to = pl::to_src(&a.state, first.source, f64::from(2 * len)) / 1000.0;
            {
                let u = a.state.project.fx_unit_mut(cut).unwrap();
                u.volume = vec![point(from, 0.2), point((from + to) / 2.0, 0.95), point(to, 0.3)];
                u.track.slots[0] = Some(crate::plugins::new_slot("Lemon Reverb"));
            }
            let index = a.plugins.effect_index("Lemon Reverb").unwrap();
            a.plugins.create_effect(fx::track_of(cut), 0, index, None).unwrap();
        }
        let path = dir.join("clip-fx-demo.flp");
        assert_eq!(a.save_copy(path.clone()), Some(Ok(())), "{:?}", a.state.message);
        eprintln!("DEMO {}", path.display());
    }

    #[test]
    fn a_saved_project_brings_its_units_and_clips_back() {
        let (mut a, row, x, y) = app();
        a.state.clip_fx.cut.insert(row, x);
        event(&mut a, row, Event::SetLane { tab: Tab::Cut, lane: Lane::Volume, points: vec![point(1.0, 0.25)] });
        event(&mut a, row, Event::SetLane { tab: Tab::Whole, lane: Lane::Pan, points: vec![point(2.0, 0.75)] });
        let path = std::env::temp_dir().join(format!("os-app-clipfx-save-{}.flp", std::process::id()));
        assert_eq!(a.save_copy(path.clone()), Some(Ok(())), "{:?}", a.state.message);
        let f = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        let p = os_flp::project::load(&f).unwrap();
        let clips = &p.arrangements.iter().find(|arr| !arr.clips.is_empty()).unwrap().clips;
        let saved: Vec<&Clip> = clips.iter().filter(|c| c.fx != 0).collect();
        assert_eq!(saved.len(), 1, "the one cut with a lane");
        let u = p.fx_unit(saved[0].fx).unwrap();
        assert_eq!(u.volume.len(), 1);
        assert_eq!(u.volume[0].value, 0.25);
        let w = p.channels[row].whole_fx;
        assert_eq!(p.fx_unit(w).unwrap().pan[0].value, 0.75);
        let _ = y;
        let _ = std::fs::remove_file(&path);
    }
}
