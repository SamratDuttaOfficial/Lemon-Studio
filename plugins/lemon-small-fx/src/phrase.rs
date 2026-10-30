// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Phrase: Lemon Studio's note-by-note pitch correction as a plugin. Where Lemon Pitch pulls the pitch onto the
//! nearest note moment by moment, Lemon Phrase listens ahead, finds each sung note as a whole and moves the whole note
//! onto the scale by one amount: its vibrato, scoops, falls and the little life inside it stay as they were sung, only
//! out-of-tune notes move, and each note, word or phrase gets its own correction.
//!
//! How (classic pitch tracking, light enough for any computer):
//! - Pitch: Lemon Pitch's YIN detector, about every 3 ms, each reading cleaned with its neighbours (a median of three).
//! - Notes: the centre of the note under each moment is the median pitch of the moments up to 85 ms either side, within
//!   the same pitched stretch. A median keeps the steps between notes sharp but sees through what wavers faster than
//!   that (vibrato, the detector's slips) and through its own octave errors, so the centre is the note the singer
//!   meant. At a note's start the stretch begins there, so the note's onset (a scoop into it) is judged by the note
//!   that follows, not on its own.
//! - Correction: the nearest note of the key and scale (set by hand, or found from the notes heard) to the centre,
//!   which is kept until the centre moves clearly closer to another; the shift is that note minus the centre, so all of
//!   the note moves together. Tighten also pulls what wanders around the centre in towards it. Changes between notes
//!   glide over the transition time. A note already within a cent of its target is left alone and plays dry.
//! - The shift: Lemon Pitch's pitch-synchronous overlap-add, which keeps the voice's formants. The input is held back
//!   by the look-ahead (about 0.1 s, plus the shifter's own), which the plugin reports to the host, so in a song it
//!   plays in time.

use std::collections::VecDeque;

use nih_plug::prelude::*;
use os_ui::Rect;

use crate::params::leading_number;
use crate::pitch::{coef, note_index, scale_mask, Detector, KeyFinder, Psola, CHROMATIC, HOP, MAJOR, MINOR, SCALES, VOICES};
use crate::view::NOTES;
use crate::{Card, Control, Kind, Mark, Setting, SmallFx, Sound, Spec};

pub const KEY: usize = 0;
pub const SCALE: usize = 1;
pub const AUTO: usize = 2;
pub const VOICE: usize = 3;
pub const AMOUNT: usize = 4;
pub const TIGHTEN: usize = 5;
pub const GLIDE: usize = 6;
const N: usize = 7;

/// The settings: the key (C = 0), the scale (Lemon Pitch's), key detection (0 manual, 1 auto), the input (Lemon
/// Pitch's), the amount and tighten in %, the transition between notes in ms.
pub const SETTINGS: [Setting; N] = [
    Setting { id: "key", name: "Key", range: (0, 11), start: 0, magnet: None },
    Setting { id: "scale", name: "Scale", range: (0, 13), start: 1, magnet: None },
    Setting { id: "auto", name: "Key detection", range: (0, 1), start: 1, magnet: None },
    Setting { id: "voice", name: "Input", range: (0, 3), start: 1, magnet: None },
    Setting { id: "amount", name: "Amount", range: (0, 100), start: 100, magnet: None },
    Setting { id: "tighten", name: "Tighten", range: (0, 100), start: 0, magnet: None },
    Setting { id: "glide", name: "Transition", range: (5, 200), start: 40, magnet: None },
];

const CARDS: [Card; 5] = [
    Card { x0: 8.0, x1: 208.0, y0: 28.0, y1: 148.0, heading: "KEY" },
    Card { x0: 214.0, x1: 340.0, y0: 28.0, y1: 148.0, heading: "DETECTION" },
    Card { x0: 346.0, x1: 438.0, y0: 28.0, y1: 148.0, heading: "AMOUNT" },
    Card { x0: 444.0, x1: 536.0, y0: 28.0, y1: 148.0, heading: "TIGHTEN" },
    Card { x0: 542.0, x1: 634.0, y0: 28.0, y1: 148.0, heading: "TRANSITION" },
];

const CONTROLS: [Control; N] = [
    Control::Knob { id: "key", cx: 58.0, cy: 82.0, r: 22.0, caption: "", bipolar: false },
    Control::Knob { id: "scale", cx: 156.0, cy: 82.0, r: 22.0, caption: "", bipolar: false },
    Control::Choice { id: "auto", x: 226.0, top: 38.0, names: &["MANUAL KEY", "AUTO KEY"] },
    Control::Choice { id: "voice", x: 226.0, top: 78.0, names: &["HIGH VOICE", "MID VOICE", "LOW VOICE", "INSTRUMENT"] },
    Control::Knob { id: "amount", cx: 392.0, cy: 82.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "tighten", cx: 490.0, cy: 82.0, r: 24.0, caption: "", bipolar: false },
    Control::Knob { id: "glide", cx: 588.0, cy: 82.0, r: 24.0, caption: "", bipolar: false },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Phrase",
    word: "PHRASE",
    about: "Lemon Studio's note-by-note pitch correction",
    settings: &SETTINGS,
    version: Some(1),
    any_version: false,
    fields: &[],
    tail: &[],
    size: (642, 186),
    cards: &CARDS,
    controls: &CONTROLS,
    display: None,
    mark: Mark { x: 634.0, y: 167.0, anchor: 1.0, stacked: false },
    hint: Some(Rect::new(8.0, 156.0, 626.0, 22.0)),
    labels: &[],
    text,
    parse,
    sound,
    rings: false,
};

fn text(k: usize, v: i32) -> String {
    match k {
        KEY => NOTES[v.clamp(0, 11) as usize].into(),
        SCALE => SCALES[v.clamp(0, 13) as usize].0.into(),
        AUTO => if v == 0 { "Manual" } else { "Auto" }.into(),
        VOICE => VOICES[v.clamp(0, 3) as usize].0.into(),
        AMOUNT | TIGHTEN => format!("{v}%"),
        _ => format!("{v}ms"),
    }
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
        _ => leading_number(&low).map(|x| (x.round() as i32).clamp(min, max)),
    }
}

/// How far either side of a moment (s) the pitch is looked at for the note's centre: longer than half a vibrato's
/// cycle, so the median sees through it.
const WINDOW: f32 = 0.085;
/// How much closer (semitones) another note must be to the centre before the target moves to it.
const HOLD: f32 = 0.1;
/// How far from a note (semitones) a centre may be and still count towards the key.
const NEAR: f32 = 0.35;
/// The fade between the corrected and the dry sound (ms), and how fast tighten follows the pitch (ms).
const MIX_MS: f32 = 8.0;
const TIGHT_MS: f32 = 3.0;
/// How many readings ahead tighten takes the pitch from: the cleaned readings lag by about one, and tighten's
/// following and the detector's own delay by about another each.
const LEAD: usize = 3;
/// A note this close to its target (cents) is left as it is, played dry.
const IN_TUNE: f32 = 1.0;

pub struct PhraseFix {
    rate: f32,
    v: [i32; N],
    det: Detector,
    shift: Psola,
    keys: KeyFinder,
    /// Input samples to a reading, and readings looked at either side of a moment.
    hop: usize,
    w: usize,
    /// The last three readings (MIDI note numbers, NaN when unpitched) and the cleaned ones kept (newest last).
    raw: [f32; 3],
    reads: VecDeque<f32>,
    target: Option<i32>,
    /// Per input sample, decided ahead: the note's correction and tighten's (cents), whether it is pitched and its
    /// period (samples).
    ctl: VecDeque<(f32, f32, bool, f32)>,
    /// The input held back to meet its correction, and by how much.
    held: VecDeque<[f32; 2]>,
    delay: usize,
    voiced: bool,
    period: f32,
    corr: f32,
    follow: f32,
    tight: f32,
    tight_follow: f32,
    mix: f32,
    mix_follow: f32,
    scratch: Vec<f32>,
}

impl PhraseFix {
    pub fn new(rate: u32, v: &[i32]) -> PhraseFix {
        let mut s = PhraseFix {
            rate: rate.max(1) as f32,
            v: [0; N],
            det: Detector::new(44100, 80.0, 700.0),
            shift: Psola::new(64, 0),
            keys: KeyFinder::default(),
            hop: 1,
            w: 1,
            raw: [f32::NAN; 3],
            reads: VecDeque::new(),
            target: None,
            ctl: VecDeque::new(),
            held: VecDeque::new(),
            delay: 0,
            voiced: false,
            period: 200.0,
            corr: 0.0,
            follow: 1.0,
            tight: 0.0,
            tight_follow: 1.0,
            mix: 0.0,
            mix_follow: 0.0,
            scratch: Vec::new(),
        };
        s.v.copy_from_slice(&v[..N]);
        s.rebuild();
        s
    }

    /// New detector, shifter and look-ahead for the input range and rate (forgets what they hold).
    fn rebuild(&mut self) {
        let (_, lo, hi) = VOICES[self.v[VOICE].clamp(0, 3) as usize];
        self.det = Detector::new(self.rate as u32, lo, hi);
        self.hop = HOP * self.det.factor;
        self.w = ((WINDOW * self.rate / self.hop as f32).round() as usize).max(LEAD);
        self.shift = Psola::new((self.rate / lo).ceil() as usize, 0);
        // A reading describes the input `det.delay()` before it, and a moment is decided `w` readings after its own:
        // the input is held back that much and a reading more. The decisions start out that far ahead of the input
        // they are for, less the first reading's place and half the stretch each one covers.
        self.delay = self.w * self.hop + self.det.delay() + self.hop;
        self.held = VecDeque::from(vec![[0.0; 2]; self.delay]);
        let ahead = (self.delay + self.hop - 1).saturating_sub(self.det.delay() + self.hop / 2);
        self.period = self.rate / 200.0;
        self.ctl = VecDeque::from(vec![(0.0, 0.0, false, self.period); ahead]);
        (self.raw, self.target, self.voiced, self.corr, self.tight, self.mix) = ([f32::NAN; 3], None, false, 0.0, 0.0, 0.0);
        self.reads.clear();
        self.mix_follow = coef(MIX_MS, self.rate);
        self.tight_follow = coef(TIGHT_MS, self.rate);
        self.follow = coef(self.v[GLIDE] as f32, self.rate);
    }

    /// The notes the target may land on: the scale set, or (with key detection) the key found, every note until then.
    fn allowed(&self) -> u16 {
        let (key, scale) = if self.v[AUTO] != 0 {
            match self.keys.key {
                Some((k, minor)) => (k, if minor { MINOR } else { MAJOR }),
                None => (0, CHROMATIC),
            }
        } else {
            (self.v[KEY], self.v[SCALE].clamp(0, 13) as usize)
        };
        match scale_mask(key, scale) {
            0 => 0xfff,
            m => m,
        }
    }

    /// A new reading: cleaned and kept, and the moment `w` readings back decided.
    fn reading(&mut self) {
        let f = self.det.freq;
        let note = if f > 0.0 { 69.0 + 12.0 * (f / 440.0).log2() } else { f32::NAN };
        self.raw = [self.raw[1], self.raw[2], note];
        let mut heard: Vec<f32> = self.raw.iter().copied().filter(|x| !x.is_nan()).collect();
        heard.sort_by(f32::total_cmp);
        let clean = match heard.len() {
            3 => heard[1],
            2 => 0.5 * (heard[0] + heard[1]),
            _ => f32::NAN,
        };
        self.reads.push_back(clean);
        if self.reads.len() > 2 * self.w + 1 {
            self.reads.pop_front();
        }
        if self.reads.len() > self.w {
            let j = self.reads.len() - 1 - self.w;
            let decided = self.decide(j);
            self.ctl.extend(std::iter::repeat_n(decided, self.hop));
        }
    }

    /// The correction for reading `j` (whose next `w` readings are in).
    fn decide(&mut self, j: usize) -> (f32, f32, bool, f32) {
        let r = self.reads[j];
        if r.is_nan() {
            self.target = None;
            return (0.0, 0.0, false, self.period);
        }
        // The pitched stretch around it, `w` readings either side at most, and its median.
        let (mut a, mut b) = (j, j);
        while a > 0 && j - a < self.w && !self.reads[a - 1].is_nan() {
            a -= 1;
        }
        while b + 1 < self.reads.len() && b - j < self.w && !self.reads[b + 1].is_nan() {
            b += 1;
        }
        self.scratch.clear();
        self.scratch.extend(self.reads.range(a..=b).copied());
        let centre = crate::spectral::median(&mut self.scratch);
        if (centre - centre.round()).abs() < NEAR {
            self.keys.add(centre.round() as i32, self.hop as f32 / self.rate);
        }
        let allowed = self.allowed();
        let on = |n: i32| allowed & (1 << n.rem_euclid(12)) != 0;
        let base = centre.round() as i32;
        let gap = |n: i32| (n as f32 - centre).abs();
        let best = (base - 6..=base + 6).filter(|&n| on(n)).min_by(|&p, &q| gap(p).total_cmp(&gap(q))).unwrap_or(base);
        let t = match self.target {
            Some(cur) if on(cur) && gap(cur) < gap(best) + HOLD => cur,
            _ => best,
        };
        self.target = Some(t);
        let amount = self.v[AMOUNT] as f32;
        let now = match self.reads[j + LEAD] {
            q if q.is_nan() => r,
            q => q,
        };
        let tighten = self.v[TIGHTEN] as f32 / 100.0;
        let period = self.rate / (440.0 * 2f32.powf((r - 69.0) / 12.0));
        (amount * (t as f32 - centre), -amount * tighten * (now - centre), true, period)
    }
}

impl Sound for PhraseFix {
    fn set(&mut self, v: &[i32]) {
        if v[..N] != self.v {
            let again = v[VOICE] != self.v[VOICE];
            self.v.copy_from_slice(&v[..N]);
            self.follow = coef(self.v[GLIDE] as f32, self.rate);
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
            self.held.push_back([*l, *r]);
            let x = self.held.pop_front().unwrap_or([0.0; 2]);
            let (cents, tight, voiced, period) = self.ctl.pop_front().unwrap_or((0.0, 0.0, false, self.period));
            // A note's correction is known before it starts: it starts on it, and glides only between notes.
            if voiced && !self.voiced {
                self.corr = cents;
            }
            self.voiced = voiced;
            if voiced {
                self.period = period;
            }
            // The note's correction glides over the transition; tighten follows the pitch's own movement closely.
            self.corr += (cents - self.corr) * self.follow;
            self.tight += (tight - self.tight) * self.tight_follow;
            let (wet, dry, pitched) = self.shift.run(x, self.period, 2f32.powf((self.corr + self.tight) / 1200.0), voiced);
            let needed = self.corr.abs().max(cents.abs()) >= IN_TUNE || self.v[TIGHTEN] > 0;
            let to = if pitched && needed && self.v[AMOUNT] > 0 { 1.0 } else { 0.0 };
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
        (self.delay + self.shift.lat) as u32
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(PhraseFix::new(rate, v))
}

pub struct PhraseKind;

impl Kind for PhraseKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-phrase";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's note-by-note pitch correction";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::PitchCorrection, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioPhras";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::PitchShift];
}

pub type LemonPhrase = SmallFx<PhraseKind>;

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44100;

    fn start() -> [i32; N] {
        SPEC.start().try_into().unwrap()
    }

    fn cents(a: f32, b: f32) -> f32 {
        1200.0 * (a / b).log2()
    }

    /// A voice-like line: per note its pitch (Hz) and length (s), a `vib` cent, 5.5 Hz vibrato, harmonics falling off
    /// as 1/k, the phase running on across the notes.
    fn line(notes: &[(f32, f32)], vib: f32) -> Vec<f32> {
        let mut phase = 0.0f32;
        let mut out = Vec::new();
        for &(hz, secs) in notes {
            for i in 0..(secs * RATE as f32) as usize {
                let t = (out.len() + i) as f32 / RATE as f32;
                let f = hz * 2f32.powf(vib * (std::f32::consts::TAU * 5.5 * t).sin() / 1200.0);
                phase = (phase + std::f32::consts::TAU * f / RATE as f32) % std::f32::consts::TAU;
                out.push((1..=6).map(|k| 0.3 / k as f32 * (k as f32 * phase).sin()).sum::<f32>());
            }
        }
        out
    }

    fn play(v: &[i32], x: &[f32]) -> (Vec<f32>, usize) {
        let mut s = PhraseFix::new(RATE, v);
        let (mut l, mut r) = (x.to_vec(), x.to_vec());
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            s.process(a, b);
        }
        assert_eq!(l, r);
        (l, s.latency() as usize)
    }

    /// The pitch track (cents from `hz`) of `x[from..to]`.
    fn track(x: &[f32], from: usize, to: usize, hz: f32) -> Vec<f32> {
        let mut d = Detector::new(RATE, 55.0, 1100.0);
        let mut c = Vec::new();
        for (i, &s) in x[..to.min(x.len())].iter().enumerate() {
            if d.push(s) && i >= from && d.freq > 0.0 {
                c.push(cents(d.freq, hz));
            }
        }
        assert!(!c.is_empty(), "no pitch read");
        c
    }

    fn median_of(mut c: Vec<f32>) -> f32 {
        crate::spectral::median(&mut c)
    }

    #[test]
    fn value_texts_and_typed_values() {
        assert_eq!(text(KEY, 10), "A#");
        assert_eq!(text(SCALE, 2), "Natural minor");
        assert_eq!(text(AUTO, 1), "Auto");
        assert_eq!(text(TIGHTEN, 30), "30%");
        assert_eq!(text(GLIDE, 40), "40ms");
        assert_eq!(parse(KEY, "Eb"), Some(3));
        assert_eq!(parse(SCALE, "dorian"), Some(5));
        assert_eq!(parse(VOICE, "high"), Some(0));
        assert_eq!(parse(GLIDE, "1000"), Some(200));
    }

    /// Three notes, each out of tune by its own amount, come out each on its own note.
    #[test]
    fn each_note_is_corrected_on_its_own() {
        let notes = [(440.0 * 2f32.powf(-30.0 / 1200.0), 0.5), (493.88 * 2f32.powf(25.0 / 1200.0), 0.5), (523.25 * 2f32.powf(-40.0 / 1200.0), 0.5)];
        let (out, lat) = play(&start(), &line(&notes, 0.0));
        let len = (0.5 * RATE as f32) as usize;
        for (k, want) in [440.0, 493.88, 523.25].into_iter().enumerate() {
            let (from, to) = (lat + k * len + len / 4, lat + (k + 1) * len - len / 8);
            let got = median_of(track(&out, from, to, want));
            assert!(got.abs() < 6.0, "note {k}: {got} cents off");
        }
    }

    /// A note sung flat with vibrato comes out on the note with its vibrato as wide as it was; tighten narrows it.
    #[test]
    fn vibrato_is_kept_and_tighten_narrows_it() {
        let x = line(&[(220.0 * 2f32.powf(-35.0 / 1200.0), 2.0)], 40.0);
        let spread = |c: &[f32]| {
            let m = c.iter().sum::<f32>() / c.len() as f32;
            (c.iter().map(|x| (x - m).powi(2)).sum::<f32>() / c.len() as f32).sqrt()
        };
        let input = track(&x, 22050, x.len(), 220.0);
        let (out, lat) = play(&start(), &x);
        let c = track(&out, lat + 22050, out.len(), 220.0);
        let mean = c.iter().sum::<f32>() / c.len() as f32;
        println!("mean {mean:.1}, spread {:.1} -> {:.1}", spread(&input), spread(&c));
        assert!(mean.abs() < 6.0, "{mean} cents off");
        assert!(spread(&c) > 0.8 * spread(&input), "vibrato {} of {}", spread(&c), spread(&input));
        let mut v = start();
        v[TIGHTEN] = 100;
        let (out, lat) = play(&v, &x);
        let c = track(&out, lat + 22050, out.len(), 220.0);
        assert!(spread(&c) < 0.4 * spread(&input), "tightened vibrato {} of {}", spread(&c), spread(&input));
    }

    /// With a key set, a note goes to the nearest note of its scale; a note in tune stays.
    #[test]
    fn the_scale_set_is_used() {
        let mut v = start();
        (v[AUTO], v[KEY], v[SCALE]) = (0, 0, MAJOR as i32);
        // C# sung 30 cents flat: C major has no C#, so it goes to C (D is further).
        let (out, lat) = play(&v, &line(&[(277.18 * 2f32.powf(-30.0 / 1200.0), 1.0)], 0.0));
        let got = median_of(track(&out, lat + 11025, out.len(), 261.63));
        assert!(got.abs() < 6.0, "{got} cents from C");
        let (out, lat) = play(&v, &line(&[(329.63, 1.0)], 0.0));
        let got = median_of(track(&out, lat + 11025, out.len(), 329.63));
        assert!(got.abs() < 2.0, "{got} cents from E");
    }

    /// A note already in tune is left alone: it plays dry, exactly the input delayed by the latency.
    #[test]
    fn an_in_tune_note_passes_unchanged() {
        let x = line(&[(220.0, 1.5)], 0.0);
        let (out, lat) = play(&start(), &x);
        let same = (lat + 22050..x.len()).filter(|&i| (out[i] - x[i - lat]).abs() < 1e-6).count();
        assert!(same as f32 > 0.99 * (x.len() - lat - 22050) as f32, "{same}");
    }

    /// Unpitched input (noise) plays dry, delayed by the latency.
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
        let same = (lat..x.len()).filter(|&i| (out[i] - x[i - lat]).abs() < 1e-6).count();
        assert!(same as f32 > 0.97 * (x.len() - lat) as f32, "{same}");
    }
}
