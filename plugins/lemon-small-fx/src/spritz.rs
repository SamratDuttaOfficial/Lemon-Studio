// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Spritz: Lemon Studio's de-esser as a plugin. It turns down hard S, SH and T sounds. A high-pass at the
//! frequency picks out the sibilance, and the reduction follows how loud that band is next to the whole signal, so it
//! acts the same on quiet and loud takes. Split mode turns down only the band above the frequency (a Linkwitz-Riley
//! crossover, so the two bands add back flat); Wide mode turns down everything while an S sounds. Listen plays what is
//! taken out, to tune the frequency and the threshold by ear.

use nih_plug::prelude::*;
use os_ui::Rect;

use crate::params::leading_number;
use crate::{Card, Control, Kind, Mark, Setting, SmallFx, Sound, Spec};

pub const FREQ: usize = 0;
pub const THRESHOLD: usize = 1;
pub const DEPTH: usize = 2;
pub const SPEED: usize = 3;
pub const MODE: usize = 4;
pub const LISTEN: usize = 5;

/// The settings: the frequency in Hz, the threshold and the depth in tenths of a dB, the release in ms, the mode
/// (0 split, 1 wide) and the listen switch.
pub const SETTINGS: [Setting; 6] = [
    Setting { id: "freq", name: "Frequency", range: (2000, 12000), start: 6000, magnet: None },
    Setting { id: "threshold", name: "Threshold", range: (-400, 0), start: -150, magnet: None },
    Setting { id: "depth", name: "Depth", range: (0, 240), start: 120, magnet: None },
    Setting { id: "speed", name: "Release", range: (10, 250), start: 60, magnet: None },
    Setting { id: "mode", name: "Mode", range: (0, 1), start: 0, magnet: None },
    Setting { id: "listen", name: "Listen", range: (0, 1), start: 0, magnet: None },
];

/// The saved state's version.
pub const VERSION: i32 = 1;

/// Editor layout: a card per knob, the mode and the listen switch together on the right; the hint well along the foot,
/// the wordmark at its right.
const CARDS: [Card; 5] = [
    Card { x0: 8.0, x1: 100.0, y0: 28.0, y1: 136.0, heading: "FREQUENCY" },
    Card { x0: 106.0, x1: 198.0, y0: 28.0, y1: 136.0, heading: "THRESHOLD" },
    Card { x0: 204.0, x1: 296.0, y0: 28.0, y1: 136.0, heading: "DEPTH" },
    Card { x0: 302.0, x1: 394.0, y0: 28.0, y1: 136.0, heading: "RELEASE" },
    Card { x0: 400.0, x1: 492.0, y0: 28.0, y1: 136.0, heading: "MODE" },
];

const CONTROLS: [Control; 6] = [
    Control::Knob { id: "freq", cx: 54.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "threshold", cx: 152.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "depth", cx: 250.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "speed", cx: 348.0, cy: 76.0, r: 24.0, caption: "", bipolar: false },
    Control::Choice { id: "mode", x: 418.0, top: 50.0, names: &["SPLIT", "WIDE"] },
    Control::Choice { id: "listen", x: 418.0, top: 102.0, names: &["NORMAL", "LISTEN"] },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Spritz",
    word: "SPRITZ",
    about: "Lemon Studio's de-esser",
    settings: &SETTINGS,
    version: Some(VERSION),
    any_version: false,
    fields: &[],
    tail: &[],
    blank: None,
    locate: None,
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
        FREQ => format!("{v}Hz"),
        THRESHOLD => format!("{:.1}dB", f64::from(v) / 10.0),
        DEPTH => match v {
            0 => "Off".into(),
            v => format!("-{:.1}dB", f64::from(v) / 10.0),
        },
        SPEED => format!("{v}ms"),
        MODE => if v == 0 { "Split" } else { "Wide" }.into(),
        _ => if v == 0 { "Normal" } else { "Listen" }.into(),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let (min, max) = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    let clamp = |x: f64| Some((x.round() as i32).clamp(min, max));
    match k {
        FREQ => {
            let x = leading_number(&low)?;
            // "6k", "6 kHz" or a plain number under 100 are kHz.
            clamp(if low.contains('k') || x.abs() < 100.0 { x * 1000.0 } else { x })
        }
        THRESHOLD => clamp(-leading_number(&low)?.abs() * 10.0),
        DEPTH if low.starts_with("off") => Some(0),
        DEPTH => clamp(leading_number(&low)?.abs() * 10.0),
        SPEED => clamp(leading_number(&low)?),
        MODE => ["split", "wide"].iter().position(|n| low.starts_with(n)).map(|i| i as i32),
        _ => ["normal", "listen"].iter().position(|n| low.starts_with(n)).map(|i| i as i32).or(match low.as_str() {
            "off" => Some(0),
            "on" => Some(1),
            _ => None,
        }),
    }
}

/// A second-order Butterworth high- or low-pass (RBJ, Q 1/sqrt 2), direct form I.
#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn tune(&mut self, high: bool, hz: f32, rate: f32) {
        let w = std::f32::consts::TAU * hz.min(rate * 0.45) / rate;
        let alpha = w.sin() / std::f32::consts::SQRT_2;
        let cos = w.cos();
        let a0 = 1.0 + alpha;
        let edge = if high { 1.0 + cos } else { 1.0 - cos };
        self.b0 = edge / 2.0 / a0;
        self.b1 = if high { -edge / a0 } else { edge / a0 };
        self.b2 = self.b0;
        self.a1 = -2.0 * cos / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    fn clear(&mut self) {
        (self.x1, self.x2, self.y1, self.y2) = (0.0, 0.0, 0.0, 0.0);
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        (self.x2, self.x1, self.y2, self.y1) = (self.x1, x, self.y1, y);
        y
    }
}

/// How much more the reduction grows than the sibilance's rise over the threshold (it reaches the depth quickly).
const SLOPE: f32 = 2.0;
/// Below this level (power, about -70 dB) nothing is reduced: noise in the pauses is left alone.
const FLOOR: f32 = 1e-7;
/// The detectors' attack.
const ATTACK_MS: f32 = 0.5;
/// The gain's glide, against clicks.
const GLIDE_MS: f32 = 1.0;

/// The de-esser's sound.
pub struct DeEsser {
    rate: f32,
    v: [i32; 6],
    /// Per side: two high-passes and two low-passes in series (the fourth-order crossover); the first high-pass's
    /// output is also the detector's band.
    hp: [[Biquad; 2]; 2],
    lp: [[Biquad; 2]; 2],
    /// Power envelopes of the band above the frequency and of the whole signal.
    band: f32,
    full: f32,
    gain: f32,
    attack: f32,
    release: f32,
    glide: f32,
}

/// One-pole coefficient for a time constant of `ms` at `rate`.
fn coef(ms: f32, rate: f32) -> f32 {
    1.0 - (-1000.0 / (ms * rate)).exp()
}

impl DeEsser {
    pub fn new(rate: u32, v: &[i32]) -> DeEsser {
        let mut d = DeEsser {
            rate: rate.max(1) as f32,
            v: [0; 6],
            hp: [[Biquad::default(); 2]; 2],
            lp: [[Biquad::default(); 2]; 2],
            band: 0.0,
            full: 0.0,
            gain: 1.0,
            attack: 0.0,
            release: 0.0,
            glide: 0.0,
        };
        d.v.copy_from_slice(&v[..6]);
        d.retune();
        d
    }

    fn retune(&mut self) {
        let hz = self.v[FREQ].clamp(SETTINGS[FREQ].range.0, SETTINGS[FREQ].range.1) as f32;
        for b in self.hp.iter_mut().flatten() {
            b.tune(true, hz, self.rate);
        }
        for b in self.lp.iter_mut().flatten() {
            b.tune(false, hz, self.rate);
        }
        self.attack = coef(ATTACK_MS, self.rate);
        self.release = coef(self.v[SPEED].max(1) as f32, self.rate);
        self.glide = coef(GLIDE_MS, self.rate);
    }

    /// The gain the band (split) or the whole signal (wide) gets for the levels now.
    fn target(&self) -> f32 {
        let depth = self.v[DEPTH].max(0) as f32 / 10.0;
        if depth <= 0.0 || self.full < FLOOR {
            return 1.0;
        }
        let rise = 10.0 * (self.band.max(1e-20) / self.full).log10() - self.v[THRESHOLD] as f32 / 10.0;
        let cut = (rise * SLOPE).clamp(0.0, depth);
        10f32.powf(-cut / 20.0)
    }
}

impl Sound for DeEsser {
    fn set(&mut self, v: &[i32]) {
        if v[..6] != self.v {
            self.v.copy_from_slice(&v[..6]);
            self.retune();
        }
    }

    fn set_sample_rate(&mut self, rate: u32) {
        self.rate = rate.max(1) as f32;
        self.retune();
        self.reset();
    }

    fn reset(&mut self) {
        for b in self.hp.iter_mut().chain(&mut self.lp).flatten() {
            b.clear();
        }
        (self.band, self.full, self.gain) = (0.0, 0.0, 1.0);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let (wide, listen) = (self.v[MODE] != 0, self.v[LISTEN] != 0);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let x = [*l, *r];
            let h1 = [self.hp[0][0].run(x[0]), self.hp[1][0].run(x[1])];
            let (b, f) = (0.5 * (h1[0] * h1[0] + h1[1] * h1[1]), 0.5 * (x[0] * x[0] + x[1] * x[1]));
            self.band += (b - self.band) * if b > self.band { self.attack } else { self.release };
            self.full += (f - self.full) * if f > self.full { self.attack } else { self.release };
            let t = self.target();
            self.gain += (t - self.gain) * self.glide;
            let g = self.gain;
            let out: [f32; 2] = std::array::from_fn(|c| {
                // The crossover runs in both modes, so switching mode does not jump.
                let high = self.hp[c][1].run(h1[c]);
                let lp1 = self.lp[c][0].run(x[c]);
                let low = self.lp[c][1].run(lp1);
                match (wide, listen) {
                    (false, false) => low + g * high,
                    (false, true) => (1.0 - g) * high,
                    (true, false) => g * x[c],
                    (true, true) => (1.0 - g) * x[c],
                }
            });
            (*l, *r) = (out[0], out[1]);
        }
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(DeEsser::new(rate, v))
}

pub struct SpritzKind;

impl Kind for SpritzKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-spritz";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's de-esser";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Deesser, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioSprtz";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

pub type LemonSpritz = SmallFx<SpritzKind>;

/// What each setting does (the editor's tooltip).
pub fn tip(id: &str) -> &'static str {
    match id {
        "freq" => "Where the S sounds start: the band above this is watched and turned down. Lower for a deep voice, higher for a bright one",
        "threshold" => "How loud the S band must get, next to the whole voice, before it is turned down. Lower catches more",
        "depth" => "The most an S is turned down, in dB",
        "speed" => "How fast the reduction lets go after an S: short keeps the words clear, long is smoother",
        "mode" => "Split turns down only the highs above the frequency; Wide turns the whole voice down while an S sounds",
        "listen" => "Listen plays only what is taken out, to set the frequency and threshold by ear",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: [i32; 6] = [6000, -150, 120, 60, 0, 0];

    fn tone(hz: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (std::f32::consts::TAU * hz * i as f32 / 44100.0).sin() * amp).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Plays `input` on both sides with settings `v`; the right side's last half.
    fn play(v: &[i32], input: &[f32]) -> Vec<f32> {
        let mut s = sound(44100, v);
        let (mut l, mut r) = (input.to_vec(), input.to_vec());
        s.process(&mut l, &mut r);
        assert_eq!(l, r);
        r[input.len() / 2..].to_vec()
    }

    #[test]
    fn value_texts_and_typed_values() {
        assert_eq!(text(FREQ, 6000), "6000Hz");
        assert_eq!(text(THRESHOLD, -150), "-15.0dB");
        assert_eq!(text(DEPTH, 0), "Off");
        assert_eq!(text(DEPTH, 120), "-12.0dB");
        assert_eq!(text(SPEED, 60), "60ms");
        assert_eq!(text(MODE, 1), "Wide");
        assert_eq!(text(LISTEN, 1), "Listen");
        assert_eq!(parse(FREQ, "7.5 kHz"), Some(7500));
        assert_eq!(parse(FREQ, "8"), Some(8000));
        assert_eq!(parse(FREQ, "4500Hz"), Some(4500));
        assert_eq!(parse(THRESHOLD, "20"), Some(-200));
        assert_eq!(parse(DEPTH, "-6 dB"), Some(60));
        assert_eq!(parse(DEPTH, "off"), Some(0));
        assert_eq!(parse(SPEED, "100 ms"), Some(100));
        assert_eq!(parse(MODE, "wide"), Some(1));
        assert_eq!(parse(LISTEN, "on"), Some(1));
    }

    /// The level change (dB) settings `v` give a tone.
    fn cut(v: &[i32], hz: f32, amp: f32) -> f32 {
        let n = 44100 / 2;
        let x = tone(hz, amp, n);
        20.0 * (rms(&play(v, &x)) / rms(&x[n / 2..])).log10()
    }

    /// A loud tone well above the frequency (an S) is turned down by about the depth; one well below it (a vowel) is
    /// not. Wide mode turns down the S by the depth.
    #[test]
    fn sibilance_is_turned_down_and_the_rest_is_left_alone() {
        let c = cut(&START, 14000.0, 0.5);
        assert!((-12.5..-9.0).contains(&c), "{c} dB");
        let c = cut(&START, 300.0, 0.5);
        assert!(c.abs() < 0.1, "{c} dB");
        let mut wide = START;
        wide[MODE] = 1;
        let c = cut(&wide, 8000.0, 0.5);
        assert!((-12.2..-11.8).contains(&c), "{c} dB");
        let c = cut(&wide, 300.0, 0.5);
        assert!(c.abs() < 0.1, "{c} dB");
    }

    /// The reduction follows the band's share, not the level: a quiet S is cut as much as a loud one.
    #[test]
    fn the_cut_does_not_depend_on_the_level() {
        let (loud, quiet) = (cut(&START, 10000.0, 0.8), cut(&START, 10000.0, 0.02));
        assert!(loud < -3.0 && (loud - quiet).abs() < 0.5, "{loud} {quiet}");
    }

    /// Depth off leaves the level alone; split leaves the lows of a mixed signal alone where wide turns them down;
    /// listen plays what is taken out.
    #[test]
    fn modes_depth_and_listen() {
        let n = 44100 / 2;
        let ess = tone(8000.0, 0.5, n);
        let mut off = START;
        off[DEPTH] = 0;
        // The crossover adds back flat: only its phase is left.
        assert!(cut(&off, 6000.0, 0.5).abs() < 0.05 && cut(&off, 14000.0, 0.5).abs() < 0.05);

        let low = tone(200.0, 0.3, n);
        let mix: Vec<f32> = ess.iter().zip(&low).map(|(a, b)| a + b).collect();
        let mut wide = START;
        wide[MODE] = 1;
        assert!(rms(&play(&wide, &mix)) < rms(&play(&START, &mix)) * 0.8);

        // Listen: the S comes through, a vowel is silent.
        let mut listen = START;
        listen[LISTEN] = 1;
        assert!(rms(&play(&listen, &ess)) > 0.1);
        assert!(rms(&play(&listen, &low)) < 1e-3);
    }
}
