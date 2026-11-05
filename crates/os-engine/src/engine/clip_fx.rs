// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Effects and automation on a piece of audio, in the engine (`os_model::clip_fx`). A unit's chain is a mixer track after
//! the project's own, unseen in the mixer: the cut's plays into the whole audio's, the whole audio's into the channel's
//! mixer track, so delay compensation, several cores, hosted plugins and their parameters work as on any track. Volume
//! and pan lanes act on the clip's voice, on the sample's clock, before the cut's plugins. Parameter lanes become
//! automation clips over the clips' spans.

use std::sync::Arc;

use os_model::clip_fx::{FxUnit, Lane};
use os_model::target::Target;
use os_model::{AutoPoint, ChannelKind, Clip, ClipSource, Project};

use super::{AutoClip, ChannelState};

/// A unit's chain among the engine's tracks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Chain {
    pub handle: u32,
    /// Its place in `Engine::tracks`.
    pub track: usize,
    /// Where it plays: the whole audio's chain, or the channel's mixer track.
    pub dest: usize,
    /// The channel whose voices reach it.
    pub row: usize,
}

/// A clip's volume and pan lanes on its voice's clock.
#[derive(Debug, Clone)]
pub(super) struct ClipLanes {
    pub volume: Option<Arc<[AutoPoint]>>,
    pub pan: Option<Arc<[AutoPoint]>>,
    /// Where in the sample the clip starts, in seconds, and the seconds of the sample that pass per frame.
    pub from_s: f64,
    pub per_frame: f64,
}

impl ClipLanes {
    fn seconds(&self, frames: u64) -> f64 {
        self.from_s + frames as f64 * self.per_frame
    }

    /// The volume lane's gain `frames` into the clip.
    pub fn gain(&self, frames: u64) -> f32 {
        self.volume.as_ref().map_or(1.0, |p| lane_gain(os_model::curve::value_at(p, self.seconds(frames)).unwrap_or(os_model::clip_fx::VOLUME_UNITY)))
    }

    /// The pan lane's pan (-1..1) `frames` into the clip; `None` without one.
    pub fn pan(&self, frames: u64) -> Option<f32> {
        let p = self.pan.as_ref()?;
        Some((2.0 * os_model::curve::value_at(p, self.seconds(frames)).unwrap_or(os_model::clip_fx::PAN_CENTRE) - 1.0).clamp(-1.0, 1.0) as f32)
    }
}

/// The gain of a volume lane's value: a mixer fader's law, 0.8 being unity and 1 about +5.6 dB.
pub fn lane_gain(v: f64) -> f32 {
    os_dsp::level::curve(v.clamp(0.0, 1.0) as f32 * 1.25)
}

/// The left and right gains of a pan lane's pan on the engine's own voices: constant power, 1 and 1 at the centre, as
/// Lemon Sampler's voices pan.
pub(super) fn pan_gains(pan: f32) -> (f32, f32) {
    let a = (f64::from(pan.clamp(-1.0, 1.0)) + 1.0) * std::f64::consts::FRAC_PI_4;
    ((std::f64::consts::SQRT_2 * a.cos()) as f32, (std::f64::consts::SQRT_2 * a.sin()) as f32)
}

/// A channel plays a sample: its clips can hold units.
fn plays_sample(kind: ChannelKind) -> bool {
    matches!(kind, ChannelKind::Sampler | ChannelKind::AudioClip)
}

/// The chain of `unit` if it has one to run.
fn unit_with_plugins(project: &Project, handle: u32) -> Option<&FxUnit> {
    project.fx_unit(handle).filter(|u| u.has_plugins())
}

/// The chains of the song's clips and channels, on tracks from `first` up. Each whole audio's chain takes over its
/// channel's insert, then each cut's plays into that. Every clip's chain is here, also while the clip is muted: muting
/// keeps the layout, and a chain's tail, as it is.
pub(super) fn plan(project: &Project, song: bool, first: usize, channels: &mut [ChannelState]) -> Vec<Chain> {
    let mut chains: Vec<Chain> = Vec::new();
    for (row, ch) in project.channels.iter().enumerate() {
        if !plays_sample(ch.kind) || unit_with_plugins(project, ch.whole_fx).is_none() || chains.iter().any(|c| c.handle == ch.whole_fx) {
            continue;
        }
        let Some(state) = channels.get_mut(row) else { continue };
        let track = first + chains.len();
        chains.push(Chain { handle: ch.whole_fx, track, dest: state.insert, row });
        state.insert = track;
    }
    if !song {
        return chains;
    }
    for c in project.arrangement().map_or(&[][..], |a| &a.clips) {
        let ClipSource::Channel(ch) = c.source else { continue };
        let row = usize::from(ch);
        let kind = project.channels.get(row).map(|x| x.kind);
        if !kind.is_some_and(plays_sample) || unit_with_plugins(project, c.fx).is_none() || chains.iter().any(|x| x.handle == c.fx) {
            continue;
        }
        let Some(state) = channels.get_mut(row) else { continue };
        let track = first + chains.len();
        chains.push(Chain { handle: c.fx, track, dest: state.insert, row });
        state.chains.push((c.fx, track));
    }
    chains
}

/// What a clip's voice takes from its units: its chain (0: none) and its lanes (the place + 1 in `ChannelState::lanes`,
/// 0: none).
pub(super) fn voice_of(project: &Project, c: &Clip, state: &mut ChannelState, from_s: f64) -> (u32, u32) {
    let unit = if state.chains.iter().any(|x| x.0 == c.fx) { c.fx } else { 0 };
    let Some((_, rate)) = state.played() else { return (unit, 0) };
    let arc = |l: Lane| project.clip_lane(c, l).map(|r| Arc::<[AutoPoint]>::from(r.points));
    let (volume, pan) = (arc(Lane::Volume), arc(Lane::Pan));
    if volume.is_none() && pan.is_none() {
        return (unit, 0);
    }
    let per_frame = state.base_rate / f64::from(rate.max(1)) / state.from_scale.max(1e-9);
    state.lanes.push(ClipLanes { volume, pan, from_s, per_frame });
    (unit, state.lanes.len() as u32)
}

/// A clip's parameter lanes as automation clips over its span: `curves` gets their curves, in beats from the clip's start.
/// A lane of the whole audio's plugins is the cut's own where it has one, else the whole audio's; a lane of the cut's own
/// plugins is only ever its own. A plugin that is not there has no lane.
#[allow(clippy::too_many_arguments)]
pub(super) fn param_clips(project: &Project, chains: &[Chain], channels: &[ChannelState], ppq: u16, tick_tempo: f64, curves: &mut Vec<os_model::Automation>, autos: &mut Vec<AutoClip>) {
    let track_of = |handle: u32| chains.iter().find(|c| c.handle == handle).map(|c| c.track);
    for c in project.arrangement().map_or(&[][..], |a| &a.clips) {
        let ClipSource::Channel(ch) = c.source else { continue };
        if !super::clip_audible(project, c) {
            continue;
        }
        let row = usize::from(ch);
        let (Some(channel), Some(state)) = (project.channels.get(row), channels.get(row)) else { continue };
        if !plays_sample(channel.kind) {
            continue;
        }
        let Some((_, rate)) = state.played() else { continue };
        // Seconds of the sample per beat of the song: the frames of a beat, times the sample's seconds per frame.
        let per_beat = 60.0 / tick_tempo.max(1e-9) / state.frame_s * state.base_rate / f64::from(rate.max(1)) / state.from_scale.max(1e-9);
        let from_s = c.window.map_or(0.0, |w| w.0.max(0.0) / 1000.0);
        let beats = f64::from(c.length) / f64::from(ppq.max(1));

        let mut lanes: Vec<Lane> = Vec::new();
        for h in [c.fx, channel.whole_fx] {
            for l in project.fx_unit(h).map_or(Vec::new(), FxUnit::lanes) {
                if matches!(l, Lane::Param { .. }) && !lanes.contains(&l) {
                    lanes.push(l);
                }
            }
        }
        for lane in lanes {
            let Lane::Param { whole, slot, param } = lane else { continue };
            // The chain the plugin is in, the plugin present.
            let holder = if whole { channel.whole_fx } else { c.fx };
            let (Some(track), Some(unit)) = (track_of(holder), project.fx_unit(holder)) else { continue };
            if unit.track.slots.get(usize::from(slot)).is_none_or(Option::is_none) {
                continue;
            }
            let Some(r) = project.clip_lane(c, lane) else { continue };
            let points = in_beats(r.points, from_s, per_beat, beats);
            if points.is_empty() {
                continue;
            }
            curves.push(os_model::Automation {
                channel: u16::MAX,
                targets: Vec::new(),
                points,
                header: Vec::new(),
                tail: Vec::new(),
                lfo: os_model::clip_lfo::ClipLfo::default(),
            });
            autos.push(AutoClip {
                start: u64::from(c.position),
                end: u64::from(c.end()),
                from_beat: 0.0,
                curve: curves.len() - 1,
                targets: vec![Target::SlotParam(track as u16, slot, param)],
                walk: None,
                out: os_model::clip_lfo::ClipOut::default(),
                env: None,
            });
        }
    }
}

/// A lane's points in beats from a clip's start: those inside the clip, with the curve's values at its two ends.
fn in_beats(points: &[AutoPoint], from_s: f64, per_beat: f64, beats: f64) -> Vec<AutoPoint> {
    if per_beat <= 0.0 || !per_beat.is_finite() {
        return Vec::new();
    }
    let at = |s: f64| (s - from_s) / per_beat;
    let level = |b: f64| os_model::curve::value_at(points, from_s + b * per_beat);
    let mut v: Vec<AutoPoint> = Vec::with_capacity(points.len() + 2);
    if let Some(value) = level(0.0) {
        v.push(AutoPoint { beat: 0.0, value, tension: 0.0, mode: os_model::AutoMode::Single, flag: 2 });
    }
    for p in points {
        let b = at(p.beat);
        if b > 0.0 && b < beats {
            v.push(AutoPoint { beat: b, ..*p });
        }
    }
    if let Some(value) = level(beats) {
        // The segment that ends here is the one that runs through the clip's end.
        let next = points.iter().find(|p| at(p.beat) >= beats);
        v.push(AutoPoint { beat: beats, value, ..next.copied().unwrap_or(AutoPoint { beat: 0.0, value, tension: 0.0, mode: os_model::AutoMode::Single, flag: 2 }) });
    }
    v
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_1_SQRT_2;
    use std::sync::{Arc, Mutex};

    use os_model::clip_fx::{point, track_of, Lane};
    use os_model::{Arrangement, Channel, ChannelKind, Clip, ClipSource, MixerTrack, Project, SlotPlugin};

    use crate::sample::{Sample, SampleLibrary};
    use crate::{Engine, HostedEffect, Instrument, NoteLevels, PlayMode, SharedEffect};

    const RATE: u32 = 48000;
    /// 120 BPM at 96 PPQ: a tick is 250 frames.
    const TICK: usize = 250;

    fn channel(index: u16) -> Channel {
        Channel {
            id: os_model::Id::NONE,
            index,
            kind: ChannelKind::AudioClip,
            name: None,
            plugin: None,
            plugin_state: None,
            color: None,
            enabled: true,
            // Unity through the channel with the audio clip level.
            volume: 0.78125,
            pan: 0.0,
            pitch_cents: 0,
            mixer_track: 1,
            sample_path: Some(format!("s{index}.wav")),
            layer_children: Vec::new(),
            whole_fx: 0,
            envelopes: Default::default(),
            rack: Default::default(),
            misc: Default::default(),
            sound: Default::default(),
        }
    }

    /// A project at 120 BPM with `channels` audio channels on insert 1 and these clips (position, length, channel, window
    /// start in seconds), all ticks.
    fn project(channels: u16, clips: &[(u32, u32, u16, f64)]) -> Project {
        let mut p = Project { ppq: 96, tempo: Some(120.0), ..Project::default() };
        p.channels = (0..channels).map(channel).collect();
        p.mixer = vec![MixerTrack::new(0), MixerTrack::new(1), MixerTrack::new(2)];
        let clips = clips.iter().map(|&(pos, len, ch, from)| Clip { window: Some((from * 1000.0, from * 1000.0 + 60000.0)), ..Clip::new(pos, len, 0, ClipSource::Channel(ch)) }).collect();
        p.arrangements = vec![Arrangement { number: 1, clips, ..Default::default() }];
        p.current_arrangement = Some(1);
        p
    }

    /// A library where each channel's sample is a steady 1.0 on both sides, ten seconds long.
    fn library(p: &Project) -> SampleLibrary {
        let mut lib = SampleLibrary::default();
        let n = RATE as usize * 10;
        for c in &p.channels {
            if let Some(path) = &c.sample_path {
                lib.insert(path, Arc::new(Sample { rate: RATE, left: vec![1.0; n], right: vec![1.0; n] }));
            }
        }
        lib
    }

    fn render(p: &Project, attach: impl FnOnce(&mut Engine)) -> Vec<f32> {
        let mut lib = library(p);
        let mut e = Engine::new(p, &mut lib, RATE, PlayMode::Song);
        attach(&mut e);
        e.render_all(256)
    }

    /// The left and right levels at tick `t` of a render (a frame well inside the tick, after any ramp).
    fn at(out: &[f32], t: usize) -> (f32, f32) {
        let f = t * TICK + TICK - 1;
        (out[f * 2], out[f * 2 + 1])
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 2e-3
    }

    /// Plays one clip, no lanes: the level everything is compared against.
    fn base_level() -> f32 {
        let p = project(1, &[(0, 192, 0, 0.0)]);
        at(&render(&p, |_| {}), 100).0
    }

    #[test]
    fn without_units_nothing_changes() {
        let p = project(1, &[(0, 192, 0, 0.0)]);
        let mut with_empty = p.clone();
        // Units that hold nothing make no difference to a single sample.
        let h = with_empty.clip_unit(0).unwrap();
        let w = with_empty.whole_unit(0).unwrap();
        assert!(h != w);
        assert_eq!(render(&p, |_| {}), render(&with_empty, |_| {}));
    }

    #[test]
    fn a_volume_lane_moves_the_voice_on_the_samples_clock() {
        let base = base_level();
        let mut p = project(1, &[(0, 192, 0, 0.0)]);
        let w = p.whole_unit(0).unwrap();
        // A second of the sample is 192 ticks here: unity to silence over the clip.
        p.fx_unit_mut(w).unwrap().volume = vec![point(0.0, 0.8), point(1.0, 0.0)];
        let out = render(&p, |_| {});
        let g = |s: f64| super::lane_gain(os_model::curve::value_at(&p.fx_unit(w).unwrap().volume, s).unwrap());
        assert!(close(at(&out, 0).0, base * g((TICK - 1) as f64 / f64::from(RATE))), "{}", at(&out, 0).0);
        for t in [48usize, 96, 144] {
            let f = t * TICK + TICK - 1;
            let want = base * g(f as f64 / f64::from(RATE));
            assert!(close(at(&out, t).0, want), "tick {t}: {} {want}", at(&out, t).0);
            assert_eq!(at(&out, t).0, at(&out, t).1, "centred");
        }
        assert!(at(&out, 190).0 < base * 0.01);
    }

    #[test]
    fn a_lane_is_read_at_the_part_of_the_sample_the_clip_plays() {
        let base = base_level();
        // The second clip plays from second 5 of the sample, wherever it stands on the playlist.
        let mut p = project(1, &[(0, 96, 0, 0.0), (96, 96, 0, 5.0)]);
        let w = p.whole_unit(0).unwrap();
        p.fx_unit_mut(w).unwrap().volume = vec![point(4.0, 0.8), point(4.5, 0.4)];
        let out = render(&p, |_| {});
        assert!(close(at(&out, 48).0, base), "before the first point it holds: {}", at(&out, 48).0);
        assert!(close(at(&out, 144).0, base * super::lane_gain(0.4)), "after the last too: {}", at(&out, 144).0);

        // Moving the second clip on the playlist does not move its lane.
        let mut moved = p.clone();
        moved.arrangement_mut().unwrap().clips[1].position = 288;
        let out = render(&moved, |_| {});
        assert!(close(at(&out, 288 + 48).0, base * super::lane_gain(0.4)), "{}", at(&out, 288 + 48).0);

        // Trimming its start moves the window, and the lane stays with the audio.
        let mut trim = project(1, &[(0, 192, 0, 4.0)]);
        let w = trim.whole_unit(0).unwrap();
        trim.fx_unit_mut(w).unwrap().volume = vec![point(4.0, 0.8), point(5.0, 0.0)];
        let out = render(&trim, |_| {});
        let g = |s: f64| super::lane_gain(os_model::curve::value_at(&trim.fx_unit(w).unwrap().volume, s).unwrap());
        let f = 96 * TICK + TICK - 1;
        assert!(close(at(&out, 96).0, base * g(4.0 + f as f64 / f64::from(RATE))), "{}", at(&out, 96).0);
    }

    // The heart of it: a lane on the whole audio moves every clip of the sample at once; a cut's own lane decides for
    // that cut alone; clearing it hands the cut back; other samples never hear any of it.
    #[test]
    fn a_cuts_lane_wins_over_the_whole_audios_for_that_cut_alone() {
        let base = base_level();
        let clips = [(0, 96, 0, 0.0), (96, 96, 0, 0.5), (192, 96, 0, 1.0), (288, 96, 1, 0.0)];
        let mut p = project(2, &clips);
        let w = p.whole_unit(0).unwrap();
        p.fx_unit_mut(w).unwrap().volume = vec![point(0.0, 0.4)];
        let cut = p.clip_unit(1).unwrap();
        p.fx_unit_mut(cut).unwrap().volume = vec![point(0.0, 0.7)];

        let out = render(&p, |_| {});
        assert!(close(at(&out, 48).0, base * super::lane_gain(0.4)), "the first cut follows the whole audio");
        assert!(close(at(&out, 144).0, base * super::lane_gain(0.7)), "the second has its own: {}", at(&out, 144).0);
        assert!(close(at(&out, 240).0, base * super::lane_gain(0.4)), "the third follows the whole audio");
        assert!(close(at(&out, 336).0, base), "another sample does not hear it: {}", at(&out, 336).0);

        // Now the whole audio's lane changes: only the cuts that follow it move.
        let mut q = p.clone();
        q.fx_unit_mut(w).unwrap().volume = vec![point(0.0, 0.2)];
        let out = render(&q, |_| {});
        assert!(close(at(&out, 48).0, base * super::lane_gain(0.2)));
        assert!(close(at(&out, 144).0, base * super::lane_gain(0.7)), "the cut's own lane is not hampered");
        assert!(close(at(&out, 240).0, base * super::lane_gain(0.2)));

        // Cleared, the cut follows the whole audio again.
        let mut r = q.clone();
        r.fx_unit_mut(cut).unwrap().volume.clear();
        let out = render(&r, |_| {});
        assert!(close(at(&out, 144).0, base * super::lane_gain(0.2)));
    }

    #[test]
    fn volume_and_pan_are_decided_one_by_one() {
        let base = base_level();
        let mut p = project(1, &[(0, 96, 0, 0.0)]);
        let w = p.whole_unit(0).unwrap();
        // The whole audio pans hard left; the cut has a volume lane of its own and none for pan.
        p.fx_unit_mut(w).unwrap().pan = vec![point(0.0, 0.0)];
        p.fx_unit_mut(w).unwrap().volume = vec![point(0.0, 0.2)];
        let cut = p.clip_unit(0).unwrap();
        p.fx_unit_mut(cut).unwrap().volume = vec![point(0.0, 0.8)];
        let (l, r) = at(&render(&p, |_| {}), 48);
        assert!(close(l, base * std::f32::consts::SQRT_2), "the cut's volume, the whole audio's pan: {l}");
        assert!(close(r, 0.0), "{r}");
    }

    #[test]
    fn a_pan_lane_is_constant_power_and_unchanged_at_the_centre() {
        let base = base_level();
        let pan_at = |v: f64| {
            let mut p = project(1, &[(0, 96, 0, 0.0)]);
            let cut = p.clip_unit(0).unwrap();
            p.fx_unit_mut(cut).unwrap().pan = vec![point(0.0, v)];
            at(&render(&p, |_| {}), 48)
        };
        let (l, r) = pan_at(0.5);
        assert!(close(l, base) && close(r, base), "{l} {r}");
        let (l, r) = pan_at(1.0);
        assert!(close(l, 0.0) && close(r, base * std::f32::consts::SQRT_2), "{l} {r}");
        let (l, r) = pan_at(0.0);
        assert!(close(l, base * std::f32::consts::SQRT_2) && close(r, 0.0), "{l} {r}");
        // Constant power anywhere.
        let (l, r) = pan_at(0.25);
        assert!((l * l + r * r - 2.0 * base * base).abs() < 1e-2, "{l} {r}");
        let (pl, pr) = super::pan_gains(0.0);
        assert!((pl - 1.0).abs() < 1e-6 && (pr - 1.0).abs() < 1e-6, "{pl} {pr}");
        let _ = FRAC_1_SQRT_2;
    }

    #[test]
    fn the_volume_law_is_a_faders() {
        assert_eq!(super::lane_gain(0.8), 1.0);
        assert_eq!(super::lane_gain(0.0), 0.0);
        assert!((super::lane_gain(1.0) - os_dsp::level::curve(1.25)).abs() < 1e-6);
        assert!(super::lane_gain(0.9) > 1.0 && super::lane_gain(0.5) < 1.0);
        assert_eq!(super::lane_gain(9.0), super::lane_gain(1.0), "values stay in range");
        assert_eq!(super::lane_gain(-1.0), 0.0);
    }

    #[test]
    fn a_lane_multiplies_with_the_clips_gain_and_fades() {
        let base = base_level();
        let mut p = project(1, &[(0, 192, 0, 0.0)]);
        p.arrangement_mut().unwrap().clips[0].set_gain(0.5);
        let cut = p.clip_unit(0).unwrap();
        p.fx_unit_mut(cut).unwrap().volume = vec![point(0.0, 0.4)];
        let out = render(&p, |_| {});
        assert!(close(at(&out, 96).0, base * 0.5 * super::lane_gain(0.4)), "{}", at(&out, 96).0);
        p.arrangement_mut().unwrap().clips[0].set_gain(1.0);
        p.arrangement_mut().unwrap().clips[0].set_fade_in(Some((500.0, 0.0)));
        let out = render(&p, |_| {});
        let half = os_model::fade_curve(((48 * TICK + TICK - 1) as f64 / f64::from(RATE)) * 1000.0 / 500.0, 0.0) as f32;
        assert!(close(at(&out, 48).0, base * super::lane_gain(0.4) * half), "{}", at(&out, 48).0);
    }

    #[test]
    fn a_stretched_clip_reads_its_lane_at_the_samples_speed() {
        let base = base_level();
        // Played at half speed (resampling): two seconds of song hold one of the sample.
        let mut p = project(1, &[(0, 384, 0, 0.0)]);
        p.channels[0].sound.set_stretch_multiplier(2.0);
        let w = p.whole_unit(0).unwrap();
        p.fx_unit_mut(w).unwrap().volume = vec![point(0.0, 0.8), point(1.0, 0.0)];
        let out = render(&p, |_| {});
        // One second of the song in: half a second of the sample.
        let g = super::lane_gain(os_model::curve::value_at(&p.fx_unit(w).unwrap().volume, (192 * TICK + TICK - 1) as f64 / f64::from(RATE) / 2.0).unwrap());
        assert!((at(&out, 192).0 - base * g).abs() < 5e-3, "{} {}", at(&out, 192).0, base * g);
    }

    /// A hosted effect that multiplies the sound by a gain, and records the parameter values it gets.
    #[derive(Default)]
    struct Scale {
        gain: f32,
        seen: Vec<(u32, f64)>,
    }

    impl HostedEffect for Scale {
        fn process_fx(&mut self, _rate: u32, left: &mut [f32], right: &mut [f32]) {
            left.iter_mut().chain(right.iter_mut()).for_each(|x| *x *= self.gain);
        }
        fn param_fx(&mut self, _offset: u32, index: u32, value: f64) {
            self.seen.push((index, value));
        }
    }

    fn scale(gain: f32) -> (Arc<Mutex<Scale>>, SharedEffect) {
        let s = Arc::new(Mutex::new(Scale { gain, seen: Vec::new() }));
        (s.clone(), s as SharedEffect)
    }

    fn plugin(name: &str) -> SlotPlugin {
        SlotPlugin { id: os_model::Id::NONE, name: name.into(), state: Vec::new() }
    }

    // A cut's plugins change that cut alone; the whole audio's change every cut of the sample, after the cut's; another
    // sample passes through neither.
    #[test]
    fn plugins_on_a_cut_and_on_the_whole_audio_are_heard_where_they_belong() {
        let base = base_level();
        let clips = [(0, 96, 0, 0.0), (96, 96, 0, 0.5), (192, 96, 1, 0.0)];
        let mut p = project(2, &clips);
        let cut = p.clip_unit(1).unwrap();
        p.fx_unit_mut(cut).unwrap().track.slots[0] = Some(plugin("Cut"));
        let out = render(&p, |e| e.set_effect(usize::from(track_of(cut)), 0, scale(0.5).1));
        assert!(close(at(&out, 48).0, base), "the other cut: {}", at(&out, 48).0);
        assert!(close(at(&out, 144).0, base * 0.5), "the cut: {}", at(&out, 144).0);
        assert!(close(at(&out, 240).0, base), "another sample: {}", at(&out, 240).0);

        let whole = p.whole_unit(0).unwrap();
        p.fx_unit_mut(whole).unwrap().track.slots[0] = Some(plugin("Whole"));
        let out = render(&p, |e| {
            e.set_effect(usize::from(track_of(cut)), 0, scale(0.5).1);
            e.set_effect(usize::from(track_of(whole)), 0, scale(0.25).1);
        });
        assert!(close(at(&out, 48).0, base * 0.25), "the whole audio's alone: {}", at(&out, 48).0);
        assert!(close(at(&out, 144).0, base * 0.125), "both: {}", at(&out, 144).0);
        assert!(close(at(&out, 240).0, base), "another sample: {}", at(&out, 240).0);
    }

    // Two chains in series, each with several slots.
    #[test]
    fn a_chains_slots_run_in_order_and_obey_their_switches() {
        let base = base_level();
        let mut p = project(1, &[(0, 96, 0, 0.0)]);
        let cut = p.clip_unit(0).unwrap();
        {
            let u = p.fx_unit_mut(cut).unwrap();
            u.track.slots[0] = Some(plugin("A"));
            u.track.slots[3] = Some(plugin("B"));
            u.track.slot_on[3] = false;
        }
        let t = usize::from(track_of(cut));
        let out = render(&p, |e| {
            e.set_effect(t, 0, scale(0.5).1);
            e.set_effect(t, 3, scale(0.5).1);
        });
        assert!(close(at(&out, 48).0, base * 0.5), "the second is off: {}", at(&out, 48).0);
        p.fx_unit_mut(cut).unwrap().track.slot_on[3] = true;
        let out = render(&p, |e| {
            e.set_effect(t, 0, scale(0.5).1);
            e.set_effect(t, 3, scale(0.5).1);
        });
        assert!(close(at(&out, 48).0, base * 0.25), "both on: {}", at(&out, 48).0);
        p.fx_unit_mut(cut).unwrap().track.slot_mix[0] = 0.0;
        let out = render(&p, |e| {
            e.set_effect(t, 0, scale(0.5).1);
            e.set_effect(t, 3, scale(0.5).1);
        });
        assert!(close(at(&out, 48).0, base * 0.5), "the first's mix at 0 leaves it dry: {}", at(&out, 48).0);
    }

    // The chain plays where the channel plays: the channel's mixer track and its effects still follow.
    #[test]
    fn a_chain_plays_into_the_channels_mixer_track() {
        let base = base_level();
        let mut p = project(1, &[(0, 96, 0, 0.0)]);
        let cut = p.clip_unit(0).unwrap();
        p.fx_unit_mut(cut).unwrap().track.slots[0] = Some(plugin("Cut"));
        p.mixer[1].volume = Some(0.0);
        let t = usize::from(track_of(cut));
        let out = render(&p, |e| e.set_effect(t, 0, scale(1.0).1));
        assert!(close(at(&out, 48).0, 0.0), "the channel's fader is down: {}", at(&out, 48).0);
        p.mixer[1].volume = Some(0.8);
        p.channels[0].mixer_track = 2;
        p.mixer[1].volume = Some(0.0);
        let out = render(&p, |e| e.set_effect(t, 0, scale(1.0).1));
        assert!(close(at(&out, 48).0, base), "it follows the channel to its new track: {}", at(&out, 48).0);
    }

    // A plugin parameter's lane reaches the plugin at the sample's clock, the cut's own over the whole audio's.
    #[test]
    fn parameter_lanes_reach_their_plugin_and_a_cuts_overrides_the_whole_audios() {
        let clips = [(0, 96, 0, 0.0), (96, 96, 0, 0.5)];
        let mut p = project(1, &clips);
        let whole = p.whole_unit(0).unwrap();
        let shared = Lane::Param { whole: true, slot: 0, param: 3 };
        {
            let u = p.fx_unit_mut(whole).unwrap();
            u.track.slots[0] = Some(plugin("Whole"));
            *u.lane_mut(shared) = vec![point(0.0, 0.25)];
        }
        let cut = p.clip_unit(1).unwrap();
        *p.fx_unit_mut(cut).unwrap().lane_mut(shared) = vec![point(0.0, 0.75)];
        let (s, fx) = scale(1.0);
        let out = render(&p, |e| e.set_effect(usize::from(track_of(whole)), 0, fx));
        assert!(!out.is_empty());
        let seen = s.lock().unwrap().seen.clone();
        assert!(seen.iter().all(|v| v.0 == 3), "{seen:?}");
        assert!(seen.iter().any(|v| (v.1 - 0.25).abs() < 1e-6), "the first cut follows the whole audio: {seen:?}");
        assert!(seen.iter().any(|v| (v.1 - 0.75).abs() < 1e-6), "the second has its own: {seen:?}");
        let a = seen.iter().position(|v| (v.1 - 0.25).abs() < 1e-6).unwrap();
        let b = seen.iter().position(|v| (v.1 - 0.75).abs() < 1e-6).unwrap();
        assert!(a < b, "in song order");
    }

    #[test]
    fn a_parameter_lane_of_a_cuts_own_plugin_moves_only_that_plugin() {
        let clips = [(0, 96, 0, 0.0), (96, 96, 0, 0.5)];
        let mut p = project(1, &clips);
        let (a, b) = (p.clip_unit(0).unwrap(), p.clip_unit(1).unwrap());
        let lane = Lane::Param { whole: false, slot: 0, param: 1 };
        for h in [a, b] {
            p.fx_unit_mut(h).unwrap().track.slots[0] = Some(plugin("Cut"));
        }
        *p.fx_unit_mut(b).unwrap().lane_mut(lane) = vec![point(0.0, 0.0), point(1.0, 1.0)];
        let ((sa, fa), (sb, fb)) = (scale(1.0), scale(1.0));
        render(&p, |e| {
            e.set_effect(usize::from(track_of(a)), 0, fa);
            e.set_effect(usize::from(track_of(b)), 0, fb);
        });
        assert!(sa.lock().unwrap().seen.is_empty());
        let seen = sb.lock().unwrap().seen.clone();
        assert!(seen.len() > 3, "{seen:?}");
        // The clip plays seconds 0.5 to 1 of the sample: half way up the lane and on.
        assert!((seen[0].1 - 0.5).abs() < 0.02, "{seen:?}");
        assert!(seen.windows(2).all(|w| w[1].1 >= w[0].1), "{seen:?}");
        assert!(seen.last().unwrap().1 > 0.9, "{seen:?}");
    }

    #[test]
    fn a_lane_of_a_plugin_that_is_gone_does_nothing() {
        let mut p = project(1, &[(0, 96, 0, 0.0)]);
        let cut = p.clip_unit(0).unwrap();
        *p.fx_unit_mut(cut).unwrap().lane_mut(Lane::Param { whole: false, slot: 4, param: 0 }) = vec![point(0.0, 1.0)];
        *p.fx_unit_mut(cut).unwrap().lane_mut(Lane::Param { whole: true, slot: 4, param: 0 }) = vec![point(0.0, 1.0)];
        let base = base_level();
        assert!(close(at(&render(&p, |_| {}), 48).0, base));
    }

    // Muting a clip silences its chain's input and keeps the layout; a clip whose unit has plugins but is muted plays
    // nothing.
    #[test]
    fn a_muted_clip_with_plugins_is_silent() {
        let mut p = project(1, &[(0, 96, 0, 0.0), (96, 96, 0, 0.5)]);
        let cut = p.clip_unit(0).unwrap();
        p.fx_unit_mut(cut).unwrap().track.slots[0] = Some(plugin("Cut"));
        p.arrangement_mut().unwrap().clips[0].muted = true;
        let t = usize::from(track_of(cut));
        let base = base_level();
        let out = render(&p, |e| e.set_effect(t, 0, scale(1.0).1));
        assert!(close(at(&out, 48).0, 0.0), "{}", at(&out, 48).0);
        assert!(close(at(&out, 144).0, base), "{}", at(&out, 144).0);
    }

    // A seek into a clip reads the lane where playback lands, and plays through the chain.
    #[test]
    fn starting_inside_a_clip_keeps_lanes_and_chain() {
        let base = base_level();
        let mut p = project(1, &[(0, 192, 0, 0.0)]);
        let cut = p.clip_unit(0).unwrap();
        {
            let u = p.fx_unit_mut(cut).unwrap();
            u.volume = vec![point(0.0, 0.8), point(1.0, 0.0)];
            u.track.slots[0] = Some(plugin("Cut"));
        }
        let mut lib = library(&p);
        let mut e = Engine::new(&p, &mut lib, RATE, PlayMode::Song);
        e.set_effect(usize::from(track_of(cut)), 0, scale(0.5).1);
        e.seek_tick(96);
        let mut out = vec![0.0; TICK * 8 * 2];
        e.process(&mut out);
        let lane = &p.fx_unit(cut).unwrap().volume;
        // Frames after the short fade a late start begins with.
        let f = TICK * 4 + TICK - 1;
        let s = (96 * TICK + f) as f64 / f64::from(RATE);
        let want = base * 0.5 * super::lane_gain(os_model::curve::value_at(lane, s).unwrap());
        assert!(close(out[f * 2], want), "{} {want}", out[f * 2]);
    }

    /// A sampler-like instrument with voices the engine names: it logs what reaches it, and plays a steady 1 per voice on
    /// that voice's output, times the voice's gain and pan.
    #[derive(Default)]
    struct Hosted {
        voices: Vec<(u32, u8, f32, f32)>,
        log: Vec<String>,
        buses: Vec<(Vec<f32>, Vec<f32>)>,
    }

    impl Instrument for Hosted {
        fn note_on(&mut self, _offset: u32, _key: u8, _velocity: f32) {}
        fn note_off(&mut self, _offset: u32, _key: u8) {}
        fn voice_start_on(&mut self, _offset: u32, _key: u8, _velocity: f32, _start: f32, _levels: NoteLevels, voice: u32, bus: u8) {
            self.log.push(format!("start {voice} on {bus}"));
            self.voices.push((voice, bus, 1.0, 0.0));
        }
        fn voice_buses(&self) -> usize {
            15
        }
        fn voice_gains(&self) -> bool {
            true
        }
        fn voice_gain(&mut self, _offset: u32, voice: u32, gain: f32) {
            if let Some(v) = self.voices.iter_mut().find(|v| v.0 == voice) {
                v.2 = gain;
            }
        }
        fn voice_pan(&mut self, _offset: u32, voice: u32, pan: f32) {
            if let Some(v) = self.voices.iter_mut().find(|v| v.0 == voice) {
                v.3 = pan;
            }
        }
        fn voice_choke(&mut self, _offset: u32, _key: u8, voice: u32) {
            self.log.push(format!("choke {voice}"));
            self.voices.retain(|v| v.0 != voice);
        }
        fn take_bus(&mut self, bus: usize, left: &mut [f32], right: &mut [f32]) -> bool {
            let Some((l, r)) = self.buses.get(bus) else { return false };
            left.copy_from_slice(&l[..left.len()]);
            right.copy_from_slice(&r[..right.len()]);
            true
        }
        fn process(&mut self, _rate: u32, left: &mut [f32], right: &mut [f32]) {
            left.fill(0.0);
            right.fill(0.0);
            self.buses = vec![(vec![0.0; left.len()], vec![0.0; left.len()]); 16];
            for &(_, bus, gain, pan) in &self.voices {
                let (pl, pr) = super::pan_gains(pan);
                if bus == 0 {
                    left.iter_mut().for_each(|x| *x += gain * pl);
                    right.iter_mut().for_each(|x| *x += gain * pr);
                } else {
                    let (l, r) = &mut self.buses[usize::from(bus)];
                    l.iter_mut().for_each(|x| *x += gain * pl);
                    r.iter_mut().for_each(|x| *x += gain * pr);
                }
            }
        }
        fn active(&self) -> bool {
            !self.voices.is_empty()
        }
    }

    fn hosted_render(p: &Project, inst: &Arc<Mutex<Hosted>>, attach: impl FnOnce(&mut Engine)) -> Vec<f32> {
        let mut lib = library(p);
        let shared: crate::SharedInstrument = inst.clone();
        let mut e = Engine::with_instruments(p, &mut lib, RATE, PlayMode::Song, &[(0, shared)]);
        attach(&mut e);
        e.render_all(256)
    }

    // With a hosted sampler: a cut's chain gets its voice's own output, the rest stay on the main output.
    #[test]
    fn a_hosted_samplers_cut_plays_on_a_bus_through_its_chain() {
        let clips = [(0, 96, 0, 0.0), (96, 96, 0, 0.5), (192, 96, 0, 1.0)];
        let mut p = project(1, &clips);
        let cut = p.clip_unit(1).unwrap();
        p.fx_unit_mut(cut).unwrap().track.slots[0] = Some(plugin("Cut"));
        let t = usize::from(track_of(cut));
        let inst = Arc::new(Mutex::new(Hosted::default()));
        let out = hosted_render(&p, &inst, |e| e.set_effect(t, 0, scale(0.5).1));
        let log = inst.lock().unwrap().log.clone();
        let starts: Vec<&String> = log.iter().filter(|l| l.starts_with("start")).collect();
        assert_eq!(starts.len(), 3, "{log:?}");
        assert!(starts[0].ends_with("on 0") && starts[1].ends_with("on 1") && starts[2].ends_with("on 0"), "{log:?}");
        let level = at(&out, 48).0;
        assert!(level > 0.1, "{level}");
        assert!(close(at(&out, 144).0, level * 0.5), "through the chain: {} {level}", at(&out, 144).0);
        assert!(close(at(&out, 240).0, level), "the main output again: {}", at(&out, 240).0);
    }

    // With a hosted sampler the lanes reach the voices as gains and pans, per clip.
    #[test]
    fn a_hosted_samplers_voices_take_their_lanes() {
        let clips = [(0, 96, 0, 0.0), (96, 96, 0, 0.5)];
        let mut plain = project(1, &clips);
        let inst = Arc::new(Mutex::new(Hosted::default()));
        let level = at(&hosted_render(&plain, &inst, |_| {}), 48).0;
        let w = plain.whole_unit(0).unwrap();
        plain.fx_unit_mut(w).unwrap().volume = vec![point(0.0, 0.4)];
        let cut = plain.clip_unit(1).unwrap();
        {
            let u = plain.fx_unit_mut(cut).unwrap();
            u.volume = vec![point(0.0, 0.8)];
            u.pan = vec![point(0.0, 1.0)];
        }
        let inst = Arc::new(Mutex::new(Hosted::default()));
        let out = hosted_render(&plain, &inst, |_| {});
        assert!(close(at(&out, 48).0, level * super::lane_gain(0.4)), "{} {level}", at(&out, 48).0);
        assert!(close(at(&out, 144).1, level * std::f32::consts::SQRT_2), "{}", at(&out, 144).1);
        assert!(close(at(&out, 144).0, 0.0), "{}", at(&out, 144).0);
    }

    // Many cuts with chains at once, more than there are buses: nothing is lost, the rest just play dry.
    #[test]
    fn more_cuts_than_buses_still_sound() {
        let clips: Vec<(u32, u32, u16, f64)> = (0..20).map(|i| (0, 96, 0, f64::from(i) * 0.1)).collect();
        let mut p = project(1, &clips);
        let mut tracks = Vec::new();
        for i in 0..20 {
            let cut = p.clip_unit(i).unwrap();
            p.fx_unit_mut(cut).unwrap().track.slots[0] = Some(plugin("Cut"));
            tracks.push(usize::from(track_of(cut)));
        }
        let inst = Arc::new(Mutex::new(Hosted::default()));
        let out = hosted_render(&p, &inst, |e| {
            for &t in &tracks {
                e.set_effect(t, 0, scale(1.0).1);
            }
        });
        let log = inst.lock().unwrap().log.clone();
        let on = |b: &str| log.iter().filter(|l| l.ends_with(b)).count();
        assert_eq!(log.iter().filter(|l| l.starts_with("start")).count(), 20);
        assert_eq!(on(" on 0"), 5, "{log:?}");
        assert!(at(&out, 48).0 > 0.1);
    }

    // Editing while the song plays: a lane reaches a clip that is sounding; a chain added under it takes over without
    // the clip playing twice; a removed one gives it back.
    #[test]
    fn edits_while_playing_reach_the_sounding_clip_once() {
        let base = base_level();
        let p = project(1, &[(0, 1920, 0, 0.0)]);
        let mut lib = library(&p);
        let mut old = Engine::new(&p, &mut lib, RATE, PlayMode::Song);
        old.set_looping(true);
        let mut buf = vec![0.0; TICK * 2 * 20];
        old.process(&mut buf);
        assert_eq!(old.voice_count(), 1);

        // A lane arrives.
        let mut q = p.clone();
        let cut = q.clip_unit(0).unwrap();
        q.fx_unit_mut(cut).unwrap().volume = vec![point(0.0, 0.4)];
        let mut e = Engine::new(&q, &mut lib, RATE, PlayMode::Song);
        e.continue_from(&mut old);
        e.process(&mut buf);
        assert_eq!(e.voice_count(), 1, "the clip goes on, no second voice");
        assert!(close(buf[buf.len() - 2], base * super::lane_gain(0.4)), "{}", buf[buf.len() - 2]);

        // The lane changes.
        let mut r = q.clone();
        r.fx_unit_mut(cut).unwrap().volume = vec![point(0.0, 0.6)];
        let mut e2 = Engine::new(&r, &mut lib, RATE, PlayMode::Song);
        e2.continue_from(&mut e);
        e2.process(&mut buf);
        assert_eq!(e2.voice_count(), 1);
        assert!(close(buf[buf.len() - 2], base * super::lane_gain(0.6)), "{}", buf[buf.len() - 2]);

        // A plugin arrives: the clip moves to the chain, one voice heard through it.
        let mut s = r.clone();
        s.fx_unit_mut(cut).unwrap().track.slots[0] = Some(plugin("Cut"));
        let mut e3 = Engine::new(&s, &mut lib, RATE, PlayMode::Song);
        e3.set_effect(usize::from(track_of(cut)), 0, scale(0.5).1);
        e3.continue_from(&mut e2);
        e3.process(&mut buf);
        e3.process(&mut buf);
        assert_eq!(e3.voice_count(), 1, "the old voice has faded");
        assert!(close(buf[buf.len() - 2], base * super::lane_gain(0.6) * 0.5), "{}", buf[buf.len() - 2]);

        // And it goes again.
        let mut e4 = Engine::new(&r, &mut lib, RATE, PlayMode::Song);
        e4.continue_from(&mut e3);
        e4.process(&mut buf);
        e4.process(&mut buf);
        assert_eq!(e4.voice_count(), 1);
        assert!(close(buf[buf.len() - 2], base * super::lane_gain(0.6)), "{}", buf[buf.len() - 2]);
    }

    // The song's tail: a chain's tail rings on past the clip and the song.
    #[test]
    fn a_chains_tail_rings_past_the_clip() {
        /// Holds the last sample it saw for 0.25 s, then is quiet: a tail.
        #[derive(Default)]
        struct Tail {
            left: u32,
            last: f32,
        }
        impl HostedEffect for Tail {
            fn process_fx(&mut self, _rate: u32, left: &mut [f32], right: &mut [f32]) {
                for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                    if *l != 0.0 {
                        self.last = *l;
                        self.left = RATE / 4;
                    } else if self.left > 0 {
                        self.left -= 1;
                        *l = self.last;
                        *r = self.last;
                    }
                }
            }
        }
        let mut p = project(1, &[(0, 96, 0, 0.0)]);
        let cut = p.clip_unit(0).unwrap();
        p.fx_unit_mut(cut).unwrap().track.slots[0] = Some(plugin("Tail"));
        let t = usize::from(track_of(cut));
        let fx: SharedEffect = Arc::new(Mutex::new(Tail::default()));
        let out = render(&p, |e| e.set_effect(t, 0, fx));
        let song = 96 * TICK * 2;
        assert!(out.len() > song, "the render runs on past the song's end: {} {song}", out.len());
        assert!(out[song + 1000].abs() > 0.1, "the tail is heard");
    }

    // The multicore mixer renders chains as the serial one does.
    #[test]
    fn chains_render_the_same_on_several_cores() {
        let clips: Vec<(u32, u32, u16, f64)> = (0..8u32).map(|i| (i * 48, 96, (i % 2) as u16, f64::from(i) * 0.1)).collect();
        let mut p = project(2, &clips);
        let mut tracks = Vec::new();
        for i in 0..8 {
            let cut = p.clip_unit(i).unwrap();
            {
                let u = p.fx_unit_mut(cut).unwrap();
                u.track.slots[0] = Some(plugin("Cut"));
                u.volume = vec![point(0.0, 0.2 + f64::from(i as u32) * 0.05)];
            }
            tracks.push(usize::from(track_of(cut)));
        }
        for row in 0..2 {
            let w = p.whole_unit(row).unwrap();
            p.fx_unit_mut(w).unwrap().track.slots[1] = Some(plugin("Whole"));
            tracks.push(usize::from(track_of(w)));
        }
        let go = |pool: Option<Arc<crate::parallel::Pool>>| {
            let mut lib = library(&p);
            let mut e = Engine::new(&p, &mut lib, RATE, PlayMode::Song);
            e.set_threads(pool);
            for (i, &t) in tracks.iter().enumerate() {
                let slot = if i < 8 { 0 } else { 1 };
                e.set_effect(t, slot, scale(0.9).1);
            }
            e.render_all(256)
        };
        let serial = go(None);
        let parallel = go(Some(crate::parallel::Pool::new(3, None)));
        assert_eq!(serial.len(), parallel.len());
        assert_eq!(super::super::multicore::tests::first_difference(&serial, &parallel), None);
    }

    // Split mixer tracks: an export sees the project's tracks only, as many as it asked for.
    #[test]
    fn the_track_capture_holds_only_the_projects_tracks() {
        let mut p = project(1, &[(0, 96, 0, 0.0)]);
        let cut = p.clip_unit(0).unwrap();
        p.fx_unit_mut(cut).unwrap().track.slots[0] = Some(plugin("Cut"));
        let mut lib = library(&p);
        let mut e = Engine::new(&p, &mut lib, RATE, PlayMode::Song);
        e.set_track_capture(true);
        let mut out = Vec::new();
        e.render_ticks(4, &mut out);
        assert_eq!(e.take_track_audio().len(), p.mixer.len());
        let mut peaks = Vec::new();
        e.take_track_peaks(&mut peaks);
        assert_eq!(peaks.len(), p.mixer.len(), "nor do the meters show more tracks");
    }
}
