// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lining one recording up with another by how they sound, not by their words, so any language: two takes of the same
//! part, a double against its lead. Every 10 ms is described by its spectral shape (third-octave levels, each band less
//! its mean over the recording's sound, so a different microphone or level does not count), and dynamic time warping
//! finds the path through both that keeps the shapes closest (Sakoe and Chiba, 1978), kept near the line the rough
//! offset between them gives (a Sakoe-Chiba band). Where they sit against each other overall comes first, from the
//! cross-correlation of their loudness.

use crate::syllables::Scan;
use crate::tone::Fft;

/// Seconds per frame of a `Look`.
pub const HOP: f64 = 0.01;

/// How a recording looks, every `HOP` seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    /// `bands` values per frame.
    pub shape: Vec<f32>,
    pub bands: usize,
    /// Loudness, dB, and whether there is sound.
    pub level: Vec<f32>,
    pub sound: Vec<bool>,
}

impl Look {
    pub fn new(scan: &Scan) -> Look {
        let n = (scan.len / HOP).ceil() as usize + 1;
        let nb = scan.bands;
        let (mut shape, mut level, mut count) = (vec![0.0f32; n * nb], vec![0.0f32; n], vec![0u32; n]);
        for f in 0..scan.frames() {
            let i = ((f as f64 * scan.hop / HOP).round() as usize).min(n - 1);
            for b in 0..nb {
                shape[i * nb + b] += scan.look[f * nb + b];
            }
            level[i] += scan.level[f];
            count[i] += 1;
        }
        // Frames no scan frame fell in take their neighbour's.
        for i in 0..n {
            if count[i] > 0 {
                let c = count[i] as f32;
                shape[i * nb..(i + 1) * nb].iter_mut().for_each(|v| *v /= c);
                level[i] /= c;
            } else if i > 0 {
                let (a, b) = shape.split_at_mut(i * nb);
                b[..nb].copy_from_slice(&a[(i - 1) * nb..]);
                level[i] = level[i - 1];
            }
        }
        let sound: Vec<bool> = level.iter().map(|l| *l >= scan.silence).collect();
        let voiced = sound.iter().filter(|s| **s).count().max(1) as f32;
        for b in 0..nb {
            let mean = (0..n).filter(|&i| sound[i]).map(|i| shape[i * nb + b]).sum::<f32>() / voiced;
            for i in 0..n {
                shape[i * nb + b] -= mean;
            }
        }
        Look { shape, bands: nb, level, sound }
    }

    pub fn frames(&self) -> usize {
        self.level.len()
    }

    fn frame(&self, i: usize) -> &[f32] {
        &self.shape[i * self.bands..(i + 1) * self.bands]
    }
}

/// How different frame `i` of `a` and frame `j` of `b` look.
fn distance(a: &Look, i: usize, b: &Look, j: usize) -> f32 {
    let k = a.bands.min(b.bands);
    let (x, y) = (a.frame(i), b.frame(j));
    ((0..k).map(|t| (x[t] - y[t]) * (x[t] - y[t])).sum::<f32>() / k as f32).sqrt()
}

/// Where `a` sits against `b`: the seconds to add to a moment of `a` to find it in `b`, within `max` either way (the
/// peak of the cross-correlation of their loudness, the silence floor taken off).
pub fn offset(a: &Look, b: &Look, max: f64) -> f64 {
    let env = |l: &Look| -> Vec<f32> {
        let floor = l.level.iter().zip(&l.sound).filter(|(_, s)| !**s).map(|(v, _)| *v).fold(f32::MIN, f32::max).max(-100.0);
        let e: Vec<f32> = l.level.iter().map(|v| (v - floor).max(0.0)).collect();
        let mean = e.iter().sum::<f32>() / e.len().max(1) as f32;
        e.iter().map(|v| v - mean).collect()
    };
    let (ea, eb) = (env(a), env(b));
    let n = (ea.len() + eb.len()).next_power_of_two();
    let fft = Fft::new(n);
    let (mut ar, mut ai) = (vec![0.0f32; n], vec![0.0f32; n]);
    let (mut br, mut bi) = (vec![0.0f32; n], vec![0.0f32; n]);
    ar[..ea.len()].copy_from_slice(&ea);
    br[..eb.len()].copy_from_slice(&eb);
    fft.run(&mut ar, &mut ai);
    fft.run(&mut br, &mut bi);
    // B times A's conjugate, then the inverse as the forward transform of the conjugate: lag k at k (mod n).
    let (mut xr, mut xi) = (vec![0.0f32; n], vec![0.0f32; n]);
    for k in 0..n {
        xr[k] = br[k] * ar[k] + bi[k] * ai[k];
        xi[k] = -(bi[k] * ar[k] - br[k] * ai[k]);
    }
    fft.run(&mut xr, &mut xi);
    let lim = ((max / HOP) as isize).min(n as isize / 2 - 1);
    let mut best = (0isize, f32::MIN);
    for lag in -lim..=lim {
        let v = xr[lag.rem_euclid(n as isize) as usize];
        if v > best.1 {
            best = (lag, v);
        }
    }
    best.0 as f64 * HOP
}

/// The path through two recordings.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// For each frame of `a`, the moment of `b` it matches, seconds.
    pub to: Vec<f64>,
    /// How different they look there.
    pub cost: Vec<f32>,
}

impl Path {
    /// The moment of `b` that `t` seconds of `a` matches.
    pub fn at(&self, t: f64) -> f64 {
        let n = self.to.len();
        if n == 0 {
            return t;
        }
        let x = (t / HOP).clamp(0.0, (n - 1) as f64);
        let i = x.floor() as usize;
        let j = (i + 1).min(n - 1);
        self.to[i] + (self.to[j] - self.to[i]) * (x - i as f64)
    }

    /// How different `a` and `b` look over `from` .. `to` seconds of `a`, on average.
    pub fn cost_over(&self, from: f64, to: f64) -> f32 {
        let (i, j) = (((from / HOP).max(0.0) as usize).min(self.cost.len()), ((to / HOP).ceil().max(0.0) as usize).min(self.cost.len()));
        if j <= i { 0.0 } else { self.cost[i..j].iter().sum::<f32>() / (j - i) as f32 }
    }
}

/// The dynamic time warping of `a` against `b` within `band` seconds of the line `b = a + offset`: each frame of `a`
/// matched to a moment of `b`, the path rising, its start and end free within the band.
pub fn align(a: &Look, b: &Look, offset: f64, band: f64) -> Path {
    let (n, m) = (a.frames(), b.frames());
    if n == 0 || m == 0 {
        return Path { to: (0..n).map(|i| i as f64 * HOP + offset).collect(), cost: vec![0.0; n] };
    }
    let w = ((band / HOP).round() as isize).max(2);
    let width = (2 * w + 1) as usize;
    let off = (offset / HOP).round() as isize;
    let lo = |i: usize| i as isize + off - w;
    // A step off the diagonal costs a little more than the typical difference, so silence keeps the line.
    let mut sample = Vec::new();
    for i in (0..n).step_by((n / 200).max(1)) {
        let j = i as isize + off;
        if j >= 0 && (j as usize) < m {
            sample.push(distance(a, i, b, j as usize));
        }
    }
    sample.sort_by(f32::total_cmp);
    let pen = 0.1 * sample.get(sample.len() / 2).copied().unwrap_or(1.0).max(0.05);
    let inf = f32::INFINITY;
    let mut d = vec![inf; n * width];
    // 0: diagonal, 1: from above (a moved on), 2: from the left (b moved on), 3: start.
    let mut from = vec![3u8; n * width];
    for i in 0..n {
        for k in 0..width {
            let j = lo(i) + k as isize;
            if j < 0 || j as usize >= m {
                continue;
            }
            let here = distance(a, i, b, j as usize);
            let mut best = (if i == 0 || j == 0 { 0.0 } else { inf }, 3u8);
            if i > 0 {
                // The cell (i - 1, j - 1) sits at the same k on the row above; (i - 1, j) one to the right.
                let up = (i - 1) * width;
                if j > 0 && d[up + k] < best.0 {
                    best = (d[up + k], 0);
                }
                if k + 1 < width && d[up + k + 1] + pen < best.0 {
                    best = (d[up + k + 1] + pen, 1);
                }
            }
            if k > 0 && d[i * width + k - 1] + pen < best.0 {
                best = (d[i * width + k - 1] + pen, 2);
            }
            d[i * width + k] = best.0 + here;
            from[i * width + k] = best.1;
        }
    }
    // The end: the cheapest cell of the last row, or of the last column of `b`.
    let mut end = (n - 1, 0usize, inf);
    for k in 0..width {
        if d[(n - 1) * width + k] < end.2 {
            end = (n - 1, k, d[(n - 1) * width + k]);
        }
    }
    let mut sum = vec![0.0f64; n];
    let mut cnt = vec![0u32; n];
    let mut cost = vec![0.0f32; n];
    let (mut i, mut k) = (end.0, end.1);
    loop {
        let j = lo(i) + k as isize;
        sum[i] += j as f64;
        cnt[i] += 1;
        cost[i] = distance(a, i, b, j.clamp(0, m as isize - 1) as usize);
        match from[i * width + k] {
            0 if i > 0 => i -= 1,
            1 if i > 0 => {
                i -= 1;
                k += 1;
            }
            2 if k > 0 => k -= 1,
            _ => break,
        }
    }
    // Frames of `a` before the path starts go on from it at the original speed.
    let first = (0..n).find(|&i| cnt[i] > 0).unwrap_or(0);
    let mut to: Vec<f64> = (0..n).map(|i| if cnt[i] > 0 { sum[i] / f64::from(cnt[i]) * HOP } else { f64::NAN }).collect();
    for i in (0..first).rev() {
        to[i] = to[i + 1] - HOP;
    }
    for i in first + 1..n {
        if to[i].is_nan() {
            to[i] = to[i - 1] + HOP;
        }
    }
    // A light smoothing (a median of five), then never back.
    let raw = to.clone();
    for i in 0..n {
        let mut v: Vec<f64> = raw[i.saturating_sub(2)..(i + 3).min(n)].to_vec();
        v.sort_by(f64::total_cmp);
        to[i] = v[v.len() / 2];
    }
    for i in 1..n {
        to[i] = to[i].max(to[i - 1]);
    }
    Path { to, cost }
}

/// How near the path must bring a syllable or phrase end to one of the other recording's for the two to be taken as
/// the same, seconds.
const SNAP: f64 = 0.05;

/// Anchors that move `scan`'s recording onto `other`'s along `path` (`strength` of the way): its syllables and the
/// ends of its phrases (so a held word ends with the other's). Where the path brings one near the other's own (within
/// `SNAP`), that one's exact moment is the target: the path finds which syllable is which, the syllables say when.
pub fn anchors(scan: &Scan, other: &Scan, path: &Path, strength: f64) -> Vec<(f64, f64)> {
    let snap = |t: f64, theirs: &[f64]| -> f64 {
        let guess = path.at(t);
        let i = theirs.partition_point(|&x| x < guess);
        [i.checked_sub(1), Some(i)]
            .into_iter()
            .flatten()
            .filter_map(|j| theirs.get(j).copied())
            .filter(|x| (x - guess).abs() <= SNAP)
            .min_by(|x, y| (x - guess).abs().total_cmp(&(y - guess).abs()))
            .unwrap_or(guess)
    };
    let their_at: Vec<f64> = other.syllables.iter().map(|s| s.at).collect();
    let their_ends = other.phrase_ends(0.08);
    let mut out: Vec<(f64, f64)> = scan.syllables.iter().map(|s| (s.at, snap(s.at, &their_at))).collect();
    out.extend(scan.phrase_ends(0.08).into_iter().map(|t| (t, snap(t, &their_ends))));
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Two taken as the same target keep the nearer; the order holds.
    let mut kept: Vec<(f64, f64)> = Vec::with_capacity(out.len());
    for (t, y) in out {
        match kept.last() {
            Some(&(_, ly)) if y <= ly => {}
            _ => kept.push((t, y)),
        }
    }
    kept.into_iter().map(|(t, y)| (t, t + strength * (y - t))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syllables::voice::*;

    const RATE: u32 = 48000;

    /// The same line sung twice: the second take's syllables a little early or late in places, its level and tone
    /// different, a second later in its file.
    fn takes() -> (Vec<f32>, Vec<f32>, Vec<f64>, Vec<f64>) {
        let lead: Vec<f64> = (0..14).map(|i| 0.4 + i as f64 * 0.21 + 0.15 * (i / 5) as f64).collect();
        let wobble = [0.0, 0.03, -0.025, 0.04, 0.0, -0.03, 0.02, 0.035, -0.02, 0.0, 0.03, -0.04, 0.01, 0.025];
        let double: Vec<f64> = lead.iter().zip(wobble).map(|(t, w)| t + 1.0 + w).collect();
        let sing = |at: &[f64], gain: f32, f0: f32, seed: u64| {
            let mut x = vec![0.0f32; RATE as usize * 6];
            let mut noise = Noise::new(seed);
            let onsets = [Onset::Burst, Onset::Soft, Onset::Hiss, Onset::Soft, Onset::Burst, Onset::Soft, Onset::Hiss];
            for (i, &t) in at.iter().enumerate() {
                // Each syllable its own vowel colour (pitch here), the same in both takes: what alignment follows.
                let mut y = vec![0.0f32; x.len()];
                syllable(&mut y, RATE, t, 0.12 + 0.02 * (i % 3) as f64, f0 + 9.0 * (i % 7) as f32, onsets[i % 7], &mut noise);
                x.iter_mut().zip(&y).for_each(|(a, b)| *a += gain * b);
            }
            x
        };
        (sing(&lead, 1.0, 120.0, 1), sing(&double, 0.6, 121.0, 2), lead, double)
    }

    /// The double is found a second later, then each of its syllables matched to the lead's.
    #[test]
    fn lines_a_double_up_with_its_lead() {
        let (lead, double, lt, dt) = takes();
        let (sl, sd) = (Scan::new(&lead, RATE), Scan::new(&double, RATE));
        let (ll, ld) = (Look::new(&sl), Look::new(&sd));
        let off = offset(&ld, &ll, 3.0);
        assert!((off + 1.0).abs() < 0.06, "offset {off}");
        let p = align(&ld, &ll, off, 0.4);
        for (d, l) in dt.iter().zip(&lt) {
            let got = p.at(d + 0.05);
            assert!((got - (l + 0.05)).abs() < 0.025, "{d} matched {got}, want {}", l + 0.05);
        }
        // Retimed along the path, the double's syllables land on the lead's.
        let m = crate::retime::map(&sd, &anchors(&sd, &sl, &p, 1.0));
        let pts: Vec<(f64, f64)> = m.iter().map(|(a, b)| (a * f64::from(RATE), b * f64::from(RATE))).collect();
        let (out, _) = crate::warp::render(&double, &double, RATE, &pts);
        let so = Scan::new(&out, RATE);
        let lead_at: Vec<f64> = sl.syllables.iter().map(|s| s.at).collect();
        assert_eq!(so.syllables.len(), lead_at.len());
        let err = so.syllables.iter().zip(&lead_at).map(|(a, b)| (a.at - b).abs()).fold(0.0, f64::max);
        assert!(err < 0.012, "worst {err}");
    }
}
