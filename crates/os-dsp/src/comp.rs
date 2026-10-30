// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Comping takes: several takes of the same part made into one, phrase by phrase the best of them, all in time with
//! each other. Nothing listens for words (any language): the takes are compared by how they sound (`align`).
//!
//! - The takes are lined up with the most typical of them (the one that looks most like all the others), the
//!   reference: its timing is the comp's.
//! - The reference is cut into phrases at its pauses, as comping is done by hand (a whole phrase from one take, so a
//!   line keeps its delivery).
//! - Each take's version of each phrase is scored: how much it looks like the other takes there (a flubbed, missing or
//!   extra word looks like none of them), how far it must be stretched to fit, clipping, a level well under the
//!   others', a noisier room. The best path through the phrases is taken, a change of take costing a little (Viterbi).
//! - Where the take changes the cut goes in the pause, just after the outgoing take's last word has ended (its tail
//!   kept), before the incoming one's next starts, with a short equal-power crossfade.

use crate::align::{self, Look, Path, HOP};
use crate::syllables::Scan;

/// A pause this long in the reference is where a phrase ends, seconds.
const PAUSE: f64 = 0.2;
/// Phrases shorter than this join the one before, seconds.
const MIN_PHRASE: f64 = 1.0;
/// Phrases longer than this without a pause are cut at their quietest moments between syllables, seconds.
const MAX_PHRASE: f64 = 8.0;
/// How far takes may wander from the line their offset gives, seconds.
const BAND: f64 = 1.0;
/// What a change of take costs, against a phrase's score.
const SWITCH: f64 = 0.15;
/// Kept of a word's tail after it ends, and before the next word starts, at a cut; the longest crossfade; seconds.
const TAIL: f64 = 0.04;
const LEAD: f64 = 0.01;
const FADE: f64 = 0.02;
/// A sample this near full scale counts as clipped.
const CLIP: f32 = 0.985;

/// One take: its sound (a mono mix) and its reading.
pub struct Take<'a> {
    pub mono: &'a [f32],
    pub scan: &'a Scan,
}

/// One phrase of the comp, in the reference's time (seconds): the take it comes from, and the crossfade it starts with.
#[derive(Debug, Clone, PartialEq)]
pub struct Phrase {
    pub from: f64,
    pub to: f64,
    pub take: usize,
    pub fade: f64,
    /// Each take's score here (lower is better), for showing why.
    pub scores: Vec<f64>,
}

/// A comp.
#[derive(Debug, Clone, PartialEq)]
pub struct Comp {
    pub reference: usize,
    pub phrases: Vec<Phrase>,
    /// Each take's path onto the reference (the reference's own: none).
    pub paths: Vec<Option<Path>>,
}

impl Comp {
    /// The time map ((input, output) seconds) that puts take `k` in the reference's time: its syllables and phrase ends
    /// on the reference's (`align::anchors`).
    pub fn map(&self, k: usize, takes: &[Take]) -> Vec<(f64, f64)> {
        match self.paths.get(k).and_then(|p| p.as_ref()) {
            None => vec![(0.0, 0.0), (takes[k].scan.len, takes[k].scan.len)],
            Some(p) => crate::retime::map(takes[k].scan, &align::anchors(takes[k].scan, takes[self.reference].scan, p, 1.0)),
        }
    }

    /// The takes the comp uses.
    pub fn used(&self) -> Vec<usize> {
        let mut u: Vec<usize> = self.phrases.iter().map(|p| p.take).collect();
        u.sort_unstable();
        u.dedup();
        u
    }
}

/// For each moment of the reference (every `HOP`), the moment of the take: the path turned round.
fn inverse(path: &Path, frames: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(frames);
    let mut i = 0usize;
    for f in 0..frames {
        let t = f as f64 * HOP;
        while i + 1 < path.to.len() && path.to[i + 1] <= t {
            i += 1;
        }
        let (a, b) = (path.to[i], path.to.get(i + 1).copied().unwrap_or(path.to[i] + HOP));
        let x = if b > a { ((t - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
        out.push((i as f64 + x) * HOP + if t < path.to[0] { t - path.to[0] } else { 0.0 });
    }
    out
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Makes the comp of `takes` (two or more).
pub fn comp(takes: &[Take]) -> Comp {
    let n = takes.len();
    let looks: Vec<Look> = takes.iter().map(|t| Look::new(t.scan)).collect();
    let paths_to = |r: usize| -> Vec<Option<Path>> {
        (0..n)
            .map(|k| {
                (k != r).then(|| {
                    let max = takes[k].scan.len.max(takes[r].scan.len);
                    let off = align::offset(&looks[k], &looks[r], max);
                    align::align(&looks[k], &looks[r], off, BAND)
                })
            })
            .collect()
    };
    // Each take's frame for every moment of the reference.
    let place = |r: usize, paths: &[Option<Path>]| -> Vec<Vec<f64>> {
        let frames = looks[r].frames();
        (0..n).map(|k| paths[k].as_ref().map_or_else(|| (0..frames).map(|f| f as f64 * HOP).collect(), |p| inverse(p, frames))).collect()
    };
    let at = |k: usize, t: f64| ((t / HOP).round().max(0.0) as usize).min(looks[k].frames() - 1);
    // How unlike take `k` is to the others at moment `f` of the reference.
    let unlike = |inv: &[Vec<f64>], k: usize, f: usize| -> f64 {
        let i = at(k, inv[k][f]);
        let d: f64 = (0..n).filter(|&j| j != k).map(|j| f64::from(shape_distance(&looks[k], i, &looks[j], at(j, inv[j][f])))).sum();
        d / (n - 1).max(1) as f64
    };
    // The reference: the take most like all the others, seen from the first.
    let mut reference = 0;
    let mut paths = paths_to(0);
    if n > 2 {
        let inv = place(0, &paths);
        let frames = looks[0].frames();
        let typical: Vec<f64> = (0..n).map(|k| (0..frames).step_by(2).map(|f| unlike(&inv, k, f)).sum::<f64>()).collect();
        reference = (0..n).min_by(|&a, &b| typical[a].total_cmp(&typical[b])).unwrap_or(0);
        if reference != 0 {
            paths = paths_to(reference);
        }
    } else if n == 2 {
        // Of two, the one with more sound in it (a take that stopped early is no reference).
        let sound = |k: usize| looks[k].sound.iter().filter(|s| **s).count();
        if sound(1) > sound(0) {
            reference = 1;
            paths = paths_to(1);
        }
    }
    let r = reference;
    let inv = place(r, &paths);
    let frames = looks[r].frames();
    let rl = &looks[r];
    // The phrases: the reference cut at its pauses.
    let mut cuts = vec![0.0];
    let mut f = 0;
    while f < frames {
        if !rl.sound[f] {
            let run = (f..frames).take_while(|&g| !rl.sound[g]).count();
            if f > 0 && f + run < frames && run as f64 * HOP >= PAUSE {
                cuts.push((f as f64 + run as f64 / 2.0) * HOP);
            }
            f += run;
        } else {
            f += 1;
        }
    }
    cuts.push(frames as f64 * HOP);
    let mut merged = vec![cuts[0]];
    for &c in &cuts[1..] {
        if c - merged.last().copied().unwrap_or(0.0) >= MIN_PHRASE || c >= frames as f64 * HOP {
            merged.push(c);
        }
    }
    if merged.len() > 2 && merged[merged.len() - 1] - merged[merged.len() - 2] < MIN_PHRASE {
        let last = merged.pop().expect("an end");
        *merged.last_mut().expect("a start") = last;
    }
    let mut bounds = vec![merged[0]];
    for w in merged.windows(2) {
        let (a, b) = (w[0], w[1]);
        let parts = ((b - a) / MAX_PHRASE).ceil().max(1.0) as usize;
        for p in 1..parts {
            // The quietest moment near each even split.
            let mid = a + (b - a) * p as f64 / parts as f64;
            let (lo, hi) = (((mid - 1.0) / HOP).max(0.0) as usize, (((mid + 1.0) / HOP) as usize).min(frames - 1));
            let q = (lo..=hi).min_by(|&x, &y| rl.level[x].total_cmp(&rl.level[y])).unwrap_or(lo);
            bounds.push(q as f64 * HOP);
        }
        bounds.push(b);
    }
    // Scores.
    let clipped = |k: usize, a: f64, b: f64| -> f64 {
        let rate = takes[k].scan.rate as f64;
        let (x, y) = (((a * rate) as usize).min(takes[k].mono.len()), ((b * rate) as usize).min(takes[k].mono.len()));
        if y <= x { 0.0 } else { takes[k].mono[x..y].iter().filter(|v| v.abs() >= CLIP).count() as f64 / (y - x) as f64 }
    };
    let floor = |k: usize| median(looks[k].level.iter().zip(&looks[k].sound).filter(|(_, s)| !**s).map(|(v, _)| f64::from(*v)).collect());
    let floors: Vec<f64> = (0..n).map(floor).collect();
    let best_floor = floors.iter().copied().fold(f64::MAX, f64::min);
    let mut raw: Vec<Vec<(f64, f64, f64, f64)>> = Vec::new();
    for w in bounds.windows(2) {
        let (fa, fb) = (((w[0] / HOP) as usize).min(frames), ((w[1] / HOP) as usize).min(frames));
        let mut row = Vec::with_capacity(n);
        for k in 0..n {
            let busy: Vec<usize> = (fa..fb).filter(|&f| rl.sound[f] || looks[k].sound[at(k, inv[k][f])]).collect();
            let cons = if busy.is_empty() { 0.0 } else { busy.iter().map(|&f| unlike(&inv, k, f)).sum::<f64>() / busy.len() as f64 };
            // How much longer or shorter the take's phrase is, and how unevenly it is stretched (over 100 ms, finer
            // than the path's own steps).
            let stretch = if fb > fa + 10 {
                let whole = ((inv[k][fb - 1] - inv[k][fa]).max(HOP) / ((fb - 1 - fa) as f64 * HOP)).ln().abs();
                let local = (fa..fb - 10).map(|f| ((inv[k][f + 10] - inv[k][f]) / (10.0 * HOP) - 1.0).abs()).sum::<f64>() / (fb - fa - 10) as f64;
                whole + 0.25 * local
            } else {
                0.0
            };
            let level = median((fa..fb).filter(|&f| looks[k].sound[at(k, inv[k][f])]).map(|f| f64::from(looks[k].level[at(k, inv[k][f])])).collect());
            let clip = clipped(k, inv[k][fa.min(frames - 1)], inv[k][fb.saturating_sub(1).min(frames - 1)]);
            row.push((cons, stretch.min(1.0), level, clip));
        }
        raw.push(row);
    }
    let scale = median(raw.iter().flatten().map(|r| r.0).filter(|c| *c > 0.0).collect()).max(1e-3);
    let scores: Vec<Vec<f64>> = raw
        .iter()
        .map(|row| {
            let loud = median(row.iter().map(|r| r.2).collect());
            row.iter()
                .enumerate()
                .map(|(k, &(cons, stretch, level, clip))| {
                    cons / scale + 0.6 * stretch + if clip > 0.0005 { 3.0 } else { 0.0 } + ((loud - level - 6.0) / 6.0).max(0.0) + ((floors[k] - best_floor - 6.0) / 10.0).max(0.0)
                })
                .collect()
        })
        .collect();
    // The best path through the phrases.
    let p = scores.len();
    let mut cost = vec![vec![0.0; n]; p];
    let mut back = vec![vec![0usize; n]; p];
    for s in 0..p {
        for k in 0..n {
            let (from, c) = if s == 0 { (k, 0.0) } else { (0..n).map(|j| (j, cost[s - 1][j] + if j == k { 0.0 } else { SWITCH })).min_by(|a, b| a.1.total_cmp(&b.1)).expect("takes") };
            cost[s][k] = c + scores[s][k];
            back[s][k] = from;
        }
    }
    let mut pick = vec![r; p];
    if p > 0 {
        let mut k = (0..n).min_by(|&a, &b| cost[p - 1][a].total_cmp(&cost[p - 1][b])).unwrap_or(r);
        for s in (0..p).rev() {
            pick[s] = k;
            k = back[s][k];
        }
    }
    // Phrases of one take run together; a change of take is cut after the outgoing take's last word.
    let mut phrases: Vec<Phrase> = Vec::new();
    for s in 0..p {
        let (a, b) = (bounds[s], bounds[s + 1]);
        match phrases.last_mut() {
            Some(last) if last.take == pick[s] => last.to = b,
            Some(last) => {
                let (k1, k2) = (last.take, pick[s]);
                let fr = ((a / HOP) as usize).min(frames - 1);
                // Where k1's last sound before the cut ends and k2's next starts, in the reference's time.
                let sounding = |k: usize, f: usize| looks[k].sound[at(k, inv[k][f])];
                let end = (0..=fr).rev().find(|&f| sounding(k1, f)).map_or(a, |f| (f + 1) as f64 * HOP);
                let start = (fr..frames).find(|&f| sounding(k2, f)).map_or(a, |f| f as f64 * HOP);
                let (cut, fade) = if start > end {
                    // The crossfade, centred on the cut, reaches neither word.
                    let cut = (end + TAIL).min(start - LEAD).max(end);
                    (cut, (2.0 * (cut - end).min(start - cut)).clamp(0.002, FADE))
                } else {
                    (a, 0.01)
                };
                last.to = cut;
                phrases.push(Phrase { from: cut, to: b, take: k2, fade, scores: Vec::new() });
            }
            None => phrases.push(Phrase { from: a, to: b, take: pick[s], fade: 0.0, scores: Vec::new() }),
        }
    }
    // The scores of the phrases each came from, averaged over what it covers.
    for ph in &mut phrases {
        let rows: Vec<&Vec<f64>> = (0..p).filter(|&s| bounds[s] < ph.to && bounds[s + 1] > ph.from).map(|s| &scores[s]).collect();
        ph.scores = (0..n).map(|k| rows.iter().map(|r| r[k]).sum::<f64>() / rows.len().max(1) as f64).collect();
    }
    Comp { reference: r, phrases, paths }
}

/// How different frame `i` of `a` and frame `j` of `b` sound, their loudness aside (each frame's mean over the bands
/// taken off): level is scored on its own.
fn shape_distance(a: &Look, i: usize, b: &Look, j: usize) -> f32 {
    let k = a.bands.min(b.bands);
    let (x, y) = (&a.shape[i * a.bands..i * a.bands + k], &b.shape[j * b.bands..j * b.bands + k]);
    let (mx, my) = (x.iter().sum::<f32>() / k as f32, y.iter().sum::<f32>() / k as f32);
    ((0..k).map(|t| (x[t] - mx - y[t] + my).powi(2)).sum::<f32>() / k as f32).sqrt()
}

/// The comp's audio from the takes in the reference's time (`parts[k]`, stereo, for each take the comp uses; the
/// others may be empty): `frames` long, each phrase from its take, equal-power crossfades at the cuts.
pub fn assemble(parts: &[(Vec<f32>, Vec<f32>)], comp: &Comp, rate: u32, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let rf = f64::from(rate);
    let count = comp.phrases.len();
    for (i, ph) in comp.phrases.iter().enumerate() {
        let (pl, pr) = &parts[ph.take];
        let fade_in = if i == 0 { 0.0 } else { ph.fade };
        let fade_out = comp.phrases.get(i + 1).map_or(0.0, |n| n.fade);
        let a = ((ph.from - fade_in / 2.0) * rf).max(0.0) as usize;
        let b = (((ph.to + fade_out / 2.0) * rf) as usize).min(frames);
        for s in a..b {
            let t = s as f64 / rf;
            let g_in = if fade_in > 0.0 { (((t - (ph.from - fade_in / 2.0)) / fade_in).clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2).sin() } else { 1.0 };
            let g_out = if fade_out > 0.0 && i + 1 < count { (((ph.to + fade_out / 2.0 - t) / fade_out).clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2).sin() } else { 1.0 };
            let g = (g_in * g_out) as f32;
            l[s] += g * pl.get(s).copied().unwrap_or(0.0);
            r[s] += g * pr.get(s).copied().unwrap_or(0.0);
        }
    }
    (l, r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syllables::voice::*;

    const RATE: u32 = 48000;

    /// Three phrases of five syllables (each its own vowel, as words differ), a pause between them. Each take wanders a
    /// little in time; take 0 clips in phrase one, take 1 fluffs phrase two (two words dropped), take 2 adds two words
    /// in phrase three.
    fn takes() -> Vec<Vec<f32>> {
        const VOWELS: [[f32; 3]; 5] = [[750.0, 1200.0, 2600.0], [300.0, 2300.0, 3000.0], [320.0, 800.0, 2300.0], [500.0, 1900.0, 2600.0], [500.0, 900.0, 2500.0]];
        let lines: [&[usize]; 3] = [&[0, 1, 2, 3, 4], &[2, 4, 1, 0, 3], &[1, 3, 0, 4, 2]];
        let mut out = Vec::new();
        for k in 0..3usize {
            let mut x = vec![0.0f32; RATE as usize * 7];
            let mut noise = Noise::new(k as u64 + 3);
            let start = 0.3 + 0.07 * k as f64;
            for ph in 0..3usize {
                let words: &[usize] = match (k, ph) {
                    (1, 1) => &[2, 1, 3],
                    (2, 2) => &[1, 3, 2, 0, 1, 4, 2],
                    _ => lines[ph],
                };
                let gain = if (k, ph) == (0, 0) { 9.0 } else { 1.0 };
                for (i, &w) in words.iter().enumerate() {
                    let wob = [0.0, 0.02, -0.015, 0.025, -0.02, 0.01, 0.0][(i + k) % 7];
                    let t = start + ph as f64 * 2.0 + i as f64 * 0.24 + wob;
                    let mut y = vec![0.0f32; x.len()];
                    sound(&mut y, RATE, t, 0.13, 120.0 + 11.0 * (i % 4) as f32, VOWELS[w], [Onset::Burst, Onset::Soft, Onset::Hiss][i % 3], &mut noise);
                    x.iter_mut().zip(&y).for_each(|(a, b)| *a += gain * b);
                }
            }
            x.iter_mut().for_each(|v| *v = v.clamp(-1.0, 1.0));
            out.push(x);
        }
        out
    }

    /// Each phrase from a take that got it right; the cuts in the pauses; the comp has every syllable once.
    #[test]
    fn takes_the_best_of_each_phrase() {
        let audio = takes();
        let scans: Vec<Scan> = audio.iter().map(|x| Scan::new(x, RATE)).collect();
        let t: Vec<Take> = audio.iter().zip(&scans).map(|(x, s)| Take { mono: x, scan: s }).collect();
        let c = comp(&t);
        assert_eq!(c.phrases.len().clamp(2, 3), c.phrases.len(), "{:?}", c.phrases);
        let take_at = |sec: f64| c.phrases.iter().find(|p| p.from <= sec && p.to > sec).map(|p| p.take).unwrap();
        // The reference's time: take `reference` starts its phrases at 0.3 + 0.07 * reference.
        let start = 0.3 + 0.07 * c.reference as f64;
        assert_ne!(take_at(start + 0.5), 0, "phrase one is not the clipped take: {:?}", c.phrases);
        assert_ne!(take_at(start + 2.5), 1, "phrase two is not the one missing syllables: {:?}", c.phrases);
        assert_ne!(take_at(start + 4.5), 2, "phrase three is not the one with extra syllables: {:?}", c.phrases);
        let rs = &scans[c.reference];
        for ph in &c.phrases[1..] {
            let f = (ph.from / rs.hop).round() as usize;
            assert!(rs.silent(f.min(rs.frames() - 1)), "cut at {} in a pause", ph.from);
        }
        let parts: Vec<(Vec<f32>, Vec<f32>)> = (0..3)
            .map(|k| {
                if !c.used().contains(&k) {
                    return (Vec::new(), Vec::new());
                }
                let m: Vec<(f64, f64)> = c.map(k, &t).iter().map(|(a, b)| (a * f64::from(RATE), b * f64::from(RATE))).collect();
                crate::warp::render(&audio[k], &audio[k], RATE, &m)
            })
            .collect();
        let (l, _) = assemble(&parts, &c, RATE, audio[c.reference].len());
        let out = Scan::new(&l, RATE);
        assert_eq!(out.syllables.len(), 15, "{:?}", out.syllables.iter().map(|s| s.at).collect::<Vec<_>>());
        assert!(l.iter().all(|v| v.abs() < 0.985), "no clipping in the comp");
    }
}
