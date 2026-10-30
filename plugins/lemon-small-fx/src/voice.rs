// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Voice: Lemon Studio's vocal enhancer as a plugin. It makes a home or phone recording sound closer to a studio
//! take: it lowers the background noise and the room, evens out the tone of a mic too close (boomy) or too far (thin,
//! distant), and brings the level up to a steady one.
//!
//! First, the AI denoise ([`crate::neural`]): DeepFilterNet 3, a neural network that keeps the voice and takes the
//! noise away, however it sounds. Then classic speech enhancement, light enough for any computer:
//! - The sound is analysed in overlapping frames (about 21 ms, a quarter apart) and every frequency gets a gain.
//! - Noise: the noise floor of each frequency is tracked as the minimum of its smoothed level over the last 1.5 s
//!   (minimum statistics: speech pauses often, noise does not); the room's late reverberation is predicted from the
//!   level 50 ms earlier decaying at the room's rate, where the level is falling (a held note is not a tail). Each
//!   frequency's gain is the Wiener gain for what stands above both, its signal-to-noise ratio judged decision-directed
//!   (Ephraim and Malah) so the gains move smoothly and leave no "musical" chirps; it never goes below the floor that
//!   denoise and room set.
//! - Tone: while a voice is heard, its long-term spectrum in third octaves is learned (over a few seconds) and an EQ
//!   steers it towards the smooth spectrum of a studio vocal (natural, broadcast or bright): a mic too close has too
//!   much low end and is cut there, one too far lacks presence and is lifted. At most +6 / -9 dB, slowly.
//! - Level: a leveller brings the voice towards -20 dBFS (RMS), moving only while the voice is heard, so pauses and
//!   noise are not pumped up; a soft limit keeps the peaks under full scale.
//! - Both channels get the same gains, so the stereo image stays. The latency (one frame) is reported to the host.

use std::collections::VecDeque;

use nih_plug::prelude::*;
use os_ui::Rect;

use crate::neural::Denoiser;
use crate::params::leading_number;
use crate::spectral::{hann, Fft};
use crate::{Card, Control, Kind, Mark, Setting, SmallFx, Sound, Spec};

pub const DENOISE: usize = 0;
pub const ROOM: usize = 1;
pub const TONE: usize = 2;
pub const LEVEL: usize = 3;
pub const TARGET: usize = 4;
pub const AI: usize = 5;

/// The settings: denoise, room, tone and level in %, the target sound ([`TARGETS`]) and the AI denoise in % (last, so
/// that the others keep their places).
pub const SETTINGS: [Setting; 6] = [
    Setting { id: "denoise", name: "Denoise", range: (0, 100), start: 20, magnet: None },
    Setting { id: "room", name: "Room", range: (0, 100), start: 30, magnet: None },
    Setting { id: "tone", name: "Tone", range: (0, 100), start: 60, magnet: None },
    Setting { id: "level", name: "Level", range: (0, 100), start: 50, magnet: None },
    Setting { id: "target", name: "Sound", range: (0, 2), start: 0, magnet: None },
    Setting { id: "ai", name: "AI denoise", range: (0, 100), start: 70, magnet: None },
];

/// The target sounds: name, the fall above 500 Hz (dB an octave), a presence lift around 3.5 kHz and an air lift
/// above 8 kHz (dB).
pub const TARGETS: [(&str, f32, f32, f32); 3] = [("Natural", 4.5, 0.0, 0.0), ("Broadcast", 4.0, 2.5, 0.0), ("Bright", 3.0, 1.5, 3.0)];

const CARDS: [Card; 6] = [
    Card { x0: 8.0, x1: 100.0, y0: 28.0, y1: 136.0, heading: "AI DENOISE" },
    Card { x0: 106.0, x1: 198.0, y0: 28.0, y1: 136.0, heading: "DENOISE" },
    Card { x0: 204.0, x1: 296.0, y0: 28.0, y1: 136.0, heading: "ROOM" },
    Card { x0: 302.0, x1: 394.0, y0: 28.0, y1: 136.0, heading: "TONE" },
    Card { x0: 400.0, x1: 492.0, y0: 28.0, y1: 136.0, heading: "LEVEL" },
    Card { x0: 498.0, x1: 590.0, y0: 28.0, y1: 136.0, heading: "SOUND" },
];

const CONTROLS: [Control; 6] = [
    Control::Knob { id: "ai", cx: 54.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "denoise", cx: 152.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "room", cx: 250.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "tone", cx: 348.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "level", cx: 446.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Choice { id: "target", x: 516.0, top: 56.0, names: &["NATURAL", "BROADCAST", "BRIGHT"] },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Voice",
    word: "VOICE",
    about: "Lemon Studio's vocal enhancer",
    settings: &SETTINGS,
    version: Some(1),
    any_version: false,
    fields: &[],
    tail: &[],
    blank: None,
    locate: None,
    size: (598, 174),
    cards: &CARDS,
    controls: &CONTROLS,
    display: None,
    mark: Mark { x: 590.0, y: 155.0, anchor: 1.0, stacked: false },
    hint: Some(Rect::new(8.0, 144.0, 582.0, 22.0)),
    labels: &[],
    text,
    parse,
    sound,
    rings: false,
};

fn text(k: usize, v: i32) -> String {
    match k {
        TARGET => TARGETS[v.clamp(0, 2) as usize].0.into(),
        AI if v <= 0 => "Off".into(),
        _ => format!("{v}%"),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let (min, max) = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    match k {
        TARGET => TARGETS.iter().position(|t| !low.is_empty() && t.0.to_ascii_lowercase().starts_with(&low)).map(|i| i as i32),
        AI if low == "off" => Some(0),
        _ => leading_number(&low).map(|x| (x.round() as i32).clamp(min, max)),
    }
}

/// Third-octave bands from 63 Hz (band `b` is centred on `63 * 2^(b/3)` Hz) to 16 kHz.
const BANDS: usize = 25;
/// How the smoothed level follows (per frame), and the noise floor's search: sub-windows (s) and how many.
const SMOOTH: f32 = 0.9;
const SUB_S: f32 = 0.19;
const SUBS: usize = 8;
/// The minimum of a smoothed level sits below the mean noise level by about this much.
const BIAS: f32 = 1.5;
/// How long after the direct sound the late reverberation counts (s), and the decision-directed weight.
const LATE_S: f32 = 0.05;
const DD: f32 = 0.98;
/// The deepest cut at full denoise or room (dB).
const DEEPEST: f32 = 25.0;
/// A frame is voice when it stands this far over the noise (power ratio).
const VOICE_SNR: f32 = 4.0;
/// A frame below this (its power over its length) is digital silence, and the frames the floor search waits after it
/// while the smoothed level settles.
const SILENT: f32 = 1e-9;
const SETTLE: usize = 10;
/// The tone EQ: how long the spectrum is learned over (s), how long it takes to move (s), its limits (dB) and how
/// much voice it hears before it acts fully (s).
const LEARN_S: f32 = 4.0;
const EQ_MOVE_S: f32 = 1.0;
const EQ_UP: f32 = 6.0;
const EQ_DOWN: f32 = 9.0;
const EQ_SURE_S: f32 = 2.0;
/// The leveller: its target (RMS), range (dB), how it measures (s) and moves (s); the soft limit's knee.
const LEVEL_TARGET: f32 = 0.1;
const LEVEL_RANGE: f32 = 12.0;
const LEVEL_RMS_S: f32 = 0.3;
const LEVEL_MOVE_S: f32 = 0.5;
const KNEE: f32 = 0.891;

fn band_hz(b: usize) -> f32 {
    63.0 * 2f32.powf(b as f32 / 3.0)
}

/// The target spectrum (dB) at `f` Hz for target `t`: flat 120 to 500 Hz, falling below and above.
fn target_db(f: f32, t: usize) -> f32 {
    let (_, fall, presence, air) = TARGETS[t.min(2)];
    let low = if f < 120.0 { -12.0 * (120.0 / f).log2() } else { 0.0 };
    let high = if f > 500.0 { -fall * (f / 500.0).log2() } else { 0.0 };
    let p = presence * (-(f / 3500.0).log2().powi(2) * 2.0).exp();
    let a = air * ((f / 8000.0).log2() * 3.0).clamp(0.0, 1.0);
    low + high + p + a
}

pub struct VoiceFix {
    rate: f32,
    v: [i32; 6],
    /// The AI denoise, before the rest.
    ai: Denoiser,
    n: usize,
    hop: usize,
    fft: Fft,
    win: Vec<f32>,
    /// The last `n` input samples per channel (a ring, the oldest at `at`), and samples taken since the last frame.
    hist: [Vec<f32>; 2],
    at: usize,
    fill: usize,
    acc: [Vec<f32>; 2],
    out: VecDeque<[f32; 2]>,
    re: [Vec<f32>; 2],
    im: [Vec<f32>; 2],
    power: Vec<f32>,
    smooth: Vec<f32>,
    /// The noise floor search: the minimum of the current sub-window, the last sub-windows' minima, frames into it.
    cur_min: Vec<f32>,
    mins: VecDeque<Vec<f32>>,
    sub_at: usize,
    sub_len: usize,
    noise: Vec<f32>,
    /// Smoothed levels of the last frames (for the late reverberation), oldest first.
    past: VecDeque<Vec<f32>>,
    late: usize,
    /// The last frame's gain squared times its signal-to-noise ratio (decision-directed), and the gains.
    prior: Vec<f32>,
    gain: Vec<f32>,
    /// Frames seen (the noise search warms up), whether the last frame was voice.
    frames: usize,
    voice: bool,
    /// Whether the last frame was digital silence, and frames to go before the floor is searched again after it.
    quiet: bool,
    settle: usize,
    /// The tone EQ: each bin's band position, the learned spectrum per band, voice heard (s), the EQ (dB).
    pos: Vec<f32>,
    ltas: [f32; BANDS],
    heard: f32,
    eq: [f32; BANDS],
    /// The leveller: the voice's mean square and the gain (dB).
    env: f32,
    lvl: f32,
}

impl VoiceFix {
    pub fn new(rate: u32, v: &[i32]) -> VoiceFix {
        VoiceFix::with(rate, v, Denoiser::new(rate, v[AI]))
    }

    /// With the AI denoise `ai` (its model kept across a new sample rate or a reset).
    fn with(rate: u32, v: &[i32], ai: Denoiser) -> VoiceFix {
        let rate = rate.max(1);
        let n = (1024 * (rate as usize).div_ceil(48000)).next_power_of_two();
        let hop = n / 4;
        let bins = n / 2 + 1;
        let r = rate as f32;
        let sub_len = ((SUB_S * r / hop as f32).round() as usize).max(1);
        let late = ((LATE_S * r / hop as f32).round() as usize).max(1);
        let pos = (0..bins)
            .map(|k| {
                let f = (k as f32 * r / n as f32).max(1.0);
                (3.0 * (f / 63.0).log2()).clamp(0.0, (BANDS - 1) as f32)
            })
            .collect();
        let mut s = VoiceFix {
            rate: r,
            v: [0; 6],
            ai,
            n,
            hop,
            fft: Fft::new(n),
            win: hann(n).into_iter().map(f32::sqrt).collect(),
            hist: [vec![0.0; n], vec![0.0; n]],
            at: 0,
            fill: 0,
            acc: [vec![0.0; n], vec![0.0; n]],
            out: VecDeque::from(vec![[0.0; 2]; hop - 1]),
            re: [vec![0.0; n], vec![0.0; n]],
            im: [vec![0.0; n], vec![0.0; n]],
            power: vec![0.0; bins],
            smooth: vec![0.0; bins],
            cur_min: vec![f32::MAX; bins],
            mins: VecDeque::new(),
            sub_at: 0,
            sub_len,
            noise: vec![0.0; bins],
            past: VecDeque::new(),
            late,
            prior: vec![1.0; bins],
            gain: vec![1.0; bins],
            frames: 0,
            voice: false,
            quiet: true,
            settle: SETTLE,
            pos,
            ltas: [0.0; BANDS],
            heard: 0.0,
            eq: [0.0; BANDS],
            env: 0.0,
            lvl: 0.0,
        };
        s.v.copy_from_slice(&v[..6]);
        s
    }

    /// The latency (samples): the AI denoise's, then one frame.
    pub fn delay(&self) -> usize {
        self.ai.latency() + self.n - 1
    }

    /// One frame: analyse, work out the gains, apply them and add the frame back.
    fn frame(&mut self) {
        let (n, bins) = (self.n, self.n / 2 + 1);
        for ch in 0..2 {
            for i in 0..n {
                self.re[ch][i] = self.hist[ch][(self.at + i) % n] * self.win[i];
                self.im[ch][i] = 0.0;
            }
            self.fft.run(&mut self.re[ch], &mut self.im[ch], false);
        }
        for k in 0..bins {
            let p = |ch: usize| self.re[ch][k] * self.re[ch][k] + self.im[ch][k] * self.im[ch][k];
            self.power[k] = 0.5 * (p(0) + p(1));
        }
        self.track_noise();
        self.gains();
        self.tone();
        for ch in 0..2 {
            for k in 0..bins {
                let g = self.gain[k];
                self.re[ch][k] *= g;
                self.im[ch][k] *= g;
                if k > 0 && k < n / 2 {
                    self.re[ch][n - k] *= g;
                    self.im[ch][n - k] *= g;
                }
            }
            self.fft.run(&mut self.re[ch], &mut self.im[ch], true);
            // Square-root Hann both ways: the windows a quarter apart add to 2.
            let scale = 0.5 / n as f32;
            for i in 0..n {
                self.acc[ch][i] += self.re[ch][i] * self.win[i] * scale;
            }
        }
        for i in 0..self.hop {
            self.out.push_back([self.acc[0][i], self.acc[1][i]]);
        }
        for ch in 0..2 {
            self.acc[ch].copy_within(self.hop.., 0);
            let len = self.acc[ch].len();
            self.acc[ch][len - self.hop..].fill(0.0);
        }
    }

    /// The noise floor (minimum statistics) and whether this frame is voice.
    fn track_noise(&mut self) {
        self.frames += 1;
        // Digital silence is not searched for the floor (the first frames still hold the silence before the start, the
        // AI denoise's delay is silent at first, a part can be muted), nor are the frames after it while the smoothed
        // level settles: a floor of nothing would take every noise for the voice. The level starts again from the
        // first frame after it.
        let p = self.power.iter().sum::<f32>();
        let quiet = self.frames * self.hop < self.n || p <= SILENT * self.n as f32;
        let search = !quiet && self.settle == 0;
        for k in 0..self.power.len() {
            let s = if quiet || self.quiet { self.power[k] } else { SMOOTH * self.smooth[k] + (1.0 - SMOOTH) * self.power[k] };
            self.smooth[k] = s;
            if search {
                self.cur_min[k] = self.cur_min[k].min(s);
            }
        }
        self.settle = if quiet { SETTLE } else { self.settle.saturating_sub(1) };
        self.quiet = quiet;
        self.sub_at += 1;
        if self.sub_at >= self.sub_len {
            self.sub_at = 0;
            self.mins.push_back(self.cur_min.clone());
            if self.mins.len() > SUBS {
                self.mins.pop_front();
            }
            if search {
                self.cur_min.copy_from_slice(&self.smooth);
            } else {
                self.cur_min.fill(f32::MAX);
            }
        }
        for k in 0..self.noise.len() {
            let m = self.mins.iter().fold(self.cur_min[k], |m, w| m.min(w[k]));
            self.noise[k] = if m == f32::MAX { self.smooth[k] } else { BIAS * m };
        }
        let q = self.noise.iter().sum::<f32>();
        self.voice = !quiet && p > VOICE_SNR * q;
        let mut keep = if self.past.len() >= self.late { self.past.pop_front().unwrap_or_default() } else { Vec::new() };
        keep.clear();
        keep.extend_from_slice(&self.smooth);
        self.past.push_back(keep);
    }

    /// The gains for noise and room (Wiener, decision-directed), smoothed across frequency.
    fn gains(&mut self) {
        let (denoise, room) = (self.v[DENOISE].clamp(0, 100) as f32 / 100.0, self.v[ROOM].clamp(0, 100) as f32 / 100.0);
        let d = if denoise > 0.0 { 0.5 + 1.5 * denoise } else { 0.0 };
        // The room's reverberation time grows with the setting; its tail `late` frames on has decayed by `fade`.
        let rt60 = 0.3 + 0.7 * room;
        let fade = (-2.0 * 3.0 * std::f32::consts::LN_10 / rt60 * (self.late * self.hop) as f32 / self.rate).exp();
        let r = room;
        let floor = 10f32.powf(-DEEPEST * denoise.max(room) / 20.0);
        // The noise search needs its window filled before it is trusted.
        let warm = (self.frames as f32 / (SUBS * self.sub_len) as f32).min(1.0);
        let full = self.past.len() >= self.late;
        for k in 0..self.power.len() {
            // Reverberation only decays: where the level holds or rises it is the voice itself (a held vowel or note),
            // and only where it falls at the room's rate is it taken for the tail.
            let tail = if full {
                let then = self.past[0][k];
                let falling = if then > 1e-20 { ((1.0 - self.smooth[k] / then) / (1.0 - fade)).clamp(0.0, 1.0) } else { 0.0 };
                fade * then * falling
            } else {
                0.0
            };
            let lambda = d * self.noise[k] + r * tail;
            if lambda <= 1e-20 {
                (self.gain[k], self.prior[k]) = (1.0, 1.0);
                continue;
            }
            let gamma = self.power[k] / lambda;
            let xi = DD * self.prior[k] + (1.0 - DD) * (gamma - 1.0).max(0.0);
            let g = (xi / (1.0 + xi)).max(floor);
            self.prior[k] = g * g * gamma;
            self.gain[k] = 1.0 - warm * (1.0 - g);
        }
        let mut before = self.gain[0];
        for k in 1..self.gain.len() - 1 {
            let here = self.gain[k];
            self.gain[k] = 0.25 * before + 0.5 * here + 0.25 * self.gain[k + 1];
            before = here;
        }
    }

    /// The tone EQ: learn the voice's spectrum, steer it towards the target, fold the EQ into the gains.
    fn tone(&mut self) {
        let amount = self.v[TONE].clamp(0, 100) as f32 / 100.0;
        let secs = self.hop as f32 / self.rate;
        if self.voice {
            let mut sum = [0.0f32; BANDS];
            let mut count = [0usize; BANDS];
            for (k, &p) in self.power.iter().enumerate().skip(1) {
                let b = self.pos[k].round() as usize;
                let f = k as f32 * self.rate / self.n as f32;
                if (56.0..18000.0).contains(&f) {
                    sum[b] += p;
                    count[b] += 1;
                }
            }
            let keep = (-secs / LEARN_S).exp();
            let first = self.heard == 0.0;
            for b in 0..BANDS {
                let m = if count[b] > 0 { sum[b] / count[b] as f32 } else { 0.0 };
                self.ltas[b] = if first { m } else { keep * self.ltas[b] + (1.0 - keep) * m };
            }
            self.heard += secs;
        }
        let sure = (self.heard / EQ_SURE_S).min(1.0);
        let target = self.v[TARGET].clamp(0, 2) as usize;
        let have: [f32; BANDS] = std::array::from_fn(|b| 10.0 * (self.ltas[b] + 1e-20).log10());
        let want: [f32; BANDS] = std::array::from_fn(|b| target_db(band_hz(b), target));
        // Only the shape matters: both lined up on the voice's core (250 Hz to 4 kHz).
        let core = || (0..BANDS).filter(|&b| (250.0..=4000.0).contains(&band_hz(b)));
        let offset = core().map(|b| want[b] - have[b]).sum::<f32>() / core().count() as f32;
        let loudest = have.iter().copied().fold(f32::MIN, f32::max);
        let step = (secs / EQ_MOVE_S).min(1.0);
        for b in 0..BANDS {
            // Bands where the voice has next to nothing are left alone rather than lifted into the noise.
            let to = if have[b] < loudest - 50.0 { 0.0 } else { (want[b] - have[b] - offset).clamp(-EQ_DOWN, EQ_UP) * amount * sure };
            self.eq[b] += (to - self.eq[b]) * step;
        }
        if amount > 0.0 || self.eq.iter().any(|e| e.abs() > 1e-3) {
            for k in 0..self.gain.len() {
                let p = self.pos[k];
                let (lo, frac) = (p.floor() as usize, p.fract());
                let hi = (lo + 1).min(BANDS - 1);
                let db = self.eq[lo] * (1.0 - frac) + self.eq[hi] * frac;
                self.gain[k] *= 10f32.powf(db / 20.0);
            }
        }
    }
}

impl Sound for VoiceFix {
    fn set(&mut self, v: &[i32]) {
        self.v.copy_from_slice(&v[..6]);
        self.ai.set(v[AI]);
    }

    fn set_sample_rate(&mut self, rate: u32) {
        let ai = self.ai.renew(rate);
        *self = VoiceFix::with(rate, &self.v.clone(), ai);
    }

    fn reset(&mut self) {
        let rate = self.rate as u32;
        let ai = self.ai.renew(rate);
        *self = VoiceFix::with(rate, &self.v.clone(), ai);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.ai.process(left, right);
        let level = self.v[LEVEL].clamp(0, 100) as f32 / 100.0;
        let rms_follow = 1.0 - (-1.0 / (LEVEL_RMS_S * self.rate)).exp();
        let move_follow = 1.0 - (-1.0 / (LEVEL_MOVE_S * self.rate)).exp();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            (self.hist[0][self.at], self.hist[1][self.at]) = (*l, *r);
            self.at = (self.at + 1) % self.n;
            self.fill += 1;
            if self.fill == self.hop {
                self.fill = 0;
                self.frame();
            }
            let [a, b] = self.out.pop_front().unwrap_or([0.0; 2]);
            // The leveller measures and moves only while the voice is heard.
            if self.voice {
                self.env += (0.5 * (a * a + b * b) - self.env) * rms_follow;
                let to = if self.env > 1e-12 { (20.0 * (LEVEL_TARGET / self.env.sqrt()).log10()).clamp(-LEVEL_RANGE, LEVEL_RANGE) } else { 0.0 };
                self.lvl += (to * level - self.lvl) * move_follow;
            }
            if level <= 0.0 && self.lvl.abs() < 1e-4 {
                (*l, *r) = (a, b);
                continue;
            }
            let g = 10f32.powf(self.lvl / 20.0);
            *l = soft_limit(a * g);
            *r = soft_limit(b * g);
        }
    }

    fn latency(&self) -> u32 {
        self.delay() as u32
    }
}

/// Unchanged up to the knee, then bending smoothly towards full scale.
fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let room = 1.0 - KNEE;
    x.signum() * (KNEE + room * ((a - KNEE) / room).tanh())
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(VoiceFix::new(rate, v))
}

pub struct VoiceKind;

impl Kind for VoiceKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-voice";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's vocal enhancer";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Restoration, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioVoice";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Restoration];
}

pub type LemonVoice = SmallFx<VoiceKind>;

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48000;
    const OFF: [i32; 6] = [0, 0, 0, 0, 0, 0];

    fn rng(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / (1 << 24) as f32 - 0.5
    }

    /// A voice-like buzz (150 Hz, harmonics through a gentle tilt), `on` for its first half of every `period`
    /// seconds when `gated`.
    fn voice(len: usize, gated: bool, period: f32) -> Vec<f32> {
        (0..len)
            .map(|t| {
                let s = t as f32 / RATE as f32;
                let on = !gated || (s % period) < period / 2.0;
                if !on {
                    return 0.0;
                }
                let f0 = 150.0 * (1.0 + 0.02 * (std::f32::consts::TAU * 3.0 * s).sin());
                (1..40).map(|h| (std::f32::consts::TAU * f0 * h as f32 * s).sin() / h as f32).sum::<f32>() * 0.1
            })
            .collect()
    }

    fn run(v: &[i32], x: &[f32]) -> (Vec<f32>, usize) {
        run_at(RATE, v, x)
    }

    fn run_at(rate: u32, v: &[i32], x: &[f32]) -> (Vec<f32>, usize) {
        let mut s = VoiceFix::new(rate, v);
        let (mut l, mut r) = (x.to_vec(), x.to_vec());
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            s.process(a, b);
        }
        assert_eq!(l, r);
        (l, s.delay())
    }

    fn energy(x: &[f32]) -> f32 {
        x.iter().map(|s| s * s).sum()
    }

    fn db(a: f32, b: f32) -> f32 {
        10.0 * (a / b).log10()
    }

    /// The energy of `x` in `lo..hi` Hz (a plain DFT over a few frames).
    fn band(x: &[f32], lo: f32, hi: f32) -> f32 {
        let n = 4096;
        let f = Fft::new(n);
        let w = hann(n);
        let mut e = 0.0;
        for start in (0..x.len().saturating_sub(n)).step_by(n) {
            let (mut re, mut im): (Vec<f32>, Vec<f32>) = (x[start..start + n].iter().zip(&w).map(|(a, b)| a * b).collect(), vec![0.0; n]);
            f.run(&mut re, &mut im, false);
            for k in 1..n / 2 {
                let hz = k as f32 * RATE as f32 / n as f32;
                if (lo..hi).contains(&hz) {
                    e += re[k] * re[k] + im[k] * im[k];
                }
            }
        }
        e
    }

    #[test]
    fn value_texts_and_typed_values() {
        assert_eq!(text(DENOISE, 50), "50%");
        assert_eq!(text(AI, 0), "Off");
        assert_eq!(parse(AI, "off"), Some(0));
        assert_eq!(text(TARGET, 1), "Broadcast");
        assert_eq!(parse(TARGET, "bri"), Some(2));
        assert_eq!(parse(LEVEL, "150"), Some(100));
        assert!(target_db(3500.0, 1) > target_db(3500.0, 0));
        assert!(target_db(60.0, 0) < -8.0);
    }

    /// With everything at zero the output is the input, delayed by the latency.
    #[test]
    fn at_zero_it_passes_unchanged() {
        let x = voice(RATE as usize, false, 1.0);
        let (y, d) = run(&OFF, &x);
        let err = x[..x.len() - d].iter().zip(&y[d..]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-4, "{err}");
    }

    /// Steady noise under a voice that pauses: the pauses get much quieter, the voice keeps nearly all of itself.
    #[test]
    fn noise_goes_and_the_voice_stays() {
        let len = 5 * RATE as usize;
        let clean = voice(len, true, 1.0);
        let mut seed = 5u32;
        let noise: Vec<f32> = (0..len).map(|_| 0.02 * rng(&mut seed)).collect();
        let x: Vec<f32> = clean.iter().zip(&noise).map(|(a, b)| a + b).collect();
        let mut v = OFF;
        v[DENOISE] = 70;
        let (y, d) = run(&v, &x);
        // The last two seconds: pauses (second halves) and the voice (first halves).
        let (mut gap_in, mut gap_out, mut on_clean, mut on_err) = (0.0, 0.0, 0.0, 0.0);
        for t in 3 * RATE as usize..len - d {
            let in_pause = (t as f32 / RATE as f32) % 1.0 > 0.55 && (t as f32 / RATE as f32) % 1.0 < 0.95;
            let in_voice = (t as f32 / RATE as f32) % 1.0 > 0.05 && (t as f32 / RATE as f32) % 1.0 < 0.45;
            if in_pause {
                gap_in += x[t] * x[t];
                gap_out += y[t + d] * y[t + d];
            }
            if in_voice {
                on_clean += clean[t] * clean[t];
                on_err += (y[t + d] - clean[t]).powi(2);
            }
        }
        println!("noise {:.1} dB, voice error {:.1} dB", db(gap_out, gap_in), db(on_err, on_clean));
        assert!(db(gap_out, gap_in) < -12.0, "noise down only {} dB", db(gap_out, gap_in));
        assert!(db(on_err, on_clean) < -12.0, "voice changed: error {} dB", db(on_err, on_clean));
    }

    /// A voice in a reverberant room: the tails in the pauses get quieter, the voice itself stays.
    #[test]
    fn the_room_goes_down() {
        let len = 4 * RATE as usize;
        let dry = voice(len, true, 0.6);
        // A room: decaying noise, 0.6 s to fall 60 dB, about as loud as the direct sound.
        let mut seed = 9u32;
        let ir: Vec<f32> = (0..(0.6 * RATE as f32) as usize).map(|t| 0.02 * rng(&mut seed) * (-6.9 * t as f32 / (0.6 * RATE as f32)).exp()).collect();
        let mut x = dry.clone();
        for (t, &s) in dry.iter().enumerate().filter(|(_, s)| **s != 0.0).step_by(1) {
            for (j, &h) in ir.iter().enumerate().step_by(8) {
                if t + j < len {
                    x[t + j] += 8.0 * s * h;
                }
            }
        }
        let mut v = OFF;
        v[ROOM] = 80;
        let (y, d) = run(&v, &x);
        let (mut tail_in, mut tail_out, mut on_dry, mut on_out) = (0.0, 0.0, 0.0, 0.0);
        for t in 2 * RATE as usize..len - d {
            let p = (t as f32 / RATE as f32) % 0.6;
            if p > 0.35 && p < 0.55 {
                tail_in += x[t] * x[t];
                tail_out += y[t + d] * y[t + d];
            }
            if p > 0.05 && p < 0.25 {
                on_dry += dry[t] * dry[t];
                on_out += y[t + d] * y[t + d];
            }
        }
        // The voice keeps (about) its own level: what is taken out is the room on top of it.
        println!("tail {:.1} dB, voice {:.1} dB of the dry voice", db(tail_out, tail_in), db(on_out, on_dry));
        assert!(db(tail_out, tail_in) < -6.0, "tail down only {} dB", db(tail_out, tail_in));
        assert!(db(on_out, on_dry) > -3.0, "voice down {} dB", db(on_out, on_dry));
    }

    /// A mic too close (a voice with +12 dB of boom below 200 Hz): the tone EQ takes most of the boom back out.
    #[test]
    fn a_boomy_close_mic_is_evened_out() {
        let len = 6 * RATE as usize;
        let x0 = voice(len, false, 1.0);
        // Boom: the voice's low part added again, three times over (a one-pole low-pass at 200 Hz).
        let c = 1.0 - (-std::f32::consts::TAU * 200.0 / RATE as f32).exp();
        let mut lp = 0.0;
        let x: Vec<f32> = x0
            .iter()
            .map(|&s| {
                lp += (s - lp) * c;
                s + 3.0 * lp
            })
            .collect();
        let mut v = OFF;
        v[TONE] = 100;
        let (y, d) = run(&v, &x);
        let ratio = |z: &[f32]| db(band(z, 60.0, 250.0), band(z, 500.0, 2000.0));
        let (before, after) = (ratio(&x[4 * RATE as usize..len - d]), ratio(&y[4 * RATE as usize + d..]));
        println!("boom {before:.1} -> {after:.1} dB");
        assert!(after < before - 4.0, "boom {before} -> {after}");
    }

    /// A quiet voice is brought up towards the target level; noise alone is not.
    #[test]
    fn a_quiet_voice_is_levelled_up() {
        let len = 4 * RATE as usize;
        let x: Vec<f32> = voice(len, false, 1.0).iter().map(|s| s * 0.1).collect();
        let mut v = OFF;
        v[LEVEL] = 100;
        let (y, d) = run(&v, &x);
        let from = 3 * RATE as usize;
        let up = db(energy(&y[from + d..]), energy(&x[from..len - d]));
        println!("voice up {up:.1} dB");
        assert!(up > 8.0, "{up} dB");
        let mut seed = 3u32;
        let hiss: Vec<f32> = (0..len).map(|_| 0.001 * rng(&mut seed)).collect();
        let (y, d) = run(&v, &hiss);
        let up = db(energy(&y[from + d..]), energy(&hiss[from..len - d]));
        assert!(up < 1.0, "noise raised {up} dB");
    }

    /// Speech-like syllables (a glottal buzz through the formants of five vowels, four syllables a second) in the first
    /// half of every second, under steady noise at `rate`: the voice, and the voice with the noise.
    fn noisy(rate: u32, secs: usize, noise: f32) -> (Vec<f32>, Vec<f32>) {
        const VOWELS: [[f32; 3]; 5] = [[730.0, 1090.0, 2440.0], [270.0, 2290.0, 3010.0], [530.0, 1840.0, 2480.0], [300.0, 870.0, 2240.0], [570.0, 840.0, 2410.0]];
        let r = rate as f32;
        let len = secs * rate as usize;
        let mut clean = vec![0.0f32; len];
        let (mut phase, mut tilt) = (0.0f32, 0.0f32);
        let mut res = [[0.0f32; 2]; 3];
        let mut seed = 23u32;
        for (t, out) in clean.iter_mut().enumerate() {
            let s = t as f32 / r;
            let (sec, at) = (s.floor() as usize, s % 1.0);
            if at >= 0.5 {
                continue;
            }
            let syl = (at / 0.125) as usize;
            let k = at % 0.125 / 0.11;
            let env = if k < 1.0 { (std::f32::consts::PI * k).sin().powf(0.6) } else { 0.0 };
            let f0 = 140.0 * (1.0 + 0.03 * (std::f32::consts::TAU * 0.7 * s).sin() + 0.05 * (syl as f32 - 1.5) / 1.5);
            phase += f0 / r;
            let pulse = if phase >= 1.0 {
                phase -= 1.0;
                1.0
            } else {
                0.0
            };
            // A glottal pulse falls about 12 dB an octave; a little breath on top.
            tilt += (pulse + 0.02 * rng(&mut seed) - tilt) * (std::f32::consts::TAU * 300.0 / r).min(1.0);
            let f = VOWELS[(sec * 4 + syl) % VOWELS.len()];
            let mut y = 0.0;
            for (i, &hz) in f.iter().enumerate() {
                let (bw, rr) = (60.0 + 40.0 * i as f32, 0.0);
                let rr = rr + (-std::f32::consts::PI * bw / r).exp();
                let a1 = 2.0 * rr * (std::f32::consts::TAU * hz / r).cos();
                let a2 = -rr * rr;
                let z = a1 * res[i][0] + a2 * res[i][1] + tilt * (1.0 - rr) * [1.0, 0.6, 0.3][i];
                res[i] = [z, res[i][0]];
                y += z;
            }
            *out = 0.3 * env * y;
        }
        let peak = clean.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        clean.iter_mut().for_each(|v| *v *= 0.5 / peak);
        let mut seed = 11u32;
        let x = clean.iter().map(|c| c + noise * rng(&mut seed)).collect();
        (clean, x)
    }

    /// In the pauses (`pause`) or the voice: the input's and the output's energy, the clean voice's and the error's.
    fn measure(rate: u32, clean: &[f32], x: &[f32], y: &[f32], d: usize, from: usize) -> (f32, f32, f32, f32) {
        let (mut gap_in, mut gap_out, mut on_clean, mut on_err) = (0.0, 0.0, 0.0, 0.0);
        for t in from * rate as usize..x.len() - d {
            let p = (t as f32 / rate as f32) % 1.0;
            if p > 0.55 && p < 0.95 {
                gap_in += x[t] * x[t];
                gap_out += y[t + d] * y[t + d];
            }
            if p > 0.05 && p < 0.45 {
                on_clean += clean[t] * clean[t];
                on_err += (y[t + d] - clean[t]).powi(2);
            }
        }
        (gap_in, gap_out, on_clean, on_err)
    }

    /// The AI denoise alone, at 48 and 44.1 kHz: the noise in the pauses goes far down, the voice keeps most of its
    /// level (the model reshapes a made-up voice more than a real one: `real_speech`), and the output lines up with the
    /// input at the reported latency.
    #[test]
    fn the_ai_takes_the_noise_away() {
        for rate in [48000u32, 44100] {
            let (clean, x) = noisy(rate, 5, 0.03);
            let mut v = OFF;
            v[AI] = 100;
            let (y, d) = run_at(rate, &v, &x);
            let (gap_in, gap_out, on_clean, on_err) = measure(rate, &clean, &x, &y, d, 2);
            let on_out = voice_energy(rate, &y, d, x.len());
            println!("{rate}: noise {:.1} dB, voice level {:.1} dB, error {:.1} dB", db(gap_out, gap_in), db(on_out, on_clean), db(on_err, on_clean));
            assert!(db(gap_out, gap_in) < -20.0, "{rate}: noise down only {} dB", db(gap_out, gap_in));
            assert!(db(on_out, on_clean) > -10.0, "{rate}: voice down {} dB", db(on_out, on_clean));
            // The voice comes out where the latency says: the best lag within 2 ms of it is the latency itself.
            let at = |lag: usize| (2 * rate as usize..x.len() - d - 200).map(|t| clean[t] * y[t + lag]).sum::<f32>();
            let best = (d - 100..=d + 100).max_by(|&a, &b| at(a).total_cmp(&at(b))).unwrap();
            assert!(best.abs_diff(d) <= 2, "{rate}: lined up at {best}, latency {d}");
        }
    }

    /// The output's energy where the voice is (as `measure` takes it).
    fn voice_energy(rate: u32, y: &[f32], d: usize, len: usize) -> f32 {
        (2 * rate as usize..len - d)
            .filter(|&t| {
                let p = (t as f32 / rate as f32) % 1.0;
                p > 0.05 && p < 0.45
            })
            .map(|t| y[t + d] * y[t + d])
            .sum()
    }

    /// Turned on while playing, the AI is built on a thread of its own and fades in; turned off, it fades out.
    #[test]
    fn the_ai_comes_in_while_playing() {
        let (_, x) = noisy(RATE, 1, 0.03);
        let mut s = VoiceFix::new(RATE, &OFF);
        let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
        let mut block = |s: &mut VoiceFix, at: usize| {
            let at = at % (x.len() - 480);
            l.copy_from_slice(&x[at..at + 480]);
            r.copy_from_slice(&x[at..at + 480]);
            s.process(&mut l, &mut r);
        };
        for n in 0..100 {
            block(&mut s, n * 480);
        }
        assert!(!s.ai.active());
        let mut v = OFF;
        v[AI] = 100;
        s.set(&v);
        let t = std::time::Instant::now();
        let mut n = 100;
        while !s.ai.active() {
            assert!(t.elapsed().as_secs() < 20, "the AI never came in");
            block(&mut s, n * 480);
            n += 1;
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        s.set(&OFF);
        for _ in 0..20 {
            block(&mut s, n * 480);
            n += 1;
        }
        assert!(!s.ai.active(), "the AI fades out");
    }

    /// A recording of speech (16-bit PCM WAV, `OS_VOICE_WAV`) under noise through the AI denoise alone and through
    /// the default Lemon Voice: what is left of the noise where nobody speaks and of the speech (`cargo test -p
    /// lemon-small-fx --release -- --ignored real_speech --nocapture`).
    #[test]
    #[ignore]
    fn real_speech() {
        let Some(path) = std::env::var_os("OS_VOICE_WAV") else { return };
        let b = std::fs::read(path).expect("reads");
        let rate = u32::from_le_bytes(b[24..28].try_into().unwrap());
        let chans = usize::from(u16::from_le_bytes(b[22..24].try_into().unwrap()));
        let data = b.windows(4).position(|w| w == b"data").expect("data") + 8;
        let clean: Vec<f32> = b[data..].chunks_exact(2 * chans).map(|f| f32::from(i16::from_le_bytes([f[0], f[1]])) / 32768.0 * 0.7).collect();
        // Where speech is (a 20 ms envelope over a tenth of its peak) and where it is not.
        let win = rate as usize / 50;
        let env: Vec<f32> = clean.chunks(win).map(|c| c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).collect();
        let top = env.iter().copied().fold(0.0f32, f32::max);
        let speech = |t: usize| env[t / win] > 0.01 * top;
        // A pause: no speech within 100 ms either way.
        let pause = |t: usize| {
            let w = t / win;
            env[w.saturating_sub(5)..(w + 6).min(env.len())].iter().all(|&e| e < 1e-5 * top)
        };
        let (n_speech, n_pause) = (0..clean.len()).fold((0, 0), |(a, b), t| (a + usize::from(speech(t)), b + usize::from(pause(t))));
        println!("{:.1} s of speech, {:.1} s of pauses", n_speech as f32 / rate as f32, n_pause as f32 / rate as f32);
        for (name, noise) in [("hiss", 0), ("hum and fan", 1)] {
            let mut seed = 7u32;
            let mut lp = 0.0f32;
            let x: Vec<f32> = clean
                .iter()
                .enumerate()
                .map(|(t, c)| {
                    let n = match noise {
                        0 => 0.02 * rng(&mut seed),
                        _ => {
                            lp += (rng(&mut seed) - lp) * 0.05;
                            let s = t as f32 / rate as f32;
                            0.01 * (std::f32::consts::TAU * 50.0 * s).sin() + 0.004 * (std::f32::consts::TAU * 150.0 * s).sin() + 0.15 * lp
                        }
                    };
                    c + n
                })
                .collect();
            let mut ai = OFF;
            ai[AI] = 100;
            let mut classic = OFF;
            classic[DENOISE] = 100;
            for (what, v) in [("AI denoise 100%", ai), ("classic denoise 100%", classic), ("Default", SPEC.start().try_into().unwrap())] {
                let (y, d) = run_at(rate, &v, &x);
                let (mut gap_in, mut gap_out, mut on_clean, mut on_out, mut on_err) = (0.0, 0.0, 0.0, 0.0, 0.0);
                for t in 2 * rate as usize..x.len() - d {
                    if speech(t) {
                        on_clean += clean[t] * clean[t];
                        on_out += y[t + d] * y[t + d];
                    } else if pause(t) {
                        gap_in += x[t] * x[t];
                        gap_out += y[t + d] * y[t + d];
                    }
                }
                // The error with the output at its best level against the speech (the leveller's gain left out).
                let g = (2 * rate as usize..x.len() - d).map(|t| clean[t] * y[t + d]).sum::<f32>() / on_out.max(1e-12);
                for t in (2 * rate as usize..x.len() - d).filter(|&t| speech(t)) {
                    on_err += (g * y[t + d] - clean[t]).powi(2);
                }
                let (in_err, _) = (2 * rate as usize..x.len() - d).filter(|&t| speech(t)).fold((0.0f32, 0), |(e, n), t| (e + (x[t] - clean[t]).powi(2), n + 1));
                println!(
                    "{name}, {what}: noise where nobody speaks {:.1} dB, speech level {:.1} dB, speech SNR {:.1} dB (was {:.1} dB)",
                    db(gap_out, gap_in),
                    db(on_out, on_clean),
                    db(on_clean, on_err),
                    db(on_clean, in_err)
                );
            }
        }
    }
}
