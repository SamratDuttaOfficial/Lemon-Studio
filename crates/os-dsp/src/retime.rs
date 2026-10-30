// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A voice's time map from anchors (this moment of the input goes there in the output): between two anchors the time
//! is found where moving it is least heard, as an editor tightening a take by hand would find it. Silences give and
//! take most, vowels some, unvoiced sound (fricatives, breaths) less; attacks and consonant-vowel transitions none,
//! so they are moved whole (the transient management of Grofit and Lavner's WSOLA, 2008). `warp` plays the audio
//! through the map.

use crate::syllables::{Class, Scan};

/// Per class: how readily it gives or takes time, and the least and most it may be stretched to.
fn give(c: Class) -> (f64, f64, f64) {
    match c {
        Class::Hold => (0.0, 1.0, 1.0),
        Class::Silent => (4.0, 0.1, 8.0),
        Class::Voiced => (1.0, 0.55, 1.8),
        Class::Other => (0.6, 0.6, 1.6),
    }
}

/// One stretch of the input between frame edges: its length (seconds) and class.
#[derive(Debug, Clone, Copy)]
struct Piece {
    len: f64,
    w: f64,
    lo: f64,
    hi: f64,
}

/// The time map ((input, output) seconds, both rising, from (0, 0) to the input's end) that puts each anchor's input
/// moment at its output moment, as near as the stretch limits allow: an anchor out of reach is moved back towards
/// where it can be (the order of the anchors always holds). `anchors` need not be sorted; the input's start is fixed.
pub fn map(scan: &Scan, anchors: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let len = scan.len;
    let classes = scan.classes();
    let hop = scan.hop;
    let mut a: Vec<(f64, f64)> = anchors.iter().filter(|p| p.0 > 0.0 && p.0 < len && p.1.is_finite()).copied().collect();
    a.sort_by(|x, y| x.0.total_cmp(&y.0));
    a.dedup_by(|x, y| x.0 - y.0 < 1e-4);
    a.insert(0, (0.0, 0.0));
    // The end goes on from the last anchor at the original speed unless one is given there.
    if a.last().is_some_and(|p| len - p.0 > 1e-4) {
        let &(x, y) = a.last().expect("one at least");
        a.push((len, y + len - x));
    }
    let edge = |f: usize| ((f as f64 + 0.5) * hop).min(len);
    let pieces = |x0: f64, x1: f64| -> Vec<(f64, Piece)> {
        let mut out = Vec::new();
        let mut f = ((x0 / hop) + 0.5).floor().max(0.0) as usize;
        let mut x = x0;
        while x < x1 - 1e-12 {
            let e = edge(f).min(x1);
            if e > x {
                let (w, lo, hi) = give(classes.get(f).copied().unwrap_or(Class::Silent));
                out.push((e, Piece { len: e - x, w, lo, hi }));
            }
            x = e.max(x);
            f += 1;
            if f > classes.len() + 1 {
                out.push((x1, Piece { len: x1 - x, w: 1.0, lo: 0.55, hi: 1.8 }));
                break;
            }
        }
        out
    };
    let segs: Vec<Vec<(f64, Piece)>> = a.windows(2).map(|w| pieces(w[0].0, w[1].0)).collect();
    let bounds: Vec<(f64, f64)> = segs.iter().map(|s| (s.iter().map(|(_, p)| p.len * p.lo).sum(), s.iter().map(|(_, p)| p.len * p.hi).sum())).collect();
    // Within reach: forward, back (the start stays), forward again.
    let n = a.len();
    let forward = |a: &mut Vec<(f64, f64)>| {
        for j in 1..n {
            let (lo, hi) = bounds[j - 1];
            a[j].1 = a[j].1.clamp(a[j - 1].1 + lo, a[j - 1].1 + hi);
        }
    };
    forward(&mut a);
    for j in (1..n - 1).rev() {
        let (lo, hi) = bounds[j];
        a[j].1 = a[j].1.clamp(a[j + 1].1 - hi, a[j + 1].1 - lo);
    }
    forward(&mut a);
    let mut out = vec![(0.0, 0.0)];
    for (j, seg) in segs.iter().enumerate() {
        let want = a[j + 1].1 - a[j].1;
        let have: f64 = seg.iter().map(|(_, p)| p.len).sum();
        let grow = want >= have;
        let rate = |p: &Piece, k: f64| if grow { (1.0 + k * p.w).min(p.hi) } else { (1.0 - k * p.w).max(p.lo) };
        let total = |k: f64| seg.iter().map(|(_, p)| p.len * rate(p, k)).sum::<f64>();
        let (mut lo, mut hi) = (0.0, 1.0);
        while (if grow { total(hi) < want } else { total(hi) > want }) && hi < 1e6 {
            hi *= 2.0;
        }
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if (total(mid) < want) == grow {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let k = 0.5 * (lo + hi);
        // Spread what rounding leaves over the segment, so it ends exactly on its anchor.
        let got = total(k);
        let fix = if got > 0.0 { want / got } else { 1.0 };
        let mut y = a[j].1;
        for (e, p) in seg {
            y += p.len * rate(p, k) * fix;
            out.push((*e, y));
        }
        if let Some(last) = out.last_mut() {
            last.1 = a[j + 1].1;
        }
    }
    out.dedup_by(|x, y| x.0 - y.0 < 1e-9);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syllables::voice::*;

    fn at(m: &[(f64, f64)], x: f64) -> f64 {
        crate::warp::Map::new(m).forward(x)
    }

    /// Two syllables moved apart: the anchors land where asked, the time comes out of the silence between them, and
    /// each syllable's onset is moved whole (the map runs at the original speed through it).
    #[test]
    fn moves_come_out_of_the_silence() {
        let rate = 48000;
        let mut x = vec![0.0f32; rate as usize];
        let mut noise = Noise::new(5);
        syllable(&mut x, rate, 0.2, 0.15, 140.0, Onset::Burst, &mut noise);
        syllable(&mut x, rate, 0.6, 0.15, 140.0, Onset::Hiss, &mut noise);
        let s = Scan::new(&x, rate);
        let (p0, p1) = (s.syllables[0].at, s.syllables[1].at);
        let m = map(&s, &[(p0, p0 + 0.03), (p1, p1 - 0.04)]);
        assert!((at(&m, p0) - (p0 + 0.03)).abs() < 1e-6 && (at(&m, p1) - (p1 - 0.04)).abs() < 1e-6);
        assert!((at(&m, s.len) - s.len + 0.04).abs() < 1e-6, "the end follows the last move");
        for p in [p0, p1] {
            let slope = (at(&m, p + 0.02) - at(&m, p - 0.01)) / 0.03;
            assert!((slope - 1.0).abs() < 1e-9, "the onset moved whole: {slope}");
        }
        // The silence between the syllables gave 70 ms; the vowels, about nothing.
        let silence = at(&m, 0.5) - at(&m, 0.4);
        assert!(silence < 0.1 - 0.02, "{silence}");
        let vowel = at(&m, p0 + 0.12) - at(&m, p0 + 0.05);
        assert!((vowel - 0.07).abs() < 0.01, "{vowel}");
    }

    /// An anchor beyond what the stretch limits allow is brought back within reach; the map still rises.
    #[test]
    fn out_of_reach_anchors_are_brought_back() {
        let rate = 48000;
        let mut x = vec![0.0f32; rate as usize / 2];
        let mut noise = Noise::new(9);
        syllable(&mut x, rate, 0.05, 0.1, 140.0, Onset::Soft, &mut noise);
        syllable(&mut x, rate, 0.17, 0.1, 140.0, Onset::Soft, &mut noise);
        let s = Scan::new(&x, rate);
        let (p0, p1) = (s.syllables[0].at, s.syllables[1].at);
        let m = map(&s, &[(p0, p0), (p1, p0 + 0.01)]);
        assert!(m.windows(2).all(|w| w[1].1 >= w[0].1 && w[1].0 > w[0].0));
        assert!(at(&m, p1) - at(&m, p0) > 0.05, "the held onsets and the vowel's least length keep them apart");
    }
}
