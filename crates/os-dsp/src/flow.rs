// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Fit to tempo: a rapped or sung take tightened to the beat grid syllable by syllable, as an editor would by hand.
//!
//! - Where each syllable lands is its perceptual centre (`syllables`).
//! - The flow: each beat is read as straight sixteenths, sixteenth triplets or thirty-seconds, whichever fits its
//!   syllables, sixteenths favoured and a change of feel costing something (a Viterbi path over the beats). Rap keeps to
//!   a sixteen-slot bar unless a flow is consistently in triplets (Ohriner, 2019); a fit that prefers the simpler grid
//!   unless the timing says otherwise is how rhythm quantization is done (Cemgil, Desain and Kappen, 2000).
//! - Each syllable then takes the line it meant: the lines in order, no two syllables on one, a syllable too far from
//!   any left as sung (it was meant off the grid).
//! - The tempo: where the singer drifts ahead or behind, the drift is followed (a running median over two beats each
//!   side of how far the syllables sit off their lines) so a phrase that drags is read as dragging, not as a slot late;
//!   the take's own pocket (an even laid-back or pushed feel) can be kept.
//! - Strength and tolerance work as quantize's Q-Strength and Q-Range: syllables already within the tolerance stay.
//! - The moves are made by `retime` (silences first, then vowels; attacks moved whole) and played by `warp`.

use crate::syllables::Scan;

/// Timing spread of a syllable meant on a line, seconds: the scale of the fit's costs.
const SPREAD: f64 = 0.028;
/// The cost of a syllable left off the grid (about two spreads off a line).
const CAP: f64 = 4.0;
/// The cost of two syllables of a beat nearest the same line.
const COLLIDE: f64 = 3.0;
/// The cost of a change of grid between beats.
const SWITCH: f64 = 2.0;
/// Auto's grids (steps per beat) and what each costs per beat: sixteenths free.
const AUTO: [(u32, f64); 3] = [(4, 0.0), (6, 1.5), (8, 2.5)];
/// The drift is followed over this many beats each side.
const DRIFT_BEATS: f64 = 2.0;
/// A pocket is kept only up to this, seconds: more is the take sitting off the beat, which is fitted.
const MAX_POCKET: f64 = 0.03;
/// A stretch of audio after the last clip this long can absorb the moves; a shorter one moves with them.
const ABSORB: f64 = 0.25;

/// One clip of the audio: the stretch of it that plays (`from` .. `to`, seconds into the audio) and where it plays
/// on the beat grid (`beat` at `from`, `beats_per_sec` from there on).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub from: f64,
    pub to: f64,
    pub beat: f64,
    pub beats_per_sec: f64,
}

/// The lines syllables go to: Auto reads each beat's own; `Steps(n)`: n to every beat (2 eighths, 4 sixteenths, 6
/// sixteenth triplets, 8 thirty-seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grid {
    Auto,
    Steps(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// How far each syllable goes to its line, 0 .. 1.
    pub strength: f64,
    /// Syllables this near their line stay, seconds.
    pub tolerance: f64,
    /// Keep the take's even laid-back or pushed feel.
    pub keep_pocket: bool,
    pub grid: Grid,
}

/// What a fit does.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    /// (input, output) seconds, both rising, from (0, 0) to the audio's end (`retime::map`).
    pub map: Vec<(f64, f64)>,
    /// Syllables found in the clips, and how many were moved and by how much on average, seconds.
    pub syllables: usize,
    pub moved: usize,
    pub mean_move: f64,
    /// The pocket the take sits in, seconds (positive: behind the beat), and whether it was kept.
    pub pocket: f64,
    pub kept_pocket: bool,
    /// The beats (whole beat numbers) read as triplets, and as thirty-seconds.
    pub triplets: Vec<i64>,
    pub fast: Vec<i64>,
}

/// The fit of `scan`'s recording, played by `spans`, to the beat grid.
pub fn plan(scan: &Scan, spans: &[Span], set: &Settings) -> Plan {
    let mut plan = Plan::default();
    let mut anchors = Vec::new();
    let mut spans: Vec<Span> = spans.iter().filter(|s| s.to > s.from && s.beats_per_sec > 0.0).copied().collect();
    spans.sort_by(|a, b| a.from.total_cmp(&b.from));
    let (mut shift, mut last_end) = (0.0, f64::NEG_INFINITY);
    let (mut moved_sum, mut pockets) = (0.0, Vec::new());
    for sp in &spans {
        let (from, to) = (sp.from.clamp(0.0, scan.len), sp.to.clamp(0.0, scan.len));
        // A clip straight after the one before carries on where that one's moves left its audio.
        let c = if (from - last_end).abs() < 1e-3 { shift } else { 0.0 };
        anchors.push((from, from + c));
        let here: Vec<f64> = scan.syllables.iter().map(|s| s.at).filter(|&t| t >= from && t < to).collect();
        let beats: Vec<f64> = here.iter().map(|t| sp.beat + (t - from) * sp.beats_per_sec).collect();
        let read = read_flow(&beats, SPREAD * sp.beats_per_sec, set.grid);
        let pocket = read.pocket / sp.beats_per_sec;
        let keep = if set.keep_pocket && pocket.abs() <= MAX_POCKET { pocket } else { 0.0 };
        let mut moves: Vec<Option<f64>> = beats
            .iter()
            .zip(&read.slot)
            .map(|(b, slot)| {
                slot.map(|s| {
                    let e = (s - b) / sp.beats_per_sec + keep;
                    if e.abs() <= set.tolerance { 0.0 } else { set.strength * e }
                })
            })
            .collect();
        fill(&here, &mut moves);
        for (t, m) in here.iter().zip(&moves) {
            let m = m.unwrap_or(0.0);
            anchors.push((*t, t + c + m));
            if m.abs() > 1e-3 {
                plan.moved += 1;
                moved_sum += m.abs();
            }
        }
        shift = c + moves.last().copied().flatten().unwrap_or(0.0);
        anchors.push((to, to + shift));
        last_end = to;
        plan.syllables += here.len();
        if !here.is_empty() {
            pockets.push((pocket, here.len()));
        }
        plan.kept_pocket |= keep != 0.0;
        for &(beat, g) in &read.grids {
            match g {
                6 => plan.triplets.push(beat),
                8 => plan.fast.push(beat),
                _ => {}
            }
        }
    }
    let end = scan.len;
    if spans.is_empty() || end - last_end >= ABSORB {
        anchors.push((end, end));
    } else {
        anchors.push((end, end + shift));
    }
    plan.mean_move = if plan.moved > 0 { moved_sum / plan.moved as f64 } else { 0.0 };
    let n: usize = pockets.iter().map(|p| p.1).sum();
    plan.pocket = if n > 0 { pockets.iter().map(|(p, k)| p * *k as f64).sum::<f64>() / n as f64 } else { 0.0 };
    plan.map = crate::retime::map(scan, &anchors);
    plan
}

/// Moves for the syllables left as sung: as their neighbours' (in proportion to time), so they keep their place
/// among them.
fn fill(at: &[f64], moves: &mut [Option<f64>]) {
    let known: Vec<usize> = (0..moves.len()).filter(|&i| moves[i].is_some()).collect();
    if known.is_empty() {
        moves.iter_mut().for_each(|m| *m = Some(0.0));
        return;
    }
    for i in 0..moves.len() {
        if moves[i].is_some() {
            continue;
        }
        let after = known.partition_point(|&k| k < i);
        moves[i] = Some(match (after.checked_sub(1).map(|j| known[j]), known.get(after).copied()) {
            (Some(a), Some(b)) => {
                let (ma, mb) = (moves[a].unwrap_or(0.0), moves[b].unwrap_or(0.0));
                let span = at[b] - at[a];
                if span > 0.0 { ma + (mb - ma) * (at[i] - at[a]) / span } else { ma }
            }
            (Some(a), None) => moves[a].unwrap_or(0.0),
            (None, Some(b)) => moves[b].unwrap_or(0.0),
            (None, None) => 0.0,
        });
    }
}

/// How a run of syllables (grid positions in beats, rising) sits on the grid.
#[derive(Debug, Clone, PartialEq, Default)]
struct Read {
    /// The line each syllable meant, beats; `None`: none near enough.
    slot: Vec<Option<f64>>,
    /// The beats with syllables and the grid each reads as.
    grids: Vec<(i64, u32)>,
    /// How far behind their lines the syllables sit, the median, beats.
    pocket: f64,
}

fn read_flow(beats: &[f64], spread: f64, grid: Grid) -> Read {
    let n = beats.len();
    if n == 0 {
        return Read::default();
    }
    let mut off = vec![0.0; n];
    let mut read = Read::default();
    for _ in 0..3 {
        let q: Vec<f64> = beats.iter().zip(&off).map(|(b, o)| b - o).collect();
        let grids = choose_grids(&q, spread, grid);
        let slot = assign(&q, &grids, spread);
        for i in 0..n {
            let near: Vec<f64> = (0..n).filter(|&j| (beats[j] - beats[i]).abs() <= DRIFT_BEATS).filter_map(|j| slot[j].map(|s| beats[j] - s)).collect();
            if near.len() >= 3 {
                off[i] = median(near).clamp(-0.2, 0.2);
            }
        }
        read = Read { slot, grids, pocket: 0.0 };
    }
    let all: Vec<f64> = beats.iter().zip(&read.slot).filter_map(|(b, s)| s.map(|s| b - s)).collect();
    read.pocket = if all.is_empty() { 0.0 } else { median(all) };
    read
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// The beat a syllable at `q` beats belongs to: its own, or the next when it is within half a triplet sixteenth of
/// that one's downbeat.
fn beat_of(q: f64) -> i64 {
    (q + 1.0 / 12.0).floor() as i64
}

/// The grid of each beat with syllables (a Viterbi path: each beat's fit, its grid's cost, a change's cost).
fn choose_grids(q: &[f64], spread: f64, grid: Grid) -> Vec<(i64, u32)> {
    let options: Vec<(u32, f64)> = match grid {
        Grid::Auto => AUTO.to_vec(),
        Grid::Steps(g) => vec![(g.max(1), 0.0)],
    };
    let mut groups: Vec<(i64, Vec<f64>)> = Vec::new();
    for &x in q {
        let b = beat_of(x);
        match groups.last_mut() {
            Some((gb, v)) if *gb == b => v.push(x),
            _ => groups.push((b, vec![x])),
        }
    }
    let fit = |xs: &[f64], g: u32| -> f64 {
        let g = f64::from(g);
        let mut cost = 0.0;
        let mut last: Option<i64> = None;
        for &x in xs {
            let line = (x * g).round();
            cost += (((x - line / g) / spread).powi(2)).min(CAP);
            if last == Some(line as i64) {
                cost += COLLIDE;
            }
            last = Some(line as i64);
        }
        cost
    };
    let k = options.len();
    let mut cost: Vec<Vec<f64>> = Vec::with_capacity(groups.len());
    let mut back: Vec<Vec<usize>> = Vec::with_capacity(groups.len());
    for (j, (_, xs)) in groups.iter().enumerate() {
        let here: Vec<f64> = options.iter().map(|&(g, prior)| fit(xs, g) + prior).collect();
        if j == 0 {
            cost.push(here);
            back.push(vec![0; k]);
            continue;
        }
        let prev = &cost[j - 1];
        let mut row = vec![0.0; k];
        let mut from = vec![0; k];
        for o in 0..k {
            let (best, c) = (0..k).map(|p| (p, prev[p] + if p == o { 0.0 } else { SWITCH })).min_by(|a, b| a.1.total_cmp(&b.1)).expect("options");
            row[o] = c + here[o];
            from[o] = best;
        }
        cost.push(row);
        back.push(from);
    }
    let mut pick = vec![0usize; groups.len()];
    if let Some(last) = cost.last() {
        let mut o = (0..k).min_by(|&a, &b| last[a].total_cmp(&last[b])).unwrap_or(0);
        for j in (0..groups.len()).rev() {
            pick[j] = o;
            o = back[j][o];
        }
    }
    groups.iter().zip(pick).map(|((b, _), o)| (*b, options[o].0)).collect()
}

/// The line each syllable meant: the lines of each beat's grid (sixteenths where a beat has none), taken in order, no
/// line twice, a syllable whose nearest free line costs more than `CAP` left without one. A dynamic program over the
/// syllables whose state is the last line taken.
fn assign(q: &[f64], grids: &[(i64, u32)], spread: f64) -> Vec<Option<f64>> {
    let n = q.len();
    if n == 0 {
        return Vec::new();
    }
    let (first, last) = (beat_of(q[0]) - 1, beat_of(q[n - 1]) + 1);
    let mut lines = Vec::new();
    for b in first..=last {
        let g = grids.iter().find(|(gb, _)| *gb == b).map_or(4, |x| x.1);
        lines.extend((0..g).map(|k| b as f64 + f64::from(k) / f64::from(g)));
    }
    lines.sort_by(f64::total_cmp);
    lines.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    #[derive(Clone, Copy)]
    struct St {
        last: Option<usize>,
        cost: f64,
        back: usize,
        pick: Option<usize>,
    }
    let mut layers: Vec<Vec<St>> = Vec::with_capacity(n);
    let mut prev = vec![St { last: None, cost: 0.0, back: 0, pick: None }];
    for &x in q {
        let lo = lines.partition_point(|&l| l < x - 0.5);
        let hi = lines.partition_point(|&l| l <= x + 0.5);
        let cands: Vec<(usize, f64)> = (lo..hi).map(|i| (i, ((x - lines[i]) / spread).powi(2))).filter(|c| c.1 < CAP).collect();
        let mut next: Vec<St> = Vec::new();
        let mut put = |st: St| match next.iter_mut().find(|s| s.last == st.last) {
            Some(s) if s.cost <= st.cost => {}
            Some(s) => *s = st,
            None => next.push(st),
        };
        for (pi, p) in prev.iter().enumerate() {
            put(St { last: p.last, cost: p.cost + CAP, back: pi, pick: None });
            for &(i, c) in &cands {
                if p.last.is_none_or(|l| i > l) {
                    put(St { last: Some(i), cost: p.cost + c, back: pi, pick: Some(i) });
                }
            }
        }
        next.sort_by(|a, b| a.cost.total_cmp(&b.cost));
        next.truncate(24);
        layers.push(next.clone());
        prev = next;
    }
    let mut out = vec![None; n];
    let mut s = 0usize;
    for j in (0..n).rev() {
        let st = layers[j][s];
        out[j] = st.pick.map(|i| lines[i]);
        s = st.back;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syllables::voice::*;

    const RATE: u32 = 48000;

    /// A take of syllables whose vowels start at `at` (seconds), with silence enough around them.
    fn take(at: &[f64], secs: f64) -> Vec<f32> {
        let mut x = vec![0.0f32; (secs * f64::from(RATE)) as usize];
        let mut noise = Noise::new(11);
        let onsets = [Onset::Burst, Onset::Soft, Onset::Hiss, Onset::Soft];
        for (i, &t) in at.iter().enumerate() {
            syllable(&mut x, RATE, t, 0.11, 120.0 + 7.0 * (i % 5) as f32, onsets[i % 4], &mut noise);
        }
        x
    }

    /// Jitter in -1 .. 1, the same every run.
    fn jitter(i: usize) -> f64 {
        ((i as f64 * 12.9898).sin() * 43758.5453).rem_euclid(1.0) * 2.0 - 1.0
    }

    fn fit(x: &[f32], set: &Settings, bps: f64) -> (Plan, Scan, Scan) {
        let s = Scan::new(x, RATE);
        let plan = plan(&s, &[Span { from: 0.0, to: s.len, beat: 0.0, beats_per_sec: bps }], set);
        let pts: Vec<(f64, f64)> = plan.map.iter().map(|(a, b)| (a * f64::from(RATE), b * f64::from(RATE))).collect();
        let (l, _) = crate::warp::render(x, x, RATE, &pts);
        let out = Scan::new(&l, RATE);
        (plan, s, out)
    }

    /// Mean distance of the syllables' centres from their nearest line of `g` per beat, seconds.
    fn off_grid(s: &Scan, bps: f64, g: f64) -> f64 {
        let d: Vec<f64> = s.syllables.iter().map(|y| y.at).map(|t| ((t * bps * g).round() / g / bps - t).abs()).collect();
        d.iter().sum::<f64>() / d.len().max(1) as f64
    }

    const TIGHT: Settings = Settings { strength: 1.0, tolerance: 0.0, keep_pocket: false, grid: Grid::Auto };

    /// A sixteenth-note flow at 90 BPM sung up to 30 ms off: fitted, every syllable lands within a few milliseconds of
    /// its line, none lost, none moved to the wrong line.
    #[test]
    fn a_loose_sixteenth_flow_lands_on_the_grid() {
        let bps = 1.5;
        // The syllables measure where they land; the vowels are placed so they land on the lines, then jittered.
        let lines: Vec<f64> = (0..24).filter(|i| i % 4 != 3).map(|i| (1.0 + f64::from(i) * 0.25) / bps).collect();
        let probe = Scan::new(&take(&lines, 5.0), RATE);
        let lead = probe.syllables.iter().zip(&lines).map(|(y, l)| y.at - l).sum::<f64>() / lines.len() as f64;
        let sung: Vec<f64> = lines.iter().enumerate().map(|(i, l)| l - lead + 0.03 * jitter(i)).collect();
        let x = take(&sung, 5.0);
        let (plan, before, after) = fit(&x, &TIGHT, bps);
        assert_eq!(plan.syllables, lines.len());
        assert_eq!(after.syllables.len(), lines.len());
        assert!(plan.triplets.is_empty() && plan.fast.is_empty(), "read as sixteenths: {:?} {:?}", plan.triplets, plan.fast);
        let (was, now) = (off_grid(&before, bps, 4.0), off_grid(&after, bps, 4.0));
        assert!(was > 0.012 && now < 0.004, "off the grid: {was} s before, {now} s after");
        for (a, l) in after.syllables.iter().zip(&lines) {
            assert!((a.at - l).abs() < 0.008, "landed at {}, line {l}", a.at);
        }
    }

    /// Beats sung in triplets among sixteenths are read as triplets, and their syllables go to triplet lines.
    #[test]
    fn reads_triplets_in_the_flow() {
        let bps = 1.5;
        let mut lines = Vec::new();
        for beat in 0..8 {
            let g = if (3..6).contains(&beat) { 6.0 } else { 4.0 };
            let steps: &[f64] = if g == 6.0 { &[0.0, 2.0, 4.0] } else { &[0.0, 1.0, 2.0] };
            lines.extend(steps.iter().map(|k| (1.0 + f64::from(beat) + k / g) / bps));
        }
        let probe = Scan::new(&take(&lines, 6.0), RATE);
        let lead = probe.syllables.iter().zip(&lines).map(|(y, l)| y.at - l).sum::<f64>() / lines.len() as f64;
        let sung: Vec<f64> = lines.iter().enumerate().map(|(i, l)| l - lead + 0.02 * jitter(i + 5)).collect();
        let (plan, _, after) = fit(&take(&sung, 6.0), &TIGHT, bps);
        // Beats 3 to 5 of the flow, after a beat's lead-in.
        assert_eq!(plan.triplets, [4, 5, 6], "triplet beats");
        for (a, l) in after.syllables.iter().zip(&lines) {
            assert!((a.at - l).abs() < 0.008, "landed at {}, line {l}", a.at);
        }
    }

    /// A take sitting an even 20 ms behind the beat: Natural (keep the pocket) leaves it there and only tightens the
    /// spread; Tight puts it on the beat.
    #[test]
    fn keeps_the_pocket_when_asked() {
        let bps = 1.5;
        let lines: Vec<f64> = (0..20).map(|i| (1.0 + f64::from(i) * 0.25) / bps).collect();
        let probe = Scan::new(&take(&lines, 4.5), RATE);
        let lead = probe.syllables.iter().zip(&lines).map(|(y, l)| y.at - l).sum::<f64>() / lines.len() as f64;
        let sung: Vec<f64> = lines.iter().enumerate().map(|(i, l)| l - lead + 0.02 + 0.012 * jitter(i + 9)).collect();
        let x = take(&sung, 4.5);
        let natural = Settings { strength: 1.0, tolerance: 0.0, keep_pocket: true, grid: Grid::Auto };
        let (plan, _, after) = fit(&x, &natural, bps);
        assert!(plan.kept_pocket && (plan.pocket - 0.02).abs() < 0.006, "pocket {}", plan.pocket);
        let late: Vec<f64> = after.syllables.iter().zip(&lines).map(|(a, l)| a.at - l).collect();
        assert!(late.iter().all(|d| (d - plan.pocket).abs() < 0.008), "{late:?}");
        let (_, _, tight) = fit(&x, &TIGHT, bps);
        assert!(off_grid(&tight, bps, 4.0) < 0.004);
    }

    /// Syllables within the tolerance are left where they are; the others are moved.
    #[test]
    fn leaves_tight_syllables_alone() {
        let bps = 1.5;
        let lines: Vec<f64> = (0..12).map(|i| (1.0 + f64::from(i) * 0.25) / bps).collect();
        let probe = Scan::new(&take(&lines, 3.0), RATE);
        let lead = probe.syllables.iter().zip(&lines).map(|(y, l)| y.at - l).sum::<f64>() / lines.len() as f64;
        let offs = [0.004, -0.003, 0.025, 0.002, -0.022, 0.001, 0.0, 0.03, -0.004, 0.003, 0.02, -0.002];
        let sung: Vec<f64> = lines.iter().zip(offs).map(|(l, o)| l - lead + o).collect();
        let set = Settings { strength: 1.0, tolerance: 0.008, keep_pocket: false, grid: Grid::Steps(4) };
        let (plan, _, _) = fit(&take(&sung, 3.0), &set, bps);
        assert_eq!(plan.moved, 4, "the four more than 8 ms off");
    }
}
