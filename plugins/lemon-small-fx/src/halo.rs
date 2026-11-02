// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Halo: where a vocal sits around the listener's head, on one dial, with a simple reverb. The dial runs Center
//! (dead in the middle of the head), Front (out ahead), Around (from all sides, outside the head) and Behind, and every
//! place between them, so it can be automated; it does the work of a Stereo Shaper, an EQ and a few short echoes at once.
//!
//! No reverb is needed for the places. What the dial moves:
//! - Width: the voice's highs go to the sides through a chain of all-passes (their phase turns a few times over the
//!   range, so each band of the voice leans another way) and in opposite polarity on the two sides. The more of that
//!   against the voice itself, the less the two ears agree: mono agrees fully (in the head), a quarter turn is diffuse
//!   (around), past that they mostly disagree, which is heard behind. The two parts are kept at a constant power, and
//!   the sides cancel in a mono sum, so a phone speaker still gets the voice. The bass stays in the middle.
//! - Tone: the directional bands (Blauert): more around 400 Hz and 3.8 kHz is heard as in front, more around 1.1 kHz and
//!   11 kHz as behind.
//! - Short echoes (7 to 30 ms, too short and few to be heard as a room), different on each side: they take the sound out
//!   of the head.
//!
//! The reverb is Lemon Reverb's plate behind the place, with two controls: how much, and how big.

use std::f32::consts::{PI, TAU};

use nih_plug::prelude::*;
use os_engine::effects::reverb::{self, Reverb};
use os_theme::design::lemon as brand;
use os_ui::kit::lemon::plug;
use os_ui::{Canvas, Font, Rect};

use crate::params::leading_number;
use crate::{Card, Control, Display, Kind, Mark, Setting, SmallFx, Sound, Spec, Stop};

pub const PLACE: usize = 0;
pub const AMOUNT: usize = 1;
pub const REVERB: usize = 2;
pub const SIZE: usize = 3;

/// The places on the dial, `STEP` apart from 0.
pub const PLACES: [&str; 4] = ["Center", "Front", "Around", "Behind"];
const STEP: i32 = 100;

/// The settings: the place, how much of it in %, the reverb's level and size in %.
pub const SETTINGS: [Setting; 4] = [
    Setting { id: "place", name: "Place", range: (0, 300), start: STEP, magnet: None },
    Setting { id: "amount", name: "Amount", range: (0, 100), start: 100, magnet: None },
    Setting { id: "reverb", name: "Reverb", range: (0, 100), start: 0, magnet: None },
    Setting { id: "size", name: "Size", range: (0, 100), start: 40, magnet: None },
];

const CARDS: [Card; 3] = [
    Card { x0: 8.0, x1: 378.0, y0: 28.0, y1: 242.0, heading: "PLACE" },
    Card { x0: 384.0, x1: 512.0, y0: 28.0, y1: 124.0, heading: "REVERB" },
    Card { x0: 384.0, x1: 512.0, y0: 146.0, y1: 242.0, heading: "SIZE" },
];

const CONTROLS: [Control; 4] = [
    Control::Dial {
        id: "place",
        cx: 282.0,
        cy: 100.0,
        r: 38.0,
        stops: &[
            Stop { value: 0, cx: 216.0, cy: 126.0, name: "CENTER" },
            Stop { value: STEP, cx: 216.0, cy: 74.0, name: "FRONT" },
            Stop { value: 2 * STEP, cx: 348.0, cy: 74.0, name: "AROUND" },
            Stop { value: 3 * STEP, cx: 348.0, cy: 126.0, name: "BEHIND" },
        ],
    },
    Control::Knob { id: "amount", cx: 282.0, cy: 186.0, r: 18.0, caption: "AMOUNT", bipolar: false },
    Control::Knob { id: "reverb", cx: 448.0, cy: 70.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "size", cx: 448.0, cy: 188.0, r: 24.0, caption: "", bipolar: false },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Halo",
    word: "HALO",
    about: "Lemon Studio's vocal placer",
    settings: &SETTINGS,
    version: Some(1),
    any_version: false,
    fields: &[],
    tail: &[],
    blank: None,
    locate: None,
    size: (520, 280),
    cards: &CARDS,
    controls: &CONTROLS,
    display: Some(Display { r: Rect::new(16.0, 36.0, 164.0, 198.0), draw: picture }),
    mark: Mark { x: 512.0, y: 261.0, anchor: 1.0, stacked: false },
    hint: Some(Rect::new(8.0, 250.0, 504.0, 22.0)),
    labels: &[],
    text,
    parse,
    sound,
    rings: true,
};

/// `Front`, or `Front to Around 30%` between two places.
fn text(k: usize, v: i32) -> String {
    match k {
        PLACE => {
            let (i, rest) = ((v.clamp(0, 300) / STEP) as usize, v.clamp(0, 300) % STEP);
            match rest {
                0 => PLACES[i].into(),
                _ => format!("{} to {} {rest}%", PLACES[i], PLACES[i + 1]),
            }
        }
        REVERB if v <= 0 => "Off".into(),
        _ => format!("{v}%"),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let (min, max) = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    match k {
        PLACE => {
            let Some(i) = PLACES.iter().position(|n| !low.is_empty() && low.starts_with(&n.to_ascii_lowercase())) else {
                return leading_number(&low).map(|x| (x.round() as i32).clamp(min, max));
            };
            let on = i as i32 * STEP;
            // What follows "to <next place>", if anything: how far along.
            let along = low.split_whitespace().last().filter(|_| low.contains(" to ")).and_then(leading_number).unwrap_or(0.0);
            Some((on + along.round() as i32).clamp(min, max))
        }
        REVERB if low == "off" => Some(0),
        _ => leading_number(&low).map(|x| (x.round() as i32).clamp(min, max)),
    }
}

/// What each setting does (the editor's tooltip).
pub fn tip(id: &str) -> &'static str {
    match id {
        "place" => "Where the voice sits around your head: in the center of it, out in front, all around you, or behind you. Turn it, or click a name. Automate it to move the voice during the song",
        "amount" => "How much of that place: 0 leaves the sound as it came in",
        "reverb" => "A simple reverb on top of the place. The place itself needs no reverb",
        "size" => "How big the reverb's space is, from a small room to a big hall",
        _ => "",
    }
}

/// A well seen from above: the head, its nose up, and where the voice is. Inside the head at Center; then an arc around
/// the head, short in front, the whole way around at Around, short behind. The reverb's rings around it.
fn picture(c: &mut Canvas, r: Rect, v: &[i32]) {
    const LABEL: Font = Font::new(10).bold();
    const HEAD: f32 = 18.0;
    const ORBIT: f32 = 50.0;
    let (cx, cy) = (r.cx(), r.cy());
    let (place, amount) = (v[PLACE] as f32 / STEP as f32, v[AMOUNT] as f32 / 100.0);
    let lh = c.line_height(LABEL);
    c.text_center(LABEL, Rect::new(r.x, r.y + 5.0, r.w, lh), plug::DIM, "FRONT");
    c.text_center(LABEL, Rect::new(r.x, r.bottom() - 5.0 - lh, r.w, lh), plug::DIM, "BACK");
    // The reverb's rings: fainter outwards, further apart as the space grows.
    let (reverb, size) = (v[REVERB] as f32 / 100.0, v[SIZE] as f32 / 100.0);
    if reverb > 0.0 {
        for k in 1..=3 {
            let ring = ORBIT + 2.0 + k as f32 * (3.0 + 6.0 * size);
            c.stroke_circle(cx, cy, ring, 1.0, brand::MAIN.with_alpha((reverb * 110.0 / k as f32) as u8));
        }
    }
    c.stroke_circle(cx, cy, ORBIT, 1.0, plug::GRID);
    // The head.
    c.fill_polygon(&[(cx - 5.0, cy - HEAD + 1.0), (cx + 5.0, cy - HEAD + 1.0), (cx, cy - HEAD - 6.0)], plug::DIM);
    c.fill_circle(cx - HEAD, cy, 3.0, plug::DIM);
    c.fill_circle(cx + HEAD, cy, 3.0, plug::DIM);
    c.fill_circle(cx, cy, HEAD, plug::TRACK);
    c.stroke_circle(cx, cy, HEAD, 1.5, plug::DIM);
    // The voice: less sure of its place the less of it there is.
    let ink = brand::MAIN.with_alpha((70.0 + 185.0 * amount) as u8);
    if place < 1.0 {
        c.fill_circle(cx, cy - ORBIT * place, 6.0, ink);
    } else {
        const SHORT: f32 = PI / 10.0;
        let (centre, half) = if place < 2.0 { (-PI / 2.0, SHORT + (PI - SHORT) * (place - 1.0)) } else { (PI / 2.0, PI - (PI - SHORT) * (place - 2.0)) };
        c.arc(cx, cy, ORBIT, centre - half, 2.0 * half.min(PI), 7.0, ink);
    }
}

/// Each place's shape.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shape {
    /// How much goes to the sides against the voice itself, as an angle (radians): gains cos and sin, a constant power.
    angle: f32,
    /// How much of the input's own stereo stays.
    keep: f32,
    /// The directional bands, in dB ([`BANDS`]).
    eq: [f32; 4],
    /// The short echoes' level.
    early: f32,
}

/// The directional bands: frequency and Q.
const BANDS: [(f32, f32); 4] = [(400.0, 1.0), (1100.0, 1.2), (3800.0, 1.0), (11000.0, 1.0)];

const fn degrees(d: f32) -> f32 {
    d * PI / 180.0
}

/// What the plugin does at no amount: nothing.
const NEUTRAL: Shape = Shape { angle: 0.0, keep: 1.0, eq: [0.0; 4], early: 0.0 };

/// [`PLACES`]' shapes.
const SHAPES: [Shape; 4] = [
    Shape { angle: 0.0, keep: 0.0, eq: [0.0; 4], early: 0.0 },
    Shape { angle: degrees(10.0), keep: 0.5, eq: [1.5, -1.0, 3.0, 0.0], early: 0.10 },
    Shape { angle: degrees(45.0), keep: 1.0, eq: [0.0, 0.0, -1.0, 0.0], early: 0.16 },
    Shape { angle: degrees(62.0), keep: 1.0, eq: [-1.5, 3.0, -6.0, 2.0], early: 0.14 },
];

/// The shape of a place on the dial, at `amount` of 100.
fn shape_at(place: i32, amount: i32) -> Shape {
    let p = place.clamp(0, 300) as f32 / STEP as f32;
    let i = (p as usize).min(SHAPES.len() - 2);
    let (a, b, t) = (SHAPES[i], SHAPES[i + 1], p - i as f32);
    let k = amount.clamp(0, 100) as f32 / 100.0;
    let at = |a: f32, b: f32, neutral: f32| neutral + (a + (b - a) * t - neutral) * k;
    Shape {
        angle: at(a.angle, b.angle, NEUTRAL.angle),
        keep: at(a.keep, b.keep, NEUTRAL.keep),
        eq: std::array::from_fn(|n| at(a.eq[n], b.eq[n], NEUTRAL.eq[n])),
        early: at(a.early, b.early, NEUTRAL.early),
    }
}

/// A second-order section (transposed direct form II): coefficients b0, b1, b2, a1, a2 and its two memories.
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    c: [f32; 5],
    z: [f32; 2],
}

impl Biquad {
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.c[0] * x + self.z[0];
        self.z[0] = self.c[1] * x - self.c[3] * y + self.z[1];
        self.z[1] = self.c[2] * x - self.c[4] * y;
        y
    }
}

/// A peaking band of `db` (the cookbook's); at 0 dB the section passes the sound as it is.
fn peak(rate: f32, hz: f32, q: f32, db: f32) -> [f32; 5] {
    let a = 10f32.powf(db / 40.0);
    let (s, c) = (TAU * hz.min(0.45 * rate) / rate).sin_cos();
    let alpha = s / (2.0 * q);
    let a0 = 1.0 + alpha / a;
    [(1.0 + alpha * a) / a0, -2.0 * c / a0, (1.0 - alpha * a) / a0, -2.0 * c / a0, (1.0 - alpha / a) / a0]
}

/// An all-pass: its phase turns once around `hz`.
fn all_pass(rate: f32, hz: f32, q: f32) -> [f32; 5] {
    let (s, c) = (TAU * hz.min(0.45 * rate) / rate).sin_cos();
    let alpha = s / (2.0 * q);
    let a0 = 1.0 + alpha;
    [(1.0 - alpha) / a0, -2.0 * c / a0, 1.0, -2.0 * c / a0, (1.0 - alpha) / a0]
}

/// The all-passes that spread the voice: where they turn (Hz), about two thirds of an octave apart.
const SPREAD: [f32; 8] = [250.0, 400.0, 640.0, 1020.0, 1630.0, 2600.0, 4150.0, 6600.0];
const SPREAD_Q: f32 = 0.8;
/// The sides come this late (s): their phase turns ever faster over the highs.
const SIDE_S: f32 = 0.0008;
/// The short echoes: when (s) and how loud, left and right.
const ECHOES: [[(f32, f32); 4]; 2] = [
    [(0.0073, 0.9), (0.0131, -0.7), (0.0197, 0.55), (0.0271, -0.4)],
    [(0.0089, -0.85), (0.0113, 0.75), (0.0233, -0.5), (0.0299, 0.35)],
];
/// Below this (Hz) the voice stays in the middle.
const BASS_HZ: f32 = 160.0;
/// The controls move this often (samples) and take about this long to arrive (s).
const CONTROL: usize = 32;
const GLIDE_S: f32 = 0.03;
/// The reverb's level at the top of its knob, and how far the voice steps back under it.
const WET_TOP: f32 = 0.5;
const DRY_BACK: f32 = 0.2;

/// The gains the shape gives: voice, sides, input's stereo, echoes.
fn gains(s: &Shape) -> [f32; 4] {
    [s.angle.cos(), s.angle.sin(), s.keep, s.early]
}

/// The plate for a size of 0 to 100. The room stays the same (a new room would empty the lines): the size is in the decay
/// and the wait before the plate starts.
fn plate(size: i32) -> reverb::Settings {
    let s = size.clamp(0, 100) as f32 / 100.0;
    let mut k = reverb::DEFAULTS;
    k[reverb::LOW_CUT] = 200;
    k[reverb::HIGH_CUT] = 90;
    k[reverb::PREDELAY] = (2.0 + 15.0 * s).round() as i32;
    k[reverb::ROOM] = 60;
    k[reverb::DECAY] = (3.0 * (50.0f32 / 3.0).powf(s)).round() as i32;
    k[reverb::DAMPING] = 70;
    k[reverb::BASS] = 60;
    k[reverb::CROSSOVER] = 400;
    k[reverb::DRY] = 0;
    k[reverb::EARLY] = 64;
    k[reverb::WET] = 128;
    k[reverb::MOD_DEPTH] = 300;
    reverb::Settings { knobs: k, tempo_sync: false, mid_side: false }
}

pub struct Halo {
    rate: f32,
    /// Where the controls are going, and where they are.
    to: Shape,
    now: Shape,
    glide: f32,
    /// The band filters of the voice and of the input's stereo.
    eq: [[Biquad; 4]; 2],
    spread: [Biquad; SPREAD.len()],
    /// The voice's bass (a one-pole follower) and its coefficient.
    bass: f32,
    bass_c: f32,
    /// The spread voice's last samples (a ring) and where the next goes.
    line: Vec<f32>,
    at: usize,
    side: usize,
    echoes: [[(usize, f32); 4]; 2],
    /// The gains at the end of the last control step.
    last: [f32; 4],
    reverb: Reverb,
    /// The reverb's level: where it is going, where it is, and whether the plate has anything in it.
    wet_to: f32,
    wet: f32,
    ringing: bool,
}

impl Halo {
    pub fn new(rate: u32, v: &[i32]) -> Halo {
        let r = rate.max(1) as f32;
        let samples = |t: f32| (t * r).round().max(1.0) as usize;
        let longest = ECHOES.iter().flatten().map(|e| e.0).fold(SIDE_S, f32::max);
        let shape = shape_at(v[PLACE], v[AMOUNT]);
        let mut spread = [Biquad::default(); SPREAD.len()];
        for (b, &hz) in spread.iter_mut().zip(&SPREAD) {
            b.c = all_pass(r, hz, SPREAD_Q);
        }
        let mut h = Halo {
            rate: r,
            to: shape,
            now: shape,
            glide: 1.0 - (-(CONTROL as f32) / (GLIDE_S * r)).exp(),
            eq: [[Biquad::default(); 4]; 2],
            spread,
            bass: 0.0,
            bass_c: 1.0 - (-TAU * BASS_HZ / r).exp(),
            line: vec![0.0; samples(longest) + 2],
            at: 0,
            side: samples(SIDE_S),
            echoes: ECHOES.map(|side| side.map(|(t, g)| (samples(t), g))),
            last: gains(&shape),
            reverb: Reverb::new(rate.max(1), plate(v[SIZE]), 120.0),
            wet_to: 0.0,
            wet: 0.0,
            ringing: false,
        };
        h.set_eq();
        h.set(v);
        h.wet = h.wet_to;
        h
    }

    /// The band filters for where the controls are.
    fn set_eq(&mut self) {
        for (b, (&(hz, q), &db)) in BANDS.iter().zip(&self.now.eq).enumerate() {
            let c = peak(self.rate, hz, q, db);
            self.eq[0][b].c = c;
            self.eq[1][b].c = c;
        }
    }

    /// The controls one step on: the gains they ask for now.
    fn step(&mut self) -> [f32; 4] {
        if self.now != self.to {
            let g = self.glide;
            let near = |now: &mut f32, to: f32| {
                *now += (to - *now) * g;
                if (to - *now).abs() < 1e-4 {
                    *now = to;
                }
            };
            near(&mut self.now.angle, self.to.angle);
            near(&mut self.now.keep, self.to.keep);
            near(&mut self.now.early, self.to.early);
            for (now, &to) in self.now.eq.iter_mut().zip(&self.to.eq) {
                near(now, to);
            }
            self.set_eq();
        }
        gains(&self.now)
    }

    #[inline]
    fn back(&self, n: usize) -> f32 {
        let len = self.line.len();
        self.line[(self.at + len - n) % len]
    }

    /// One sample through the place, with the gains `g` (voice, sides, input's stereo, echoes).
    #[inline]
    fn place(&mut self, l: f32, r: f32, g: [f32; 4]) -> (f32, f32) {
        let (mut mid, mut stereo) = (0.5 * (l + r), 0.5 * (l - r));
        let [eq_mid, eq_stereo] = &mut self.eq;
        for (m, s) in eq_mid.iter_mut().zip(eq_stereo) {
            mid = m.run(mid);
            stereo = s.run(stereo);
        }
        self.bass += (mid - self.bass) * self.bass_c;
        let high = mid - self.bass;
        let mut spread = high;
        for a in &mut self.spread {
            spread = a.run(spread);
        }
        self.line[self.at] = spread;
        let sides = self.back(self.side);
        let echo = |this: &Halo, k: usize| this.echoes[k].iter().map(|&(n, gain)| gain * this.back(n)).sum::<f32>();
        let (el, er) = (echo(self, 0), echo(self, 1));
        self.at = (self.at + 1) % self.line.len();
        let voice = self.bass + g[0] * high;
        let wide = g[1] * sides + g[2] * stereo;
        (voice + wide + g[3] * el, voice - wide + g[3] * er)
    }
}

impl Sound for Halo {
    fn set(&mut self, v: &[i32]) {
        self.to = shape_at(v[PLACE], v[AMOUNT]);
        let x = v[REVERB].clamp(0, 100) as f32 / 100.0;
        self.wet_to = x * x.sqrt() * WET_TOP;
        self.reverb.set(plate(v[SIZE]));
    }

    fn set_sample_rate(&mut self, rate: u32) {
        let reverb = *self.reverb.settings();
        let mut h = Halo::new(rate, &[0, 0, 0, 0]);
        h.reverb = Reverb::new(rate.max(1), reverb, 120.0);
        (h.to, h.now, h.last) = (self.to, self.to, gains(&self.to));
        (h.wet_to, h.wet) = (self.wet_to, self.wet_to);
        h.set_eq();
        *self = h;
    }

    /// Everything empty; the controls stand at the settings.
    fn reset(&mut self) {
        self.eq = self.eq.map(|side| side.map(|b| Biquad { z: [0.0; 2], ..b }));
        self.spread = self.spread.map(|b| Biquad { z: [0.0; 2], ..b });
        self.bass = 0.0;
        self.line.fill(0.0);
        self.at = 0;
        self.now = self.to;
        self.last = gains(&self.to);
        self.set_eq();
        self.reverb.clear();
        self.wet = self.wet_to;
        self.ringing = false;
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (ls, rs) in left.chunks_mut(CONTROL).zip(right.chunks_mut(CONTROL)) {
            let (from, to) = (self.last, self.step());
            self.last = to;
            let (wet_from, wet_end) = (self.wet, self.wet + (self.wet_to - self.wet) * self.glide);
            self.wet = if (self.wet_to - wet_end).abs() < 1e-5 { self.wet_to } else { wet_end };
            let n = ls.len() as f32;
            for (i, (l, r)) in ls.iter_mut().zip(rs.iter_mut()).enumerate() {
                let t = (i + 1) as f32 / n;
                let g: [f32; 4] = std::array::from_fn(|k| from[k] + (to[k] - from[k]) * t);
                let (ol, or) = self.place(*l, *r, g);
                let wet = wet_from + (self.wet - wet_from) * t;
                (*l, *r) = if wet > 0.0 || self.wet_to > 0.0 {
                    let (wl, wr) = self.reverb.frame(ol, or);
                    let dry = 1.0 - DRY_BACK * (wet / WET_TOP);
                    (dry * ol + wet * wl, dry * or + wet * wr)
                } else {
                    (ol, or)
                };
            }
            // Turned off: the plate is emptied, so nothing old comes out when it is turned on again.
            if self.wet > 0.0 {
                self.ringing = true;
            } else if self.ringing {
                self.reverb.clear();
                self.ringing = false;
            }
        }
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(Halo::new(rate, v))
}

pub struct HaloKind;

impl Kind for HaloKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-halo";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's vocal placer: center, front, around or behind, with a simple reverb";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Utility, ClapFeature::Reverb, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioHalo_";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Spatial];
}

pub type LemonHalo = SmallFx<HaloKind>;

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48000;
    const CENTER: i32 = 0;
    const FRONT: i32 = STEP;
    const AROUND: i32 = 2 * STEP;
    const BEHIND: i32 = 3 * STEP;

    /// Noise leaning to the lows, as a voice's spectrum does.
    fn voice_like(len: usize, seed: u32) -> Vec<f32> {
        let mut seed = seed;
        let mut low = 0.0f32;
        (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let w = (seed >> 8) as f32 / (1 << 24) as f32 - 0.5;
                low += (w - low) * 0.2;
                0.3 * w + 0.7 * low
            })
            .collect()
    }

    fn run(v: [i32; 4], l: &[f32], r: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut s = Halo::new(RATE, &v);
        let (mut a, mut b) = (l.to_vec(), r.to_vec());
        for (x, y) in a.chunks_mut(256).zip(b.chunks_mut(256)) {
            s.process(x, y);
        }
        (a, b)
    }

    fn energy(x: &[f32]) -> f32 {
        x.iter().map(|s| s * s).sum()
    }

    fn db(a: f32, b: f32) -> f32 {
        10.0 * (a / b).log10()
    }

    /// How much the two ears agree: 1 the same sound, 0 unrelated, -1 opposite.
    fn agreement(l: &[f32], r: &[f32]) -> f32 {
        let lr: f32 = l.iter().zip(r).map(|(a, b)| a * b).sum();
        lr / (energy(l) * energy(r)).sqrt()
    }

    /// The energy in `lo..hi` Hz.
    fn band(x: &[f32], lo: f32, hi: f32) -> f32 {
        let n = 4096;
        let fft = crate::spectral::Fft::new(n);
        let w = crate::spectral::hann(n);
        let mut e = 0.0;
        for start in (0..x.len().saturating_sub(n)).step_by(n) {
            let (mut re, mut im): (Vec<f32>, Vec<f32>) = (x[start..start + n].iter().zip(&w).map(|(a, b)| a * b).collect(), vec![0.0; n]);
            fft.run(&mut re, &mut im, false);
            for k in 1..n / 2 {
                if (lo..hi).contains(&(k as f32 * RATE as f32 / n as f32)) {
                    e += re[k] * re[k] + im[k] * im[k];
                }
            }
        }
        e
    }

    #[test]
    fn value_texts_and_typed_values() {
        assert_eq!(text(PLACE, 0), "Center");
        assert_eq!(text(PLACE, 100), "Front");
        assert_eq!(text(PLACE, 130), "Front to Around 30%");
        assert_eq!(text(PLACE, 299), "Around to Behind 99%");
        assert_eq!(text(PLACE, 300), "Behind");
        assert_eq!(text(REVERB, 0), "Off");
        assert_eq!(text(REVERB, 25), "25%");
        assert_eq!(text(AMOUNT, 0), "0%");
        assert_eq!(parse(PLACE, "behind"), Some(300));
        assert_eq!(parse(PLACE, "Front to Around 30%"), Some(130));
        assert_eq!(parse(PLACE, "250"), Some(250));
        assert_eq!(parse(PLACE, "999"), Some(300));
        assert_eq!(parse(REVERB, "off"), Some(0));
        assert_eq!(parse(SIZE, "150"), Some(100));
        assert_eq!(parse(PLACE, "nowhere"), None);
    }

    /// At no amount and no reverb the output is the input, whatever the dial says.
    #[test]
    fn at_no_amount_it_passes_unchanged() {
        let (l, r) = (voice_like(RATE as usize, 1), voice_like(RATE as usize, 2));
        for place in [CENTER, FRONT, AROUND, BEHIND] {
            let (a, b) = run([place, 0, 0, 40], &l, &r);
            let err = a.iter().zip(&l).chain(b.iter().zip(&r)).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
            assert!(err < 1e-4, "{place}: {err}");
        }
    }

    /// Center is the same sound in both ears, also from a stereo input: dead in the middle of the head.
    #[test]
    fn center_is_the_same_in_both_ears() {
        let (l, r) = (voice_like(RATE as usize, 1), voice_like(RATE as usize, 2));
        let (a, b) = run([CENTER, 100, 0, 40], &l, &r);
        assert_eq!(a, b);
    }

    /// The ears agree less and less along the dial: fully at Center, nearly in front, not at all around, mostly opposite
    /// behind. From a mono voice.
    #[test]
    fn the_ears_agree_less_along_the_dial() {
        let x = voice_like(2 * RATE as usize, 3);
        let at = |place: i32| {
            let (a, b) = run([place, 100, 0, 40], &x, &x);
            agreement(&a, &b)
        };
        let (center, front, around, behind) = (at(CENTER), at(FRONT), at(AROUND), at(BEHIND));
        println!("agreement: center {center:.2}, front {front:.2}, around {around:.2}, behind {behind:.2}");
        assert!(center > 0.999, "{center}");
        assert!(front > 0.7 && front < 0.98, "{front}");
        assert!(around.abs() < 0.35, "{around}");
        assert!(behind < -0.15, "{behind}");
    }

    /// The sides cancel in a mono sum (only the short echoes are left of them): a phone speaker still gets the voice, at
    /// any place. The bass stays in the middle.
    #[test]
    fn a_mono_sum_keeps_the_voice_and_the_bass_stays_in_the_middle() {
        let x = voice_like(2 * RATE as usize, 4);
        for place in [FRONT, AROUND, BEHIND] {
            let (a, b) = run([place, 100, 0, 40], &x, &x);
            let sum: Vec<f32> = a.iter().zip(&b).map(|(l, r)| 0.5 * (l + r)).collect();
            let kept = db(energy(&sum), energy(&x));
            println!("{place}: mono sum {kept:.1} dB");
            assert!(kept > -9.0, "{place}: {kept} dB");
            let side: Vec<f32> = a.iter().zip(&b).map(|(l, r)| 0.5 * (l - r)).collect();
            let low = db(band(&side, 20.0, 60.0), band(&sum, 20.0, 60.0));
            assert!(low < -9.0, "{place}: the bass goes to the sides at {low} dB");
        }
    }

    /// The places differ in loudness by little, whichever the voice is: moving the dial is not a volume change.
    #[test]
    fn loudness_stays_about_the_same_along_the_dial() {
        let x = voice_like(2 * RATE as usize, 5);
        for place in (0..=300).step_by(25) {
            let (a, b) = run([place, 100, 0, 40], &x, &x);
            let level = db(energy(&a) + energy(&b), 2.0 * energy(&x));
            println!("{place}: {level:.1} dB");
            assert!(level.abs() < 3.0, "{place}: {level} dB");
        }
    }

    /// In front more of the 3.8 kHz band against 1.1 kHz, behind less: the tone that tells front from back.
    #[test]
    fn front_and_behind_differ_in_tone() {
        let x = voice_like(2 * RATE as usize, 6);
        let lean = |place: i32| {
            let (a, b) = run([place, 100, 0, 40], &x, &x);
            db(band(&a, 3000.0, 4800.0) + band(&b, 3000.0, 4800.0), band(&a, 900.0, 1300.0) + band(&b, 900.0, 1300.0))
        };
        let (front, behind) = (lean(FRONT), lean(BEHIND));
        println!("presence against middle: front {front:.1} dB, behind {behind:.1} dB");
        assert!(front > behind + 6.0, "front {front}, behind {behind}");
    }

    /// The dial turned all the way during a steady tone: the output has no jump bigger than the tone's own swing.
    #[test]
    fn turning_the_dial_makes_no_clicks() {
        let tone: Vec<f32> = (0..RATE as usize).map(|t| 0.5 * (TAU * 220.0 * t as f32 / RATE as f32).sin()).collect();
        let mut s = Halo::new(RATE, &[0, 100, 0, 40]);
        let (mut l, mut r) = (tone.clone(), tone.clone());
        for (k, (x, y)) in l.chunks_mut(128).zip(r.chunks_mut(128)).enumerate() {
            let place = (k * 128 * 300 / RATE as usize) as i32;
            s.set(&[place, 100, 0, 40]);
            s.process(x, y);
        }
        let jump = |x: &[f32]| x.windows(2).skip(4800).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        let swing = tone.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(jump(&l) < 3.0 * swing && jump(&r) < 3.0 * swing, "{} {} against {swing}", jump(&l), jump(&r));
    }

    /// The reverb rings on after the sound stops, the longer the bigger, at a sensible level; off, nothing rings on; turned
    /// off while it rings, it is quiet soon and has forgotten the old sound.
    #[test]
    fn the_reverb_rings_on_and_goes_off() {
        let mut x = voice_like(RATE as usize, 7);
        let on = x.len();
        x.resize(3 * RATE as usize, 0.0);
        let after = |v: [i32; 4], from: f32, to: f32| {
            let (a, b) = run(v, &x, &x);
            let (from, to) = ((from * RATE as f32) as usize, (to * RATE as f32) as usize);
            energy(&a[from..to]) + energy(&b[from..to])
        };
        assert!(after([AROUND, 100, 0, 40], 1.05, 1.5) < 1e-6, "no reverb, nothing after the sound");
        let (small, big) = (after([AROUND, 100, 100, 0], 1.3, 2.0), after([AROUND, 100, 100, 100], 1.3, 2.0));
        println!("tail: small {small:.4}, big {big:.4}");
        assert!(big > 10.0 * small && big > 1e-3, "small {small}, big {big}");
        // A sustained voice under the knob at its top: the voice stays on top of the reverb.
        let (a, b) = run([AROUND, 100, 0, 40], &x[..on], &x[..on]);
        let (c, d) = run([AROUND, 100, 100, 100], &x[..on], &x[..on]);
        let rise = db(energy(&c) + energy(&d), energy(&a) + energy(&b));
        println!("reverb at its top: {rise:.1} dB");
        assert!(rise.abs() < 3.0, "{rise} dB");
        let mut s = Halo::new(RATE, &[AROUND, 100, 100, 100]);
        let (mut l, mut r) = (x[..on].to_vec(), x[..on].to_vec());
        s.process(&mut l, &mut r);
        s.set(&[AROUND, 100, 0, 100]);
        let (mut l, mut r) = (vec![0.0; RATE as usize / 2], vec![0.0; RATE as usize / 2]);
        s.process(&mut l, &mut r);
        let tail = energy(&l[RATE as usize / 4..]) + energy(&r[RATE as usize / 4..]);
        assert!(tail < 1e-6, "{tail}");
        s.set(&[AROUND, 100, 100, 100]);
        let (mut l, mut r) = (vec![0.0; RATE as usize / 2], vec![0.0; RATE as usize / 2]);
        s.process(&mut l, &mut r);
        assert!(energy(&l) + energy(&r) < 1e-6, "the old sound came back");
    }

    /// A reset forgets the sound; every rate gives a sound without surprises, at the same place as before a change.
    #[test]
    fn resets_and_sample_rates() {
        let x = voice_like(RATE as usize / 2, 8);
        let mut s = Halo::new(RATE, &[BEHIND, 100, 60, 70]);
        let (mut l, mut r) = (x.clone(), x.clone());
        s.process(&mut l, &mut r);
        Sound::reset(&mut s);
        let (mut l, mut r) = (vec![0.0; 2048], vec![0.0; 2048]);
        s.process(&mut l, &mut r);
        assert!(energy(&l) + energy(&r) < 1e-9, "something left after the reset");
        for rate in [8000, 22050, 44100, 96000, 192000] {
            s.set_sample_rate(rate);
            let (mut l, mut r) = (x.clone(), x.clone());
            s.process(&mut l, &mut r);
            assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 8.0), "{rate}");
            assert!(s.side < s.line.len() && s.echoes.iter().flatten().all(|e| e.0 < s.line.len()), "{rate}");
            assert_eq!(s.to, shape_at(BEHIND, 100));
            Sound::reset(&mut s);
        }
    }

    /// The picture draws at every place and amount.
    #[test]
    fn the_picture_draws() {
        let Ok(mut c) = Canvas::new(200, 220, 1.0) else { return };
        for place in (0..=300).step_by(20) {
            c.begin_frame(None);
            picture(&mut c, Rect::new(0.0, 0.0, 164.0, 198.0), &[place, place / 3, place / 3, 50]);
        }
    }
}
