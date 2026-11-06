// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The S sounds pages in the app (`crate::s_page` draws them, `os_dsp::sibilant` listens): each file's S sounds found on
//! a thread of their own when its page first shows, the picture of the part in view, what a part chosen sounds like, and
//! a change made to it. A change writes a new copy of the channel's audio to the data folder's Edited folder and gives
//! the channel that copy, one undo step, worked out on a thread behind a progress panel (`busy`), as the vocal timing
//! tools do (`crate::timing`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use os_dsp::sibilant::{self as sib, Change, Kind, Model};
use os_engine::sample::Sample;
use os_model::{Clip, ClipSource};

use crate::app::{App, State};
use crate::clip_page::{Page, Tab};
use crate::history::Snap;
use crate::s_page::{FoundS, Picture, Sounds, TOP_HZ};

/// The picture's size: stretched over the page's picture, which it is about half of.
const PIC_W: usize = 362;
const PIC_H: usize = 160;

/// What was found in one file.
#[derive(Debug, Clone)]
struct Found {
    list: Arc<Vec<FoundS>>,
    model: Model,
}

/// One file read: its samples, and their mono for listening.
#[derive(Clone)]
struct Audio {
    path: String,
    sample: Arc<Sample>,
    mono: Arc<Vec<f32>>,
}

type Answer = Result<(Audio, Vec<sib::Found>, Model), String>;

/// A file being looked through.
struct Job {
    path: String,
    /// Permille done.
    progress: Arc<AtomicU32>,
    rx: Receiver<Answer>,
}

/// The app's state of the S sounds pages.
#[derive(Default)]
pub struct Store {
    found: HashMap<String, Found>,
    job: Option<Job>,
    /// The file read last.
    audio: Option<Audio>,
    /// Pictures made: by file, span and the file's version (a change makes a new file).
    pictures: RefCell<HashMap<(String, u64, u64), Arc<Picture>>>,
    /// What each row's part chosen sounds like (row, Source audio tab).
    heard: HashMap<(usize, bool), String>,
    /// A change being worked out.
    changing: bool,
    /// How far a change goes; `None`: as at first.
    strength: Option<f32>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("found", &self.found.len()).field("looking", &self.job.as_ref().map(|j| &j.path)).finish()
    }
}

impl Store {
    pub fn strength(&self) -> f32 {
        self.strength.unwrap_or(0.9)
    }
}

/// The file rack row `row` plays.
fn path_of(s: &State, row: usize) -> Option<String> {
    s.project.channels.get(row)?.sample_path.clone()
}

/// Reads `path` and looks through it, on a thread; `progress` in permille.
fn look(path: String, progress: Arc<AtomicU32>) -> Answer {
    let file = crate::plugins::resolve_sample(&path);
    let sample = os_engine::sample::load_shared(Path::new(&file)).map_err(|e| e.to_string())?;
    progress.store(100, Ordering::Relaxed);
    let mono: Vec<f32> = sample.left.iter().zip(&sample.right).map(|(a, b)| 0.5 * (a + b)).collect();
    let (found, model) = sib::find(&mono, sample.rate, &mut |k| progress.store(100 + (k * 900.0) as u32, Ordering::Relaxed));
    Ok((Audio { path, sample, mono: Arc::new(mono) }, found, model))
}

fn found_s(f: &sib::Found, rate: u32) -> FoundS {
    let r = f64::from(rate.max(1));
    FoundS { from_s: f.from as f64 / r, to_s: f.to as f64 / r, kind: f.kind, voiced: f.voiced, sure: f.sure }
}

/// Each tick: a look finished is kept; the S sounds page in front gets its file looked through (and read).
pub fn tick(app: &mut App) {
    let st = &mut app.state.clip_fx.sounds;
    let mut changed = false;
    if let Some(j) = &st.job {
        match j.rx.try_recv() {
            Ok(Ok((audio, found, model))) => {
                let rate = audio.sample.rate;
                st.found.insert(j.path.clone(), Found { list: Arc::new(found.iter().map(|f| found_s(f, rate)).collect()), model });
                st.audio = Some(audio);
                st.job = None;
                changed = true;
            }
            Ok(Err(e)) => {
                app.state.message = Some(format!("S sounds: {e}"));
                app.state.clip_fx.sounds.job = None;
                changed = true;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                app.state.clip_fx.sounds.job = None;
            }
        }
    }
    let s = &mut app.state;
    if s.clip_fx.shown_page == Some(Page::Sounds) && s.clip_fx.sounds.job.is_none() {
        if let Some(path) = s.clip_fx.shown.and_then(|(row, _)| path_of(s, row)) {
            let st = &s.clip_fx.sounds;
            let have = st.found.contains_key(&path) && st.audio.as_ref().is_some_and(|a| a.path == path);
            if !have {
                start(s, path);
            }
        }
    }
    if changed {
        app.invalidate();
    }
}

/// Looks through `path` on a thread.
fn start(s: &mut State, path: String) {
    let progress = Arc::new(AtomicU32::new(0));
    let (tx, rx) = channel();
    let (p, file) = (progress.clone(), path.clone());
    let spawned = std::thread::Builder::new().name("lemon-s-sounds".into()).spawn(move || {
        let _ = tx.send(look(file, p));
        os_clap::wake();
    });
    if spawned.is_ok() {
        s.clip_fx.sounds.job = Some(Job { path, progress, rx });
    }
}

/// The page's values for rack row `row`'s `tab`, the tab spanning `span` (seconds of the sample) and showing `view`.
pub fn values(s: &State, row: usize, tab: Tab, span: (f64, f64), view: (f64, f64)) -> Sounds {
    let st = &s.clip_fx.sounds;
    let key = (row, tab == Tab::Whole);
    let path = path_of(s, row).unwrap_or_default();
    let looking = st.job.as_ref().filter(|j| j.path == path).map(|j| j.progress.load(Ordering::Relaxed) as f32 / 1000.0);
    let found = st.found.get(&path);
    let list: Arc<Vec<FoundS>> = match (found, tab) {
        (Some(f), Tab::Whole) => f.list.clone(),
        (Some(f), Tab::Cut) => Arc::new(f.list.iter().filter(|x| x.to_s > span.0 && x.from_s < span.1).copied().collect()),
        (None, _) => Arc::default(),
    };
    let status = match found {
        None => "Not looked through yet: the S sounds of this audio are looked for as soon as this page shows.".into(),
        Some(f) => status_text(&list, &f.model, tab),
    };
    let audio = st.audio.as_ref().filter(|a| a.path == path);
    let in_front = s.clip_fx.shown == Some((row, tab)) && s.clip_fx.shown_page == Some(Page::Sounds);
    let picture = audio.filter(|_| in_front).map(|a| picture_of(st, a, view));
    let busy = if st.changing {
        Some("A change is being worked out...".into())
    } else if audio.is_none() {
        Some("The audio is still being read: a change can be made once it is.".into())
    } else {
        None
    };
    let playing = s.clip_fx.play_tick.is_some() && s.clip_fx.audition.is_some_and(|a| a.row == row && a.tab == tab) && s.clip_fx.sel.contains_key(&key);
    Sounds {
        found: list,
        looking,
        status,
        centres: found.filter(|f| f.model.learned).map(|f| (f.model.s, f.model.sh)),
        sel: s.clip_fx.sel.get(&key).copied(),
        heard: st.heard.get(&key).cloned(),
        picture,
        strength: st.strength(),
        playing,
        busy,
    }
}

/// "Found 20 here: 5 S, 10 SH, 3 Z, 2 ZH. ..."
fn status_text(list: &[FoundS], model: &Model, tab: Tab) -> String {
    let count = |n: &str| list.iter().filter(|f| f.name() == n).count();
    let mut parts = Vec::new();
    for n in ["S", "SH", "Z", "ZH"] {
        let k = count(n);
        if k > 0 {
            parts.push(format!("{k} {n}"));
        }
    }
    let here = if tab == Tab::Cut { "in this clip" } else { "in this audio" };
    let found = if list.is_empty() { format!("No S sounds found {here}.") } else { format!("Found {} S sounds {here}: {}.", list.len(), parts.join(", ")) };
    let voice = if model.learned {
        format!(" In this voice an S sits near {:.1} kHz and an SH near {:.1} kHz (learned from the audio).", model.s / 1000.0, model.sh / 1000.0)
    } else {
        " Not both S and SH were heard, so the usual places are used to tell them apart.".to_string()
    };
    found + &voice
}

/// The picture of `view` of `audio`, made once per view.
fn picture_of(st: &Store, audio: &Audio, view: (f64, f64)) -> Arc<Picture> {
    let key = (audio.path.clone(), view.0.to_bits(), view.1.to_bits());
    if let Some(p) = st.pictures.borrow().get(&key) {
        return p.clone();
    }
    let rate = audio.sample.rate;
    let r = f64::from(rate);
    let top = TOP_HZ.min(rate as f32 / 2.0);
    let db = sib::picture(&audio.mono, rate, view.0 * r, view.1 * r, PIC_W, PIC_H, top);
    let loud = db.iter().copied().fold(-160.0f32, f32::max);
    let px = db.iter().map(|&v| heat(((v - (loud - 80.0)) / 80.0).clamp(0.0, 1.0))).collect();
    let pic = Arc::new(Picture { w: PIC_W, h: PIC_H, px, span: view, top_hz: top });
    let mut cache = st.pictures.borrow_mut();
    if cache.len() > 24 {
        cache.clear();
    }
    cache.insert(key, pic.clone());
    pic
}

/// The heat map's colour for a level 0..1: near black, deep blue-green, the accent green, yellow, near white.
fn heat(v: f32) -> u32 {
    const STOPS: [(f32, [f32; 3]); 5] = [(0.0, [10.0, 13.0, 16.0]), (0.35, [16.0, 70.0, 92.0]), (0.62, [40.0, 190.0, 150.0]), (0.85, [250.0, 214.0, 90.0]), (1.0, [255.0, 250.0, 235.0])];
    let i = STOPS.iter().rposition(|s| s.0 <= v).unwrap_or(0).min(STOPS.len() - 2);
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    let t = ((v - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
    let c = |k: usize| (a.1[k] + (b.1[k] - a.1[k]) * t) as u32;
    c(0) << 16 | c(1) << 8 | c(2)
}

/// What part `from..to` of rack row `row`'s audio sounds like, for the page.
fn heard_text(st: &Store, path: &str, from: f64, to: f64) -> Option<String> {
    let a = st.audio.as_ref().filter(|a| a.path == path)?;
    let model = st.found.get(path).map_or_else(Model::default, |f| f.model);
    let r = f64::from(a.sample.rate);
    match sib::heard(&a.mono, a.sample.rate, (from * r) as usize, (to * r) as usize, &model) {
        None => Some("No S sound in it: nothing hissy to change.".into()),
        Some(h) => {
            let sure = if h.sure >= 0.8 { "sure" } else if h.sure >= 0.65 { "fairly sure" } else { "it could be either" };
            let article = if matches!(h.name(), "S" | "SH") { "an" } else { "a" };
            Some(format!("It sounds like {article} {} ({sure}, centred near {:.1} kHz).", h.name(), h.centre / 1000.0))
        }
    }
}

/// The page's events for the S sounds: false for one that is not for it.
pub fn event(app: &mut App, row: usize, e: &crate::clip_page::Event) -> bool {
    use crate::clip_page::Event;
    let s = &mut app.state;
    match *e {
        Event::Select { tab, sel } => {
            let key = (row, tab == Tab::Whole);
            let path = path_of(s, row).unwrap_or_default();
            let sel = match sel {
                // A click: the sound found there, a little either side; elsewhere nothing.
                Some((a, b)) if b - a < 1e-6 => s.clip_fx.sounds.found.get(&path).and_then(|f| f.list.iter().find(|x| x.from_s <= a && a <= x.to_s)).map(|x| {
                    let pad = ((x.to_s - x.from_s) * 0.15).max(0.005);
                    (x.from_s - pad, x.to_s + pad)
                }),
                other => other,
            };
            match sel {
                Some((a, b)) => {
                    s.clip_fx.sel.insert(key, (a, b));
                    match heard_text(&s.clip_fx.sounds, &path, a, b) {
                        Some(h) => s.clip_fx.sounds.heard.insert(key, h),
                        None => s.clip_fx.sounds.heard.remove(&key),
                    };
                }
                None => {
                    s.clip_fx.sel.remove(&key);
                    s.clip_fx.sounds.heard.remove(&key);
                }
            }
            app.invalidate();
        }
        Event::Strength { k, .. } => s.clip_fx.sounds.strength = Some(k.clamp(0.0, 1.0)),
        Event::FindS { .. } => {
            if let Some(path) = path_of(s, row) {
                s.clip_fx.sounds.found.remove(&path);
                if s.clip_fx.sounds.job.is_none() {
                    start(s, path);
                }
            }
            app.invalidate();
        }
        Event::PlayPart { tab } => {
            if app.transport.playing() {
                app.act(crate::app::Action::Stop);
            } else if s.clip_fx.sel.contains_key(&(row, tab == Tab::Whole)) {
                // The part, looped, even with the loop switch off.
                s.clip_fx.part_play = true;
                app.start_playback_counted(0);
            }
        }
        Event::ChangeS { tab, from, to, into } => {
            if let Some(why) = change(app, row, tab, from, to, into) {
                app.state.message = Some(why);
            }
        }
        _ => return false,
    }
    true
}

/// The Edited folder, beside Recorded and Fitted.
fn out_dir(s: &State) -> PathBuf {
    crate::data_folder::folder(s).or_else(crate::app::theme_dir).unwrap_or_else(std::env::temp_dir).join("Edited")
}

/// A file's name without its folder, extension, and an earlier change's ending.
fn stem(path: &str) -> String {
    let s = Path::new(path).file_stem().map_or_else(|| "Audio".to_string(), |n| n.to_string_lossy().into_owned());
    let s = match s.rfind(" S fixed") {
        Some(i) if s[i + 8..].trim().chars().all(|c| c.is_ascii_digit()) => s[..i].to_string(),
        _ => s,
    };
    s
}

/// The other clips of row `row`'s audio that play some of `from..to`, This clip left out.
fn others_playing(s: &State, row: usize, from: f64, to: f64, this: Option<u32>) -> usize {
    let Some(ch) = s.project.channels.get(row) else { return 0 };
    let src = ClipSource::Channel(ch.index);
    s.project
        .arrangement()
        .map_or(&[][..], |a| &a.clips)
        .iter()
        .filter(|c: &&Clip| c.source == src && Some(c.id.0) != this && !c.muted)
        .filter(|c| {
            let (a, b) = crate::clip_fx::clip_span(s, c);
            b > from && a < to
        })
        .count()
}

/// Starts the change of part `from..to` of row `row`'s audio. Why it cannot, if it cannot.
fn change(app: &mut App, row: usize, tab: Tab, from: f64, to: f64, into: Change) -> Option<String> {
    let s = &app.state;
    let path = path_of(s, row)?;
    let st = &s.clip_fx.sounds;
    if st.changing {
        return Some("S sounds: a change is being worked out already".into());
    }
    let Some(audio) = st.audio.clone().filter(|a| a.path == path) else {
        return Some("S sounds: the audio is still being read, try again in a moment".into());
    };
    let found = st.found.get(&path).cloned();
    let model = found.as_ref().map_or_else(Model::default, |f| f.model);
    // The voice's own sounds of the kind wanted (not the part itself): the surest few.
    let want = match into {
        Change::S => Some(Kind::S),
        Change::Sh => Some(Kind::Sh),
        Change::Softer => None,
    };
    let rate = audio.sample.rate;
    let r = f64::from(rate);
    let mut examples: Vec<FoundS> = found
        .as_ref()
        .map(|f| f.list.iter().filter(|x| Some(x.kind) == want && !x.voiced && (x.to_s < from || x.from_s > to)).copied().collect())
        .unwrap_or_default();
    examples.sort_by(|a, b| b.sure.total_cmp(&a.sure));
    examples.truncate(30);
    let parts: Vec<(usize, usize)> = examples.iter().map(|x| ((x.from_s * r) as usize, (x.to_s * r) as usize)).collect();
    let strength = st.strength();
    let this = match tab {
        Tab::Cut => s.clip_fx.cut.get(&row).copied(),
        Tab::Whole => None,
    };
    let others = if tab == Tab::Cut { others_playing(s, row, from, to, this) } else { 0 };
    let ch = s.project.channels.get(row)?.index;
    let dir = out_dir(s);
    let title: &'static str = match into {
        Change::S => "Make it an S",
        Change::Sh => "Make it an SH",
        Change::Softer => "Make it softer",
    };
    app.state.clip_fx.sounds.changing = true;
    let started = crate::busy::run(app, title, move |p| {
        p.set(0.05, "Changing the part");
        let (a, b) = ((from * r).max(0.0) as usize, (to * r) as usize);
        let target = if parts.is_empty() { None } else { sib::spectrum_of(&audio.mono, rate, &parts) };
        let mut left = audio.sample.left.clone();
        let mut right = audio.sample.right.clone();
        let done = sib::change(&mut [&mut left, &mut right], rate, a, b, into, strength, target.as_deref(), &model);
        if !done {
            return Box::new(move |app: &mut App| {
                app.state.clip_fx.sounds.changing = false;
                "Nothing changed: the part chosen holds no S sound".into()
            });
        }
        p.set(0.5, "Writing the new audio");
        let file = match write(&dir, &stem(&audio.path), rate, &left, &right) {
            Ok(f) => f,
            Err(e) => {
                return Box::new(move |app: &mut App| {
                    app.state.clip_fx.sounds.changing = false;
                    format!("S sounds: {e}")
                })
            }
        };
        let mono: Vec<f32> = audio.mono.iter().enumerate().map(|(i, &m)| if (a.saturating_sub(4096)..b + 4096).contains(&i) { 0.5 * (left[i] + right[i]) } else { m }).collect();
        let sample = Arc::new(Sample { rate, left, right });
        let new = Audio { path: file.to_string_lossy().into_owned(), sample, mono: Arc::new(mono) };
        Box::new(move |app: &mut App| {
            app.state.clip_fx.sounds.changing = false;
            apply(app, row, ch, title, new, found, (from, to), tab, others, target.is_some(), into)
        })
    });
    if !started {
        app.state.clip_fx.sounds.changing = false;
    }
    None
}

/// "<stem> S fixed.wav" (a number after it when taken) in folder `dir`.
fn write(dir: &Path, stem: &str, rate: u32, l: &[f32], r: &[f32]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut path = dir.join(format!("{stem} S fixed.wav"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} S fixed {n}.wav"));
        n += 1;
    }
    let inter: Vec<f32> = l.iter().zip(r).flat_map(|(a, b)| [*a, *b]).collect();
    let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut w = std::io::BufWriter::new(file);
    os_engine::wav::write_f32_stereo(&mut w, rate, &inter).and_then(|()| std::io::Write::flush(&mut w)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// The change worked out: the channel plays the new copy, one undo step; what was found carries over, the part heard
/// again.
#[allow(clippy::too_many_arguments)]
fn apply(app: &mut App, row: usize, ch: u16, title: &'static str, new: Audio, found: Option<Found>, part: (f64, f64), tab: Tab, others: usize, own: bool, into: Change) -> String {
    let s = &mut app.state;
    if s.project.channels.get(row).is_none_or(|c| c.index != ch) {
        return "S sounds: the channel moved while the change was worked out; nothing changed".into();
    }
    let Some(snap) = Snap::sample(s, row) else { return String::new() };
    crate::playlist::snapshot_with(s, title, snap);
    let path = new.path.clone();
    s.project.channels[row].sample_path = Some(path.clone());
    s.sample_loads.push((row, path.clone()));
    s.piano.dirty = true;
    // What was found, the part's sound heard again.
    let rate = f64::from(new.sample.rate);
    let model = found.as_ref().map_or_else(Model::default, |f| f.model);
    let mut list: Vec<FoundS> = found.map(|f| f.list.as_ref().clone()).unwrap_or_default();
    let again = sib::heard(&new.mono, new.sample.rate, (part.0 * rate) as usize, (part.1 * rate) as usize, &model);
    for f in list.iter_mut().filter(|f| f.to_s > part.0 && f.from_s < part.1) {
        if let Some(h) = again {
            f.kind = h.kind;
            f.sure = h.sure;
        }
    }
    let st = &mut s.clip_fx.sounds;
    st.found.insert(path.clone(), Found { list: Arc::new(list), model });
    st.audio = Some(new);
    let key = (row, tab == Tab::Whole);
    if let Some(h) = heard_text(st, &path, part.0, part.1) {
        st.heard.insert(key, h);
    }
    let what = match into {
        Change::Softer => "Made the part softer".to_string(),
        c => format!("Made the part {}{}", if c == Change::S { "an S" } else { "an SH" }, if own { ", like this voice's own" } else { "" }),
    };
    let also = match others {
        0 => String::new(),
        1 => ". 1 other clip plays this part too and changed with it".into(),
        n => format!(". {n} other clips play this part too and changed with it"),
    };
    let name = Path::new(&path).file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    app.invalidate();
    format!("{what} (new audio: {name}){also}")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::clip_page::Event;

    /// An app with one audio clip of a voice-like take: vowels with an S and an SH between them, twice. Its row, the
    /// clip's id and where each hiss is (seconds).
    pub(crate) fn app() -> (App, usize, u32, Vec<(f64, f64, Kind)>) {
        let mut a = App::new(None);
        a.plugins = crate::plugins::Plugins::at_48k();
        let rate = 48000.0f32;
        let mut seed: u64 = 11;
        let mut next = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f32 / (1u64 << 31) as f32
        };
        let vowel = |secs: f32| -> Vec<f32> {
            (0..(secs * rate) as usize).map(|i| (1..30).map(|h| (std::f32::consts::TAU * 150.0 * h as f32 * i as f32 / rate).sin() / (h * h) as f32).sum::<f32>() * 0.3).collect()
        };
        let mut x = vowel(0.3);
        let mut at = Vec::new();
        for k in [Kind::S, Kind::Sh, Kind::S, Kind::Sh] {
            let peak = if k == Kind::S { 7000.0 } else { 3300.0 };
            let n = (0.15 * rate) as usize;
            let mut h = vec![0.0f32; n];
            for p in 0..200 {
                let f = 600.0 + 15000.0 * (p as f32 + next()) / 200.0;
                let amp = (-((f / peak).log2() / 0.45).powi(2)).exp() + 0.03;
                let ph = next() * std::f32::consts::TAU;
                for (i, v) in h.iter_mut().enumerate() {
                    *v += amp * (std::f32::consts::TAU * f * i as f32 / rate + ph).sin();
                }
            }
            let m = h.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            h.iter_mut().for_each(|v| *v *= 0.15 / m);
            at.push((x.len() as f64 / 48000.0, (x.len() + n) as f64 / 48000.0, k));
            x.extend(h);
            x.extend(vowel(0.3));
        }
        let wav = std::env::temp_dir().join(format!("os-app-ssounds-{}-{:?}.wav", std::process::id(), std::thread::current().id()));
        let inter: Vec<f32> = x.iter().flat_map(|&v| [v, v]).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &inter).unwrap();
        crate::playlist::add_audio_clip(&mut a.state, &wav.to_string_lossy(), 0, 0).expect("an audio clip");
        let s = &mut a.state;
        s.control.settle(&mut s.project);
        let row = a.state.project.channels.len() - 1;
        let id = crate::playlist::clips(&a.state).last().unwrap().id.0;
        a.state.clip_fx.cut.insert(row, id);
        (a, row, id, at)
    }

    /// The S sounds page in front: its file is looked through until the answer is in.
    pub(crate) fn looked(a: &mut App, row: usize, tab: Tab) {
        a.state.clip_fx.shown = Some((row, tab));
        a.state.clip_fx.shown_page = Some(Page::Sounds);
        for _ in 0..500 {
            tick(a);
            if a.state.clip_fx.sounds.job.is_none() && !a.state.clip_fx.sounds.found.is_empty() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("never looked through");
    }

    fn busy_done(a: &mut App) {
        for _ in 0..2000 {
            crate::busy::tick(a);
            if a.busy.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the change never finished");
    }

    #[test]
    fn the_page_in_front_finds_the_s_sounds_and_shows_its_picture() {
        let (mut a, row, _, at) = app();
        let v = crate::clip_fx::page_vals(&a.state, row).unwrap();
        assert!(v.whole.sounds.status.starts_with("Not looked through"), "{}", v.whole.sounds.status);
        looked(&mut a, row, Tab::Whole);
        let v = crate::clip_fx::page_vals(&a.state, row).unwrap();
        let s = &v.whole.sounds;
        assert_eq!(s.found.len(), 4, "{:?}", s.found);
        for (f, &(from, to, k)) in s.found.iter().zip(&at) {
            assert_eq!(f.kind, k);
            assert!((f.from_s - from).abs() < 0.03 && (f.to_s - to).abs() < 0.03, "{f:?}");
        }
        assert!(s.status.contains("2 S, 2 SH"), "{}", s.status);
        assert!(s.centres.is_some(), "learned from the voice");
        let pic = s.picture.as_ref().expect("the page in front has its picture");
        assert_eq!((pic.w, pic.h, pic.px.len()), (PIC_W, PIC_H, PIC_W * PIC_H));
        assert!(v.cut.sounds.picture.is_none(), "the tab behind has none");
        assert!(s.busy.is_none());
    }

    #[test]
    fn a_click_chooses_a_sound_and_says_what_it_sounds_like() {
        let (mut a, row, _, at) = app();
        looked(&mut a, row, Tab::Whole);
        let mid = (at[1].0 + at[1].1) / 2.0;
        assert!(event(&mut a, row, &Event::Select { tab: Tab::Whole, sel: Some((mid, mid)) }));
        let sel = a.state.clip_fx.sel[&(row, true)];
        assert!(sel.0 < at[1].0 && sel.1 > at[1].1, "{sel:?}");
        let v = crate::clip_fx::page_vals(&a.state, row).unwrap();
        let h = v.whole.sounds.heard.clone().unwrap();
        assert!(h.contains("an SH"), "{h}");
        // A click away from any: nothing chosen.
        event(&mut a, row, &Event::Select { tab: Tab::Whole, sel: Some((0.05, 0.05)) });
        assert!(!a.state.clip_fx.sel.contains_key(&(row, true)));
        // A part of a vowel: no S sound in it.
        event(&mut a, row, &Event::Select { tab: Tab::Whole, sel: Some((0.02, 0.2)) });
        assert!(a.state.clip_fx.sounds.heard[&(row, true)].starts_with("No S sound"));
    }

    #[test]
    fn a_change_gives_the_channel_a_new_copy_in_one_undo_step() {
        let (mut a, row, _, at) = app();
        looked(&mut a, row, Tab::Whole);
        let old = a.state.project.channels[row].sample_path.clone().unwrap();
        let (from, to) = (at[1].0 - 0.01, at[1].1 + 0.01);
        event(&mut a, row, &Event::Select { tab: Tab::Whole, sel: Some((from, to)) });
        assert!(event(&mut a, row, &Event::ChangeS { tab: Tab::Whole, from, to, into: Change::S }));
        assert!(a.state.clip_fx.sounds.changing);
        busy_done(&mut a);
        let new = a.state.project.channels[row].sample_path.clone().unwrap();
        assert_ne!(new, old);
        assert!(new.ends_with("S fixed.wav") && Path::new(&new).parent().unwrap().ends_with("Edited"), "{new}");
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Made the part an S, like this voice's own")), "{:?}", a.state.message);
        // The page knows it as an S now, without looking again.
        let v = crate::clip_fx::page_vals(&a.state, row).unwrap();
        assert_eq!(v.whole.sounds.found[1].kind, Kind::S, "{:?}", v.whole.sounds.found);
        assert!(v.whole.sounds.heard.as_deref().is_some_and(|h| h.contains("an S ")), "{:?}", v.whole.sounds.heard);
        // The new file is what the old was, but for that part.
        let o = os_engine::sample::load(Path::new(&old)).unwrap();
        let n = os_engine::sample::load(Path::new(&new)).unwrap();
        assert_eq!(o.left.len(), n.left.len());
        let edge = (0.04 * 48000.0) as usize;
        let (pa, pb) = ((from * 48000.0) as usize, (to * 48000.0) as usize);
        assert!((0..pa - edge).chain(pb + edge..o.left.len()).all(|i| (o.left[i] - n.left[i]).abs() < 1e-4), "nothing else moved");
        assert!((pa..pb).any(|i| (o.left[i] - n.left[i]).abs() > 1e-3), "the part did");
        // One step back: the old file again.
        crate::history::undo(&mut a.state);
        assert_eq!(a.state.project.channels[row].sample_path.as_deref(), Some(old.as_str()));
        crate::history::redo(&mut a.state);
        assert_eq!(a.state.project.channels[row].sample_path.as_deref(), Some(new.as_str()));
        let _ = std::fs::remove_file(&new);
        // A name made from the copy's goes back to the take's own.
        assert_eq!(stem("C:/x/take S fixed 3.wav"), "take");
        assert_eq!(stem("C:/x/take.wav"), "take");
    }

    #[test]
    fn this_clip_says_when_other_clips_play_the_same_part() {
        let (mut a, row, id, at) = app();
        // A copy of the clip later on: it plays the same part of the file.
        let c = *crate::playlist::clips(&a.state).iter().find(|c| c.id.0 == id).unwrap();
        crate::playlist::arr_mut(&mut a.state).clips.push(Clip { position: c.end() + 96, ..c });
        let s = &mut a.state;
        s.control.settle(&mut s.project);
        looked(&mut a, row, Tab::Cut);
        let (from, to) = (at[0].0, at[0].1);
        assert_eq!(others_playing(&a.state, row, from, to, Some(id)), 1);
        event(&mut a, row, &Event::ChangeS { tab: Tab::Cut, from, to, into: Change::Softer });
        busy_done(&mut a);
        let m = a.state.message.clone().unwrap_or_default();
        assert!(m.starts_with("Made the part softer") && m.contains("1 other clip plays this part too"), "{m}");
        let new = a.state.project.channels[row].sample_path.clone().unwrap();
        let _ = std::fs::remove_file(&new);
    }

    #[test]
    fn the_part_chosen_is_what_play_loops() {
        let (mut a, row, _, at) = app();
        looked(&mut a, row, Tab::Whole);
        let full = crate::clip_fx::audition(&a.state, row, Tab::Whole).unwrap();
        event(&mut a, row, &Event::Select { tab: Tab::Whole, sel: Some((at[2].0, at[2].1)) });
        let part = crate::clip_fx::audition(&a.state, row, Tab::Whole).unwrap();
        assert!(part.range.0 > full.range.0 && part.range.1 < full.range.1, "{part:?} in {full:?}");
        assert_eq!(crate::clip_fx::audition_start(&a.state, &part), part.range.0);
        // On the plugins page the whole audio plays.
        a.state.clip_fx.shown_page = Some(Page::Fx);
        assert_eq!(crate::clip_fx::audition(&a.state, row, Tab::Whole).unwrap().range, full.range);
    }
}
