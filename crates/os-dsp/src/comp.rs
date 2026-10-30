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
/// In one recording of several takes (`split`): a pause this long parts its phrases, seconds.
const TAKE_GAP: f64 = 0.3;
/// The shortest take, seconds.
const MIN_TAKE: f64 = 2.0;
/// How much of the part's opening is looked for where it may start again, seconds.
const OPENING: f64 = 4.0;
/// A place starts the part again when it looks at most this much as unlike the opening as the opening does to what
/// follows it (measured on spoken takes: about 0.25 where it does, 0.8 or more elsewhere).
const RESTART: f64 = 0.55;
/// Words between takes ("one more time") this short or shorter, that look like nothing in the other takes, are left
/// out, seconds.
const CHATTER: f64 = 2.5;

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

/// Frames `a` .. `b` of `look`.
fn part(look: &Look, a: usize, b: usize) -> Look {
    let b = b.min(look.frames());
    let a = a.min(b);
    Look { shape: look.shape[a * look.bands..b * look.bands].to_vec(), bands: look.bands, level: look.level[a..b].to_vec(), sound: look.sound[a..b].to_vec() }
}

/// How unlike frames `x` of `look` are to the closest stretch of frames `y` (dynamic time warping within `band`
/// seconds of the line that puts `x`'s start `offset` seconds into `y`): the mean over `x`'s sound.
fn unlike_in(look: &Look, x: (usize, usize), y: (usize, usize), offset: f64, band: f64) -> f64 {
    let (a, mut b) = (part(look, x.0, x.1), part(look, y.0, y.1));
    if a.frames() == 0 || b.frames() == 0 {
        return f64::MAX;
    }
    // Where `y` ends before `x` could (the recording's end), silence: what of `x` finds nothing there counts as unlike
    // (the path would otherwise end early, the rest of `x` not counted).
    let need = a.frames() + ((offset.max(0.0) + band) / HOP).ceil() as usize + 1;
    if b.frames() < need {
        let floor = b.level.iter().copied().fold(f32::MAX, f32::min);
        b.shape.resize(need * b.bands, 0.0);
        b.level.resize(need, floor);
        b.sound.resize(need, false);
    }
    let p = align::align(&a, &b, offset, band);
    let (sum, n) = a.sound.iter().zip(&p.cost).filter(|(s, _)| **s).fold((0.0, 0usize), |(s, n), (_, c)| (s + f64::from(*c), n + 1));
    if n == 0 { f64::MAX } else { sum / n as f64 }
}

/// The phrases of `look`: its sound in runs parted by pauses of `TAKE_GAP` or more (a click or breath under 0.1 s
/// left out), frames.
fn phrases(look: &Look) -> Vec<(usize, usize)> {
    let gap = (TAKE_GAP / HOP).round() as usize;
    let n = look.frames();
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut f = 0;
    while f < n {
        if !look.sound[f] {
            f += 1;
            continue;
        }
        let a = f;
        while f < n && look.sound[f] {
            f += 1;
        }
        match out.last_mut() {
            Some(last) if a - last.1 < gap => last.1 = f,
            _ => out.push((a, f)),
        }
    }
    out.retain(|(a, b)| (b - a) as f64 * HOP >= 0.1);
    out
}

/// The takes in one recording of a part done several times over, each from the top, as (from, to) seconds: where the
/// recording starts the part again, its sound looking like the part's opening (its first `OPENING` seconds) much more
/// than the opening looks like what follows it. The opening may be the recording's first phrase or, after a few words
/// before the part ("take one"), its second or third: whichever is found again most often. Words between takes that
/// look like nothing in the other takes ("one more time") are left out. Fewer than two: the recording holds one take.
pub fn split(look: &Look) -> Vec<(f64, f64)> {
    let ph = phrases(look);
    let n = look.frames();
    let (w, min_take) = ((OPENING / HOP) as usize, (MIN_TAKE / HOP) as usize);
    let shifts = [1.5, 2.5, 3.5].map(|s: f64| (s / HOP) as usize);
    // For each opening tried: its phrase, the phrases found starting the part again, and how unlike the opening a
    // stretch that does not start it is.
    let mut best: Option<(usize, Vec<usize>, f64)> = None;
    for o in 0..ph.len().min(3) {
        let start = ph[o].0;
        if start > n / 2 {
            break;
        }
        let open = (start, (start + w).min(n));
        // How unlike the opening each later phrase's start is.
        let costs: Vec<(usize, f64)> = (o + 1..ph.len())
            .filter(|&p| ph[p].0 >= start + min_take)
            .map(|p| (p, unlike_in(look, open, (ph[p].0, (ph[p].0 + w + 100).min(n)), 0.0, 1.0)))
            .collect();
        // Not the part starting again: the opening against what follows it (the most alike of three places), or the
        // later phrases' starts as most of them are, whichever is less alike (a short take's next one can be within
        // reach of the first, and with a phrase a take most later phrases do start it again).
        let after = shifts.iter().map(|&s| unlike_in(look, open, (start + s, (start + s + w + 100).min(n)), 0.0, 1.0)).fold(f64::MAX, f64::min);
        let other = if costs.len() >= 3 { after.max(median(costs.iter().map(|c| c.1).collect())) } else { after };
        if other == f64::MAX {
            continue;
        }
        let mut found: Vec<(usize, f64)> = costs.into_iter().filter(|&(_, c)| c < RESTART * other).collect();
        // Two places closer than a take: the more alike.
        found.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut kept: Vec<usize> = Vec::new();
        for (p, _) in found {
            if kept.iter().all(|&q| ph[q].0.abs_diff(ph[p].0) >= min_take) {
                kept.push(p);
            }
        }
        kept.sort_unstable();
        kept.insert(0, o);
        if best.as_ref().is_none_or(|b| kept.len() > b.1.len()) {
            best = Some((o, kept, other));
        }
    }
    let Some((o, mut starts, other)) = best.filter(|b| b.1.len() >= 2) else { return Vec::new() };
    // The part may begin before the opening found again (its first phrase spoilt in the first take, a cough over it):
    // when what comes before the opening in one take, as long as before it in the first, is found before it in
    // another, every take starts that much earlier, at the phrase nearest. Otherwise it was words before the part.
    if o > 0 {
        let lead = ph[o].0 - ph[0].0;
        let before: Vec<(usize, usize)> = starts.iter().map(|&p| (ph[p].0.saturating_sub(lead), ph[p].0)).collect();
        let back = (50.0 * 0.01 / HOP) as usize;
        let alike = (0..before.len())
            .any(|i| (i + 1..before.len()).any(|j| unlike_in(look, before[i], (before[j].0.saturating_sub(back), before[j].1), back as f64 * HOP, 1.0) < RESTART * other));
        if alike {
            for k in 0..starts.len() {
                let lo = if k == 0 { 0 } else { starts[k - 1] + 1 };
                let want = ph[starts[k]].0.saturating_sub(lead);
                starts[k] = (lo..=starts[k]).min_by_key(|&p| ph[p].0.abs_diff(want)).unwrap_or(starts[k]);
            }
        }
    }
    // Each take: its phrases, from its start to the next take's.
    let mut takes: Vec<Vec<(usize, usize)>> = starts.iter().enumerate().map(|(k, &s)| ph[s..starts.get(k + 1).copied().unwrap_or(ph.len())].to_vec()).collect();
    // Chatter at a take's end: a short last phrase found in none of the other takes, near where it would be there,
    // and without which the take is still as long as the others (else it is the take's own last line, flubbed).
    let span = |t: &[(usize, usize)]| (t[0].0, t[t.len() - 1].1);
    for k in 0..takes.len() {
        while takes[k].len() >= 2 {
            let last = takes[k][takes[k].len() - 1];
            let (s, e) = span(&takes[k]);
            let rest = takes[k][takes[k].len() - 2].1 - s;
            let shortest = (0..takes.len()).filter(|&j| j != k).map(|j| span(&takes[j]).1 - span(&takes[j]).0).min().unwrap_or(0);
            if (last.1 - last.0) as f64 * HOP > CHATTER || (rest as f64) < 0.75 * shortest as f64 {
                break;
            }
            let len = (e - s) as f64;
            let alike = (0..takes.len())
                .filter(|&j| j != k)
                .map(|j| {
                    let (js, je) = span(&takes[j]);
                    // Where it would be in take `j`, its share of the way through its take.
                    let at = (last.0 - s) as f64 / len * (je - js) as f64 * HOP;
                    unlike_in(look, last, (js, je), at, (0.25 * (je - js) as f64 * HOP).max(1.5))
                })
                .fold(f64::MAX, f64::min);
            if alike < RESTART * other {
                break;
            }
            takes[k].pop();
        }
    }
    // From just before a take's first sound to just after its last (`TAIL`), within the pauses either side.
    takes
        .iter()
        .map(|t| {
            let (s, e) = span(t);
            ((s as f64 * HOP - LEAD).max(0.0), (e as f64 * HOP + TAIL).min(n as f64 * HOP))
        })
        .collect()
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

    /// One recording of the three takes, a second apart, with a few other words said after the first: three takes
    /// are found, each from its first sound to its last (the third's flubbed last line kept), the words left out.
    #[test]
    fn finds_the_takes_in_one_recording() {
        let audio = takes();
        let gap = RATE as usize;
        let mut x = Vec::new();
        let mut starts = Vec::new();
        for (k, t) in audio.iter().enumerate() {
            // Each take from its first syllable (0.3 s + 0.07 s a take in its own file) to its last.
            let (a, b) = ((0.25 + 0.07 * k as f64) * f64::from(RATE), (6.3 + 0.07 * k as f64) * f64::from(RATE));
            starts.push(x.len() as f64 / f64::from(RATE) + 0.05);
            x.extend_from_slice(&t[a as usize..b as usize]);
            if k == 0 {
                // "One more time": three syllables of other vowels.
                let mut y = vec![0.0f32; RATE as usize * 2];
                let mut noise = Noise::new(9);
                for i in 0..3 {
                    sound(&mut y, RATE, 0.6 + 0.25 * f64::from(i), 0.14, 180.0, [[250.0, 600.0, 2400.0], [650.0, 1700.0, 2500.0], [400.0, 1000.0, 2200.0]][i as usize], Onset::Soft, &mut noise);
                }
                x.extend(std::iter::repeat_n(0.0, gap / 2));
                x.extend(y);
            }
            x.extend(std::iter::repeat_n(0.0, gap));
        }
        let look = Look::new(&Scan::new(&x, RATE));
        let found = split(&look);
        assert_eq!(found.len(), 3, "{found:?}");
        for (k, (&(a, b), s)) in found.iter().zip(&starts).enumerate() {
            assert!((a - s).abs() < 0.15, "take {k} starts at {a}, not {s}");
            // Its last syllable ends 5.2 s in (the third take's, with two more words, 5.7 s).
            let want = if k == 2 { 5.7 } else { 5.2 };
            assert!((b - a - want).abs() < 0.25, "take {k} is {} s long: {found:?}", b - a);
        }
        // One take alone is one take.
        let one = split(&Look::new(&Scan::new(&audio[0], RATE)));
        assert!(one.is_empty(), "{one:?}");
    }

    /// One recording of a part done several times (16-bit PCM WAV, `OS_TAKES_WAV`): the takes found in it (`cargo
    /// test -p os-dsp --release -- --ignored real_takes --nocapture`).
    #[test]
    #[ignore]
    fn real_takes() {
        let Some(path) = std::env::var_os("OS_TAKES_WAV") else { return };
        let b = std::fs::read(path).expect("reads");
        let rate = u32::from_le_bytes(b[24..28].try_into().unwrap());
        let chans = usize::from(u16::from_le_bytes(b[22..24].try_into().unwrap()));
        let data = b.windows(4).position(|w| w == b"data").expect("data") + 8;
        let x: Vec<f32> = b[data..].chunks_exact(2 * chans).map(|f| f32::from(i16::from_le_bytes([f[0], f[1]])) / 32768.0).collect();
        let t0 = std::time::Instant::now();
        let look = Look::new(&Scan::new(&x, rate));
        let takes = split(&look);
        println!("{} takes in {:?}: {takes:.2?}", takes.len(), t0.elapsed());
    }
}
