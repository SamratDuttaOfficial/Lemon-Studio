// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Pitch: Lemon Studio's pitch correction as a plugin. It hears the pitch of a voice or an instrument, picks the
//! nearest note of a key and scale (set by hand or found from what it hears) and moves the sound onto it.
//!
//! - Pitch: YIN on the input mixed to mono and taken down to about 11 kHz, every 32 of those samples (about 3 ms),
//!   over the range the input choice sets; the last three readings' median, lightly smoothed. A note holds through
//!   brief dropouts and short octave errors rather than letting go.
//! - Key: how long each note has been heard (fading over half a minute), against Krumhansl's major and minor key
//!   profiles; a new key takes over only when it fits clearly better.
//! - Correction: the cents to the target note, scaled by the amount; flex leaves small deviations partly alone;
//!   vibrato, its depth fading in after the onset; transpose. The retune speed is how fast the correction follows
//!   (0 snaps at once), humanize slows it on held notes so they keep their natural movement.
//! - The shift: pitch-synchronous overlap-add (TD-PSOLA). Grains two periods long, Hann windowed and centred on the
//!   waveform's peaks a period apart, are laid out at the new period (to the fraction of a sample) and the sum divided
//!   by the windows' sum, so an unshifted note comes back as it went in. Each grain takes the correction worked out
//!   for its own moment (the pitch reading lags the input). Grains are not resampled, so the voice keeps its formants.
//!   Unpitched sound (breaths, S and T) plays dry, delayed by the same latency, which the plugin reports to the host.

use std::collections::VecDeque;

use nih_plug::prelude::*;
use os_theme::design::lemon as brand;
use os_ui::kit::lemon::plug;
use os_ui::{Canvas, Font, Rect};

use crate::params::leading_number;
use crate::view::NOTES;
use crate::{Card, Control, Display, Kind, Mark, Setting, SmallFx, Sound, Spec, LIVE};

pub const KEY: usize = 0;
pub const SCALE: usize = 1;
pub const AUTO: usize = 2;
pub const VOICE: usize = 3;
pub const RETUNE: usize = 4;
pub const HUMANIZE: usize = 5;
pub const FLEX: usize = 6;
pub const AMOUNT: usize = 7;
pub const VIB_RATE: usize = 8;
pub const VIB_DEPTH: usize = 9;
pub const VIB_DELAY: usize = 10;
pub const TRANSPOSE: usize = 11;
pub const TUNING: usize = 12;
pub const NOTES_OFF: usize = 13;
const N: usize = 14;

/// The settings: the key (C = 0), the scale ([`SCALES`]), key detection (0 manual, 1 auto), the input ([`VOICES`]),
/// the retune speed in ms, humanize in %, flex in cents, the amount in %, the vibrato's rate in tenths of a Hz, depth
/// in cents and onset in ms, the transpose in semitones, A4 in tenths of a Hz and the notes turned off (bit k = note
/// k).
pub const SETTINGS: [Setting; N] = [
    Setting { id: "key", name: "Key", range: (0, 11), start: 0, magnet: None },
    Setting { id: "scale", name: "Scale", range: (0, 13), start: 1, magnet: None },
    Setting { id: "auto", name: "Key detection", range: (0, 1), start: 0, magnet: None },
    Setting { id: "voice", name: "Input", range: (0, 3), start: 1, magnet: None },
    Setting { id: "retune", name: "Retune speed", range: (0, 400), start: 40, magnet: None },
    Setting { id: "humanize", name: "Humanize", range: (0, 100), start: 20, magnet: None },
    Setting { id: "flex", name: "Flex", range: (0, 100), start: 0, magnet: None },
    Setting { id: "amount", name: "Amount", range: (0, 100), start: 100, magnet: None },
    Setting { id: "vib_rate", name: "Vibrato rate", range: (10, 100), start: 55, magnet: None },
    Setting { id: "vib_depth", name: "Vibrato depth", range: (0, 100), start: 0, magnet: None },
    Setting { id: "vib_delay", name: "Vibrato onset", range: (0, 1000), start: 300, magnet: None },
    Setting { id: "transpose", name: "Transpose", range: (-12, 12), start: 0, magnet: Some(0) },
    Setting { id: "tuning", name: "A4 tuning", range: (4300, 4500), start: 4400, magnet: Some(4400) },
    Setting { id: "notes", name: "Notes off", range: (0, 4095), start: 0, magnet: None },
];

/// The scales: name and the steps above the key in it.
pub const SCALES: [(&str, &[u8]); 14] = [
    ("Chromatic", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
    ("Major", &[0, 2, 4, 5, 7, 9, 11]),
    ("Natural minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("Harmonic minor", &[0, 2, 3, 5, 7, 8, 11]),
    ("Melodic minor", &[0, 2, 3, 5, 7, 9, 11]),
    ("Dorian", &[0, 2, 3, 5, 7, 9, 10]),
    ("Phrygian", &[0, 1, 3, 5, 7, 8, 10]),
    ("Lydian", &[0, 2, 4, 6, 7, 9, 11]),
    ("Mixolydian", &[0, 2, 4, 5, 7, 9, 10]),
    ("Locrian", &[0, 1, 3, 5, 6, 8, 10]),
    ("Major pentatonic", &[0, 2, 4, 7, 9]),
    ("Minor pentatonic", &[0, 3, 5, 7, 10]),
    ("Blues", &[0, 3, 5, 6, 7, 10]),
    ("Whole tone", &[0, 2, 4, 6, 8, 10]),
];
pub const CHROMATIC: usize = 0;
pub const MAJOR: usize = 1;
pub const MINOR: usize = 2;

/// The inputs: name and the lowest and highest pitch (Hz) listened for.
pub const VOICES: [(&str, f32, f32); 4] =
    [("High voice", 150.0, 1100.0), ("Mid voice", 80.0, 700.0), ("Low voice", 55.0, 400.0), ("Instrument", 40.0, 1600.0)];

/// The live values ([`Sound::live`]): pitched (0/1), the input's and the output's pitch in cents above MIDI note 0,
/// the key found (-1: none yet) and whether it is minor, how well it fits (%), the notes of the scale in use (bit k =
/// note k) and the target note (-1: none).
pub const L_VOICED: usize = 0;
pub const L_IN: usize = 1;
pub const L_OUT: usize = 2;
pub const L_KEY: usize = 3;
pub const L_MINOR: usize = 4;
pub const L_FIT: usize = 5;
pub const L_SCALE: usize = 6;
pub const L_TARGET: usize = 7;

/// The notes of `scale` on `key` (bit k = note k, C = 0).
pub fn scale_mask(key: i32, scale: usize) -> u16 {
    SCALES[scale.min(SCALES.len() - 1)].1.iter().fold(0, |m, &i| m | 1 << (i32::from(i) + key).rem_euclid(12))
}

// ---------------------------------------------------------------------------------------------------------------
// The editor

/// Editor layout: the key and scale, detection and the live readout on top; the notes strip and the correction knobs
/// in the middle; vibrato, transpose and tuning along the bottom with the wordmark beside them; the hint well at the
/// foot.
const CARDS: [Card; 13] = [
    Card { x0: 8.0, x1: 208.0, y0: 28.0, y1: 148.0, heading: "KEY" },
    Card { x0: 214.0, x1: 340.0, y0: 28.0, y1: 148.0, heading: "DETECTION" },
    Card { x0: 346.0, x1: 712.0, y0: 28.0, y1: 148.0, heading: "PITCH" },
    Card { x0: 8.0, x1: 340.0, y0: 172.0, y1: 262.0, heading: "NOTES" },
    Card { x0: 346.0, x1: 434.0, y0: 172.0, y1: 262.0, heading: "RETUNE" },
    Card { x0: 440.0, x1: 528.0, y0: 172.0, y1: 262.0, heading: "HUMANIZE" },
    Card { x0: 534.0, x1: 622.0, y0: 172.0, y1: 262.0, heading: "FLEX" },
    Card { x0: 628.0, x1: 712.0, y0: 172.0, y1: 262.0, heading: "AMOUNT" },
    Card { x0: 8.0, x1: 112.0, y0: 286.0, y1: 376.0, heading: "VIB RATE" },
    Card { x0: 118.0, x1: 222.0, y0: 286.0, y1: 376.0, heading: "VIB DEPTH" },
    Card { x0: 228.0, x1: 340.0, y0: 286.0, y1: 376.0, heading: "VIB ONSET" },
    Card { x0: 346.0, x1: 434.0, y0: 286.0, y1: 376.0, heading: "TRANSPOSE" },
    Card { x0: 440.0, x1: 528.0, y0: 286.0, y1: 376.0, heading: "A4" },
];

const CONTROLS: [Control; N] = [
    Control::Knob { id: "key", cx: 58.0, cy: 82.0, r: 22.0, caption: "", bipolar: false },
    Control::Knob { id: "scale", cx: 156.0, cy: 82.0, r: 22.0, caption: "", bipolar: false },
    Control::Choice { id: "auto", x: 226.0, top: 38.0, names: &["MANUAL KEY", "AUTO KEY"] },
    Control::Choice { id: "voice", x: 226.0, top: 78.0, names: &["HIGH VOICE", "MID VOICE", "LOW VOICE", "INSTRUMENT"] },
    Control::Knob { id: "retune", cx: 390.0, cy: 210.0, r: 20.0, caption: "", bipolar: false },
    Control::Knob { id: "humanize", cx: 484.0, cy: 210.0, r: 20.0, caption: "", bipolar: false },
    Control::Knob { id: "flex", cx: 578.0, cy: 210.0, r: 20.0, caption: "", bipolar: false },
    Control::Knob { id: "amount", cx: 670.0, cy: 210.0, r: 20.0, caption: "", bipolar: false },
    Control::Knob { id: "vib_rate", cx: 60.0, cy: 324.0, r: 20.0, caption: "", bipolar: false },
    Control::Knob { id: "vib_depth", cx: 170.0, cy: 324.0, r: 20.0, caption: "", bipolar: false },
    Control::Knob { id: "vib_delay", cx: 284.0, cy: 324.0, r: 20.0, caption: "", bipolar: false },
    Control::Knob { id: "transpose", cx: 390.0, cy: 324.0, r: 20.0, caption: "", bipolar: true },
    Control::Knob { id: "tuning", cx: 484.0, cy: 324.0, r: 20.0, caption: "", bipolar: true },
    Control::Keys { id: "notes", x0: 18.0, y0: 184.0, x1: 332.0, y1: 252.0, lit },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Pitch",
    word: "PITCH",
    about: "Lemon Studio's pitch correction",
    settings: &SETTINGS,
    version: Some(1),
    any_version: false,
    fields: &[],
    tail: &[],
    size: (720, 414),
    cards: &CARDS,
    controls: &CONTROLS,
    display: Some(Display { r: Rect::new(354.0, 36.0, 350.0, 104.0), draw: readout }),
    mark: Mark { x: 623.0, y: 306.0, anchor: 0.5, stacked: true },
    hint: Some(Rect::new(8.0, 384.0, 704.0, 22.0)),
    labels: &[],
    text,
    parse,
    sound,
    rings: false,
};

/// The notes the strip lights: the scale in use (with key detection, the one the sound found).
fn lit(v: &[i32]) -> u16 {
    if v[AUTO] != 0 {
        v.get(N + L_SCALE).map_or(0, |&m| m as u16)
    } else {
        scale_mask(v[KEY], v[SCALE] as usize)
    }
}

/// `C4`-style name of a MIDI note.
fn note_name(n: i32) -> String {
    format!("{}{}", NOTES[n.rem_euclid(12) as usize], n.div_euclid(12) - 1)
}

/// The live well: the note heard and its offset, the note played and its offset, the key in use, and a meter of how
/// far the voice is from the target (white) and where the output sits (lemon).
fn readout(c: &mut Canvas, r: Rect, v: &[i32]) {
    let big = Font::condensed(28, 0).bold();
    let small = Font::new(11).bold();
    let label = Font::new(10).bold();
    let live = v.get(N..).unwrap_or(&[]);
    let at = |k: usize| live.get(k).copied().unwrap_or(0);
    let voiced = at(L_VOICED) != 0;
    let lh = c.line_height(label);
    let column = |c: &mut Canvas, x: f32, head: &str, cents: Option<i32>, ink| {
        c.text(label, x, r.y + 8.0, plug::DIM, head);
        let (name, off) = match cents {
            Some(cents) => {
                let n = (cents as f32 / 100.0).round() as i32;
                (note_name(n), format!("{:+}c", cents - n * 100))
            }
            None => ("--".into(), String::new()),
        };
        c.text(big, x, r.y + 10.0 + lh, ink, &name);
        c.text(small, x, r.y + 48.0 + lh, plug::DIM, &off);
    };
    column(c, r.x + 12.0, "HEARD", voiced.then(|| at(L_IN)), brand::TEXT);
    column(c, r.x + 104.0, "PLAYED", voiced.then(|| at(L_OUT)), brand::MAIN);
    let key = if v[AUTO] == 0 {
        format!("{} {}", NOTES[v[KEY].clamp(0, 11) as usize], SCALES[v[SCALE].clamp(0, 13) as usize].0)
    } else if at(L_KEY) >= 0 && at(L_FIT) > 0 {
        format!("{} {}", NOTES[at(L_KEY).clamp(0, 11) as usize], if at(L_MINOR) != 0 { "minor" } else { "major" })
    } else {
        "Listening...".into()
    };
    let kx = r.x + 200.0;
    c.text(label, kx, r.y + 8.0, plug::DIM, if v[AUTO] == 0 { "KEY" } else { "KEY FOUND" });
    c.text(small, kx, r.y + 12.0 + lh, brand::MAIN, &key);
    if v[AUTO] != 0 && at(L_FIT) > 0 {
        c.text(label, kx, r.y + 30.0 + lh, plug::DIM, &format!("fit {}%", at(L_FIT)));
    }
    if v[TRANSPOSE] != 0 {
        c.text(label, kx, r.y + 48.0 + lh, plug::DIM, &format!("transpose {:+}", v[TRANSPOSE]));
    }
    // The meter: -50 to +50 cents around the target.
    let (x0, x1, y) = (r.x + 12.0, r.right() - 12.0, r.bottom() - 14.0);
    c.line(x0, y, x1, y, 1.0, plug::FAINT);
    for k in 0..=4 {
        let x = x0 + (x1 - x0) * k as f32 / 4.0;
        let h = if k == 2 { 6.0 } else { 3.0 };
        c.line(x, y - h, x, y + h, 1.0, plug::FAINT);
    }
    if voiced && at(L_TARGET) >= 0 {
        let pos = |cents: i32| x0 + (x1 - x0) * (((cents - at(L_TARGET) * 100) as f32).clamp(-50.0, 50.0) + 50.0) / 100.0;
        c.fill_rrect(Rect::new(pos(at(L_IN)) - 2.0, y - 6.0, 4.0, 12.0), 2.0, brand::TEXT);
        c.fill_rrect(Rect::new(pos(at(L_OUT)) - 3.0, y - 4.0, 6.0, 8.0), 3.0, brand::MAIN);
    }
}

fn text(k: usize, v: i32) -> String {
    match k {
        KEY => NOTES[v.clamp(0, 11) as usize].into(),
        SCALE => SCALES[v.clamp(0, 13) as usize].0.into(),
        AUTO => if v == 0 { "Manual" } else { "Auto" }.into(),
        VOICE => VOICES[v.clamp(0, 3) as usize].0.into(),
        RETUNE if v == 0 => "0ms (hard)".into(),
        RETUNE | VIB_DELAY => format!("{v}ms"),
        HUMANIZE | AMOUNT => format!("{v}%"),
        FLEX | VIB_DEPTH if v == 0 => "Off".into(),
        FLEX | VIB_DEPTH => format!("{v} cents"),
        VIB_RATE | TUNING => format!("{:.1}Hz", f64::from(v) / 10.0),
        TRANSPOSE if v == 0 => "0 st".into(),
        TRANSPOSE => format!("{v:+} st"),
        _ => notes_text(v),
    }
}

/// `All notes`, or `Off: C#, F` for the notes turned off.
fn notes_text(mask: i32) -> String {
    if mask & 0xfff == 0 {
        return "All notes".into();
    }
    let off: Vec<&str> = (0..12).filter(|k| mask & (1 << k) != 0).map(|k| NOTES[k]).collect();
    format!("Off: {}", off.join(", "))
}

/// A note name (`C`, `F#`, `Bb`; an octave after it is ignored) as 0 to 11.
fn note_index(s: &str) -> Option<i32> {
    let s = s.trim().to_ascii_lowercase();
    let mut c = s.chars();
    let base = match c.next()? {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let accidental = match c.next() {
        Some('#') => 1,
        Some('b') => -1,
        _ => 0,
    };
    Some((base + accidental as i32).rem_euclid(12))
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let (min, max) = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    let named = |names: &[&str]| {
        let exact = names.iter().position(|n| n.to_ascii_lowercase() == low);
        exact.or_else(|| names.iter().position(|n| !low.is_empty() && n.to_ascii_lowercase().starts_with(&low))).map(|i| i as i32)
    };
    match k {
        KEY => note_index(&low),
        SCALE => named(&SCALES.map(|s| s.0)),
        AUTO => named(&["manual", "auto"]),
        VOICE => named(&VOICES.map(|v| v.0)),
        FLEX | VIB_DEPTH if low.starts_with("off") => Some(0),
        VIB_RATE | TUNING => leading_number(&low).map(|x| ((x * 10.0).round() as i32).clamp(min, max)),
        NOTES_OFF if low.starts_with("all") => Some(0),
        NOTES_OFF => low.strip_prefix("off:").unwrap_or(&low).split(',').try_fold(0, |m, n| Some(m | 1 << note_index(n)?)),
        _ => leading_number(&low).map(|x| (x.round() as i32).clamp(min, max)),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Pitch detection

/// A second-order Butterworth low-pass (RBJ), direct form I.
#[derive(Clone, Copy, Default)]
struct LowPass {
    b0: f32,
    b1: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl LowPass {
    fn new(hz: f32, rate: f32) -> LowPass {
        let w = std::f32::consts::TAU * hz.min(rate * 0.45) / rate;
        let alpha = w.sin() / std::f32::consts::SQRT_2;
        let a0 = 1.0 + alpha;
        let b0 = (1.0 - w.cos()) / 2.0 / a0;
        LowPass { b0, b1: 2.0 * b0, a1: -2.0 * w.cos() / a0, a2: (1.0 - alpha) / a0, ..LowPass::default() }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * (x + self.x2) + self.b1 * self.x1 - self.a1 * self.y1 - self.a2 * self.y2;
        (self.x2, self.x1, self.y2, self.y1) = (self.x1, x, self.y1, y);
        y
    }
}

/// Decimated samples between readings.
const HOP: usize = 32;
const RING: usize = 4096;
/// YIN's threshold, and the worst dip still taken as pitched when none goes under it.
const YIN: f32 = 0.15;
const YIN_WORST: f32 = 0.3;
/// The quietest pitched input (mean square, about -60 dB).
const QUIET: f32 = 1e-6;

/// YIN pitch detection on a low-passed, decimated signal.
pub struct Detector {
    /// Input samples to one decimated sample.
    pub factor: usize,
    phase: usize,
    lp: [LowPass; 2],
    ring: Vec<f32>,
    at: usize,
    filled: usize,
    since: usize,
    /// The decimated rate.
    rate: f32,
    min_lag: usize,
    max_lag: usize,
    frame: Vec<f32>,
    diff: Vec<f32>,
    norm: Vec<f32>,
    /// The fundamental of the last reading (Hz); 0 when the input is unpitched or quiet.
    pub freq: f32,
}

impl Detector {
    pub fn new(rate: u32, fmin: f32, fmax: f32) -> Detector {
        let factor = ((rate as f32 / 11025.0).round() as usize).max(1);
        let r = rate as f32 / factor as f32;
        let min_lag = ((r / fmax).floor() as usize).max(2);
        let max_lag = ((r / fmin).ceil() as usize).max(min_lag + 2);
        Detector {
            factor,
            phase: 0,
            lp: [LowPass::new(0.4 * r, rate as f32); 2],
            ring: vec![0.0; RING],
            at: 0,
            filled: 0,
            since: 0,
            rate: r,
            min_lag,
            max_lag,
            frame: vec![0.0; 2 * max_lag],
            diff: vec![0.0; max_lag + 1],
            norm: vec![0.0; max_lag + 1],
            freq: 0.0,
        }
    }

    /// About how far (input samples) behind the newest input a reading's middle lies: YIN compares the frame's start
    /// with itself a period on, so it mostly sees the older part of its frame.
    pub fn delay(&self) -> usize {
        (1.4 * (self.max_lag * self.factor) as f32) as usize
    }

    /// Takes an input sample; true when it made a new reading.
    pub fn push(&mut self, x: f32) -> bool {
        let y = self.lp[0].run(x);
        let y = self.lp[1].run(y);
        self.phase += 1;
        if self.phase < self.factor {
            return false;
        }
        self.phase = 0;
        self.ring[self.at] = y;
        self.at = (self.at + 1) % RING;
        self.filled += 1;
        self.since += 1;
        if self.since < HOP {
            return false;
        }
        self.since = 0;
        self.freq = self.read().unwrap_or(0.0);
        true
    }

    /// YIN over the last `2 * max_lag` decimated samples: the difference function, normalised by its running mean,
    /// the first dip under the threshold (else the deepest, if deep enough), refined with a parabola through the raw
    /// difference.
    fn read(&mut self) -> Option<f32> {
        let w = self.max_lag;
        let total = 2 * w;
        if self.filled < total {
            return None;
        }
        for (j, f) in self.frame.iter_mut().enumerate() {
            *f = self.ring[(self.at + RING - total + j) % RING];
        }
        let f = &self.frame;
        if f[w..].iter().map(|x| x * x).sum::<f32>() / (w as f32) < QUIET {
            return None;
        }
        let mut sum = 0.0;
        self.norm[0] = 1.0;
        for tau in 1..=w {
            let d: f32 = (0..w).map(|j| (f[j] - f[j + tau]).powi(2)).sum();
            self.diff[tau] = d;
            sum += d;
            self.norm[tau] = if sum > 0.0 { d * tau as f32 / sum } else { 1.0 };
        }
        let mut tau = (self.min_lag..w).find(|&t| self.norm[t] < YIN);
        if let Some(t) = &mut tau {
            while *t + 1 < w && self.norm[*t + 1] < self.norm[*t] {
                *t += 1;
            }
        }
        let tau = match tau {
            Some(t) => t,
            None => {
                let t = (self.min_lag..w).min_by(|&a, &b| self.norm[a].total_cmp(&self.norm[b]))?;
                if self.norm[t] > YIN_WORST {
                    return None;
                }
                t
            }
        };
        let (a, b, c) = (self.diff[tau - 1], self.diff[tau], self.diff[tau + 1]);
        let den = a - 2.0 * b + c;
        let shift = if den > 1e-12 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) } else { 0.0 };
        Some(self.rate / (tau as f32 + shift))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Key detection

/// Krumhansl-Kessler key profiles, from the key note up.
const MAJOR_PROFILE: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR_PROFILE: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
/// How long what is heard counts (s): older notes fade with this time constant.
const MEMORY: f32 = 30.0;
/// Seconds of clear notes heard before a key is named.
const ENOUGH: f32 = 1.5;
/// How much better (correlation) another key must fit to take over.
const SWITCH: f32 = 0.05;

/// The correlation of how long each note was heard with a key's profile.
fn fit(h: &[f32; 12], key: i32, minor: bool) -> f32 {
    let p = if minor { &MINOR_PROFILE } else { &MAJOR_PROFILE };
    let x: [f32; 12] = std::array::from_fn(|i| h[(i as i32 + key).rem_euclid(12) as usize]);
    let (mx, mp) = (x.iter().sum::<f32>() / 12.0, p.iter().sum::<f32>() / 12.0);
    let (mut xy, mut xx, mut yy) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        let (a, b) = (x[i] - mx, p[i] - mp);
        (xy, xx, yy) = (xy + a * b, xx + a * a, yy + b * b);
    }
    if xx <= 0.0 { 0.0 } else { xy / (xx * yy).sqrt() }
}

/// The key (note, minor) that fits how long each note was heard best, and how well.
pub fn best_key(h: &[f32; 12]) -> ((i32, bool), f32) {
    let mut best = ((0, false), f32::MIN);
    for key in 0..12 {
        for minor in [false, true] {
            let f = fit(h, key, minor);
            if f > best.1 {
                best = ((key, minor), f);
            }
        }
    }
    best
}

/// Finds the key from the notes heard.
#[derive(Default)]
pub struct KeyFinder {
    heard: [f32; 12],
    /// The key named (note, minor) and how well it fits.
    pub key: Option<(i32, bool)>,
    pub fit: f32,
}

impl KeyFinder {
    /// Note `pc` (0 = C) was heard for `secs`.
    pub fn add(&mut self, pc: i32, secs: f32) {
        let keep = (-secs / MEMORY).exp();
        for h in &mut self.heard {
            *h *= keep;
        }
        self.heard[pc.rem_euclid(12) as usize] += secs;
        if self.heard.iter().sum::<f32>() < ENOUGH {
            return;
        }
        let (best, f) = best_key(&self.heard);
        match self.key {
            Some(k) if k != best && f < fit(&self.heard, k.0, k.1) + SWITCH => {}
            _ => self.key = Some(best),
        }
        self.fit = self.key.map_or(0.0, |k| fit(&self.heard, k.0, k.1));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The shift

/// Pitch-synchronous overlap-add on two channels, with a constant latency.
struct Psola {
    x: [Vec<f32>; 2],
    acc: [Vec<f32>; 2],
    wsum: Vec<f32>,
    /// The ratio asked for and whether the input was pitched, by the frame they were given with.
    ctl: Vec<(f32, bool)>,
    /// Frames written.
    n: i64,
    /// Analysis marks: position and the period there (the distance to the next mark once it is placed).
    marks: VecDeque<(i64, f32)>,
    next_a: f64,
    next_s: f64,
    /// The analysis mark the last grain came from.
    last_a: i64,
    /// The longest period (samples).
    pmax: usize,
    /// How far behind the input a pitch reading is (samples): the ratio for a moment was given this much later.
    look: usize,
    lat: usize,
}

impl Psola {
    fn new(pmax: usize, look: usize) -> Psola {
        let size = (6 * pmax + 64).next_power_of_two();
        Psola {
            x: [vec![0.0; size], vec![0.0; size]],
            acc: [vec![0.0; size], vec![0.0; size]],
            wsum: vec![0.0; size],
            ctl: vec![(1.0, false); size],
            n: 0,
            marks: VecDeque::new(),
            next_a: 0.0,
            next_s: 0.0,
            last_a: i64::MIN,
            pmax,
            look: look.min(2 * pmax),
            lat: 3 * pmax + 2,
        }
    }

    fn mono(&self, i: i64) -> f32 {
        let k = i as usize & (self.wsum.len() - 1);
        self.x[0][k] + self.x[1][k]
    }

    /// Channel `ch` at `i + t` (`t` within a sample), by four-point Hermite interpolation.
    fn read(&self, ch: usize, i: i64, t: f32) -> f32 {
        let m = self.wsum.len() - 1;
        let (i, t) = if t < 0.0 { (i - 1, t + 1.0) } else { (i, t) };
        let x = |k: i64| self.x[ch][(i + k) as usize & m];
        let (y0, y1, y2, y3) = (x(-1), x(0), x(1), x(2));
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * t + c2) * t + c1) * t + y1
    }

    /// The ratio and pitched flag that apply to frame `i` (read from when its pitch was known).
    fn at(&self, i: i64) -> (f32, bool) {
        self.ctl[(i + self.look as i64).min(self.n - 1).max(0) as usize & (self.wsum.len() - 1)]
    }

    /// Takes a frame with the period now (samples), the pitch ratio to play it at and whether it is pitched; gives the
    /// shifted frame, the input frame `lat` frames ago and whether that one was pitched.
    fn run(&mut self, x: [f32; 2], period: f32, ratio: f32, voiced: bool) -> ([f32; 2], [f32; 2], bool) {
        let m = self.wsum.len() - 1;
        let i = self.n as usize & m;
        (self.x[0][i], self.x[1][i]) = (x[0], x[1]);
        self.ctl[i] = (ratio.clamp(0.25, 4.0), voiced);
        self.n += 1;
        let p = period.clamp(16.0, self.pmax as f32);
        // Analysis marks a period apart, moved onto the waveform's peak within a quarter period while it is pitched;
        // each mark's period becomes the true distance to the next, so grains laid out at that distance rebuild the
        // input exactly.
        let reach = (p / 4.0) as i64;
        while (self.n - 1) as f64 >= self.next_a + reach as f64 {
            let c = self.next_a.round() as i64;
            let at = if voiced { (c - reach..=c + reach).max_by(|&a, &b| self.mono(a).total_cmp(&self.mono(b))).unwrap_or(c) } else { c };
            if let Some(last) = self.marks.back_mut() {
                let gap = (at - last.0) as f32;
                if gap >= 0.6 * last.1 && gap <= 1.6 * last.1 {
                    last.1 = gap.min(self.pmax as f32);
                }
            }
            self.marks.push_back((at, p));
            self.next_a = (at as f64 + f64::from(p)).max(self.next_a + 1.0);
        }
        // Synthesis marks at the new period. Each takes the grain of the mark after the last one used while that stays
        // within three quarters of a period (so jitter cannot flip between repeating and skipping a period), else the
        // nearest; while unpitched the marks line up again, so an unshifted grain lands where it was.
        while self.next_s + 2.0 * self.pmax as f64 <= self.n as f64 {
            let s = self.next_s.round() as i64;
            while self.marks.len() > 2 && self.marks[1].0 < s - 2 * self.pmax as i64 {
                self.marks.pop_front();
            }
            let (ratio, pitched) = self.at(s);
            let nearest = self.marks.iter().copied().min_by_key(|(a, _)| (a - s).abs());
            let next = self.marks.iter().copied().find(|&(a, _)| a > self.last_a);
            let Some((a, pg)) = (match (next, nearest) {
                (Some(n), _) if pitched && ((n.0 - s).abs() as f32) < 0.75 * n.1 => Some(n),
                (_, near) => near,
            }) else {
                self.next_s += f64::from(p);
                continue;
            };
            let (s, from) = if pitched { (s, self.next_s) } else { (a, a as f64) };
            self.last_a = a;
            // The grain lands at the fractional synthesis position (whole samples would jitter the period by up to
            // half a sample, several cents on a high voice): read back from the source by that fraction.
            let frac = (from - s as f64) as f32;
            // Two periods, longer when the marks spread out (a lower pitch), so the grains always overlap.
            let half = ((pg.max(pg / ratio)).ceil() as i64).min(self.pmax as i64).min(self.n - 3 - a).max(1);
            for j in -half..=half {
                let u = j as f32 - frac;
                let w = if u.abs() >= half as f32 { 0.0 } else { 0.5 + 0.5 * (std::f32::consts::PI * u / half as f32).cos() };
                let dst = (s + j) as usize & m;
                for ch in 0..2 {
                    self.acc[ch][dst] += self.read(ch, a + j, -frac) * w;
                }
                self.wsum[dst] += w;
            }
            let step = if pitched { pg / ratio } else { pg };
            self.next_s = (from + f64::from(step).max(1.0)).max(self.next_s + 1.0);
        }
        let o = self.n - 1 - self.lat as i64;
        if o < 0 {
            return ([0.0; 2], [0.0; 2], false);
        }
        let k = o as usize & m;
        let g = 1.0 / self.wsum[k].max(0.5);
        let out = [self.acc[0][k] * g, self.acc[1][k] * g];
        (self.acc[0][k], self.acc[1][k], self.wsum[k]) = (0.0, 0.0, 0.0);
        (out, [self.x[0][k], self.x[1][k]], self.at(o).1)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The sound

/// How far (semitones) the voice must move past the note it is on before the target moves to another.
const HOLD: f32 = 0.25;
/// The most humanize adds to the retune time on a held note (ms), and when a note counts as held (s).
const HUMANIZE_MS: f32 = 300.0;
const HELD_FROM: f32 = 0.12;
const HELD_OVER: f32 = 0.4;
/// The vibrato's fade in after its onset (s).
const VIB_FADE: f32 = 0.25;
/// The fade between the corrected and the dry sound (ms).
const MIX_MS: f32 = 8.0;
/// How long a note holds through readings that lose it (s), and how long a jump of about an octave must last before
/// it is believed (s): shorter ones are the detector's octave errors.
const HANG: f32 = 0.06;
const OCTAVE_HOLD: f32 = 0.04;
/// How smoothly the pitch heard is followed (s), and the jump (semitones) taken as a new note, followed at once.
const SMOOTH: f32 = 0.005;
const NEW_NOTE: f32 = 0.8;
/// The quickest the correction follows (ms), so it never steps from one reading to the next.
const FOLLOW_MIN_MS: f32 = 3.0;

pub struct PitchFix {
    rate: f32,
    v: [i32; N],
    det: Detector,
    shift: Psola,
    keys: KeyFinder,
    /// The last three readings (MIDI note numbers) and how many are fresh.
    recent: [f32; 3],
    fresh: usize,
    voiced: bool,
    /// How long the readings have lost the note (s), and how long they have heard a jump of about an octave.
    lost: f32,
    octave: f32,
    /// The period (samples) and the pitch heard (MIDI note number, smoothed).
    period: f32,
    heard: f32,
    target: Option<i32>,
    /// How long the target has held (s).
    held: f32,
    scale_in_use: u16,
    vib_phase: f32,
    /// The vibrato (cents): where it is heading and where it is, added after the correction's smoothing so the retune
    /// speed does not damp it.
    vib_to: f32,
    vib: f32,
    /// The correction (cents), where it is heading and how fast it follows (per sample).
    corr: f32,
    corr_to: f32,
    follow: f32,
    /// The share of the corrected sound.
    mix: f32,
    mix_follow: f32,
    vib_follow: f32,
}

/// One-pole coefficient for a time constant of `ms` at `rate` (1 for none).
fn coef(ms: f32, rate: f32) -> f32 {
    if ms < 0.05 { 1.0 } else { 1.0 - (-1000.0 / (ms * rate)).exp() }
}

impl PitchFix {
    pub fn new(rate: u32, v: &[i32]) -> PitchFix {
        let mut s = PitchFix {
            rate: rate.max(1) as f32,
            v: [0; N],
            det: Detector::new(44100, 80.0, 700.0),
            shift: Psola::new(64, 0),
            keys: KeyFinder::default(),
            recent: [0.0; 3],
            fresh: 0,
            voiced: false,
            lost: 0.0,
            octave: 0.0,
            period: 200.0,
            heard: 0.0,
            target: None,
            held: 0.0,
            scale_in_use: 0,
            vib_phase: 0.0,
            vib_to: 0.0,
            vib: 0.0,
            corr: 0.0,
            corr_to: 0.0,
            follow: 1.0,
            mix: 0.0,
            mix_follow: 0.0,
            vib_follow: 1.0,
        };
        s.v.copy_from_slice(&v[..N]);
        s.rebuild();
        s
    }

    /// New detector and shifter for the input range and rate (forgets what they hold).
    fn rebuild(&mut self) {
        let (_, lo, hi) = VOICES[self.v[VOICE].clamp(0, 3) as usize];
        let rate = self.rate as u32;
        self.det = Detector::new(rate, lo, hi);
        let pmax = (self.rate / lo).ceil() as usize;
        self.shift = Psola::new(pmax, self.det.delay() + HOP * self.det.factor + (SMOOTH * self.rate) as usize);
        self.period = self.rate / 200.0;
        self.mix_follow = coef(MIX_MS, self.rate);
        self.vib_follow = coef(FOLLOW_MIN_MS, self.rate);
        (self.voiced, self.target, self.fresh, self.corr, self.corr_to, self.mix) = (false, None, 0, 0.0, 0.0, 0.0);
        (self.lost, self.octave) = (0.0, 0.0);
    }

    /// The scale in use and the notes the target may land on.
    fn notes(&self) -> (u16, u16) {
        let (key, scale) = if self.v[AUTO] != 0 {
            match self.keys.key {
                Some((k, minor)) => (k, if minor { MINOR } else { MAJOR }),
                None => (0, CHROMATIC),
            }
        } else {
            (self.v[KEY], self.v[SCALE].clamp(0, 13) as usize)
        };
        let scale = scale_mask(key, scale);
        let off = self.v[NOTES_OFF] as u16 & 0xfff;
        let allowed = match scale & !off {
            0 => 0xfff & !off,
            a => a,
        };
        (scale, if allowed == 0 { 0xfff } else { allowed })
    }

    /// A new reading: the target note and the correction.
    fn reading(&mut self) {
        let secs = (HOP * self.det.factor) as f32 / self.rate;
        let transpose = self.v[TRANSPOSE] as f32 * 100.0;
        let f = self.det.freq;
        let a4 = self.v[TUNING] as f32 / 10.0;
        // A reading of about an octave (or a twelfth) away from the note held is believed only once it lasts.
        let note = 69.0 + 12.0 * (f / a4).log2();
        let leap = (note - self.heard).abs();
        let octave_error = self.voiced && f > 0.0 && [12.0, 19.0, 24.0].iter().any(|&o| (leap - o).abs() < 1.5);
        self.octave = if octave_error { self.octave + secs } else { 0.0 };
        if f <= 0.0 || (octave_error && self.octave < OCTAVE_HOLD) {
            // Lost (or doubted) for a moment: hold the note and its correction rather than let go of it.
            self.lost += secs;
            if self.voiced && self.lost < HANG {
                return;
            }
            (self.voiced, self.target, self.held, self.fresh, self.corr_to, self.vib_to) = (false, None, 0.0, 0, transpose, 0.0);
            return;
        }
        self.lost = 0.0;
        self.recent = [self.recent[1], self.recent[2], note];
        self.fresh = (self.fresh + 1).min(3);
        let heard = if self.fresh == 3 {
            let mut r = self.recent;
            r.sort_by(f32::total_cmp);
            r[1]
        } else {
            self.recent[2]
        };
        // Smoothed against the detector's jitter, but a new note is taken at once.
        let heard = if !self.voiced || (heard - self.heard).abs() > NEW_NOTE {
            heard
        } else {
            self.heard + (heard - self.heard) * (1.0 - (-secs / SMOOTH).exp())
        };
        self.voiced = true;
        self.heard = heard;
        self.period = self.rate / (a4 * 2f32.powf((heard - 69.0) / 12.0));
        let near = heard.round();
        if (heard - near).abs() < 0.35 {
            self.keys.add(near as i32, secs);
        }
        let (scale, allowed) = self.notes();
        self.scale_in_use = scale;
        let on = |n: i32| allowed & (1 << n.rem_euclid(12)) != 0;
        let base = near as i32;
        let best = (base - 6..=base + 6).filter(|&n| on(n)).min_by(|&a, &b| (a as f32 - heard).abs().total_cmp(&(b as f32 - heard).abs())).unwrap_or(base);
        let t = match self.target {
            Some(cur) if on(cur) && (cur as f32 - heard).abs() < (best as f32 - heard).abs() + HOLD => cur,
            _ => best,
        };
        if self.target == Some(t) {
            self.held += secs;
        } else {
            (self.target, self.held) = (Some(t), 0.0);
        }
        let mut dev = (t as f32 - heard) * 100.0;
        let flex = self.v[FLEX] as f32;
        if flex > 0.0 {
            dev *= (dev.abs() / flex).min(1.0).powi(2);
        }
        let rate = self.v[VIB_RATE] as f32 / 10.0;
        self.vib_phase = (self.vib_phase + std::f32::consts::TAU * rate * secs) % std::f32::consts::TAU;
        let fade = ((self.held - self.v[VIB_DELAY] as f32 / 1000.0) / VIB_FADE).clamp(0.0, 1.0);
        self.vib_to = self.v[VIB_DEPTH] as f32 * fade * self.vib_phase.sin();
        self.corr_to = self.v[AMOUNT] as f32 / 100.0 * dev + transpose;
        let sustain = ((self.held - HELD_FROM) / HELD_OVER).clamp(0.0, 1.0);
        let ms = self.v[RETUNE] as f32 + self.v[HUMANIZE] as f32 / 100.0 * HUMANIZE_MS * sustain;
        self.follow = coef(ms.max(FOLLOW_MIN_MS), self.rate);
    }

    /// Nothing to change: no correction, vibrato or transpose.
    fn idle(&self) -> bool {
        self.v[AMOUNT] == 0 && self.v[VIB_DEPTH] == 0 && self.v[TRANSPOSE] == 0
    }
}

impl Sound for PitchFix {
    fn set(&mut self, v: &[i32]) {
        if v[..N] != self.v {
            let again = v[VOICE] != self.v[VOICE];
            self.v.copy_from_slice(&v[..N]);
            if again {
                self.rebuild();
            }
        }
    }

    fn set_sample_rate(&mut self, rate: u32) {
        self.rate = rate.max(1) as f32;
        self.rebuild();
    }

    fn reset(&mut self) {
        self.rebuild();
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if self.det.push(0.5 * (*l + *r)) {
                self.reading();
            }
            self.corr += (self.corr_to - self.corr) * self.follow;
            self.vib += (self.vib_to - self.vib) * self.vib_follow;
            let ratio = 2f32.powf((self.corr + self.vib) / 1200.0);
            let (wet, dry, pitched) = self.shift.run([*l, *r], self.period, ratio, self.voiced);
            let to = if pitched && !self.idle() { 1.0 } else { 0.0 };
            self.mix += (to - self.mix) * self.mix_follow;
            if self.mix <= 1e-4 {
                (*l, *r) = (dry[0], dry[1]);
            } else {
                *l = self.mix * wet[0] + (1.0 - self.mix) * dry[0];
                *r = self.mix * wet[1] + (1.0 - self.mix) * dry[1];
            }
        }
    }

    fn latency(&self) -> u32 {
        self.shift.lat as u32
    }

    fn live(&self, out: &mut [i32; LIVE]) {
        out[L_VOICED] = i32::from(self.voiced);
        out[L_IN] = (self.heard * 100.0).round() as i32;
        out[L_OUT] = (self.heard * 100.0 + self.corr + self.vib).round() as i32;
        let (key, minor) = self.keys.key.map_or((-1, false), |k| k);
        out[L_KEY] = key;
        out[L_MINOR] = i32::from(minor);
        out[L_FIT] = (self.keys.fit * 100.0).round() as i32;
        out[L_SCALE] = i32::from(self.scale_in_use);
        out[L_TARGET] = self.target.unwrap_or(-1);
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(PitchFix::new(rate, v))
}

pub struct PitchKind;

impl Kind for PitchKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-pitch";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's pitch correction";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::PitchCorrection, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioPitch";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::PitchShift];
}

pub type LemonPitch = SmallFx<PitchKind>;

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44100;

    fn start() -> [i32; N] {
        SPEC.start().try_into().unwrap()
    }

    /// A voice-like tone: the fundamental and two quieter harmonics.
    fn tone(hz: f32, secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        (0..n)
            .map(|i| {
                let t = std::f32::consts::TAU * hz * i as f32 / RATE as f32;
                0.3 * t.sin() + 0.15 * (2.0 * t).sin() + 0.08 * (3.0 * t).sin()
            })
            .collect()
    }

    fn cents(a: f32, b: f32) -> f32 {
        1200.0 * (a / b).log2()
    }

    /// The median pitch a detector reads over `x` after `skip` samples.
    fn pitch_of(x: &[f32], skip: usize) -> f32 {
        let mut d = Detector::new(RATE, 55.0, 1100.0);
        let mut f: Vec<f32> = Vec::new();
        for (i, &s) in x.iter().enumerate() {
            if d.push(s) && i > skip && d.freq > 0.0 {
                f.push(d.freq);
            }
        }
        assert!(!f.is_empty(), "no pitch read");
        f.sort_by(f32::total_cmp);
        f[f.len() / 2]
    }

    /// Plays `x` on both sides with settings `v`.
    fn play(v: &[i32], x: &[f32]) -> (Vec<f32>, u32) {
        let mut s = PitchFix::new(RATE, v);
        let (mut l, mut r) = (x.to_vec(), x.to_vec());
        s.process(&mut l, &mut r);
        assert_eq!(l, r);
        (l, s.latency())
    }

    #[test]
    fn value_texts_and_typed_values() {
        assert_eq!(text(KEY, 1), "C#");
        assert_eq!(text(SCALE, 3), "Harmonic minor");
        assert_eq!(text(RETUNE, 0), "0ms (hard)");
        assert_eq!(text(TRANSPOSE, -5), "-5 st");
        assert_eq!(text(TUNING, 4400), "440.0Hz");
        assert_eq!(text(NOTES_OFF, 0b1000_0000_0010), "Off: C#, B");
        assert_eq!(parse(KEY, "Bb"), Some(10));
        assert_eq!(parse(KEY, "f#"), Some(6));
        assert_eq!(parse(SCALE, "major"), Some(1));
        assert_eq!(parse(SCALE, "minor pent"), Some(11));
        assert_eq!(parse(VOICE, "low"), Some(2));
        assert_eq!(parse(TUNING, "432"), Some(4320));
        assert_eq!(parse(TRANSPOSE, "+7"), Some(7));
        assert_eq!(parse(NOTES_OFF, "off: c#, b"), Some(0b1000_0000_0010));
        assert_eq!(parse(NOTES_OFF, "all notes"), Some(0));
        assert_eq!(scale_mask(9, MINOR), scale_mask(0, MAJOR));
    }

    /// The detector reads tones across the range to within a few cents.
    #[test]
    fn pitch_detection_is_accurate() {
        for hz in [82.4, 110.0, 196.0, 261.6, 440.0, 659.3, 987.8] {
            let got = pitch_of(&tone(hz, 0.5), 4096);
            assert!(cents(got, hz).abs() < 5.0, "{hz} Hz read as {got} Hz");
        }
        // Silence and noise are unpitched.
        let mut d = Detector::new(RATE, 80.0, 700.0);
        assert!((0..8192).all(|_| !d.push(0.0) || d.freq == 0.0));
        let mut seed = 1u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
        };
        let mut d = Detector::new(RATE, 80.0, 700.0);
        let pitched = (0..44100).filter(|_| d.push(noise()) && d.freq > 0.0).count();
        assert!(pitched < 10, "{pitched} readings on noise");
    }

    /// Notes heard long enough name their key: a C major tune, an A (harmonic) minor one.
    #[test]
    fn the_key_is_found_from_the_notes() {
        let tune = |notes: &[i32]| {
            let mut k = KeyFinder::default();
            for _ in 0..4 {
                for &n in notes {
                    k.add(n, 0.25);
                }
            }
            k.key
        };
        assert_eq!(tune(&[0, 4, 7, 0, 2, 4, 5, 7, 9, 11, 0, 7, 4, 0]), Some((0, false)));
        assert_eq!(tune(&[9, 0, 4, 9, 11, 0, 2, 4, 5, 8, 9, 4, 0, 9]), Some((9, true)));
        // Too little heard: no key yet.
        let mut k = KeyFinder::default();
        k.add(0, 0.5);
        assert_eq!(k.key, None);
    }

    /// A note 40 cents sharp comes out on the note; with A turned off it goes to the nearest note left on.
    #[test]
    fn a_sharp_note_is_pulled_onto_the_scale() {
        let mut v = start();
        v[RETUNE] = 0;
        let (out, lat) = play(&v, &tone(440.0 * 2f32.powf(40.0 / 1200.0), 1.0));
        let got = pitch_of(&out, lat as usize + 8192);
        assert!(cents(got, 440.0).abs() < 6.0, "{got} Hz");

        v[NOTES_OFF] = 1 << 9;
        let (out, lat) = play(&v, &tone(440.0 * 2f32.powf(20.0 / 1200.0), 1.0));
        let got = pitch_of(&out, lat as usize + 8192);
        assert!(cents(got, 493.9).abs() < 8.0 || cents(got, 392.0).abs() < 8.0, "{got} Hz");
    }

    /// Transpose moves the note by semitones (formants kept: the grains are not resampled).
    #[test]
    fn transpose_moves_by_semitones() {
        let mut v = start();
        (v[SCALE], v[RETUNE], v[TRANSPOSE]) = (CHROMATIC as i32, 0, 7);
        let (out, lat) = play(&v, &tone(220.0, 1.0));
        let got = pitch_of(&out, lat as usize + 8192);
        assert!(cents(got, 220.0 * 2f32.powf(7.0 / 12.0)).abs() < 8.0, "{got} Hz");
        v[TRANSPOSE] = -5;
        let (out, lat) = play(&v, &tone(220.0, 1.0));
        let got = pitch_of(&out, lat as usize + 8192);
        assert!(cents(got, 220.0 * 2f32.powf(-5.0 / 12.0)).abs() < 8.0, "{got} Hz");
    }

    /// A slow retune leaves the start of a note uncorrected.
    #[test]
    fn the_retune_speed_sets_how_fast_the_note_is_pulled_in() {
        let mut v = start();
        v[RETUNE] = 400;
        let x = tone(440.0 * 2f32.powf(40.0 / 1200.0), 0.4);
        let (out, lat) = play(&v, &x);
        let early = pitch_of(&out[..lat as usize + 8000], lat as usize + 2048);
        assert!(cents(early, 440.0) > 25.0, "{early} Hz");
    }

    /// Unpitched input (noise) plays dry, delayed by the latency; silence stays silent.
    #[test]
    fn unpitched_input_plays_dry_after_the_latency() {
        let mut seed = 7u32;
        let x: Vec<f32> = (0..22050)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.5
            })
            .collect();
        let (out, lat) = play(&start(), &x);
        let lat = lat as usize;
        assert_eq!(lat, 3 * (44100.0f32 / 80.0).ceil() as usize + 2);
        let same = (lat..x.len()).filter(|&i| (out[i] - x[i - lat]).abs() < 1e-6).count();
        assert!(same as f32 > 0.97 * (x.len() - lat) as f32, "{same}");
        let (out, _) = play(&start(), &vec![0.0; 8192]);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    /// A sung-like note: 220 Hz with a natural 5 Hz, +/-30 cent vibrato, `harmonics` falling off as 1/k (more like a
    /// voice's buzz) and a little noise.
    fn sung(secs: f32, harmonics: usize) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let mut seed = 3u32;
        let mut phase = 0.0f32;
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let hz = 220.0 * 2f32.powf(30.0 * (std::f32::consts::TAU * 5.0 * t).sin() / 1200.0);
                phase = (phase + std::f32::consts::TAU * hz / RATE as f32) % std::f32::consts::TAU;
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 8) as f32 / (1 << 24) as f32 - 0.5;
                (1..=harmonics).map(|k| 0.3 / k as f32 * (k as f32 * phase).sin()).sum::<f32>() + 0.004 * noise
            })
            .collect()
    }

    /// The envelope's ripple: the spread of 20 ms RMS frames (5 ms apart) over their mean.
    fn ripple(x: &[f32]) -> f32 {
        let rms: Vec<f32> =
            x.windows(882).step_by(220).map(|w| (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).sqrt()).collect();
        let mean = rms.iter().sum::<f32>() / rms.len() as f32;
        (rms.iter().map(|r| (r - mean).powi(2)).sum::<f32>() / rms.len() as f32).sqrt() / mean
    }

    /// The pitch track (cents from `hz`) of `x`.
    fn track(x: &[f32], hz: f32) -> Vec<f32> {
        let mut d = Detector::new(RATE, 55.0, 1100.0);
        x.iter().filter_map(|&s| (d.push(s) && d.freq > 0.0).then(|| cents(d.freq, hz))).collect()
    }

    /// At the default settings a sung note comes out steady: no warble in its level, a pitch track that wavers no more
    /// than the voice did and no rougher, no reading off by more than a few cents from the next.
    #[test]
    fn a_sung_note_comes_out_steady() {
        for harmonics in [3, 12] {
            sung_note_comes_out_steady(&sung(2.0, harmonics));
        }
    }

    fn sung_note_comes_out_steady(x: &[f32]) {
        let (out, lat) = play(&start(), x);
        let from = lat as usize + 22050;
        let (r_in, r_out) = (ripple(&x[22050..]), ripple(&out[from..]));
        let t_in = track(&x[22050..], 220.0);
        let t = track(&out[from..], 220.0);
        let spread = |t: &[f32]| (t.iter().map(|c| c * c).sum::<f32>() / t.len() as f32).sqrt();
        let jump = |t: &[f32]| t.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        let (jump_in, jump) = (jump(&t_in), jump(&t));
        let jitter = |t: &[f32]| (t.windows(3).map(|w| (w[2] - 2.0 * w[1] + w[0]).powi(2)).sum::<f32>() / t.len() as f32).sqrt();
        let (j_in, jitter) = (jitter(&t_in), jitter(&t));
        println!("ripple {r_in:.4} -> {r_out:.4}, spread {:.1} -> {:.1}, jump {jump_in:.1} -> {jump:.1}, jitter {j_in:.2} -> {jitter:.2}", spread(&t_in), spread(&t));
        assert!(r_out < r_in + 0.01, "level ripple {r_out} (input {r_in})");
        assert!(spread(&t) < spread(&t_in), "pitch spread {} of {}", spread(&t), spread(&t_in));
        assert!(jump < jump_in + 1.5, "pitch jumps {jump} cents (input {jump_in})");
        assert!(jitter < j_in + 0.3, "pitch jitter {jitter} (input {j_in})");
    }

    /// A steady note already in tune passes nearly unchanged (the input, delayed by the latency).
    #[test]
    fn an_in_tune_note_passes_nearly_unchanged() {
        let x = tone(220.0, 1.5);
        let (out, lat) = play(&start(), &x);
        let lat = lat as usize;
        let (mut err, mut sig) = (0.0, 0.0);
        for i in lat + 22050..x.len() {
            err += (out[i] - x[i - lat]).powi(2);
            sig += x[i - lat].powi(2);
        }
        let snr = 10.0 * (sig / err).log10();
        println!("in tune: {snr:.1} dB");
        assert!(snr > 25.0, "{snr} dB");
        let got = pitch_of(&out, lat + 22050);
        assert!(cents(got, 220.0).abs() < 2.0, "{got} Hz");
    }

    /// Vibrato makes a steady note waver by about its depth once its onset has passed.
    #[test]
    fn vibrato_wavers_after_its_onset() {
        let mut v = start();
        (v[RETUNE], v[VIB_DEPTH], v[VIB_DELAY], v[VIB_RATE]) = (0, 50, 100, 50);
        let (out, lat) = play(&v, &tone(440.0, 1.5));
        let mut d = Detector::new(RATE, 55.0, 1100.0);
        let mut c: Vec<f32> = Vec::new();
        for (i, &s) in out.iter().enumerate() {
            if d.push(s) && i > lat as usize + 22050 && d.freq > 0.0 {
                c.push(cents(d.freq, 440.0));
            }
        }
        let (lo, hi) = c.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
        assert!(hi > 30.0 && lo < -30.0, "{lo} {hi}");
    }
}
