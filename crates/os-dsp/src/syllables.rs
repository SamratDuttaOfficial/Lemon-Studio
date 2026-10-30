// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What a voice recording is doing, moment by moment, read from the sound alone (no words, so any language): where
//! its syllables land, where its consonant attacks are, which stretches are voiced, unvoiced or silent, and how each
//! moment looks (its spectral shape, for lining one take up with another). The retiming tools (`flow`, `align`,
//! `comp`) build on it.
//!
//! - A syllable's nucleus, its vowel, is a peak of the voiced speech-band loudness with a dip either side (de Jong and
//!   Wempe's syllable nuclei, 2009: a 2 dB dip, 25 dB under the loudest); bursts and hiss, unvoiced, are no nuclei.
//! - The moment a syllable is heard to land, its perceptual centre, is the fastest rise of its syllabic loudness on the
//!   way to the peak (Morton, Marcus and Frankish, 1976; an acoustic model of 2024 places it at the fastest rise), read
//!   in the first-formant band where Scott's model (1993) finds the cue: the vowel onset, whatever consonant is before
//!   it.
//! - Consonant attacks are the onsets of the spectral flux, its frame-to-frame rises taken over a maximum filter across
//!   neighbouring bands so that a voice's vibrato does not count (SuperFlux; Böck and Widmer, 2013).
//! - Voiced: a periodicity (normalised autocorrelation peak between 70 and 400 Hz) of 0.4 and up.

use crate::tone::Fft;

/// Speech-band edges, Hz.
const SPEECH: (f64, f64) = (250.0, 4000.0);
/// Nuclei: louder than the loudest (99th percentile) less this, dB.
const NUCLEUS_FLOOR: f32 = 25.0;
/// The dip a nucleus needs on either side, dB.
const DIP: f32 = 2.0;
/// Nuclei closer than this are one, seconds.
const MIN_GAP: f64 = 0.045;
/// Periodicity that counts as voiced.
pub const VOICED: f32 = 0.4;
/// The loudness is smoothed over this, seconds.
const SMOOTH: f64 = 0.015;
/// The syllabic loudness whose fastest rise is a syllable's centre: the first-formant band (Hz), smoothed over this
/// (a Hann window, seconds), read every `FINE` seconds.
const FORMANT: (f64, f64) = (300.0, 1500.0);
const SYLLABIC: f64 = 0.025;
const FINE: f64 = 0.001;
/// How far back from a nucleus its consonant may start, seconds.
const REACH: f64 = 0.25;
/// Flux onsets: above this share of the voice's frames (a percentile), and the biggest in this many frames each side.
const ATTACK_SHARE: f64 = 0.85;
const ATTACK_SPAN: usize = 2;
/// Kept as it is around a syllable: from its attack (or this long before its centre when it has none), to this long
/// after its centre (the consonant-vowel transition); around an attack, this long either side and after. Longer than
/// half a `warp` frame, so a moved attack is copied whole.
const HOLD_BEFORE: f64 = 0.04;
const HOLD_AFTER: f64 = 0.035;
const HOLD_PRE: f64 = 0.012;
const HOLD_POST: f64 = 0.02;

/// One syllable, times in seconds from the recording's start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Syllable {
    /// Its perceptual centre: where it lands.
    pub at: f64,
    /// Where its consonant starts (its first attack before the centre, else a little before it).
    pub attack: f64,
    /// Its vowel's loudest moment, and how loud, dB.
    pub peak: f64,
    pub level: f32,
}

/// How a stretch of the recording may be retimed (`retime`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// An attack or a consonant-vowel transition: moved, never stretched.
    Hold,
    Silent,
    Voiced,
    /// Unvoiced sound: fricatives, breaths.
    Other,
}

/// The reading of a recording.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    pub rate: u32,
    /// Seconds per frame; frame `f` is centred on `f * hop`.
    pub hop: f64,
    /// The recording's length, seconds.
    pub len: f64,
    /// Speech-band level, dB, smoothed.
    pub speech: Vec<f32>,
    /// Whole level, dB.
    pub level: Vec<f32>,
    /// Onset strength (spectral flux).
    pub flux: Vec<f32>,
    /// Periodicity, 0 .. 1.
    pub voiced: Vec<f32>,
    /// How each frame looks: `bands` log band levels (third octaves, 100 Hz up) per frame.
    pub look: Vec<f32>,
    pub bands: usize,
    /// Under this whole level a frame is silence, dB.
    pub silence: f32,
    pub syllables: Vec<Syllable>,
    /// Consonant attacks, seconds.
    pub attacks: Vec<f64>,
}

impl Scan {
    pub fn new(mono: &[f32], rate: u32) -> Scan {
        let rf = f64::from(rate.max(1));
        let n = 1usize << ((rf * 0.023).log2().round() as u32).clamp(8, 14);
        let hop_n = n / 4;
        let frames = mono.len() / hop_n + 1;
        let fft = Fft::new(n);
        let win: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
        let bin = rf / n as f64;
        let k = |hz: f64| ((hz / bin).round() as usize).clamp(1, n / 2);
        let (s_lo, s_hi) = (k(SPEECH.0), k(SPEECH.1));
        let low = k(60.0);
        let top = (0.95 * rf / 2.0).min(12500.0);
        let mut edges = Vec::new();
        let mut f = 100.0;
        while f < top {
            edges.push(f);
            f *= 2f64.powf(1.0 / 3.0);
        }
        edges.push(top);
        let bands: Vec<(usize, usize)> = edges.windows(2).map(|e| (k(e[0]), k(e[1]).max(k(e[0]) + 1))).collect();
        let nb = bands.len();
        // A full-scale sine's bin to 1.
        let scale = 4.0 / n as f32;
        let (mut speech_pow, mut level, mut look) = (vec![0.0f32; frames], vec![0.0f32; frames], vec![0.0f32; frames * nb]);
        let (mut re, mut im, mut mag) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n / 2 + 1]);
        for f in 0..frames {
            let c = (f * hop_n) as isize - (n / 2) as isize;
            for i in 0..n {
                let j = c + i as isize;
                re[i] = if j >= 0 && (j as usize) < mono.len() { mono[j as usize] * win[i] } else { 0.0 };
                im[i] = 0.0;
            }
            fft.run(&mut re, &mut im);
            for b in 0..=n / 2 {
                mag[b] = (re[b] * re[b] + im[b] * im[b]).sqrt() * scale;
            }
            speech_pow[f] = mag[s_lo..s_hi].iter().map(|m| m * m).sum();
            level[f] = 10.0 * (mag[low..=n / 2].iter().map(|m| m * m).sum::<f32>() + 1e-12).log10();
            for (b, &(lo, hi)) in bands.iter().enumerate() {
                let mean = mag[lo..hi.min(n / 2 + 1)].iter().sum::<f32>() / (hi.min(n / 2 + 1) - lo).max(1) as f32;
                look[f * nb + b] = (1.0 + 1000.0 * mean).ln();
            }
        }
        // SuperFlux: the rise of each band over the frame two before, its neighbours' maximum.
        let mut flux = vec![0.0f32; frames];
        for f in 2..frames {
            let (now, then) = (&look[f * nb..(f + 1) * nb], &look[(f - 2) * nb..(f - 1) * nb]);
            flux[f] = (0..nb).map(|b| (now[b] - then[b.saturating_sub(1)..(b + 2).min(nb)].iter().fold(f32::MIN, |m, v| m.max(*v))).max(0.0)).sum();
        }
        let w = (((SMOOTH / (hop_n as f64 / rf)).round() as usize) | 1).max(1);
        let smooth: Vec<f32> = (0..frames)
            .map(|f| {
                let (a, b) = (f.saturating_sub(w / 2), (f + w / 2 + 1).min(frames));
                speech_pow[a..b].iter().sum::<f32>() / (b - a) as f32
            })
            .collect();
        let speech: Vec<f32> = smooth.iter().map(|p| 10.0 * (p + 1e-12).log10()).collect();
        let voiced = periodicity(mono, rate, hop_n, frames);
        // Nuclei are looked for in the voiced part of the loudness: the level less what the periodicity lacks.
        let nuclei: Vec<f32> = speech.iter().zip(&voiced).map(|(l, v)| l + 20.0 * v.max(0.05).log10()).collect();
        let fine = syllabic(mono, rate);
        let (top_l, low_l) = (percentile(&level, 0.99), percentile(&level, 0.10));
        let silence = (low_l + 10.0).max(top_l - 50.0).min(top_l - 30.0);
        let mut scan = Scan {
            rate,
            hop: hop_n as f64 / rf,
            len: mono.len() as f64 / rf,
            speech,
            level,
            flux,
            voiced,
            look,
            bands: nb,
            silence,
            syllables: Vec::new(),
            attacks: Vec::new(),
        };
        scan.attacks = scan.find_attacks();
        scan.syllables = scan.find_syllables(&nuclei, &fine);
        scan
    }

    pub fn frames(&self) -> usize {
        self.speech.len()
    }

    /// The frame nearest `t` seconds.
    pub fn frame(&self, t: f64) -> usize {
        ((t / self.hop).round().max(0.0) as usize).min(self.frames().saturating_sub(1))
    }

    pub fn silent(&self, f: usize) -> bool {
        self.level.get(f).is_none_or(|l| *l < self.silence)
    }

    fn find_attacks(&self) -> Vec<f64> {
        let voice: Vec<f32> = (0..self.frames()).filter(|&f| !self.silent(f)).map(|f| self.flux[f]).collect();
        if voice.is_empty() {
            return Vec::new();
        }
        let thr = percentile(&voice, ATTACK_SHARE).max(1e-3);
        let n = self.frames();
        (0..n)
            .filter(|&f| {
                let (a, b) = (f.saturating_sub(ATTACK_SPAN), (f + ATTACK_SPAN + 1).min(n));
                self.flux[f] > thr && !self.silent(f) && (a..b).all(|g| g == f || self.flux[g] < self.flux[f] || (self.flux[g] == self.flux[f] && g > f))
            })
            .map(|f| f as f64 * self.hop)
            .collect()
    }

    fn find_syllables(&self, s: &[f32], fine: &[f32]) -> Vec<Syllable> {
        let n = self.frames();
        if n < 3 {
            return Vec::new();
        }
        let floor = percentile(&self.speech, 0.99) - NUCLEUS_FLOOR;
        let gap = (MIN_GAP / self.hop).round() as usize;
        let mut kept: Vec<usize> = Vec::new();
        for f in 1..n - 1 {
            if !(s[f] > s[f - 1] && s[f] >= s[f + 1] && s[f] > floor && self.voiced[f] >= VOICED) {
                continue;
            }
            if let Some(&last) = kept.last() {
                let valley = s[last..=f].iter().fold(f32::MAX, |m, v| m.min(*v));
                if f - last < gap || s[last].min(s[f]) - valley < DIP {
                    if s[f] > s[last] {
                        *kept.last_mut().expect("one kept") = f;
                    }
                    continue;
                }
            }
            kept.push(f);
        }
        let reach = (REACH / self.hop).round() as usize;
        let mut out = Vec::with_capacity(kept.len());
        let mut prev = 0usize;
        for (i, &pk) in kept.iter().enumerate() {
            let from = if i == 0 { pk.saturating_sub(reach) } else { prev.max(pk.saturating_sub(reach)) };
            let valley = self.valley_before(pk, from, floor);
            let v_t = valley as f64 * self.hop;
            // The fastest rise of the syllabic loudness from the valley to the peak, to a fraction of a millisecond.
            let m = fine.len();
            let d = |i: usize| if i < 2 || i + 2 >= m { 0.0 } else { fine[i + 2] - fine[i - 2] };
            let (a, b) = (((v_t / FINE) as usize).min(m.saturating_sub(1)), ((pk as f64 * self.hop / FINE) as usize).min(m.saturating_sub(1)));
            let best = (a..=b.max(a)).max_by(|&x, &y| d(x).total_cmp(&d(y))).unwrap_or(a);
            let (y0, y1, y2) = (d(best.saturating_sub(1)), d(best), d(best + 1));
            let den = y0 - 2.0 * y1 + y2;
            let off = if best > 0 && den.abs() > 1e-12 { (0.5 * (y0 - y2) / den).clamp(-0.5, 0.5) } else { 0.0 };
            let at = (best as f64 + f64::from(off)) * FINE;
            let attack = self.attacks.iter().copied().find(|&a| a >= v_t - self.hop && a <= at).unwrap_or((at - HOLD_BEFORE).max(v_t));
            out.push(Syllable { at, attack: attack.min(at), peak: pk as f64 * self.hop, level: self.speech[pk] });
            prev = pk;
        }
        out
    }

    /// The dip the vowel peaking at frame `pk` rises from: back from the peak, the lowest level before the level climbs
    /// `DIP` above it again (a consonant before it) or falls into silence, no further back than `from`.
    fn valley_before(&self, pk: usize, from: usize, floor: f32) -> usize {
        let s = &self.speech;
        let mut low = pk;
        for f in (from..pk).rev() {
            if s[f] < s[low] {
                low = f;
            }
            if s[f] > s[low] + DIP || s[f] < floor - 10.0 {
                break;
            }
        }
        low
    }

    /// How each frame may be retimed: silent, voiced or other, and held around attacks and syllable onsets.
    pub fn classes(&self) -> Vec<Class> {
        let n = self.frames();
        let mut c: Vec<Class> = (0..n)
            .map(|f| if self.silent(f) { Class::Silent } else if self.voiced[f] >= VOICED { Class::Voiced } else { Class::Other })
            .collect();
        let mut hold = |a: f64, b: f64| {
            let (fa, fb) = (((a / self.hop) - 0.5).ceil().max(0.0) as usize, (((b / self.hop) + 0.5).floor().max(0.0) as usize).min(n.saturating_sub(1)));
            for x in c.iter_mut().take(fb + 1).skip(fa) {
                *x = Class::Hold;
            }
        };
        for s in &self.syllables {
            hold(s.attack - HOLD_PRE, s.at + HOLD_AFTER);
        }
        for &a in &self.attacks {
            hold(a - HOLD_PRE, a + HOLD_POST);
        }
        c
    }

    /// Where the voice stops before a silence (phrase ends), seconds: the end of the last frame with sound before at
    /// least `pause` seconds of silence.
    pub fn phrase_ends(&self, pause: f64) -> Vec<f64> {
        let need = (pause / self.hop).ceil() as usize;
        let n = self.frames();
        let mut out = Vec::new();
        let mut f = 0;
        while f < n {
            if !self.silent(f) && f + 1 < n && self.silent(f + 1) {
                let run = (f + 1..n).take_while(|&g| self.silent(g)).count();
                if run >= need || f + 1 + run == n {
                    out.push((f as f64 + 0.5) * self.hop);
                }
                f += run + 1;
            } else {
                f += 1;
            }
        }
        out
    }
}

/// The syllabic loudness: the first-formant band (zero-phase band-pass), its power under a Hann window `SYLLABIC`
/// long, as an amplitude every `FINE` seconds.
fn syllabic(mono: &[f32], rate: u32) -> Vec<f32> {
    let rf = f64::from(rate.max(1));
    let mut y: Vec<f64> = mono.iter().map(|v| f64::from(*v)).collect();
    for (hz, high) in [(FORMANT.0, true), (FORMANT.1, false)] {
        let b = Biquad::new(hz / rf, high);
        b.run(&mut y);
        y.reverse();
        b.run(&mut y);
        y.reverse();
    }
    // The power in bins of half a step, then the window over them, a step apart.
    let bin = ((FINE * rf / 2.0).round() as usize).max(1);
    let bins: Vec<f64> = y.chunks(bin).map(|c| c.iter().map(|v| v * v).sum::<f64>() / c.len() as f64).collect();
    let taps = ((SYLLABIC * rf / bin as f64).round() as usize).max(3) | 1;
    let w: Vec<f64> = (0..taps).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * (i as f64 + 0.5) / taps as f64).cos()).collect();
    let total: f64 = w.iter().sum();
    let half = taps / 2;
    (0..bins.len().div_ceil(2))
        .map(|i| {
            let c = 2 * i;
            let acc: f64 = (0..taps).filter_map(|k| (c + k).checked_sub(half).and_then(|j| bins.get(j)).map(|p| p * w[k])).sum();
            (acc / total).sqrt() as f32
        })
        .collect()
}

/// A second-order Butterworth high- or low-pass (RBJ), `f` the cutoff over the rate.
struct Biquad([f64; 5]);

impl Biquad {
    fn new(f: f64, high: bool) -> Biquad {
        let w = std::f64::consts::TAU * f.min(0.49);
        let (c, alpha) = (w.cos(), w.sin() / std::f64::consts::SQRT_2);
        let a0 = 1.0 + alpha;
        let (b0, b1) = if high { ((1.0 + c) / 2.0, -(1.0 + c)) } else { ((1.0 - c) / 2.0, 1.0 - c) };
        Biquad([b0 / a0, b1 / a0, b0 / a0, -2.0 * c / a0, (1.0 - alpha) / a0])
    }

    fn run(&self, y: &mut [f64]) {
        let [b0, b1, b2, a1, a2] = self.0;
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for v in y.iter_mut() {
            let x = *v;
            let out = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            (x2, x1, y2, y1) = (x1, x, y1, out);
            *v = out;
        }
    }
}

/// Periodicity per frame: the normalised autocorrelation peak between 70 and 400 Hz, on a copy at about 12 kHz, every
/// other frame (the ones between take their neighbours' larger).
fn periodicity(mono: &[f32], rate: u32, hop_n: usize, frames: usize) -> Vec<f32> {
    let q = ((rate / 12000) as usize).max(1);
    let dec: Vec<f32> = mono.chunks(q).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
    let dr = f64::from(rate) / q as f64;
    let m = 1usize << ((dr * 0.04).log2().round() as u32).clamp(6, 12);
    let fft = Fft::new(2 * m);
    let (tmin, tmax) = (((dr / 400.0) as usize).max(1), ((dr / 70.0) as usize).min(m * 3 / 4));
    let mut out = vec![0.0f32; frames];
    let (mut re, mut im) = (vec![0.0f32; 2 * m], vec![0.0f32; 2 * m]);
    for f in (0..frames).step_by(2) {
        let c = (f * hop_n / q) as isize - (m / 2) as isize;
        let at = |i: usize| -> f32 {
            let j = c + i as isize;
            if j >= 0 && (j as usize) < dec.len() { dec[j as usize] } else { 0.0 }
        };
        let mean = (0..m).map(at).sum::<f32>() / m as f32;
        for i in 0..2 * m {
            re[i] = if i < m { at(i) - mean } else { 0.0 };
            im[i] = 0.0;
        }
        fft.run(&mut re, &mut im);
        for i in 0..2 * m {
            re[i] = re[i] * re[i] + im[i] * im[i];
            im[i] = 0.0;
        }
        fft.run(&mut re, &mut im);
        let r0 = re[0];
        if r0 <= 1e-9 {
            continue;
        }
        let best = (tmin..=tmax).map(|t| re[t] / r0 * m as f32 / (m - t) as f32).fold(0.0f32, f32::max);
        out[f] = best.min(1.0);
    }
    for f in (1..frames).step_by(2) {
        out[f] = out[f - 1].max(out.get(f + 1).copied().unwrap_or(0.0));
    }
    out
}

/// The value `share` of the way up `x` sorted (0: the least, 1: the most).
pub fn percentile(x: &[f32], share: f64) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    let mut v = x.to_vec();
    v.sort_by(f32::total_cmp);
    v[((share.clamp(0.0, 1.0) * (v.len() - 1) as f64).round() as usize).min(v.len() - 1)]
}

#[cfg(test)]
pub(crate) mod voice {
    //! A rough voice for the tests: syllables of a consonant (a plosive burst, a fricative or none) and a vowel (a
    //! pulse train through three formants), at chosen times.

    /// A syllable's consonant.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Onset {
        Burst,
        Hiss,
        Soft,
    }

    /// Noise in -1 .. 1, the same every run.
    pub struct Noise(u64);

    impl Noise {
        pub fn new(seed: u64) -> Noise {
            Noise(seed | 1)
        }

        pub fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 23) as f32 - 1.0
        }
    }

    fn resonate(x: &[f32], rate: f32, hz: f32, bw: f32) -> Vec<f32> {
        let r = (-std::f32::consts::PI * bw / rate).exp();
        let (a1, a2) = (2.0 * r * (std::f32::consts::TAU * hz / rate).cos(), -r * r);
        let (mut y1, mut y2) = (0.0f32, 0.0f32);
        x.iter()
            .map(|&v| {
                let y = (1.0 - r) * v + a1 * y1 + a2 * y2;
                (y2, y1) = (y1, y);
                y
            })
            .collect()
    }

    /// Adds a syllable whose vowel starts at `vowel` seconds and lasts `len` seconds (pitch `f0` Hz) to `out`.
    pub fn syllable(out: &mut [f32], rate: u32, vowel: f64, len: f64, f0: f32, onset: Onset, noise: &mut Noise) {
        sound(out, rate, vowel, len, f0, [700.0, 1220.0, 2600.0], onset, noise);
    }

    /// `syllable` with its vowel's three formants, Hz.
    #[allow(clippy::too_many_arguments)]
    pub fn sound(out: &mut [f32], rate: u32, vowel: f64, len: f64, f0: f32, formants: [f32; 3], onset: Onset, noise: &mut Noise) {
        let rf = rate as f32;
        let n = (len * f64::from(rate)) as usize;
        let mut src = vec![0.0f32; n];
        let period = rf / f0;
        let mut t = 0.0f32;
        while (t as usize) < n {
            src[t as usize] = 1.0;
            t += period;
        }
        let a = resonate(&src, rf, formants[0], 90.0);
        let b = resonate(&src, rf, formants[1], 110.0);
        let c = resonate(&src, rf, formants[2], 160.0);
        let start = (vowel * f64::from(rate)) as usize;
        let rise = (0.012 * rf) as usize;
        let fall = (0.03 * rf) as usize;
        for i in 0..n {
            let env = if i < rise { 0.5 - 0.5 * (std::f32::consts::PI * i as f32 / rise as f32).cos() } else if i + fall > n { (n - i) as f32 / fall as f32 } else { 1.0 };
            if let Some(o) = out.get_mut(start + i) {
                *o += 1.6 * env * (a[i] + 0.7 * b[i] + 0.4 * c[i]);
            }
        }
        let (before, level) = match onset {
            Onset::Burst => (0.012, 0.3),
            Onset::Hiss => (0.06, 0.08),
            Onset::Soft => return,
        };
        let m = (before * f64::from(rate)) as usize;
        let from = start.saturating_sub(m);
        let mut prev = 0.0f32;
        for i in 0..m {
            let x = noise.next();
            // Bursts decay; both are high-passed noise, as a consonant's noise is.
            let v = match onset {
                Onset::Burst => (x - prev) * (-(i as f32) / (0.003 * rf)).exp(),
                _ => x - prev,
            };
            prev = x;
            if let Some(o) = out.get_mut(from + i) {
                *o += level * v;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::voice::*;
    use super::*;

    /// Syllables a third of a second apart, each of the three onsets: one each, where the vowel starts (within 15 ms
    /// either side: a burst draws the centre a little earlier, a soft onset's rise a little later), the consonants'
    /// attacks found, the vowels voiced, the gaps silent.
    #[test]
    fn finds_the_syllables_where_they_land() {
        let rate = 48000;
        let mut x = vec![0.0f32; rate as usize * 3];
        let mut noise = Noise::new(7);
        let onsets = [Onset::Burst, Onset::Hiss, Onset::Soft];
        let starts: Vec<f64> = (0..8).map(|i| 0.2 + i as f64 * 0.33).collect();
        for (i, &t) in starts.iter().enumerate() {
            syllable(&mut x, rate, t, 0.16, 130.0 + 10.0 * i as f32, onsets[i % 3], &mut noise);
        }
        let s = Scan::new(&x, rate);
        let at: Vec<f64> = s.syllables.iter().map(|y| y.at).collect();
        assert_eq!(at.len(), starts.len(), "{at:?}");
        for (a, t) in at.iter().zip(&starts) {
            assert!((a - t).abs() <= 0.015, "landed at {a}, vowel at {t}");
        }
        for (i, y) in s.syllables.iter().enumerate() {
            assert!(y.attack <= y.at && y.attack >= starts[i] - 0.08, "attack {} for {}", y.attack, starts[i]);
            assert!(s.voiced[s.frame(y.peak)] >= VOICED);
        }
        assert!(s.silent(s.frame(0.1)) && s.silent(s.frame(starts[0] + 0.22)));
        let c = s.classes();
        assert_eq!(c[s.frame(starts[0] + 0.1)], Class::Voiced);
        assert_eq!(c[s.frame(starts[0] + 0.005)], Class::Hold);
        assert_eq!(c[s.frame(starts[0] + 0.22)], Class::Silent);
        let ends = s.phrase_ends(0.05);
        assert_eq!(ends.len(), starts.len());
        assert!((ends[0] - (starts[0] + 0.16)).abs() < 0.03, "{}", ends[0]);
    }

    /// Fast syllables, eighth notes at 140 BPM with no silence between them, are still told apart.
    #[test]
    fn tells_apart_syllables_run_together() {
        let rate = 44100;
        let mut x = vec![0.0f32; rate as usize * 2];
        let mut noise = Noise::new(3);
        let step = 60.0 / 140.0 / 2.0;
        for i in 0..8 {
            syllable(&mut x, rate, 0.1 + i as f64 * step, step + 0.02, 150.0, if i % 2 == 0 { Onset::Burst } else { Onset::Soft }, &mut noise);
        }
        let s = Scan::new(&x, rate);
        assert_eq!(s.syllables.len(), 8, "{:?}", s.syllables.iter().map(|y| y.at).collect::<Vec<_>>());
    }
}
