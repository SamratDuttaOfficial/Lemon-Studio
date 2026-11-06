// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! S sounds in a voice: finding the hissy consonants (S, SH, and the voiced Z and ZH) in a recording, telling an S from
//! an SH, and turning one into the other, or softening it, where the user chooses.
//!
//! How (classic speech analysis, light enough for any computer, nothing to download):
//! - Finding them: the voice is cut into short overlapping frames (about 21 ms, a quarter apart) and each frame's
//!   spectrum is read. A hissy consonant is noise high up: most of its energy is above 2.5 kHz and its centre of gravity
//!   (the spectrum's mean frequency) is high, where a vowel's is low. Frames like that, clearly above the recording's
//!   noise floor, joined into stretches of at least 30 ms, are the S sounds found; a short burst (a T, a K) is left out.
//!   Energy below 1 kHz as well (the voice going on under the hiss) makes one voiced: a Z or a ZH.
//! - S or SH: the centre of gravity tells them apart (spectral moments, Forrest et al. 1988; Jongman et al. 2000). An S's
//!   noise is made in the short space in front of the teeth and sits high (about 5 to 9 kHz); an SH's, with the tongue
//!   further back and the lips rounded, lower (about 2.5 to 4.5 kHz). Where the two part differs from voice to voice,
//!   so it is learned from the recording itself: the stretches found are sorted into two groups by their centres
//!   (two-means on a log-frequency scale), as long as two groups clearly stand apart; else the usual values are used.
//! - Changing one: the part chosen is analysed and put back together frame by frame (short-time Fourier transform,
//!   overlap-add). Each frequency gets the gain that turns the part's average spectrum into the target's: the average
//!   spectrum of the recording's own S's (or SH's) when it has some, so the result sounds like the same voice; else the
//!   part's own spectrum moved up or down in frequency to where the other sound sits. The noise itself and its phase
//!   stay, only its colour changes; nothing below 1.2 kHz is touched (a Z keeps its voice). Each frame changes as much
//!   as it is hissy, fading in and out at the part's ends, so nothing around the part moves.

use crate::tone::Fft;

/// An S or an SH (voiced: a Z or a ZH).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    S,
    Sh,
}

impl Kind {
    /// The sound's name as the user reads it.
    pub fn name(self, voiced: bool) -> &'static str {
        match (self, voiced) {
            (Kind::S, false) => "S",
            (Kind::S, true) => "Z",
            (Kind::Sh, false) => "SH",
            (Kind::Sh, true) => "ZH",
        }
    }
}

/// What a change makes of the part chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Change {
    /// Into an S (from an SH, or a dull S made clearer).
    S,
    /// Into an SH.
    Sh,
    /// The same sound with less hiss.
    Softer,
}

impl Change {
    pub fn name(self) -> &'static str {
        match self {
            Change::S => "S",
            Change::Sh => "SH",
            Change::Softer => "softer",
        }
    }
}

/// One S sound: samples `from..to` of the recording.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Found {
    pub from: usize,
    pub to: usize,
    pub kind: Kind,
    pub voiced: bool,
    /// How sure the S or SH call is: 0.5 (it could be either) to 1.
    pub sure: f32,
    /// Its centre of gravity, Hz.
    pub centre: f32,
}

impl Found {
    pub fn name(&self) -> &'static str {
        self.kind.name(self.voiced)
    }
}

/// Where this voice's S and SH sit (their centres of gravity, Hz), learned from its recording.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Model {
    pub s: f32,
    pub sh: f32,
    /// Learned from the recording (both sounds heard in it); else the usual values.
    pub learned: bool,
}

/// Where an S and an SH usually sit.
const USUAL_S: f32 = 6500.0;
const USUAL_SH: f32 = 3600.0;

impl Default for Model {
    fn default() -> Self {
        Model { s: USUAL_S, sh: USUAL_SH, learned: false }
    }
}

impl Model {
    /// Where an S turns into an SH: halfway between them on a log scale.
    pub fn boundary(&self) -> f32 {
        (self.s * self.sh).sqrt()
    }

    /// The call for a sound with its centre of gravity at `centre` Hz, and how sure it is.
    pub fn call(&self, centre: f32) -> (Kind, f32) {
        let b = self.boundary();
        let kind = if centre >= b { Kind::S } else { Kind::Sh };
        // Octaves from the boundary, against the half-distance between the two centres: at a centre, very sure.
        let half = ((self.s / self.sh).log2() / 2.0).max(0.1);
        let d = (centre.max(1.0) / b).log2().abs() / half;
        (kind, 0.5 + 0.5 * (1.5 * d).tanh())
    }

    /// Where `kind` sits in this voice.
    pub fn centre(&self, kind: Kind) -> f32 {
        match kind {
            Kind::S => self.s,
            Kind::Sh => self.sh,
        }
    }
}

/// The analysis frame for `rate`: about 21 ms, a power of two.
fn frame_len(rate: u32) -> usize {
    ((rate as f32 * 0.0213) as usize).next_power_of_two().max(256)
}

/// One frame's reading.
#[derive(Clone, Copy, Debug, Default)]
struct Reading {
    /// dB.
    level: f32,
    /// The share of the energy above 2.5 kHz, and below 1 kHz.
    hi: f32,
    lo: f32,
    /// The centre of gravity of 1.5 .. 12 kHz, Hz.
    centre: f32,
    /// The energy of that band.
    band: f32,
}

impl Reading {
    /// How hissy the frame is, 0..1: how much a change acts on it.
    fn hiss(&self) -> f32 {
        if self.centre < 2000.0 {
            return 0.0;
        }
        ((self.hi - 0.3) / 0.3).clamp(0.0, 1.0)
    }
}

/// Spectra of frames of a signal.
struct Stft {
    n: usize,
    hop: usize,
    rate: f32,
    win: Vec<f32>,
    fft: Fft,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Stft {
    fn new(rate: u32) -> Stft {
        let n = frame_len(rate);
        let win = (0..n).map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32).collect();
        Stft { n, hop: n / 4, rate: rate as f32, win, fft: Fft::new(n), re: vec![0.0; n], im: vec![0.0; n] }
    }

    fn bins(&self) -> usize {
        self.n / 2 + 1
    }

    fn hz(&self, k: usize) -> f32 {
        k as f32 * self.rate / self.n as f32
    }

    fn bin(&self, hz: f32) -> usize {
        ((hz * self.n as f32 / self.rate).round() as usize).min(self.n / 2)
    }

    /// The windowed frame starting at sample `start` of `x` (outside it: silence) into `re` / `im`, transformed.
    fn forward(&mut self, x: &[f32], start: isize) {
        for i in 0..self.n {
            let j = start + i as isize;
            self.re[i] = if j >= 0 && (j as usize) < x.len() { x[j as usize] * self.win[i] } else { 0.0 };
            self.im[i] = 0.0;
        }
        self.fft.run(&mut self.re, &mut self.im);
    }

    /// The power spectrum of the frame starting at `start`.
    fn power(&mut self, x: &[f32], start: isize, out: &mut [f32]) {
        self.forward(x, start);
        for (k, o) in out.iter_mut().enumerate().take(self.bins()) {
            *o = self.re[k] * self.re[k] + self.im[k] * self.im[k];
        }
    }

    /// What a frame's power spectrum says.
    fn read(&self, p: &[f32]) -> Reading {
        let top = self.rate / 2.0;
        let sum = |a: f32, b: f32| p[self.bin(a)..=self.bin(b.min(top))].iter().sum::<f32>();
        let total = sum(80.0, 16000.0) + 1e-12;
        let (b0, b1) = (self.bin(1500.0), self.bin(12000.0f32.min(top)));
        let (mut e, mut fe) = (1e-12f32, 0.0f32);
        for (k, &v) in p.iter().enumerate().take(b1 + 1).skip(b0) {
            e += v;
            fe += v * self.hz(k);
        }
        Reading {
            level: 10.0 * (total / (self.n * self.n) as f32).log10(),
            hi: sum(2500.0, 12000.0) / total,
            lo: sum(80.0, 1000.0) / total,
            centre: fe / e,
            band: e,
        }
    }
}

/// The readings of every frame of `x`, a hop apart, frame `i` starting at `i * hop - n / 2` (centred on `i * hop`).
fn readings(x: &[f32], rate: u32, progress: &mut dyn FnMut(f32)) -> (Vec<Reading>, usize) {
    let mut st = Stft::new(rate);
    let (n, hop) = (st.n, st.hop);
    let frames = x.len() / hop + 1;
    let mut p = vec![0.0; st.bins()];
    let mut out = Vec::with_capacity(frames);
    let every = (frames / 100).max(1);
    for i in 0..frames {
        if i % every == 0 {
            progress(i as f32 / frames as f32);
        }
        st.power(x, (i * hop) as isize - (n / 2) as isize, &mut p);
        out.push(st.read(&p));
    }
    (out, hop)
}

fn percentile(v: &[f32], q: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    s[((s.len() - 1) as f32 * q) as usize]
}

/// The S sounds of mono recording `x` at `rate`, and where this voice's S and SH sit. `progress` hears 0..1.
pub fn find(x: &[f32], rate: u32, progress: &mut dyn FnMut(f32)) -> (Vec<Found>, Model) {
    let (r, hop) = readings(x, rate, progress);
    let levels: Vec<f32> = r.iter().map(|f| f.level).collect();
    // Above the noise floor (the quietest frames, when the recording has pauses: they stand well below the loudest),
    // and not too far below the loudest.
    let floor = percentile(&levels, 0.1);
    let loud = percentile(&levels, 0.97);
    let gate = if loud - floor > 20.0 { (floor + 8.0).max(loud - 50.0) } else { loud - 50.0 };
    let hissy: Vec<bool> = r.iter().map(|f| f.level > gate && f.hi > 0.5 && f.centre > 2400.0).collect();
    // Runs of hissy frames, joined across gaps of up to two frames.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (i, &h) in hissy.iter().enumerate() {
        if !h {
            continue;
        }
        match runs.last_mut() {
            Some(last) if i <= last.1 + 3 => last.1 = i + 1,
            _ => runs.push((i, i + 1)),
        }
    }
    let min_frames = ((0.03 * rate as f32) / hop as f32).ceil() as usize;
    let mut found: Vec<Found> = runs
        .into_iter()
        .filter(|(a, b)| b - a >= min_frames.max(2))
        .map(|(a, b)| {
            let part = &r[a..b];
            let (mut w, mut c, mut lo) = (1e-12f32, 0.0f32, 0.0f32);
            for f in part {
                w += f.band;
                c += f.band * f.centre.max(1.0).ln();
                lo += f.lo;
            }
            let centre = (c / w).exp();
            Found {
                from: (a * hop).saturating_sub(hop / 2),
                to: ((b - 1) * hop + hop / 2).min(x.len()),
                kind: Kind::S,
                voiced: lo / part.len() as f32 > 0.15,
                sure: 0.5,
                centre,
            }
        })
        .collect();
    let model = learn(&found.iter().map(|f| f.centre).collect::<Vec<_>>());
    for f in &mut found {
        (f.kind, f.sure) = model.call(f.centre);
    }
    progress(1.0);
    (found, model)
}

/// Where a voice's S and SH sit, from the centres of the S sounds found in it: two groups, if two clearly stand apart.
pub fn learn(centres: &[f32]) -> Model {
    let logs: Vec<f32> = centres.iter().filter(|&&c| c > 1000.0).map(|c| c.log2()).collect();
    let (mut s, mut sh) = (USUAL_S.log2(), USUAL_SH.log2());
    let mut counts = (0, 0);
    for _ in 0..30 {
        let mid = (s + sh) / 2.0;
        let (hi, lo): (Vec<f32>, Vec<f32>) = logs.iter().partition(|&&c| c >= mid);
        counts = (hi.len(), lo.len());
        if hi.is_empty() || lo.is_empty() {
            break;
        }
        s = hi.iter().sum::<f32>() / hi.len() as f32;
        sh = lo.iter().sum::<f32>() / lo.len() as f32;
    }
    let (s_hz, sh_hz) = (s.exp2(), sh.exp2());
    // Two groups, a good third of an octave apart, the lower one where an SH can be.
    if counts.0 >= 2 && counts.1 >= 2 && s_hz / sh_hz >= 1.3 && sh_hz < 5200.0 {
        Model { s: s_hz, sh: sh_hz, learned: true }
    } else {
        Model::default()
    }
}

/// What part `from..to` of mono `x` sounds like: its hissy stretch (`None` if it has none) called by `model`.
pub fn heard(x: &[f32], rate: u32, from: usize, to: usize, model: &Model) -> Option<Found> {
    let to = to.min(x.len());
    if to <= from {
        return None;
    }
    let mut st = Stft::new(rate);
    let (n, hop) = (st.n, st.hop);
    let mut p = vec![0.0; st.bins()];
    let mut frames = Vec::new();
    let mut c = from;
    while c < to {
        st.power(x, c as isize - (n / 2) as isize, &mut p);
        frames.push((c, st.read(&p)));
        c += hop;
    }
    let loud = frames.iter().map(|f| f.1.level).fold(f32::MIN, f32::max);
    let hissy: Vec<&(usize, Reading)> = frames.iter().filter(|(_, f)| f.hi > 0.4 && f.centre > 2000.0 && f.level > loud - 30.0).collect();
    let (first, last) = (hissy.first()?, hissy.last()?);
    let (mut w, mut cs, mut lo) = (1e-12f32, 0.0f32, 0.0f32);
    for (_, f) in &hissy {
        w += f.band;
        cs += f.band * f.centre.max(1.0).ln();
        lo += f.lo;
    }
    let centre = (cs / w).exp();
    let (kind, sure) = model.call(centre);
    Some(Found {
        from: first.0.saturating_sub(hop / 2).max(from),
        to: (last.0 + hop / 2).min(to),
        kind,
        voiced: lo / hissy.len() as f32 > 0.15,
        sure,
        centre,
    })
}

/// The average spectrum (its power, per frequency of the analysis at `rate`) of parts `parts` of mono `x`: each part's
/// hissy frames, each part counted alike. `None` without a hissy frame.
pub fn spectrum_of(x: &[f32], rate: u32, parts: &[(usize, usize)]) -> Option<Vec<f32>> {
    let mut st = Stft::new(rate);
    let (n, hop, bins) = (st.n, st.hop, st.bins());
    let (b0, b1) = (st.bin(1500.0), st.bin(12000.0f32.min(rate as f32 / 2.0)));
    let mut sum = vec![0.0f64; bins];
    let mut parts_in = 0;
    let mut p = vec![0.0; bins];
    for &(a, b) in parts {
        let mut acc = vec![0.0f64; bins];
        let mut w = 0.0f64;
        let mut c = a;
        while c < b.min(x.len()) {
            st.power(x, c as isize - (n / 2) as isize, &mut p);
            let h = f64::from(st.read(&p).hiss());
            if h > 0.0 {
                let band: f64 = p[b0..=b1].iter().map(|&v| f64::from(v)).sum::<f64>().max(1e-20);
                for (s, &v) in acc.iter_mut().zip(&p) {
                    *s += h * f64::from(v) / band;
                }
                w += h;
            }
            c += hop;
        }
        if w > 0.0 {
            for (s, a) in sum.iter_mut().zip(&acc) {
                *s += a / w;
            }
            parts_in += 1;
        }
    }
    (parts_in > 0).then(|| sum.iter().map(|&s| (s / f64::from(parts_in)) as f32).collect())
}

/// A picture of mono `x`'s spectrum over time, from sample `a` to `b`: `w` columns, each the loudest of up to three
/// spectra across it, of `h` rows from `top` Hz (row 0) down to 0, each row a band of the mel scale (as the ear spaces
/// frequencies: `mel`). Levels in dB, row by row.
pub fn picture(x: &[f32], rate: u32, a: f64, b: f64, w: usize, h: usize, top: f32) -> Vec<f32> {
    let mut st = Stft::new(rate);
    let (n, bins) = (st.n, st.bins());
    let top = top.min(rate as f32 / 2.0);
    let mel = |f: f32| 2595.0 * (1.0 + f / 700.0).log10();
    let hz = |m: f32| 700.0 * (10f32.powf(m / 2595.0) - 1.0);
    // Each row's bins.
    let rows: Vec<(usize, usize)> = (0..h)
        .map(|j| {
            let (m0, m1) = (mel(top) * (1.0 - (j + 1) as f32 / h as f32), mel(top) * (1.0 - j as f32 / h as f32));
            let (k0, k1) = (st.bin(hz(m0)), st.bin(hz(m1)));
            (k0.min(k1), k1.max(k0))
        })
        .collect();
    let mut out = vec![-160.0f32; w * h];
    let mut p = vec![0.0; bins];
    let mut col = vec![0.0f32; bins];
    let per = (b - a) / w.max(1) as f64;
    let norm = (n * n) as f32 / 4.0;
    for i in 0..w {
        let (c0, c1) = (a + i as f64 * per, a + (i + 1) as f64 * per);
        let k = ((c1 - c0) / st.hop as f64).ceil().clamp(1.0, 3.0) as usize;
        col.iter_mut().for_each(|v| *v = 0.0);
        for j in 0..k {
            let c = c0 + (c1 - c0) * (j as f64 + 0.5) / k as f64;
            if c < 0.0 || c >= x.len() as f64 {
                continue;
            }
            st.power(x, c as isize - (n / 2) as isize, &mut p);
            for (m, &v) in col.iter_mut().zip(&p) {
                *m = m.max(v);
            }
        }
        for (j, &(k0, k1)) in rows.iter().enumerate() {
            let v = col[k0..=k1.min(bins - 1)].iter().fold(0.0f32, |m, &x| m.max(x));
            out[j * w + i] = 10.0 * (v / norm + 1e-16).log10();
        }
    }
    out
}

/// `p` smoothed over a sixth of an octave either way (in dB), the low end over at least two bins.
fn smooth(p: &[f32], st: &Stft) -> Vec<f32> {
    let db: Vec<f32> = p.iter().map(|&v| 10.0 * (v + 1e-20).log10()).collect();
    let mut prefix = vec![0.0f64; db.len() + 1];
    for (i, &v) in db.iter().enumerate() {
        prefix[i + 1] = prefix[i] + f64::from(v);
    }
    (0..db.len())
        .map(|k| {
            let f = st.hz(k).max(st.hz(1));
            let (a, b) = (st.bin(f / 2f32.powf(1.0 / 6.0)).min(k.saturating_sub(2)), (st.bin(f * 2f32.powf(1.0 / 6.0)).max(k + 2)).min(db.len() - 1));
            let mean = (prefix[b + 1] - prefix[a]) / (b + 1 - a) as f64;
            10f32.powf(mean as f32 / 10.0)
        })
        .collect()
}

/// Changes samples `from..to` of the recording, given as its channels (`chans`, alike in length) at `rate`: into an S
/// or an SH, or softer, by `strength` (0..1). `target` is the average spectrum (`spectrum_of`) of the recording's own
/// sounds of the kind wanted, if it has any; `model` says where its sounds sit. Whether anything hissy was there to
/// change.
pub fn change(chans: &mut [&mut [f32]], rate: u32, from: usize, to: usize, into: Change, strength: f32, target: Option<&[f32]>, model: &Model) -> bool {
    let len = chans.iter().map(|c| c.len()).min().unwrap_or(0);
    let to = to.min(len);
    let strength = strength.clamp(0.0, 1.0);
    if to <= from || chans.is_empty() || strength == 0.0 {
        return false;
    }
    let mut st = Stft::new(rate);
    let (n, hop, bins) = (st.n, st.hop, st.bins());
    // The span worked on: the part, its fades and a frame either side, from the recording's mono.
    let fade = (0.01 * rate as f32) as usize;
    let a = from.saturating_sub(fade + n);
    let b = (to + fade + n).min(len);
    let mono: Vec<f32> = (a..b).map(|i| chans.iter().map(|c| c[i]).sum::<f32>() / chans.len() as f32).collect();
    // The frames, starting `hop` apart from a frame before the span, and how much each changes.
    let starts: Vec<isize> = (0..).map(|j| j as isize * hop as isize - n as isize + hop as isize).take_while(|&s| s < mono.len() as isize).collect();
    let edge = |c: f32| {
        let (f, t) = ((from - a) as f32, (to - a) as f32);
        let d = if c < f { f - c } else if c > t { c - t } else { 0.0 };
        if d >= fade as f32 { 0.0 } else { 0.5 + 0.5 * (std::f32::consts::PI * d / fade.max(1) as f32).cos() }
    };
    let mut p = vec![0.0; bins];
    let mut src = vec![0.0f32; bins];
    let mut weights = Vec::with_capacity(starts.len());
    let mut total = 0.0;
    let mut cs = (0.0f32, 1e-12f32);
    for &s in &starts {
        st.power(&mono, s, &mut p);
        let r = st.read(&p);
        let centre = s as f32 + n as f32 / 2.0;
        let w = r.hiss() * edge(centre);
        if w > 0.0 {
            for (a, &v) in src.iter_mut().zip(&p) {
                *a += w * v;
            }
            total += w;
            cs.0 += w * r.band * r.centre.max(1.0).ln();
            cs.1 += w * r.band;
        }
        weights.push(w);
    }
    if total < 1e-3 {
        return false;
    }
    let src = smooth(&src, &st);
    let centre = (cs.0 / cs.1).exp();
    let (b0, b1) = (st.bin(1500.0), st.bin(12000.0f32.min(rate as f32 / 2.0)));
    let band = |v: &[f32]| v[b0..=b1].iter().map(|&x| f64::from(x)).sum::<f64>().max(1e-30);
    // The spectrum wanted, as loud as the part's.
    let mut want: Vec<f32> = match (into, target) {
        (Change::Softer, _) => src
            .iter()
            .enumerate()
            .map(|(k, &v)| {
                let f = st.hz(k);
                let down = ((f - 3000.0) / 2500.0).clamp(0.0, 1.0);
                v * 10f32.powf(-1.0 * down)
            })
            .collect(),
        (Change::S | Change::Sh, Some(t)) if t.len() == bins => smooth(t, &st),
        (c, _) => {
            // The part's own spectrum moved to where the other sound sits in this voice.
            let goal = model.centre(if c == Change::S { Kind::S } else { Kind::Sh });
            let k = (goal / centre).clamp(0.4, 2.5);
            (0..bins)
                .map(|i| {
                    let f = (i as f32 / k).min((bins - 1) as f32);
                    let (j, t) = (f as usize, f.fract());
                    src[j] * (1.0 - t) + src[(j + 1).min(bins - 1)] * t
                })
                .collect()
        }
    };
    if into != Change::Softer {
        let scale = (band(&src) / band(&want)) as f32;
        want.iter_mut().for_each(|v| *v *= scale);
    }
    // Each frequency's gain, none below 1.2 kHz.
    let gain: Vec<f32> = (0..bins)
        .map(|k| {
            let f = st.hz(k);
            let reach = ((f - 1000.0) / 500.0).clamp(0.0, 1.0);
            let g = (want[k] / src[k].max(1e-30)).sqrt().clamp(0.0316, 31.6);
            g.powf(strength * reach)
        })
        .collect();
    // Each channel, frame by frame: Hann in and out, a quarter apart (their squares add up to 1.5).
    for ch in chans.iter_mut() {
        let x: Vec<f32> = ch[a..b].to_vec();
        let mut out = vec![0.0f32; x.len()];
        for (&s, &w) in starts.iter().zip(&weights) {
            st.forward(&x, s);
            if w > 0.0 {
                for k in 0..bins {
                    let g = gain[k].powf(w);
                    st.re[k] *= g;
                    st.im[k] *= g;
                    if k > 0 && k < n / 2 {
                        st.re[n - k] *= g;
                        st.im[n - k] *= g;
                    }
                }
            }
            // The inverse: the forward transform of the conjugate, conjugated, over n.
            st.im.iter_mut().for_each(|v| *v = -*v);
            let (re, im) = (&mut st.re, &mut st.im);
            st.fft.run(re, im);
            for i in 0..n {
                let j = s + i as isize;
                if j >= 0 && (j as usize) < out.len() {
                    out[j as usize] += st.re[i] / n as f32 * st.win[i] / 1.5;
                }
            }
        }
        ch[a..b].copy_from_slice(&out);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48000;

    /// Noise in a band: many sines at random phases between `lo` and `hi` Hz, peaking at `peak`.
    fn hiss(lo: f32, peak: f32, hi: f32, secs: f32, seed: &mut u64) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let mut x = vec![0.0f32; n];
        let mut next = || {
            *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (*seed >> 33) as f32 / (1u64 << 31) as f32
        };
        // Turbulence: a broad noise, loudest around `peak` and from `lo` to `hi`, a little everywhere else.
        let parts = 240;
        for p in 0..parts {
            let f = 600.0 + 15000.0 * (p as f32 + next()) / parts as f32;
            let inside = if f >= lo && f <= hi { 1.0 } else { 0.0 };
            let a = (-((f / peak).log2() / 0.45).powi(2)).exp() * inside + 0.03;
            let ph = next() * std::f32::consts::TAU;
            let w = std::f32::consts::TAU * f / RATE as f32;
            for (i, v) in x.iter_mut().enumerate() {
                *v += a * (w * i as f32 + ph).sin();
            }
        }
        let peak_v = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        x.iter_mut().for_each(|v| *v *= 0.2 / peak_v);
        x
    }

    /// A vowel: a 150 Hz buzz with its harmonics falling off.
    fn vowel(secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                (1..30).map(|h| (std::f32::consts::TAU * 150.0 * h as f32 * t).sin() / (h * h) as f32).sum::<f32>() * 0.3
            })
            .collect()
    }

    /// "a-S-a-SH-a-S-a-SH-a": vowels with two S's and two SH's between, and where each hiss is.
    fn speech() -> (Vec<f32>, Vec<(usize, usize, Kind)>) {
        let mut seed = 7;
        let mut x = vowel(0.2);
        let mut at = Vec::new();
        for k in [Kind::S, Kind::Sh, Kind::S, Kind::Sh] {
            let h = match k {
                Kind::S => hiss(4000.0, 7000.0, 11000.0, 0.15, &mut seed),
                Kind::Sh => hiss(1800.0, 3300.0, 6000.0, 0.15, &mut seed),
            };
            at.push((x.len(), x.len() + h.len(), k));
            x.extend(h);
            x.extend(vowel(0.2));
        }
        (x, at)
    }

    fn centre(x: &[f32]) -> f32 {
        let st = Stft::new(RATE);
        let mut st2 = Stft::new(RATE);
        let mut p = vec![0.0; st.bins()];
        let (mut e, mut c) = (0.0, 0.0);
        let mut s = 0;
        while s + st.n <= x.len() {
            st2.power(x, s as isize, &mut p);
            let r = st.read(&p);
            e += r.band;
            c += r.band * r.centre.ln();
            s += st.hop;
        }
        (c / e).exp()
    }

    #[test]
    fn finds_the_s_and_sh_sounds_and_tells_them_apart() {
        let (x, at) = speech();
        let (found, model) = find(&x, RATE, &mut |_| {});
        assert_eq!(found.len(), 4, "{found:?}");
        assert!(model.learned, "both heard: learned from the voice, {model:?}");
        for (f, &(a, b, k)) in found.iter().zip(&at) {
            assert_eq!(f.kind, k, "{f:?}");
            assert!(!f.voiced);
            assert!(f.sure > 0.8, "{f:?}");
            let slack = (0.03 * RATE as f32) as usize;
            assert!(f.from.abs_diff(a) < slack && f.to.abs_diff(b) < slack, "{f:?} for {a}..{b}");
        }
    }

    #[test]
    fn a_part_chosen_roughly_is_heard_for_what_it_is() {
        let (x, at) = speech();
        let (_, model) = find(&x, RATE, &mut |_| {});
        let (a, b, _) = at[1];
        let h = heard(&x, RATE, a - 4000, b + 4000, &model).expect("a hiss");
        assert_eq!(h.name(), "SH");
        assert!(h.from >= a - 4000 && h.to <= b + 4000);
        // A vowel alone is no S sound.
        assert_eq!(heard(&x, RATE, 0, 8000, &model), None);
    }

    #[test]
    fn an_sh_becomes_an_s_like_the_voices_own_and_nothing_else_moves() {
        let (x, at) = speech();
        let (found, model) = find(&x, RATE, &mut |_| {});
        let (a, b, _) = at[1];
        let own: Vec<(usize, usize)> = found.iter().filter(|f| f.kind == Kind::S).map(|f| (f.from, f.to)).collect();
        let target = spectrum_of(&x, RATE, &own).expect("the voice's S");
        let (mut l, mut r) = (x.clone(), x.clone());
        assert!(change(&mut [&mut l, &mut r], RATE, a, b, Change::S, 1.0, Some(&target), &model));
        let before = centre(&x[a + 1000..b - 1000]);
        let after = centre(&l[a + 1000..b - 1000]);
        let s = centre(&x[at[0].0 + 1000..at[0].1 - 1000]);
        assert!(before < 4500.0 && after > 5500.0, "{before} -> {after}");
        assert!((after / s).log2().abs() < 0.3, "close to the voice's own S: {after} against {s}");
        assert_eq!(l, r, "both channels alike");
        // Outside the part and its fades, the very same samples.
        let edge = 1024 + 480 + 64;
        let same = |i: usize| (l[i] - x[i]).abs() < 1e-4;
        assert!((0..a.saturating_sub(edge)).all(same), "before it");
        assert!((b + edge..x.len()).all(same), "after it");
        // Called again, it is an S now.
        let h = heard(&l, RATE, a, b, &model).unwrap();
        assert_eq!(h.kind, Kind::S, "{h:?}");
    }

    #[test]
    fn an_s_moves_down_to_an_sh_without_examples_and_softer_takes_the_hiss_down() {
        let (x, at) = speech();
        let model = Model::default();
        let (a, b, _) = at[0];
        let mut y = x.clone();
        assert!(change(&mut [&mut y], RATE, a, b, Change::Sh, 1.0, None, &model));
        let after = centre(&y[a + 1000..b - 1000]);
        assert!(after < model.boundary(), "{after}");
        let mut z = x.clone();
        assert!(change(&mut [&mut z], RATE, a, b, Change::Softer, 1.0, None, &model));
        let rms = |v: &[f32]| (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt();
        let (was, now) = (rms(&x[a..b]), rms(&z[a..b]));
        assert!(now < was * 0.6, "{was} -> {now}");
        // Half the strength, half the way (in dB).
        let mut h = x.clone();
        change(&mut [&mut h], RATE, a, b, Change::Softer, 0.5, None, &model);
        let half = rms(&h[a..b]);
        assert!(half < was && half > now, "{was} {half} {now}");
        // A vowel has nothing to change.
        let mut v = x.clone();
        assert!(!change(&mut [&mut v], RATE, 0, 8000, Change::S, 1.0, None, &model));
    }

    #[test]
    fn the_picture_shows_the_hiss_high_up_and_the_vowel_low_down() {
        let (x, at) = speech();
        let (w, h) = (40, 64);
        let (a, b, _) = at[0];
        // A picture of the S alone, then of a vowel alone.
        let s = picture(&x, RATE, a as f64, b as f64, w, h, 16000.0);
        let v = picture(&x, RATE, 0.0, 8000.0, w, h, 16000.0);
        assert_eq!(s.len(), w * h);
        let row_of = |p: &[f32]| (0..h).max_by(|&i, &j| p[i * w + w / 2].total_cmp(&p[j * w + w / 2])).unwrap();
        // Row 0 is the top: the S's loudest row is high, the vowel's near the bottom.
        assert!(row_of(&s) < h / 3, "{}", row_of(&s));
        assert!(row_of(&v) > h * 3 / 4, "{}", row_of(&v));
    }

    #[test]
    fn a_voice_with_only_s_keeps_the_usual_boundary() {
        let m = learn(&[6000.0, 6400.0, 7100.0, 6800.0, 5900.0]);
        assert!(!m.learned);
        assert_eq!(m.call(6500.0).0, Kind::S);
        let m = learn(&[7000.0, 7200.0, 3300.0, 3500.0, 6900.0]);
        assert!(m.learned && m.sh < 4000.0 && m.s > 6500.0, "{m:?}");
    }
}
