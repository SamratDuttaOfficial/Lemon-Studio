// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Mouth: Lemon Studio's mouth-noise remover as a plugin. It takes out the clicks, smacks and wet, slurpy
//! crackle saliva makes in a vocal, and keeps the words: what is said stays audible and only the noise goes.
//!
//! How (statistical signal modelling, light enough for any computer):
//! - Finding the noises: as in a speech codec, a short-term linear-prediction model (order 16, refitted every hop)
//!   predicts each sample from the ones before, and a long-term (pitch) predictor then predicts what is left from one
//!   voice period earlier. A voice, its glottal pulses included, is predicted well; a mouth click, an impulse that
//!   does not repeat, is not. Samples whose prediction error stands far above both its robust spread (median
//!   absolute deviation) and the error level around it are click candidates (a hissy consonant raises the level
//!   around itself, so it never stands out). Candidates close together make one event, and an event longer than the
//!   click length is a consonant, not a click, and is left. A slurp is a quick run of clicks and goes click by click.
//! - Taking them out: the sound is analysed in short overlapping frames (about 10 ms, a quarter apart). In a frame
//!   holding a click, every frequency above the low limit is brought down to the median of that frequency over the
//!   three frames before and after that hold no click (in a dense run of crackle, the quietest of them): the steady
//!   voice under the click is kept, the burst on top of it goes. Frames
//!   without clicks pass untouched, and the frames add back to the input exactly.
//! - Listen plays only what is taken out.

use nih_plug::prelude::*;
use os_ui::Rect;

use crate::params::leading_number;
use crate::spectral::{hann, lpc, median, Fft};
use crate::{Card, Control, Kind, Mark, Setting, SmallFx, Sound, Spec};

pub const SENSITIVITY: usize = 0;
pub const AMOUNT: usize = 1;
pub const LENGTH: usize = 2;
pub const LOW: usize = 3;
pub const LISTEN: usize = 4;

/// The settings: sensitivity and amount in %, the longest click in tenths of a ms, the low limit in Hz, listen.
pub const SETTINGS: [Setting; 5] = [
    Setting { id: "sensitivity", name: "Sensitivity", range: (0, 100), start: 50, magnet: None },
    Setting { id: "amount", name: "Amount", range: (0, 100), start: 100, magnet: None },
    Setting { id: "length", name: "Click length", range: (5, 60), start: 30, magnet: None },
    Setting { id: "low", name: "Low limit", range: (200, 4000), start: 700, magnet: None },
    Setting { id: "listen", name: "Listen", range: (0, 1), start: 0, magnet: None },
];

const CARDS: [Card; 5] = [
    Card { x0: 8.0, x1: 100.0, y0: 28.0, y1: 136.0, heading: "SENSITIVITY" },
    Card { x0: 106.0, x1: 198.0, y0: 28.0, y1: 136.0, heading: "AMOUNT" },
    Card { x0: 204.0, x1: 296.0, y0: 28.0, y1: 136.0, heading: "CLICK LENGTH" },
    Card { x0: 302.0, x1: 394.0, y0: 28.0, y1: 136.0, heading: "LOW LIMIT" },
    Card { x0: 400.0, x1: 492.0, y0: 28.0, y1: 136.0, heading: "OUTPUT" },
];

const CONTROLS: [Control; 5] = [
    Control::Knob { id: "sensitivity", cx: 54.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "amount", cx: 152.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "length", cx: 250.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "low", cx: 348.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Choice { id: "listen", x: 418.0, top: 64.0, names: &["NORMAL", "LISTEN"] },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Mouth",
    word: "MOUTH",
    about: "Lemon Studio's mouth-noise remover",
    settings: &SETTINGS,
    version: Some(1),
    any_version: false,
    fields: &[],
    tail: &[],
    size: (500, 174),
    cards: &CARDS,
    controls: &CONTROLS,
    display: None,
    mark: Mark { x: 492.0, y: 155.0, anchor: 1.0, stacked: false },
    hint: Some(Rect::new(8.0, 144.0, 484.0, 22.0)),
    labels: &[],
    text,
    parse,
    sound,
    rings: false,
};

fn text(k: usize, v: i32) -> String {
    match k {
        SENSITIVITY | AMOUNT => format!("{v}%"),
        LENGTH => format!("{:.1}ms", f64::from(v) / 10.0),
        LOW => format!("{v}Hz"),
        _ => if v == 0 { "Normal" } else { "Listen" }.into(),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let (min, max) = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    let clamp = |x: f64| Some((x.round() as i32).clamp(min, max));
    match k {
        SENSITIVITY | AMOUNT => clamp(leading_number(&low)?),
        LENGTH => clamp(leading_number(&low)? * 10.0),
        LOW => {
            let x = leading_number(&low)?;
            clamp(if low.contains('k') { x * 1000.0 } else { x })
        }
        _ => ["normal", "listen"].iter().position(|n| low.starts_with(n)).map(|i| i as i32).or(match low.as_str() {
            "off" => Some(0),
            "on" => Some(1),
            _ => None,
        }),
    }
}

/// Frames looked at either side of a frame being cleaned.
const SIDE: usize = 3;
/// The prediction model's order.
const ORDER: usize = 16;
/// Prediction errors kept for their spread.
const SPREAD: usize = 2048;
/// Candidates this close (ms) are one event.
const MERGE_MS: f32 = 0.6;
/// What is cleaned around an event (ms each side).
const PAD_MS: f32 = 0.5;
/// How far over the line (times it) a click goes at its peak.
const PEAK: f32 = 2.5;
/// Or how far over the line its samples go together (the line crossings summed): a crackle is a few samples over,
/// what is left of a glottal pulse one or two.
const MASS: f32 = 4.0;
/// Within this long (ms) after a click, a run of crackles: the mass then needed.
const RUN_MS: f32 = 25.0;
const RUN_MASS: f32 = 2.5;
/// How long after a sample it is judged (ms): the error level is measured this far either side of it.
const DECIDE_MS: f32 = 2.0;

/// A run of prediction errors over the line: where it started and last went over (samples back from the newest),
/// and the most any went over by (times the line).
#[derive(Clone, Copy)]
struct Event {
    start: usize,
    last: usize,
    peak: f32,
    mass: f32,
}

/// The mouth-noise remover's sound.
pub struct MouthClean {
    rate: u32,
    v: [i32; 5],
    n: usize,
    hop: usize,
    fft: Fft,
    win: Vec<f32>,
    /// Input still short of a hop, per side.
    fresh: [Vec<f32>; 2],
    /// The last `n` input samples per side (the newest frame).
    hist: [Vec<f32>; 2],
    /// Spectra of the last `2 * SIDE + 1` frames per side (oldest first), and their frames' samples.
    spec_re: Vec<[Vec<f32>; 2]>,
    spec_im: Vec<[Vec<f32>; 2]>,
    frames: Vec<[Vec<f32>; 2]>,
    /// Click marks of the samples of the last `n + 2 * SIDE * hop` input samples (index 0 the oldest).
    marks: Vec<bool>,
    /// Pre-emphasised mono history for the prediction model, its error spread, the model.
    pre: Vec<f32>,
    pre_last: f32,
    /// The last `SPREAD` prediction errors (a ring), and how many have come.
    errs: Vec<f32>,
    err_at: usize,
    /// When (in `err_at` samples) the last click was marked.
    last_click: Option<usize>,
    a: Vec<f32>,
    /// The short-term prediction error, and what is left of it after the pitch predictor (newest last).
    res: Vec<f32>,
    res2: Vec<f32>,
    /// The open event while one is open (`Event`).
    event: Option<Event>,
    /// Overlap-add output per side, and samples ready to be played.
    ola: [Vec<f32>; 2],
    out: [std::collections::VecDeque<f32>; 2],
    /// Frames cleaned so far (tests).
    pub cleaned: u64,
    scratch: Vec<f32>,
}

impl MouthClean {
    pub fn new(rate: u32, v: &[i32]) -> MouthClean {
        let rate = rate.max(8000);
        // About 10.7 ms frames: 512 at 44.1 / 48 kHz, longer at higher rates.
        let n = (512 * (rate as usize).div_ceil(48000)).next_power_of_two();
        let hop = n / 4;
        let k = 2 * SIDE + 1;
        let zeros = || [vec![0.0; n], vec![0.0; n]];
        let mut m = MouthClean {
            rate,
            v: [0; 5],
            n,
            hop,
            fft: Fft::new(n),
            win: hann(n).into_iter().map(f32::sqrt).collect(),
            fresh: [Vec::with_capacity(hop), Vec::with_capacity(hop)],
            hist: zeros(),
            spec_re: (0..k).map(|_| zeros()).collect(),
            spec_im: (0..k).map(|_| zeros()).collect(),
            frames: (0..k).map(|_| zeros()).collect(),
            marks: vec![false; n + 2 * SIDE * hop],
            pre: vec![0.0; n],
            pre_last: 0.0,
            errs: vec![0.0; SPREAD],
            err_at: 0,
            last_click: None,
            a: Vec::new(),
            res: vec![0.0; 4 * n],
            res2: vec![0.0; 4 * n],
            event: None,
            ola: [vec![0.0; n], vec![0.0; n]],
            out: Default::default(),
            cleaned: 0,
            scratch: Vec::with_capacity(SPREAD),
        };
        m.v.copy_from_slice(&v[..5]);
        m.prime();
        m
    }

    /// The delay the analysis needs: a frame and the frames after it, less the hop being gathered.
    pub fn delay(&self) -> usize {
        self.n + SIDE * self.hop - 1
    }

    fn prime(&mut self) {
        for o in &mut self.out {
            o.clear();
            o.extend(std::iter::repeat_n(0.0, self.hop - 1));
        }
    }

    fn ms(&self, ms: f32) -> usize {
        (ms * self.rate as f32 / 1000.0).round() as usize
    }

    /// One hop of input is in `fresh`: find the clicks in it, analyse the newest frame, clean the middle frame and
    /// add it to the output.
    fn hop(&mut self) {
        let (n, hop) = (self.n, self.hop);
        for c in 0..2 {
            self.hist[c].copy_within(hop.., 0);
            self.hist[c][n - hop..].copy_from_slice(&self.fresh[c]);
            self.fresh[c].clear();
        }
        self.detect();
        // The newest frame's spectrum.
        self.spec_re.rotate_left(1);
        self.spec_im.rotate_left(1);
        self.frames.rotate_left(1);
        let last = 2 * SIDE;
        for c in 0..2 {
            let (re, im, fr) = (&mut self.spec_re[last][c], &mut self.spec_im[last][c], &mut self.frames[last][c]);
            for i in 0..n {
                fr[i] = self.hist[c][i] * self.win[i];
                re[i] = fr[i];
                im[i] = 0.0;
            }
            self.fft.run(re, im, false);
        }
        // Which frames have a click in their middle half; the middle one is cleaned.
        let clicky: [bool; 2 * SIDE + 1] = std::array::from_fn(|f| {
            let start = self.marks.len() - n - (last - f) * hop;
            self.marks[start + n / 4..start + 3 * n / 4].iter().any(|&m| m)
        });
        let clicked = clicky[SIDE];
        let calm = clicky.iter().filter(|&&c| !c).count();
        let amount = self.v[AMOUNT].clamp(0, 100) as f32 / 100.0;
        let listen = self.v[LISTEN] != 0;
        for c in 0..2 {
            self.ola[c].copy_within(hop.., 0);
            let tail = n - hop;
            self.ola[c][tail..].fill(0.0);
        }
        if clicked && amount > 0.0 {
            self.cleaned += 1;
            let bin_hz = self.rate as f32 / n as f32;
            let first = ((self.v[LOW] as f32 / bin_hz).floor() as usize).clamp(1, n / 2);
            for c in 0..2 {
                let (mut re, mut im) = (self.spec_re[SIDE][c].clone(), self.spec_im[SIDE][c].clone());
                for k in first..=n / 2 {
                    let mag = (re[k] * re[k] + im[k] * im[k]).sqrt();
                    // What the voice holds there: the median of the frames around without clicks, or in a dense run
                    // of crackle (a slurp) the quietest frame.
                    self.scratch.clear();
                    for f in (0..=last).filter(|&f| calm < 2 || !clicky[f]) {
                        let (r, i) = (self.spec_re[f][c][k], self.spec_im[f][c][k]);
                        self.scratch.push((r * r + i * i).sqrt());
                    }
                    let floor = if calm < 2 {
                        self.scratch.iter().copied().fold(f32::MAX, f32::min)
                    } else {
                        median(&mut self.scratch)
                    };
                    // Above the low limit, eased in over its first bins.
                    let ease = ((k - first) as f32 / 3.0).min(1.0);
                    let g = if mag > floor && mag > 0.0 { 1.0 - amount * ease * (1.0 - floor / mag) } else { 1.0 };
                    let g = if listen { 1.0 - g } else { g };
                    re[k] *= g;
                    im[k] *= g;
                    if k > 0 && k < n / 2 {
                        re[n - k] *= g;
                        im[n - k] *= g;
                    }
                }
                if listen {
                    for k in 0..first {
                        for b in [k, (n - k) % n] {
                            re[b] = 0.0;
                            im[b] = 0.0;
                        }
                    }
                }
                self.fft.run(&mut re, &mut im, true);
                let scale = 1.0 / (n as f32 * 2.0);
                for i in 0..n {
                    self.ola[c][i] += re[i] * self.win[i] * scale;
                }
            }
        } else if !listen {
            // Untouched: the frame as it was (what its spectrum would give back).
            for c in 0..2 {
                for i in 0..n {
                    self.ola[c][i] += self.frames[SIDE][c][i] * self.win[i] * 0.5;
                }
            }
        }
        for c in 0..2 {
            self.out[c].extend(self.ola[c][..hop].iter().copied());
        }
    }

    /// Finds the clicks and marks them (with their pads): the newest hop is modelled, and the hop `DECIDE_MS` before
    /// it judged (it has the error level on both sides by then).
    fn detect(&mut self) {
        let (n, hop) = (self.n, self.hop);
        self.marks.copy_within(hop.., 0);
        let len = self.marks.len();
        self.marks[len - hop..].fill(false);
        // Pre-emphasised mono of the new hop, and the short-term model.
        self.pre.copy_within(hop.., 0);
        for i in 0..hop {
            let x = 0.5 * (self.hist[0][n - hop + i] + self.hist[1][n - hop + i]);
            self.pre[n - hop + i] = x - 0.95 * self.pre_last;
            self.pre_last = x;
        }
        let mut windowed = std::mem::take(&mut self.scratch);
        windowed.clear();
        windowed.extend(self.pre.iter().zip(&self.win).map(|(x, w)| x * w * w));
        lpc(&windowed, ORDER, &mut self.a);
        self.scratch = windowed;
        let rl = self.res.len();
        self.res.copy_within(hop.., 0);
        for i in 0..hop {
            let t = n - hop + i;
            self.res[rl - hop + i] = self.pre[t] + (1..=ORDER).map(|k| self.a[k] * self.pre[t - k]).sum::<f32>();
        }
        // The long-term (pitch) predictor, a sample of jitter allowed.
        let (lag, beta) = self.pitch();
        self.res2.copy_within(hop.., 0);
        for t in rl - hop..rl {
            let e = self.res[t];
            self.res2[t] = if beta > 0.0 {
                (lag - 1..=lag + 1).map(|l| e - beta * self.res[t - l]).min_by(|a, b| a.abs().total_cmp(&b.abs())).unwrap_or(e)
            } else {
                e
            };
            self.errs[self.err_at % SPREAD] = self.res2[t].abs();
            self.err_at += 1;
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.errs[..self.err_at.min(SPREAD)]);
        let spread = 1.4826 * median(&mut self.scratch);
        let k = 12.0 - 0.06 * self.v[SENSITIVITY].clamp(0, 100) as f32;
        let (merge, pad, d) = (self.ms(MERGE_MS).max(1), self.ms(PAD_MS), self.ms(DECIDE_MS).max(1));
        let longest = self.ms(self.v[LENGTH].max(1) as f32 / 10.0).max(1);
        // The error level around each judged sample (`d` either side): the larger of the median spread over the hop
        // (scattered crackles do not raise that, a hissy consonant does) and the mean size right around the sample
        // (the pulses a voice leaves raise that where they are).
        let from = rl - hop - 2 * d;
        self.scratch.clear();
        self.scratch.extend(self.res2[from..].iter().map(|e| e.abs()));
        let wide = 1.4826 * median(&mut self.scratch);
        let mut sums = Vec::with_capacity(hop + 2 * d + 1);
        sums.push(0.0f32);
        for t in from..rl {
            sums.push(sums.last().copied().unwrap_or(0.0) + self.res2[t].abs());
        }
        // `event` is counted back from the newest sample: age the open one by a hop first.
        let mut ev = self.event.map(|e| Event { start: e.start + hop, last: e.last + hop, ..e });
        for i in 0..hop {
            let t = rl - d - hop + i;
            let age = rl - 1 - t;
            let near = (sums[t + d + 1 - from] - sums[t - d - from]) / (2 * d + 1) as f32;
            let limit = (k * spread.max(wide).max(1.25 * near)).max(1e-6);
            let ratio = self.res2[t].abs() / limit;
            let over = ratio > 1.0;
            let new = Event { start: age, last: age, peak: ratio, mass: ratio };
            ev = match ev {
                Some(e) if e.last - age > merge => {
                    self.close(e, longest, pad);
                    over.then_some(new)
                }
                Some(e) if over => Some(Event { last: age, peak: e.peak.max(ratio), mass: e.mass + ratio, ..e }),
                None if over => Some(new),
                ev => ev,
            };
        }
        // Closed once it has been quiet for `merge` samples (one longer than a click is left as it is: `close`).
        if let Some(e) = ev {
            if e.last - d > merge {
                self.close(e, longest, pad);
                ev = None;
            }
        }
        self.event = ev;
    }

    /// The pitch predictor of the newest frame's short-term error: (lag, gain), gain 0 when nothing repeats.
    fn pitch(&self) -> (usize, f32) {
        let rl = self.res.len();
        let (lo, hi) = (self.ms(1.0).max(2), self.ms(16.0).min(rl - self.n - 2));
        let now = &self.res[rl - self.n..];
        let e0: f32 = now.iter().map(|x| x * x).sum();
        if e0 <= 1e-12 {
            return (lo, 0.0);
        }
        let score = |lag: usize| {
            let past = &self.res[rl - self.n - lag..rl - lag];
            let (num, et) = now.iter().zip(past).fold((0.0f32, 0.0f32), |(n, e), (a, b)| (n + a * b, e + b * b));
            (num / (e0 * et).sqrt().max(1e-12), num / et.max(1e-12))
        };
        let mut best = (lo, f32::MIN, 0.0);
        for lag in (lo..=hi).step_by(2) {
            let (c, b) = score(lag);
            if c > best.1 {
                best = (lag, c, b);
            }
        }
        for lag in best.0.saturating_sub(1).max(lo)..=(best.0 + 1).min(hi) {
            let (c, b) = score(lag);
            if c > best.1 {
                best = (lag, c, b);
            }
        }
        if best.1 < 0.3 { (best.0, 0.0) } else { (best.0, best.2.clamp(0.0, 1.0)) }
    }

    /// Marks an event if it is a click: short enough, and standing well over the line at its peak or over several
    /// samples (what is left of a glottal pulse the pitch predictor missed only just crosses it, for a sample or two).
    /// Right after a click (a slurp is a run of them) less is needed. Nothing is marked before the error spread has
    /// been measured.
    fn close(&mut self, e: Event, longest: usize, pad: usize) {
        let (s, l) = (e.start, e.last);
        let at = self.err_at - s;
        let run = self.last_click.is_some_and(|c| at - c <= self.ms(RUN_MS));
        let clear = e.peak >= PEAK || e.mass >= MASS || (run && e.mass >= RUN_MASS);
        if s - l + 1 > longest || !clear || self.err_at < SPREAD / 2 {
            return;
        }
        self.last_click = Some(at);
        let len = self.marks.len();
        let from = len.saturating_sub(1 + s + pad);
        let to = (len - 1 - l + pad).min(len - 1);
        self.marks[from..=to].fill(true);
    }
}

impl Sound for MouthClean {
    fn set(&mut self, v: &[i32]) {
        self.v.copy_from_slice(&v[..5]);
    }

    fn set_sample_rate(&mut self, rate: u32) {
        *self = MouthClean::new(rate, &self.v.clone());
    }

    fn reset(&mut self) {
        *self = MouthClean::new(self.rate, &self.v.clone());
    }

    fn latency(&self) -> u32 {
        self.delay() as u32
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.fresh[0].push(*l);
            self.fresh[1].push(*r);
            if self.fresh[0].len() == self.hop {
                self.hop();
            }
            *l = self.out[0].pop_front().unwrap_or(0.0);
            *r = self.out[1].pop_front().unwrap_or(0.0);
        }
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(MouthClean::new(rate, v))
}

pub struct MouthKind;

impl Kind for MouthKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-mouth";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's mouth-noise remover";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Restoration, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioMouth";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Restoration];
}

pub type LemonMouth = SmallFx<MouthKind>;

#[cfg(test)]
mod tests {
    use super::*;

    const START: [i32; 5] = [50, 100, 30, 700, 0];
    const RATE: u32 = 48000;

    /// A voice-like test signal: a 150 Hz buzz with harmonics through a formant-ish tilt, slowly swelling.
    fn voice(len: usize) -> Vec<f32> {
        (0..len)
            .map(|t| {
                let s = t as f32 / RATE as f32;
                let env = 0.3 + 0.1 * (std::f32::consts::TAU * 2.0 * s).sin();
                (1..30).map(|h| (std::f32::consts::TAU * 150.0 * h as f32 * s).sin() / h as f32).sum::<f32>() * env * 0.3
            })
            .collect()
    }

    /// Clicks: 0.3 ms decaying bursts of alternating sign every `every` samples from `from`.
    fn add_clicks(x: &mut [f32], from: usize, every: usize, amp: f32) -> Vec<usize> {
        let mut at = Vec::new();
        let mut t = from;
        while t + 20 < x.len() {
            for i in 0..14 {
                x[t + i] += amp * if i % 2 == 0 { 1.0 } else { -1.0 } * (-(i as f32) / 4.0).exp();
            }
            at.push(t);
            t += every;
        }
        at
    }

    fn run(v: &[i32], x: &[f32]) -> (Vec<f32>, MouthClean) {
        let mut m = MouthClean::new(RATE, v);
        let (mut l, mut r) = (x.to_vec(), x.to_vec());
        for (a, b) in l.chunks_mut(300).zip(r.chunks_mut(300)) {
            m.process(a, b);
        }
        assert_eq!(l, r);
        (l, m)
    }

    fn energy(x: &[f32]) -> f32 {
        x.iter().map(|s| s * s).sum()
    }

    #[test]
    fn value_texts_and_typed_values() {
        assert_eq!(text(SENSITIVITY, 50), "50%");
        assert_eq!(text(LENGTH, 30), "3.0ms");
        assert_eq!(text(LOW, 700), "700Hz");
        assert_eq!(parse(LENGTH, "2.5 ms"), Some(25));
        assert_eq!(parse(LOW, "1.2k"), Some(1200));
        assert_eq!(parse(LISTEN, "on"), Some(1));
    }

    /// With nothing to clean the output is the input, delayed by the reported latency.
    #[test]
    fn a_clean_voice_passes_unchanged_after_the_latency() {
        let x = voice(RATE as usize);
        let (y, m) = run(&START, &x);
        let d = m.delay();
        assert_eq!(m.cleaned, 0, "no clicks found in a clean voice");
        let err = x[..x.len() - d].iter().zip(&y[d..]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-4, "{err}");
    }

    /// Clicks on the voice are found and mostly removed; the voice between them is kept.
    #[test]
    fn clicks_go_and_the_voice_stays() {
        let clean = voice(RATE as usize);
        let mut x = clean.clone();
        let at = add_clicks(&mut x, 4000, 3100, 0.5);
        let (y, m) = run(&START, &x);
        let d = m.delay();
        assert!(m.cleaned >= at.len() as u64, "{} frames for {} clicks", m.cleaned, at.len());
        // Around each click: what is left of the click energy over the clean voice.
        let (mut before, mut after) = (0.0, 0.0);
        for &t in &at[..at.len() - 1] {
            let span = t - 200..t + 400;
            let noise_in: Vec<f32> = span.clone().map(|i| x[i] - clean[i]).collect();
            let noise_out: Vec<f32> = span.clone().map(|i| y[i + d] - clean[i]).collect();
            before += energy(&noise_in);
            after += energy(&noise_out);
        }
        assert!(after < before * 0.25, "click energy {before} -> {after}");
        // Far from the clicks the voice is untouched.
        let quiet = 1500..2500;
        let err = quiet.map(|i| (y[i + d] - clean[i]).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-4, "{err}");
    }

    /// A slurp: a quick, uneven run of small crackles over a word. Most of it goes, the word stays.
    #[test]
    fn a_slurp_is_cleaned_crackle_by_crackle() {
        let clean = voice(RATE as usize / 2);
        let mut x = clean.clone();
        let (mut t, mut seed) = (9000usize, 7u32);
        let mut at = Vec::new();
        while t < 9000 + 2400 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let amp = 0.15 + 0.2 * ((seed >> 16) as f32 / 65536.0);
            for i in 0..8 {
                x[t + i] += amp * if (seed >> i) & 1 == 0 { 1.0 } else { -1.0 } * (-(i as f32) / 2.5).exp();
            }
            at.push(t);
            t += 60 + (seed >> 24) as usize; // 1.3 to 6.6 ms apart
        }
        let (y, m) = run(&START, &x);
        let d = m.delay();
        let span = 8800..11600;
        let before = energy(&span.clone().map(|i| x[i] - clean[i]).collect::<Vec<_>>());
        let after = energy(&span.clone().map(|i| y[i + d] - clean[i]).collect::<Vec<_>>());
        assert!(after < before * 0.35, "{} crackles: {before} -> {after}", at.len());
        // The word under it is still there: the cleaned span keeps the voice's energy.
        let voice_e = energy(&clean[span.clone()]);
        let kept = energy(&span.map(|i| y[i + d]).collect::<Vec<_>>());
        assert!(kept > voice_e * 0.8, "{kept} of {voice_e}");
    }

    /// A long burst (a consonant) is not taken for a click.
    #[test]
    fn consonants_are_kept() {
        let mut x = voice(RATE as usize / 2);
        let mut seed = 1u32;
        for t in 8000..8000 + 960 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            x[t] += ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.4;
        }
        let (_, m) = run(&START, &x);
        assert_eq!(m.cleaned, 0);
    }

    /// Listen plays what is taken out: the clicks, not the voice.
    #[test]
    fn listen_plays_what_is_taken_out() {
        let clean = voice(RATE as usize / 2);
        let mut x = clean.clone();
        add_clicks(&mut x, 4000, 3100, 0.5);
        let mut v = START;
        v[LISTEN] = 1;
        let (y, _) = run(&v, &x);
        assert!(energy(&y) > 0.01 && energy(&y) < energy(&clean) * 0.05, "{} vs {}", energy(&y), energy(&clean));
    }
}
