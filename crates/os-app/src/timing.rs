// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The vocal timing tools on audio clips (the playlist's clip and track menus, TOOLS ▸ Vocal timing). None listens
//! for words, so they work in any language: they go by how the voice sounds (`os_dsp::syllables`, `os_dsp::align`).
//!
//! - Fit to tempo: each syllable moved onto the beat grid as the flow reads (`os_dsp::flow`), the time taken where it
//!   is least heard (pauses first, then vowels), attacks moved whole.
//! - Match timing to track: a double or a backing vocal moved onto another track's timing, syllable by syllable, for
//!   layering.
//! - Comp takes: several takes of one part made into one, phrase by phrase the best of them, lined up with each other
//!   and joined in the pauses after the last word (`os_dsp::comp`).
//!
//! The new audio goes to the data folder's Fitted folder. Fit and Match give the clips' channel the new file and move
//! the clips' trims with the audio; Comp puts a new audio clip under the takes and mutes them. Each is one undo step.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use os_dsp::flow::{Grid, Settings, Span};
use os_dsp::syllables::Scan;
use os_dsp::{align, comp, flow, retime, warp};
use os_engine::sample::Sample;
use os_model::{Clip, ClipSource};

use crate::app::{App, State};
use crate::history::Snap;
use crate::playlist::{self, clips, Cmd, Mark, PItem, PickKind};

/// How closely Fit to tempo pulls the syllables onto the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feel {
    Tight,
    Natural,
    Loose,
}

impl Feel {
    pub const ALL: [Feel; 3] = [Feel::Tight, Feel::Natural, Feel::Loose];

    pub fn label(self) -> &'static str {
        match self {
            Feel::Tight => "Tight",
            Feel::Natural => "Natural",
            Feel::Loose => "Loose",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Feel::Tight => "Every syllable right on its grid line, the take's own laid-back or pushed feel taken out too",
            Feel::Natural => "Most of the way onto the grid; syllables already within 8 ms stay, and the take keeps its pocket",
            Feel::Loose => "Just the clearly late or early syllables brought in; within 15 ms they stay, the pocket is kept",
        }
    }

    fn settings(self, grid: Grid) -> Settings {
        match self {
            Feel::Tight => Settings { strength: 1.0, tolerance: 0.0, keep_pocket: false, grid },
            Feel::Natural => Settings { strength: 0.85, tolerance: 0.008, keep_pocket: true, grid },
            Feel::Loose => Settings { strength: 0.6, tolerance: 0.015, keep_pocket: true, grid },
        }
    }
}

/// The clips a tool works on: a right-clicked clip (with the others selected along with it), a track's, or the
/// selected ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Clip(usize),
    Track(u16),
    Selected,
}

/// The tools' commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TCmd {
    Fit(Target, Feel),
    /// Fit to tempo's grid: steps per beat, 0 for Auto.
    Grid(u32),
    /// Match timing to track onto the given guide track.
    Match(Target, u16),
    /// Match timing to track: all the way (false) or most of it (true).
    MatchLoose(bool),
    Comp(Target),
}

/// Fit to tempo's grids: steps per beat (0: Auto), name, what it does.
pub const GRIDS: [(u32, &str, &str); 5] = [
    (0, "Auto (follows the flow)", "Reads each beat as sixteenths, sixteenth triplets or thirty-seconds, whichever its syllables fit, as a flow switches between them"),
    (2, "1/8", "Every syllable to its nearest eighth note"),
    (4, "1/16", "Every syllable to its nearest sixteenth, the usual rap grid"),
    (6, "1/16 triplets", "Every syllable to its nearest sixteenth triplet, for a triplet flow"),
    (8, "1/32", "Every syllable to its nearest thirty-second, for a double-time flow"),
];

const GRID_KEY: &str = "fit_tempo_grid";
const LOOSE_KEY: &str = "match_timing_loose";

/// Fit to tempo's grid now (steps per beat, 0: Auto).
fn grid() -> u32 {
    crate::settings_file::get(GRID_KEY).and_then(|v| v.parse().ok()).filter(|g| GRIDS.iter().any(|x| x.0 == *g)).unwrap_or(0)
}

fn loose() -> bool {
    crate::settings_file::flag(LOOSE_KEY, false)
}

/// What a tool says about itself (the menus' tips).
pub const FIT_TIP: &str = "Moves each syllable onto the beat grid, stretching and squeezing tiny pieces where it is least heard (pauses, then vowels); reads the flow by itself";
pub const MATCH_TIP: &str = "Moves the timing onto another track's, syllable by syllable, so a double or backing vocal sits exactly with the lead";
pub const COMP_TIP: &str = "Makes one take of several takes of the same part: lines them up, keeps the best version of each phrase and joins them in the pauses";

/// Fit to tempo's submenu: the feels (each fits), then the grid.
pub fn fit_items(target: Target) -> Vec<PItem> {
    let g = grid();
    let mut v = vec![PItem::head("Feel")];
    v.extend(Feel::ALL.map(|f| PItem::new(f.label(), Cmd::Timing(TCmd::Fit(target, f))).tip(f.tip())));
    v.push(PItem::head("Grid"));
    v.extend(GRIDS.iter().map(|&(n, label, tip)| PItem::new(label, Cmd::Timing(TCmd::Grid(n))).mark(Mark::Radio(g == n)).tip(tip)));
    v
}

/// Match timing to track's submenu: how close, then the tracks with audio (all but `skip`) to match.
pub fn match_items(s: &State, target: Target, skip: Option<u16>) -> Vec<PItem> {
    let l = loose();
    let mut v = vec![
        PItem::head("How close"),
        PItem::new("Tight", Cmd::Timing(TCmd::MatchLoose(false))).mark(Mark::Radio(!l)).tip("Every syllable and every phrase end exactly where the guide track has them"),
        PItem::new("Natural", Cmd::Timing(TCmd::MatchLoose(true))).mark(Mark::Radio(l)).tip("Most of the way to the guide track, so the layer still sounds like a second performance"),
        PItem::head("Guide track"),
    ];
    let mut tracks: Vec<u16> = clips(s).iter().filter(|c| !c.muted && is_audio(s, c)).map(|c| c.track).filter(|t| Some(*t) != skip).collect();
    tracks.sort_unstable();
    tracks.dedup();
    if tracks.is_empty() {
        v.push(PItem::new("(no other track has audio)", Cmd::Nothing).off());
    }
    for t in tracks {
        let name = playlist::track_name(s, t);
        let tip = format!("Moves the timing onto {name}'s");
        v.push(PItem::new(name, Cmd::Timing(TCmd::Match(target, t))).tip(tip));
    }
    v
}

/// The Vocal timing part of an audio clip's menu.
pub fn clip_items(s: &State, i: usize) -> Vec<PItem> {
    let track = clips(s).get(i).map(|c| c.track);
    let comp = PItem::new("Comp selected takes", Cmd::Timing(TCmd::Comp(Target::Clip(i)))).tip(COMP_TIP);
    vec![
        PItem::head("Vocal timing"),
        PItem::sub("Fit to tempo", fit_items(Target::Clip(i))).tip(FIT_TIP),
        PItem::sub("Match timing to track", match_items(s, Target::Clip(i), track)).tip(MATCH_TIP),
        if targets(s, Target::Clip(i)).len() >= 2 { comp } else { comp.off() },
    ]
}

/// The Vocal timing part of a track's menu.
pub fn track_items(s: &State, t: u16) -> Vec<PItem> {
    let comp = PItem::new("Comp takes on this track", Cmd::Timing(TCmd::Comp(Target::Track(t)))).tip(COMP_TIP);
    vec![
        PItem::head("Vocal timing"),
        PItem::sub("Fit audio to tempo", fit_items(Target::Track(t))).tip(FIT_TIP),
        PItem::sub("Match audio to track", match_items(s, Target::Track(t), Some(t))).tip(MATCH_TIP),
        if targets(s, Target::Track(t)).len() >= 2 { comp } else { comp.off() },
    ]
}

fn is_audio(s: &State, c: &Clip) -> bool {
    matches!(c.source, ClipSource::Channel(_)) && playlist::source_kind(s, c.source) == PickKind::Audio
}

/// The audio clips `target` names, in song order: a right-clicked clip that is selected brings the other selected
/// ones; a track's are its clips that play (not muted).
pub fn targets(s: &State, target: Target) -> Vec<usize> {
    let all = clips(s);
    let audio = |i: &usize| all.get(*i).is_some_and(|c| is_audio(s, c));
    let mut v: Vec<usize> = match target {
        Target::Clip(i) if s.playlist.sel.contains(&i) => s.playlist.sel.iter().copied().filter(audio).collect(),
        Target::Clip(i) => [i].into_iter().filter(audio).collect(),
        Target::Track(t) => (0..all.len()).filter(|&i| all[i].track == t && !all[i].muted).filter(audio).collect(),
        Target::Selected => s.playlist.sel.iter().copied().filter(audio).collect(),
    };
    v.sort_by_key(|&i| (all[i].position, all[i].track));
    v.dedup();
    v
}

/// Runs a command; returns the status line.
pub fn run(app: &mut App, c: TCmd) -> Option<String> {
    let msg = match c {
        TCmd::Grid(n) => {
            crate::settings_file::write(GRID_KEY, &n.to_string());
            return None;
        }
        TCmd::MatchLoose(on) => {
            crate::settings_file::set_flag(LOOSE_KEY, on);
            return None;
        }
        TCmd::Fit(t, feel) => fit(&mut app.state, t, feel),
        TCmd::Match(t, guide) => match_to(&mut app.state, t, guide),
        TCmd::Comp(t) => comp_takes(&mut app.state, t),
    };
    app.invalidate();
    Some(msg)
}

fn load(path: &str) -> Result<Arc<Sample>, String> {
    let file = crate::plugins::resolve_sample(path);
    os_engine::sample::load_shared(Path::new(&file)).map_err(|e| e.to_string())
}

fn mono(l: &[f32], r: &[f32]) -> Vec<f32> {
    l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)).collect()
}

/// A stretch of a sample, frames `a` .. `b`.
fn cut(x: &[f32], a: usize, b: usize) -> Vec<f32> {
    x.get(a.min(x.len())..b.min(x.len())).map_or_else(Vec::new, <[f32]>::to_vec)
}

/// `x` at `from` Hz played at `to` Hz (linear: the takes of a comp are rarely at different rates).
fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let k = f64::from(from) / f64::from(to);
    let n = (x.len() as f64 / k) as usize;
    (0..n)
        .map(|i| {
            let p = i as f64 * k;
            let j = p as usize;
            let f = (p - j as f64) as f32;
            let a = x[j.min(x.len() - 1)];
            a + (x[(j + 1).min(x.len() - 1)] - a) * f
        })
        .collect()
}

/// A time map in seconds as frames at `rate`.
fn frames(map: &[(f64, f64)], rate: u32) -> Vec<(f64, f64)> {
    let r = f64::from(rate);
    map.iter().map(|(a, b)| (a * r, b * r)).collect()
}

/// The Fitted folder, beside Recorded (`one_click`).
fn out_dir(s: &State) -> PathBuf {
    crate::data_folder::folder(s).or_else(crate::app::theme_dir).unwrap_or_else(std::env::temp_dir).join("Fitted")
}

/// A file's name without its folder, extension, and an earlier tool's ending.
fn stem(path: &str) -> String {
    let s = Path::new(path).file_stem().map_or_else(|| "Audio".to_string(), |n| n.to_string_lossy().into_owned());
    [" fitted", " matched", " comp"].iter().find_map(|e| s.strip_suffix(e)).map_or(s.clone(), str::to_string)
}

/// Writes "<stem> <what>.wav" (a number after it when taken) to the Fitted folder.
fn write(s: &State, stem: &str, what: &str, rate: u32, l: &[f32], r: &[f32]) -> Result<PathBuf, String> {
    let dir = out_dir(s);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut path = dir.join(format!("{stem} {what}.wav"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} {what} {n}.wav"));
        n += 1;
    }
    let inter: Vec<f32> = l.iter().zip(r).flat_map(|(a, b)| [*a, *b]).collect();
    let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut w = std::io::BufWriter::new(file);
    os_engine::wav::write_f32_stereo(&mut w, rate, &inter).and_then(|()| std::io::Write::flush(&mut w)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// A channel's new audio: its rack row and index, the file, the map from the old file's seconds to the new one's, and
/// both lengths, seconds.
struct Swap {
    row: usize,
    ch: u16,
    path: PathBuf,
    map: Vec<(f64, f64)>,
    old: f64,
    new: f64,
}

/// Gives each channel its new audio, one undo step `name`: the channel's clips in this arrangement keep playing the
/// same audio (their trims through the map), a fitted stretch time keeps the speed.
fn apply(s: &mut State, name: &'static str, swaps: &[Swap]) {
    let snaps: Vec<Snap> = swaps.iter().filter_map(|w| Snap::sample(s, w.row)).collect();
    playlist::snapshot_with(s, name, Snap::Several(snaps));
    for w in swaps {
        let path = w.path.to_string_lossy().into_owned();
        let Some(ch) = s.project.channels.get_mut(w.row) else { continue };
        ch.sample_path = Some(path.clone());
        if ch.sound.stretch_time().is_some() && w.old > 0.0 {
            ch.sound.stretch[0] = (f32::from_bits(ch.sound.stretch[0]) * (w.new / w.old) as f32).to_bits();
        }
        s.sample_loads.push((w.row, path));
        let m = warp::Map::new(&w.map);
        let src = ClipSource::Channel(w.ch);
        if let Some(a) = s.project.arrangement_mut() {
            for c in a.clips.iter_mut().filter(|c| c.source == src) {
                if let Some((w0, w1)) = c.window {
                    c.window = Some((m.forward(w0 / 1000.0) * 1000.0, m.forward(w1 / 1000.0) * 1000.0));
                }
            }
        }
    }
    s.piano.dirty = true;
}

fn row_of(s: &State, ch: u16) -> Option<usize> {
    s.project.channels.iter().position(|c| c.index == ch)
}

/// Where clip `c` plays its sample against the beat grid (`flow::Span`).
fn span(s: &State, c: &Clip) -> Span {
    let ppq = f64::from(playlist::ppq(s));
    let ms_per_tick = playlist::to_src(s, c.source, 1.0);
    let (w0, w1) = playlist::window(s, c);
    Span { from: w0 / 1000.0, to: w1 / 1000.0, beat: f64::from(c.position) / ppq, beats_per_sec: 1000.0 / (ms_per_tick * ppq) }
}

fn ms(secs: f64) -> String {
    format!("{} ms", (secs * 1000.0).round())
}

/// Fit to tempo (`os_dsp::flow`) on `target`'s clips, each channel's file once.
fn fit(s: &mut State, target: Target, feel: Feel) -> String {
    let list = targets(s, target);
    if list.is_empty() {
        return "Fit to tempo works on audio clips: right-click one, or select some".into();
    }
    let g = grid();
    let set = feel.settings(if g == 0 { Grid::Auto } else { Grid::Steps(g) });
    let mut per: BTreeMap<u16, Vec<Span>> = BTreeMap::new();
    for &i in &list {
        let c = clips(s)[i];
        if let ClipSource::Channel(ch) = c.source {
            per.entry(ch).or_default().push(span(s, &c));
        }
    }
    let (mut found, mut moved, mut total, mut triplets, mut fast) = (0usize, 0usize, 0.0, 0usize, 0usize);
    let (mut swaps, mut errors) = (Vec::new(), Vec::new());
    for (ch, spans) in per {
        let Some(row) = row_of(s, ch) else { continue };
        let Some(path) = s.project.channels[row].sample_path.clone() else { continue };
        let sample = match load(&path) {
            Ok(x) => x,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let scan = Scan::new(&mono(&sample.left, &sample.right), sample.rate);
        let plan = flow::plan(&scan, &spans, &set);
        found += plan.syllables;
        moved += plan.moved;
        total += plan.mean_move * plan.moved as f64;
        triplets += plan.triplets.len();
        fast += plan.fast.len();
        if plan.moved == 0 {
            continue;
        }
        let (l, r) = warp::render(&sample.left, &sample.right, sample.rate, &frames(&plan.map, sample.rate));
        let new = l.len() as f64 / f64::from(sample.rate);
        match write(s, &stem(&path), "fitted", sample.rate, &l, &r) {
            Ok(p) => swaps.push(Swap { row, ch, path: p, map: plan.map, old: scan.len, new }),
            Err(e) => errors.push(e),
        }
    }
    if !swaps.is_empty() {
        apply(s, "Fit to tempo", &swaps);
    }
    if let Some(e) = errors.first() {
        return format!("Fit to tempo: {e}");
    }
    if found == 0 {
        return "Fit to tempo: no syllables found in the clip".into();
    }
    if moved == 0 {
        return format!("Fit to tempo: all {found} syllables are already on the beat");
    }
    let mut flow = Vec::new();
    if triplets > 0 {
        flow.push(format!("{triplets} beats in triplets"));
    }
    if fast > 0 {
        flow.push(format!("{fast} in thirty-seconds"));
    }
    let flow = if flow.is_empty() { String::new() } else { format!(" ({})", flow.join(", ")) };
    format!("Fit to tempo: moved {moved} of {found} syllables, {} on average{flow}", ms(total / moved as f64))
}

/// How far either side of where they are placed a clip and its guide may be found apart, seconds.
const MATCH_REACH: f64 = 0.5;

/// Match timing to track (`os_dsp::align`): each of `target`'s clips (those not on `guide`) moved onto what the guide
/// track plays over it.
fn match_to(s: &mut State, target: Target, guide: u16) -> String {
    let name = playlist::track_name(s, guide);
    let list: Vec<usize> = targets(s, target).into_iter().filter(|&i| clips(s)[i].track != guide).collect();
    if list.is_empty() {
        return format!("Match timing to {name}: choose audio clips on another track");
    }
    let strength = if loose() { 0.7 } else { 1.0 };
    let spt = playlist::secs_per_tick(s);
    // The guide track's audio clips that play, with their samples.
    let mut guides = Vec::new();
    for c in clips(s).iter().filter(|c| c.track == guide && !c.muted && is_audio(s, c)) {
        let ClipSource::Channel(ch) = c.source else { continue };
        let Some(path) = row_of(s, ch).and_then(|r| s.project.channels[r].sample_path.clone()) else { continue };
        if let Ok(x) = load(&path) {
            let (w0, _) = playlist::window(s, c);
            let speed = playlist::audio_speed(s, c.source);
            guides.push((f64::from(c.position) * spt, f64::from(c.end()) * spt, w0 / 1000.0, speed, mono(&x.left, &x.right), x.rate));
        }
    }
    if guides.is_empty() {
        return format!("Match timing to {name}: it has no audio clips that play");
    }
    let mut per: BTreeMap<u16, Vec<(f64, f64)>> = BTreeMap::new();
    let (mut count, mut moves, mut total, mut errors) = (0usize, 0usize, 0.0, Vec::new());
    let mut samples: BTreeMap<u16, Arc<Sample>> = BTreeMap::new();
    for &i in &list {
        let c = clips(s)[i];
        let ClipSource::Channel(ch) = c.source else { continue };
        let Some(path) = row_of(s, ch).and_then(|r| s.project.channels[r].sample_path.clone()) else { continue };
        let sample = match samples.get(&ch) {
            Some(x) => x.clone(),
            None => match load(&path) {
                Ok(x) => {
                    samples.insert(ch, x.clone());
                    x
                }
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            },
        };
        let rate = sample.rate;
        let rf = f64::from(rate);
        let (w0, w1) = playlist::window(s, &c);
        let (a, b) = ((w0 / 1000.0 * rf) as usize, (w1 / 1000.0 * rf) as usize);
        let dub = mono(&cut(&sample.left, a, b), &cut(&sample.right, a, b));
        if dub.is_empty() {
            continue;
        }
        // What the guide plays over the clip, in the clip's own time (its sample's seconds from its start).
        let speed = playlist::audio_speed(s, c.source).max(1e-6);
        let start = f64::from(c.position) * spt;
        let mut lead = vec![0.0f32; dub.len()];
        for (g0, g1, gw, gspeed, gm, grate) in &guides {
            for (u, v) in lead.iter_mut().enumerate() {
                let song = start + u as f64 / rf / speed;
                if song < *g0 || song >= *g1 {
                    continue;
                }
                let p = (gw + (song - g0) * gspeed) * f64::from(*grate);
                let j = p as usize;
                if j + 1 < gm.len() {
                    let f = (p - j as f64) as f32;
                    *v += gm[j] + (gm[j + 1] - gm[j]) * f;
                }
            }
        }
        let (sd, sg) = (Scan::new(&dub, rate), Scan::new(&lead, rate));
        let (ld, lg) = (align::Look::new(&sd), align::Look::new(&sg));
        let off = align::offset(&ld, &lg, MATCH_REACH);
        let path = align::align(&ld, &lg, off, MATCH_REACH);
        let anchors = align::anchors(&sd, &sg, &path, strength);
        count += 1;
        let at = w0 / 1000.0;
        for (x, y) in anchors {
            if (y - x).abs() > 1e-3 {
                moves += 1;
                total += (y - x).abs();
            }
            per.entry(ch).or_default().push((at + x, at + y));
        }
    }
    let mut swaps = Vec::new();
    for (ch, mut anchors) in per {
        let (Some(row), Some(sample)) = (row_of(s, ch), samples.get(&ch)) else { continue };
        let Some(path) = s.project.channels[row].sample_path.clone() else { continue };
        // The clips' own starts and ends stay where they are.
        for &i in &list {
            let c = clips(s)[i];
            if c.source == ClipSource::Channel(ch) {
                let (w0, w1) = playlist::window(s, &c);
                anchors.extend([(w0 / 1000.0 + 1e-3, w0 / 1000.0 + 1e-3), (w1 / 1000.0 - 1e-3, w1 / 1000.0 - 1e-3)]);
            }
        }
        anchors.sort_by(|a, b| a.0.total_cmp(&b.0));
        let scan = Scan::new(&mono(&sample.left, &sample.right), sample.rate);
        let map = retime::map(&scan, &anchors);
        let (l, r) = warp::render(&sample.left, &sample.right, sample.rate, &frames(&map, sample.rate));
        let new = l.len() as f64 / f64::from(sample.rate);
        match write(s, &stem(&path), "matched", sample.rate, &l, &r) {
            Ok(p) => swaps.push(Swap { row, ch, path: p, map, old: scan.len, new }),
            Err(e) => errors.push(e),
        }
    }
    if let Some(e) = errors.first() {
        if swaps.is_empty() {
            return format!("Match timing to {name}: {e}");
        }
    }
    if moves == 0 || swaps.is_empty() {
        return format!("Match timing to {name}: already in time with it");
    }
    apply(s, "Match timing to track", &swaps);
    let clips_word = if count == 1 { "clip" } else { "clips" };
    format!("Matched {count} {clips_word} to {name}'s timing: {moves} syllables moved, {} on average", ms(total / moves as f64))
}

/// Comp takes (`os_dsp::comp`): `target`'s clips are the takes; the comp goes on a free track under them, at the
/// reference take's place, and the takes are muted.
fn comp_takes(s: &mut State, target: Target) -> String {
    let list = targets(s, target);
    if list.len() < 2 {
        return "Comp takes: select two or more takes of the same part (audio clips)".into();
    }
    // Each take as it plays: its clip's stretch of its sample, at the first take's rate.
    let mut rate = 0u32;
    let mut audio: Vec<(Vec<f32>, Vec<f32>)> = Vec::with_capacity(list.len());
    let mut first_path = String::new();
    for &i in &list {
        let c = clips(s)[i];
        let ClipSource::Channel(ch) = c.source else { continue };
        let Some(path) = row_of(s, ch).and_then(|r| s.project.channels[r].sample_path.clone()) else {
            return "Comp takes: a take has no audio".into();
        };
        let x = match load(&path) {
            Ok(x) => x,
            Err(e) => return format!("Comp takes: {e}"),
        };
        if rate == 0 {
            rate = x.rate;
            first_path = path;
        }
        let rf = f64::from(x.rate);
        let (w0, w1) = playlist::window(s, &c);
        let (a, b) = ((w0 / 1000.0 * rf) as usize, (w1 / 1000.0 * rf) as usize);
        audio.push((resample(&cut(&x.left, a, b), x.rate, rate), resample(&cut(&x.right, a, b), x.rate, rate)));
    }
    if audio.iter().any(|(l, _)| (l.len() as f64) < f64::from(rate) * 0.5) {
        return "Comp takes: a take is too short (less than half a second)".into();
    }
    let monos: Vec<Vec<f32>> = audio.iter().map(|(l, r)| mono(l, r)).collect();
    let scans: Vec<Scan> = monos.iter().map(|m| Scan::new(m, rate)).collect();
    let takes: Vec<comp::Take> = monos.iter().zip(&scans).map(|(m, sc)| comp::Take { mono: m, scan: sc }).collect();
    let c = comp::comp(&takes);
    let used = c.used();
    let parts: Vec<(Vec<f32>, Vec<f32>)> = (0..audio.len())
        .map(|k| {
            if !used.contains(&k) {
                (Vec::new(), Vec::new())
            } else if k == c.reference {
                audio[k].clone()
            } else {
                warp::render(&audio[k].0, &audio[k].1, rate, &frames(&c.map(k, &takes), rate))
            }
        })
        .collect();
    let (l, r) = comp::assemble(&parts, &c, rate, audio[c.reference].0.len());
    let path = match write(s, &stem(&first_path), "comp", rate, &l, &r) {
        Ok(p) => p,
        Err(e) => return format!("Comp takes: {e}"),
    };
    let reference = clips(s)[list[c.reference]];
    let below = list.iter().map(|&i| clips(s)[i].track).max().unwrap_or(0).saturating_add(1).min(playlist::TRACKS - 1);
    let track = playlist::free_track(s, below, reference.position, reference.end());
    let takes_at: Vec<(u32, u16, ClipSource)> = list.iter().map(|&i| clips(s)[i]).map(|c| (c.position, c.track, c.source)).collect();
    let text = path.to_string_lossy().into_owned();
    if playlist::add_audio_clip_as(s, &text, track, reference.position, "Comp takes").is_none() {
        return format!("Comp takes: {text} could not be read");
    }
    if let Some(a) = s.project.arrangement_mut() {
        for cl in a.clips.iter_mut().filter(|cl| takes_at.contains(&(cl.position, cl.track, cl.source))) {
            cl.muted = true;
        }
    }
    s.piano.dirty = true;
    let order: Vec<String> = c.phrases.iter().map(|p| (p.take + 1).to_string()).collect();
    let phrases = if c.phrases.len() == 1 { "1 phrase".to_string() } else { format!("{} phrases", c.phrases.len()) };
    format!(
        "Comp of {} takes on {}: {phrases}, from takes {} in turn, in time with take {}",
        list.len(),
        playlist::track_name(s, track),
        order.join(", "),
        c.reference + 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48000;

    fn app() -> App {
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        app
    }

    /// A rough rapped line: vowels (a pulse train's harmonics around a formant) each after a short consonant burst,
    /// at `at` seconds, written to a WAV in the temp folder.
    fn take(name: &str, at: &[f64], secs: f64) -> PathBuf {
        let n = (secs * f64::from(RATE)) as usize;
        let mut x = vec![0.0f32; n];
        let mut seed = 0x9e37_79b9_u32;
        for (k, &t) in at.iter().enumerate() {
            let f0 = 120.0 + 10.0 * (k % 4) as f64;
            let formant = [700.0, 400.0, 550.0, 300.0][k % 4];
            let start = (t * f64::from(RATE)) as usize;
            for i in 0..(0.012 * f64::from(RATE)) as usize {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                if let Some(v) = (start + i).checked_sub((0.012 * f64::from(RATE)) as usize).and_then(|j| x.get_mut(j)) {
                    *v += 0.15 * ((seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0) * (-(i as f32) / 150.0).exp();
                }
            }
            let len = (0.12 * f64::from(RATE)) as usize;
            for i in 0..len {
                let tt = i as f64 / f64::from(RATE);
                let env = (i as f64 / 480.0).min(1.0) * ((len - i) as f64 / 1400.0).min(1.0);
                let mut v = 0.0;
                for h in 1..30 {
                    let f = f0 * f64::from(h);
                    let w = 1.0 / (1.0 + ((f - formant) / 150.0).powi(2)) + 0.3 / (1.0 + ((f - 1200.0) / 200.0).powi(2));
                    v += w * (std::f64::consts::TAU * f * tt).sin();
                }
                if let Some(o) = x.get_mut(start + i) {
                    *o += (0.08 * env * v) as f32;
                }
            }
        }
        let path = std::env::temp_dir().join(format!("lemon-timing-{}-{name}.wav", std::process::id()));
        let inter: Vec<f32> = x.iter().flat_map(|v| [*v, *v]).collect();
        let mut w = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        os_engine::wav::write_f32_stereo(&mut w, RATE, &inter).unwrap();
        std::io::Write::flush(&mut w).unwrap();
        path
    }

    /// The beat at 120 BPM: half a second.
    fn at_tempo(app: &mut App) {
        app.state.project.tempo = Some(120.0);
        app.state.tempo = 120.0;
    }

    /// Fit to tempo: a loose line on sixteenths gets a fitted file in the Fitted folder; the clip's channel plays it,
    /// one undo step puts the old one back.
    #[test]
    fn fit_to_tempo_gives_the_channel_a_fitted_file() {
        let mut app = app();
        at_tempo(&mut app);
        let wob = [0.0, 0.03, -0.025, 0.035, -0.03, 0.025, 0.0, -0.035, 0.03, -0.02, 0.025, 0.0];
        let at: Vec<f64> = (0..12).map(|i| 0.5 + f64::from(i) * 0.25 + wob[i as usize]).collect();
        let file = take("fit", &at, 4.0);
        let text = file.to_string_lossy().into_owned();
        let s = &mut app.state;
        let i = playlist::add_audio_clip_as(s, &text, 0, 0, "Test").expect("clip");
        let ClipSource::Channel(ch) = clips(s)[i].source else { panic!() };
        let row = row_of(s, ch).unwrap();
        let msg = run(&mut app, TCmd::Fit(Target::Clip(i), Feel::Tight)).unwrap();
        assert!(msg.starts_with("Fit to tempo: moved "), "{msg}");
        let now = app.state.project.channels[row].sample_path.clone().unwrap();
        assert!(now.ends_with("fitted.wav") && Path::new(&now).parent().unwrap().ends_with("Fitted"), "{now}");
        // The syllables, an eighth note apart, now sit on the sixteenths (an eighth of a second at 120 BPM).
        let out = load(&now).unwrap();
        let scan = Scan::new(&mono(&out.left, &out.right), out.rate);
        assert_eq!(scan.syllables.len(), at.len());
        let worst = scan.syllables.iter().map(|y| (y.at / 0.125 - (y.at / 0.125).round()).abs() * 0.125).fold(0.0, f64::max);
        assert!(worst < 0.012, "{worst}");
        assert!(app.state.history.entries().iter().any(|e| e.1 == "Fit to tempo"));
        crate::history::undo(&mut app.state);
        assert_eq!(app.state.project.channels[row].sample_path.as_deref(), Some(text.as_str()));
        let _ = std::fs::remove_file(&now);
        let _ = std::fs::remove_file(&file);
    }

    /// Match timing to track: a double that wanders gets a matched file whose syllables land on the lead's.
    #[test]
    fn match_timing_lines_a_double_up_with_the_lead() {
        let mut app = app();
        at_tempo(&mut app);
        let lead: Vec<f64> = (0..10).map(|i| 0.4 + f64::from(i) * 0.22).collect();
        let wob = [0.03, -0.025, 0.035, 0.0, -0.03, 0.02, 0.04, -0.02, 0.025, -0.035];
        let double: Vec<f64> = lead.iter().zip(wob).map(|(t, w)| t + w).collect();
        let (fl, fd) = (take("lead", &lead, 3.0), take("double", &double, 3.0));
        let (tl, td) = (fl.to_string_lossy().into_owned(), fd.to_string_lossy().into_owned());
        playlist::add_audio_clip_as(&mut app.state, &tl, 0, 0, "Test").expect("lead");
        let i = playlist::add_audio_clip_as(&mut app.state, &td, 1, 0, "Test").expect("double");
        app.state.playlist.sel.clear();
        let msg = run(&mut app, TCmd::Match(Target::Clip(i), 0)).unwrap();
        assert!(msg.starts_with("Matched 1 clip to Track 1's timing"), "{msg}");
        let ClipSource::Channel(ch) = clips(&app.state)[i].source else { panic!() };
        let now = app.state.project.channel(ch).and_then(|c| c.sample_path.clone()).unwrap();
        assert!(now.ends_with("matched.wav"), "{now}");
        let scan = |p: &str| {
            let x = load(p).unwrap();
            Scan::new(&mono(&x.left, &x.right), x.rate)
        };
        let (sl, sm) = (scan(&tl), scan(&now));
        assert_eq!(sm.syllables.len(), sl.syllables.len());
        let worst = sm.syllables.iter().zip(&sl.syllables).map(|(a, b)| (a.at - b.at).abs()).fold(0.0, f64::max);
        assert!(worst < 0.015, "{worst}");
        for f in [fl, fd, PathBuf::from(now)] {
            let _ = std::fs::remove_file(f);
        }
    }

    /// Comp takes: three takes on three tracks become one clip on a free track under them; the takes are muted.
    #[test]
    fn comp_takes_puts_one_clip_under_the_takes() {
        let mut app = app();
        at_tempo(&mut app);
        let line = |shift: f64| -> Vec<f64> {
            let mut v: Vec<f64> = (0..6).map(|i| 0.3 + shift + f64::from(i) * 0.2).collect();
            v.extend((0..6).map(|i| 2.0 + shift + f64::from(i) * 0.2));
            v
        };
        let files: Vec<PathBuf> = (0..3).map(|k| take(&format!("comp{k}"), &line(0.02 * f64::from(k)), 3.6)).collect();
        let mut idx = Vec::new();
        for (k, f) in files.iter().enumerate() {
            let text = f.to_string_lossy().into_owned();
            playlist::add_audio_clip_as(&mut app.state, &text, k as u16, 0, "Test").expect("clip");
            idx.push(text);
        }
        let s = &mut app.state;
        s.playlist.sel = (0..clips(s).len()).collect();
        let n = clips(s).len();
        let msg = run(&mut app, TCmd::Comp(Target::Selected)).unwrap();
        assert!(msg.starts_with("Comp of 3 takes on Track 4"), "{msg}");
        let all = clips(&app.state);
        assert_eq!(all.len(), n + 1);
        assert_eq!(all.iter().filter(|c| c.muted).count(), 3, "the takes are muted");
        let new = all.iter().find(|c| !c.muted).unwrap();
        assert_eq!((new.track, new.position), (3, 0));
        let ClipSource::Channel(ch) = new.source else { panic!() };
        let path = app.state.project.channel(ch).and_then(|c| c.sample_path.clone()).unwrap();
        assert!(path.ends_with("comp.wav"), "{path}");
        let _ = std::fs::remove_file(&path);
        for f in files {
            let _ = std::fs::remove_file(f);
        }
    }

    /// The menus: an audio clip's has the three tools, Comp greyed out for one clip; the grid radio follows the setting.
    #[test]
    fn the_menus_have_the_tools_with_tips() {
        let app = app();
        let v = fit_items(Target::Selected);
        assert_eq!(v.iter().filter(|p| matches!(p.cmd, Cmd::Timing(TCmd::Fit(..)))).count(), 3);
        assert!(v.iter().filter(|p| !p.head).all(|p| !p.tip.is_empty()), "every entry says what it does");
        assert_eq!(v.iter().filter(|p| matches!(p.mark, Mark::Radio(_))).count(), GRIDS.len());
        let m = match_items(&app.state, Target::Selected, None);
        assert!(m.iter().any(|p| p.label == "(no other track has audio)" && !p.enabled));
    }
}
