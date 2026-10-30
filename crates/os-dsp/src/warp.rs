// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plays audio through a time map: WSOLA (waveform-similarity overlap-add; Verhelst and Roelands, 1993), the
//! time-domain stretch that suits a single voice. Each output frame is read from where the map says, moved by up to a
//! few milliseconds to the spot whose waveform best continues the frame before, so pitch periods line up and nothing
//! phases or smears. Where the map runs at the original speed the audio is copied as it was, untouched: the retiming
//! tools (`retime`) keep consonant attacks there, so they are moved, never stretched.

/// Output frame step, seconds (frames are twice as long, Hann windowed, half overlapping).
const STEP: f64 = 0.010;
/// How far a frame may move to line up with the one before, seconds: more than half the longest voice period.
const TOLERANCE: f64 = 0.007;
/// How much a move away from the map's position costs against a better match (a share of the tolerance): keeps the
/// output within a few milliseconds of the map.
const PULL: f32 = 0.25;

/// Where the map puts input position `x` (frames), and back. `map` is (input, output) frame positions, both rising.
#[derive(Debug, Clone, PartialEq)]
pub struct Map<'a> {
    points: &'a [(f64, f64)],
}

impl<'a> Map<'a> {
    pub fn new(points: &'a [(f64, f64)]) -> Map<'a> {
        Map { points }
    }

    /// The output position of input position `x` (straight on at the original speed past the ends).
    pub fn forward(&self, x: f64) -> f64 {
        lerp(self.points, x, |p| p.0, |p| p.1)
    }

    /// The input position of output position `y`.
    pub fn back(&self, y: f64) -> f64 {
        lerp(self.points, y, |p| p.1, |p| p.0)
    }
}

fn lerp(points: &[(f64, f64)], v: f64, from: impl Fn(&(f64, f64)) -> f64, to: impl Fn(&(f64, f64)) -> f64) -> f64 {
    let n = points.len();
    if n == 0 {
        return v;
    }
    let i = points.partition_point(|p| from(p) <= v);
    if i == 0 {
        return to(&points[0]) + v - from(&points[0]);
    }
    if i == n {
        return to(&points[n - 1]) + v - from(&points[n - 1]);
    }
    let (a, b) = (&points[i - 1], &points[i]);
    let span = from(b) - from(a);
    if span <= 0.0 { to(b) } else { to(a) + (v - from(a)) * (to(b) - to(a)) / span }
}

/// `left` / `right` (`rate` Hz) played through `map` ((input, output) frame positions, both rising): as long as the
/// map makes the input's end. The search runs on the two channels' mix and both move together, so the image holds.
pub fn render(left: &[f32], right: &[f32], rate: u32, map: &[(f64, f64)]) -> (Vec<f32>, Vec<f32>) {
    let n_in = left.len().min(right.len());
    let m = Map::new(map);
    let out_len = m.forward(n_in as f64).round().max(0.0) as usize;
    let hs = ((f64::from(rate) * STEP).round() as usize).max(8);
    let n = 2 * hs;
    let tol = (f64::from(rate) * TOLERANCE).round() as isize;
    // A periodic Hann window: copies `hs` apart add up to exactly one.
    let win: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
    let mid: Vec<f32> = left[..n_in].iter().zip(&right[..n_in]).map(|(a, b)| 0.5 * (a + b)).collect();
    let search = Search::new(&mid, rate, n, tol);
    let (mut out_l, mut out_r) = (vec![0.0f32; out_len + n], vec![0.0f32; out_len + n]);
    let mut prev: Option<(isize, f64)> = None;
    // From half a frame before the start, so the first output frames are covered twice like the rest.
    let mut o = -(hs as isize);
    while o < out_len as isize {
        let nominal = m.back(o as f64 + hs as f64) - hs as f64;
        let start = match prev {
            None => nominal.round() as isize,
            // The map runs at the original speed here: the frame before goes straight on.
            Some((p, pn)) if (nominal - pn - hs as f64).abs() < 1e-6 => p + hs as isize,
            Some((p, _)) => search.best(nominal, p + hs as isize),
        };
        for i in 0..n {
            let (src, dst) = (start + i as isize, o + i as isize);
            if dst < 0 || src < 0 || src as usize >= n_in {
                continue;
            }
            let (d, s) = (dst as usize, src as usize);
            out_l[d] += left[s] * win[i];
            out_r[d] += right[s] * win[i];
        }
        prev = Some((start, nominal));
        o += hs as isize;
    }
    out_l.truncate(out_len);
    out_r.truncate(out_len);
    (out_l, out_r)
}

/// The search for the frame start that best continues the frame before: coarse on a decimated copy of the mix, then
/// to the sample.
struct Search<'a> {
    mid: &'a [f32],
    /// The mix averaged in blocks of `q`, and the running sum of its squares.
    small: Vec<f32>,
    energy: Vec<f64>,
    q: usize,
    n: usize,
    tol: isize,
}

impl<'a> Search<'a> {
    fn new(mid: &'a [f32], rate: u32, n: usize, tol: isize) -> Search<'a> {
        let q = ((rate / 11025) as usize).max(1);
        let small: Vec<f32> = mid.chunks(q).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
        let mut energy = Vec::with_capacity(small.len() + 1);
        energy.push(0.0);
        let mut acc = 0.0f64;
        for v in &small {
            acc += f64::from(*v) * f64::from(*v);
            energy.push(acc);
        }
        Search { mid, small, energy, q, n, tol }
    }

    /// Of the starts within the tolerance of `nominal`, the one most like the natural continuation `natural`, a move
    /// from `nominal` costing a little.
    fn best(&self, nominal: f64, natural: isize) -> isize {
        let (q, nq) = (self.q as isize, (self.n / self.q) as isize);
        let at = |v: isize| -> f32 { if v >= 0 && (v as usize) < self.small.len() { self.small[v as usize] } else { 0.0 } };
        let energy = |a: isize| -> f64 {
            let (lo, hi) = (a.clamp(0, self.small.len() as isize) as usize, (a + nq).clamp(0, self.small.len() as isize) as usize);
            self.energy[hi] - self.energy[lo]
        };
        let nat = natural.div_euclid(q);
        let target: Vec<f32> = (0..nq).map(|i| at(nat + i)).collect();
        let te = energy(nat);
        let centre = nominal.round() as isize;
        if te < 1e-12 {
            return centre;
        }
        let score = |c: isize, corr: f64, e: f64| -> f32 {
            let ncc = if e > 1e-12 { (corr / (te * e).sqrt()) as f32 } else { 0.0 };
            ncc - PULL * (c - centre).abs() as f32 / self.tol as f32
        };
        let (lo, hi) = ((centre - self.tol).div_euclid(q), (centre + self.tol).div_euclid(q));
        let mut best = (centre, f32::MIN);
        for b in lo..=hi {
            let corr: f64 = (0..nq).map(|i| f64::from(target[i as usize] * at(b + i))).sum();
            let s = score(b * q, corr, energy(b));
            if s > best.1 {
                best = (b * q, s);
            }
        }
        if q == 1 {
            return best.0;
        }
        // To the sample, on the full-rate mix over the middle half of the frame.
        let full = |v: isize| -> f32 { if v >= 0 && (v as usize) < self.mid.len() { self.mid[v as usize] } else { 0.0 } };
        let (from, len) = ((self.n / 4) as isize, (self.n / 2) as isize);
        let t: Vec<f32> = (0..len).map(|i| full(natural + from + i)).collect();
        let tn: f64 = t.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
        let mut fine = best;
        fine.1 = f32::MIN;
        for c in best.0 - q..=best.0 + q {
            if (c - centre).abs() > self.tol {
                continue;
            }
            let (mut corr, mut e) = (0.0f64, 0.0f64);
            for i in 0..len {
                let v = f64::from(full(c + from + i));
                corr += f64::from(t[i as usize]) * v;
                e += v * v;
            }
            let ncc = if e > 1e-12 && tn > 1e-12 { (corr / (tn * e).sqrt()) as f32 } else { 0.0 };
            let s = ncc - PULL * (c - centre).abs() as f32 / self.tol as f32;
            if s > fine.1 {
                fine = (c, s);
            }
        }
        fine.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, rate: u32, secs: f64) -> Vec<f32> {
        (0..(secs * f64::from(rate)) as usize).map(|i| (0.5 * (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin()) as f32).collect()
    }

    /// Rising zero crossings per second.
    fn hz(x: &[f32], rate: u32) -> f64 {
        let ups = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        ups as f64 * f64::from(rate) / x.len() as f64
    }

    /// The map at the original speed copies the audio exactly.
    #[test]
    fn the_original_speed_copies() {
        let x = tone(220.0, 48000, 1.0);
        let (l, r) = render(&x, &x, 48000, &[(0.0, 0.0), (48000.0, 48000.0)]);
        assert_eq!(l.len(), x.len());
        let err = l.iter().zip(&x).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-5, "{err}");
        assert_eq!(l, r);
    }

    /// Stretched by a fifth, a voice-like tone keeps its pitch, gets its new length and stays smooth (no frame steps:
    /// the level of every 10 ms holds).
    #[test]
    fn a_stretch_keeps_the_pitch() {
        let rate = 48000;
        let x = tone(180.0, rate, 1.0);
        let (l, _) = render(&x, &x, rate, &[(0.0, 0.0), (48000.0, 57600.0)]);
        assert_eq!(l.len(), 57600);
        let f = hz(&l[4800..52800], rate);
        assert!((f - 180.0).abs() < 2.0, "{f} Hz");
        for c in l[4800..52800].chunks(480) {
            let rms = (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt();
            assert!((rms - 0.3536).abs() < 0.03, "a smooth level: {rms}");
        }
        let (s, _) = render(&x, &x, rate, &[(0.0, 0.0), (48000.0, 38400.0)]);
        let f = hz(&s[4800..33600], rate);
        assert!((f - 180.0).abs() < 2.0, "squeezed: {f} Hz");
    }

    /// Clicks in silence land where the map puts them, sharp as they were: the time is made up in the silence.
    #[test]
    fn clicks_land_on_the_map() {
        let rate = 48000;
        let mut x = vec![0.0f32; rate as usize];
        for at in [10000usize, 30000] {
            for (i, v) in x[at..at + 48].iter_mut().enumerate() {
                *v = 0.8 * (1.0 - i as f32 / 48.0);
            }
        }
        // The first click 1000 frames later, the second 500 earlier; the map runs straight through each click, a frame
        // either side of it.
        let map = [(0.0, 0.0), (9000.0, 10000.0), (11000.0, 12000.0), (29000.0, 28500.0), (31000.0, 30500.0), (48000.0, 48000.0)];
        let (l, _) = render(&x, &x, rate, &map);
        let peak = |from: usize, to: usize| (from..to).max_by(|&a, &b| l[a].abs().total_cmp(&l[b].abs())).unwrap();
        assert_eq!(peak(5000, 20000), 11000);
        assert_eq!(peak(20000, 40000), 29500);
        assert!((l[11000] - 0.8).abs() < 1e-3 && (l[29500] - 0.8).abs() < 1e-3, "untouched: {} {}", l[11000], l[29500]);
    }
}
