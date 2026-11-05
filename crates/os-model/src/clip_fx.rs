// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Effects and automation on a piece of audio (a Lemon Studio feature, no FL counterpart). An audio clip on the playlist
//! is a cut of its channel's sample. Each cut, and the whole sample, can hold a [`FxUnit`]: a chain of plugins like a
//! mixer track's, automation lanes for volume and pan, and lanes for the plugins' parameters.
//!
//! The signal runs: a clip's voice (its fades and gain, then its volume and pan lanes) → the cut's plugins → the whole
//! audio's plugins, one chain every clip of the sample shares → the channel's mixer track.
//!
//! Which lane moves what: a lane's points stand on the sample's own clock, in seconds from its start, so they stay with
//! the audio wherever a clip of it is moved or however it is trimmed. A lane of the whole audio moves every clip of the
//! sample, at the part of the sample each clip plays. A cut that has its own lane for the same thing uses that one
//! instead, for the whole of the cut; every other cut, and every lane the cut does not have, goes on following the whole
//! audio. Without either lane the voice plays as it is.

use crate::{curve, AutoMode, AutoPoint, Channel, Clip, ClipSource, MixerTrack, Project};

/// Track numbers from here up are a unit's chain, not a mixer track: `TRACK_BASE + handle`. They never reach a project
/// file's mixer records.
pub const TRACK_BASE: u16 = 0x4000;
/// The volume lane's value that plays at unity: as a mixer fader, 0.8 (1 is +5.6 dB).
pub const VOLUME_UNITY: f64 = 0.8;
/// The pan lane's value at the centre (0 hard left, 1 hard right).
pub const PAN_CENTRE: f64 = 0.5;

/// What a lane moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lane {
    Volume,
    Pan,
    /// Parameter `param` (its place in the plugin's list) of the plugin in slot `slot`; `whole`: of the whole audio's
    /// chain, else of the cut's own.
    Param { whole: bool, slot: u8, param: u16 },
}

impl Lane {
    /// The value a lane of this kind stands at by default, for drawing a lane without points.
    pub fn neutral(self) -> f64 {
        match self {
            Lane::Volume => VOLUME_UNITY,
            Lane::Pan => PAN_CENTRE,
            Lane::Param { .. } => 0.5,
        }
    }
}

/// A plugin parameter's lane.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamLane {
    /// Of the whole audio's chain, else the cut's own.
    pub whole: bool,
    pub slot: u8,
    pub param: u16,
    pub points: Vec<AutoPoint>,
}

/// The plugins and automation of a cut or of a whole sample.
#[derive(Debug, Clone, PartialEq)]
pub struct FxUnit {
    /// Its stable number: clips and channels point at it, `track.index` follows it.
    pub handle: u32,
    /// Its chain: the slots of a mixer track, no fader or routing of its own.
    pub track: MixerTrack,
    /// Sorted by time. A point's `beat` is seconds from the sample's start; its value 0..1.
    pub volume: Vec<AutoPoint>,
    pub pan: Vec<AutoPoint>,
    pub params: Vec<ParamLane>,
}

impl FxUnit {
    pub fn new(handle: u32) -> FxUnit {
        FxUnit { handle, track: MixerTrack::new(track_of(handle)), volume: Vec::new(), pan: Vec::new(), params: Vec::new() }
    }

    /// Some slot holds a plugin: the audio takes a path through the chain.
    pub fn has_plugins(&self) -> bool {
        self.track.slots.iter().any(Option::is_some)
    }

    /// Holds nothing at all: as if it were not there.
    pub fn is_empty(&self) -> bool {
        !self.has_plugins() && self.volume.is_empty() && self.pan.is_empty() && self.params.iter().all(|l| l.points.is_empty())
    }

    /// The points of `lane`, sorted; empty when it has none.
    pub fn lane(&self, lane: Lane) -> &[AutoPoint] {
        match lane {
            Lane::Volume => &self.volume,
            Lane::Pan => &self.pan,
            Lane::Param { whole, slot, param } => self.params.iter().find(|l| l.whole == whole && l.slot == slot && l.param == param).map_or(&[], |l| &l.points),
        }
    }

    /// The points of `lane`, which is made when missing. Keep them sorted ([`lane_add`], [`lane_move`]).
    pub fn lane_mut(&mut self, lane: Lane) -> &mut Vec<AutoPoint> {
        match lane {
            Lane::Volume => &mut self.volume,
            Lane::Pan => &mut self.pan,
            Lane::Param { whole, slot, param } => {
                let at = match self.params.iter().position(|l| l.whole == whole && l.slot == slot && l.param == param) {
                    Some(i) => i,
                    None => {
                        self.params.push(ParamLane { whole, slot, param, points: Vec::new() });
                        self.params.len() - 1
                    }
                };
                &mut self.params[at].points
            }
        }
    }

    /// Every lane that has points, volume and pan first.
    pub fn lanes(&self) -> Vec<Lane> {
        let mut v = Vec::new();
        if !self.volume.is_empty() {
            v.push(Lane::Volume);
        }
        if !self.pan.is_empty() {
            v.push(Lane::Pan);
        }
        v.extend(self.params.iter().filter(|l| !l.points.is_empty()).map(|l| Lane::Param { whole: l.whole, slot: l.slot, param: l.param }));
        v
    }

    /// The lanes of the plugin in slot `slot` of this unit's own chain go with the plugin.
    pub fn forget_slot(&mut self, whole_chain: bool, slot: u8) {
        self.params.retain(|l| !(l.whole == whole_chain && l.slot == slot));
    }

    /// Two slots of this unit's own chain swapped: their lanes follow.
    pub fn swap_slots(&mut self, whole_chain: bool, a: u8, b: u8) {
        for l in self.params.iter_mut().filter(|l| l.whole == whole_chain) {
            if l.slot == a {
                l.slot = b;
            } else if l.slot == b {
                l.slot = a;
            }
        }
    }
}

/// The chain's track number of unit `handle`.
pub fn track_of(handle: u32) -> u16 {
    TRACK_BASE.saturating_add(u16::try_from(handle).unwrap_or(u16::MAX - TRACK_BASE))
}

/// The handle behind track number `track`, when it is a unit's chain.
pub fn handle_of(track: u16) -> Option<u32> {
    (track >= TRACK_BASE).then(|| u32::from(track - TRACK_BASE))
}

/// A new point on a lane: a plain line to it.
pub fn point(seconds: f64, value: f64) -> AutoPoint {
    AutoPoint { beat: seconds.max(0.0), value: value.clamp(0.0, 1.0), tension: 0.0, mode: AutoMode::Single, flag: 2 }
}

/// Adds a point at `seconds` keeping the lane sorted; a point already at that very time takes the value. Its place.
pub fn lane_add(points: &mut Vec<AutoPoint>, seconds: f64, value: f64) -> usize {
    let p = point(seconds, value);
    let at = points.partition_point(|q| q.beat < p.beat);
    if points.get(at).is_some_and(|q| q.beat == p.beat) {
        points[at].value = p.value;
    } else {
        points.insert(at, p);
    }
    at
}

/// Moves point `i` to (`seconds`, `value`), between its neighbours (a point cannot pass another). Its place stays.
pub fn lane_move(points: &mut [AutoPoint], i: usize, seconds: f64, value: f64) {
    let lo = if i > 0 { points[i - 1].beat } else { 0.0 };
    let hi = points.get(i + 1).map_or(f64::INFINITY, |p| p.beat);
    if let Some(p) = points.get_mut(i) {
        p.beat = seconds.clamp(lo, hi);
        p.value = value.clamp(0.0, 1.0);
    }
}

/// A lane's value at `seconds` of the sample: the curve between its points, the first or last point's value outside
/// them; `None` without points.
pub fn lane_value(points: &[AutoPoint], seconds: f64) -> Option<f64> {
    curve::value_at(points, seconds)
}

/// A lane that applies to a clip: its points, and whether they are the cut's own (else the whole audio's).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved<'a> {
    pub points: &'a [AutoPoint],
    pub own: bool,
}

impl Project {
    pub fn fx_unit(&self, handle: u32) -> Option<&FxUnit> {
        if handle == 0 {
            return None;
        }
        self.fx_units.iter().find(|u| u.handle == handle)
    }

    pub fn fx_unit_mut(&mut self, handle: u32) -> Option<&mut FxUnit> {
        if handle == 0 {
            return None;
        }
        self.fx_units.iter_mut().find(|u| u.handle == handle)
    }

    /// A new unit, empty; its handle.
    pub fn add_fx_unit(&mut self) -> u32 {
        let handle = self.fx_units.iter().map(|u| u.handle).max().unwrap_or(0) + 1;
        self.fx_units.push(FxUnit::new(handle));
        handle
    }

    /// The unit of clip `index` of the current arrangement, made when it has none. `None` for a clip that is not there.
    pub fn clip_unit(&mut self, index: usize) -> Option<u32> {
        let have = self.arrangement()?.clips.get(index)?.fx;
        if self.fx_unit(have).is_some() {
            return Some(have);
        }
        let handle = self.add_fx_unit();
        self.arrangement_mut()?.clips[index].fx = handle;
        Some(handle)
    }

    /// The unit of the whole audio of rack row `row`, made when it has none.
    pub fn whole_unit(&mut self, row: usize) -> Option<u32> {
        let have = self.channels.get(row)?.whole_fx;
        if self.fx_unit(have).is_some() {
            return Some(have);
        }
        let handle = self.add_fx_unit();
        self.channels[row].whole_fx = handle;
        Some(handle)
    }

    /// The channel a clip's source is.
    fn clip_channel(&self, c: &Clip) -> Option<&Channel> {
        match c.source {
            ClipSource::Channel(ch) => self.channels.iter().find(|x| x.index == ch),
            ClipSource::Pattern(_) => None,
        }
    }

    /// The lane that moves `lane` while clip `c` plays: the cut's own when it has points, else the whole audio's. A
    /// parameter of the cut's own chain has no lane of the whole audio to fall back on.
    pub fn clip_lane(&self, c: &Clip, lane: Lane) -> Option<Resolved<'_>> {
        if let Some(points) = self.fx_unit(c.fx).map(|u| u.lane(lane)).filter(|p| !p.is_empty()) {
            return Some(Resolved { points, own: true });
        }
        if matches!(lane, Lane::Param { whole: false, .. }) {
            return None;
        }
        let points = self.fx_unit(self.clip_channel(c)?.whole_fx)?.lane(lane);
        (!points.is_empty()).then_some(Resolved { points, own: false })
    }

    /// The mixer track or unit chain numbered `index`.
    pub fn slot_track(&self, index: u16) -> Option<&MixerTrack> {
        match handle_of(index) {
            Some(h) => self.fx_unit(h).map(|u| &u.track),
            None => self.mixer.iter().find(|m| m.index == index),
        }
    }

    pub fn slot_track_mut(&mut self, index: u16) -> Option<&mut MixerTrack> {
        match handle_of(index) {
            Some(h) => self.fx_unit_mut(h).map(|u| &mut u.track),
            None => self.mixer.iter_mut().find(|m| m.index == index),
        }
    }

    /// Every mixer track, then every unit's chain: all that holds plugins in slots.
    pub fn slot_tracks(&self) -> impl Iterator<Item = &MixerTrack> {
        self.mixer.iter().chain(self.fx_units.iter().map(|u| &u.track))
    }

    pub fn slot_tracks_mut(&mut self) -> impl Iterator<Item = &mut MixerTrack> {
        self.mixer.iter_mut().chain(self.fx_units.iter_mut().map(|u| &mut u.track))
    }

    /// Gives a clip or channel that shares its unit with another (a copied clip, a split clip's second part, a cloned
    /// channel) a copy of its own: what is done to a cut stays on that cut. The first holder of a handle, in project
    /// order, keeps it. Returns (handle copied from, the copy's handle), for hosting the copies' plugins.
    pub fn separate_fx_units(&mut self) -> Vec<(u32, u32)> {
        let mut made = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in 0..self.channels.len() {
            let h = self.channels[row].whole_fx;
            if h != 0 && !seen.insert(h) {
                let copy = self.copy_fx_unit(h);
                self.channels[row].whole_fx = copy;
                made.push((h, copy));
            }
        }
        for a in 0..self.arrangements.len() {
            for i in 0..self.arrangements[a].clips.len() {
                let h = self.arrangements[a].clips[i].fx;
                if h != 0 && !seen.insert(h) {
                    let copy = self.copy_fx_unit(h);
                    self.arrangements[a].clips[i].fx = copy;
                    made.push((h, copy));
                }
            }
        }
        made
    }

    fn copy_fx_unit(&mut self, handle: u32) -> u32 {
        let copy = self.add_fx_unit();
        let Some(mut u) = self.fx_unit(handle).cloned() else { return copy };
        u.handle = copy;
        u.track.index = track_of(copy);
        for s in u.track.slots.iter_mut().flatten() {
            s.id = crate::Id::NONE;
        }
        if let Some(to) = self.fx_unit_mut(copy) {
            *to = u;
        }
        copy
    }

    /// The units a clip or channel points at now, each once.
    pub fn used_fx_units(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.channels.iter().map(|c| c.whole_fx).chain(self.arrangements.iter().flat_map(|a| a.clips.iter().map(|c| c.fx))).filter(|&h| h != 0).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Arrangement, ClipSource, SlotPlugin};

    fn lane(points: &[(f64, f64)]) -> Vec<AutoPoint> {
        points.iter().map(|&(s, v)| point(s, v)).collect()
    }

    fn channel(index: u16) -> Channel {
        Channel {
            id: crate::Id::NONE,
            index,
            kind: crate::ChannelKind::AudioClip,
            name: None,
            plugin: None,
            plugin_state: None,
            color: None,
            enabled: true,
            volume: 0.78125,
            pan: 0.0,
            pitch_cents: 0,
            mixer_track: 0,
            sample_path: None,
            layer_children: Vec::new(),
            whole_fx: 0,
            envelopes: Default::default(),
            rack: Default::default(),
            misc: Default::default(),
            sound: Default::default(),
        }
    }

    /// A project with two clips of channel 0 and one of channel 1.
    fn project() -> Project {
        let mut p = Project::default();
        p.channels = vec![channel(0), channel(1)];
        let clip = |pos: u32, c: u16| Clip::new(pos, 96, 0, ClipSource::Channel(c));
        p.arrangements = vec![Arrangement { number: 1, clips: vec![clip(0, 0), clip(96, 0), clip(192, 1)], ..Default::default() }];
        p.current_arrangement = Some(1);
        p
    }

    #[test]
    fn a_clip_without_any_lane_has_nothing_to_follow() {
        let p = project();
        let c = p.arrangement().unwrap().clips[0];
        assert_eq!(p.clip_lane(&c, Lane::Volume), None);
        assert_eq!(p.clip_lane(&c, Lane::Pan), None);
    }

    #[test]
    fn the_whole_audio_lane_moves_every_clip_of_its_sample_and_only_those() {
        let mut p = project();
        let h = p.whole_unit(0).unwrap();
        p.fx_unit_mut(h).unwrap().volume = lane(&[(0.0, 0.2), (10.0, 0.6)]);
        let clips = p.arrangement().unwrap().clips.clone();
        for c in &clips[..2] {
            let r = p.clip_lane(c, Lane::Volume).expect("a clip of the sample follows the whole audio");
            assert!(!r.own);
            assert_eq!(r.points.len(), 2);
        }
        assert_eq!(p.clip_lane(&clips[2], Lane::Volume), None, "another sample's clip does not");
        assert_eq!(p.clip_lane(&clips[0], Lane::Pan), None, "nor does another parameter have a lane");
    }

    #[test]
    fn a_cut_with_its_own_lane_wins_only_for_that_cut_and_that_parameter() {
        let mut p = project();
        let whole = p.whole_unit(0).unwrap();
        {
            let u = p.fx_unit_mut(whole).unwrap();
            u.volume = lane(&[(0.0, 0.2)]);
            u.pan = lane(&[(0.0, 0.1)]);
        }
        let cut = p.clip_unit(1).unwrap();
        p.fx_unit_mut(cut).unwrap().volume = lane(&[(0.0, 0.9)]);
        let clips = p.arrangement().unwrap().clips.clone();

        let own = p.clip_lane(&clips[1], Lane::Volume).unwrap();
        assert!(own.own);
        assert_eq!(own.points[0].value, 0.9);
        // The cut's other parameter still follows the whole audio.
        let pan = p.clip_lane(&clips[1], Lane::Pan).unwrap();
        assert!(!pan.own);
        assert_eq!(pan.points[0].value, 0.1);
        // The other cut still follows the whole audio.
        let other = p.clip_lane(&clips[0], Lane::Volume).unwrap();
        assert!(!other.own);
        assert_eq!(other.points[0].value, 0.2);

        // Clearing the cut's lane gives it back to the whole audio.
        p.fx_unit_mut(cut).unwrap().volume.clear();
        let back = p.clip_lane(&clips[1], Lane::Volume).unwrap();
        assert!(!back.own);
        assert_eq!(back.points[0].value, 0.2);
    }

    #[test]
    fn a_whole_plugin_parameter_can_be_overridden_by_a_cut_but_its_own_chain_has_no_fallback() {
        let mut p = project();
        let whole = p.whole_unit(0).unwrap();
        let shared = Lane::Param { whole: true, slot: 2, param: 7 };
        *p.fx_unit_mut(whole).unwrap().lane_mut(shared) = lane(&[(0.0, 0.3)]);
        let cut = p.clip_unit(0).unwrap();
        let clips = p.arrangement().unwrap().clips.clone();
        assert!(!p.clip_lane(&clips[0], shared).unwrap().own);
        *p.fx_unit_mut(cut).unwrap().lane_mut(shared) = lane(&[(0.0, 0.8)]);
        let r = p.clip_lane(&clips[0], shared).unwrap();
        assert!(r.own && r.points[0].value == 0.8);
        assert!(!p.clip_lane(&clips[1], shared).unwrap().own);
        // A lane of the cut's own chain is only ever the cut's.
        let own = Lane::Param { whole: false, slot: 2, param: 7 };
        *p.fx_unit_mut(whole).unwrap().lane_mut(own) = lane(&[(0.0, 0.5)]);
        assert_eq!(p.clip_lane(&clips[1], own), None);
    }

    #[test]
    fn lane_editing_keeps_points_sorted_and_in_range() {
        let mut l = Vec::new();
        assert_eq!(lane_add(&mut l, 2.0, 0.5), 0);
        assert_eq!(lane_add(&mut l, 1.0, 0.25), 0);
        assert_eq!(lane_add(&mut l, 3.0, 2.0), 2);
        assert_eq!(l[2].value, 1.0, "values are kept in range");
        assert_eq!(lane_add(&mut l, 2.0, 0.75), 1, "a point at that very time takes the value");
        assert_eq!(l.len(), 3);
        assert_eq!(l[1].value, 0.75);
        // A point stays between its neighbours.
        lane_move(&mut l, 1, 99.0, -1.0);
        assert_eq!((l[1].beat, l[1].value), (3.0, 0.0));
        lane_move(&mut l, 1, -5.0, 0.5);
        assert_eq!(l[1].beat, 1.0);
        lane_move(&mut l, 0, -5.0, 0.5);
        assert_eq!(l[0].beat, 0.0, "not before the sample's start");
        assert!(l.windows(2).all(|w| w[0].beat <= w[1].beat));
    }

    #[test]
    fn a_lane_holds_its_ends_and_interpolates_between_points() {
        let l = lane(&[(1.0, 0.25), (3.0, 0.75)]);
        assert_eq!(lane_value(&l, 0.0), Some(0.25));
        assert_eq!(lane_value(&l, 1.0), Some(0.25));
        assert!((lane_value(&l, 2.0).unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(lane_value(&l, 3.0), Some(0.75));
        assert_eq!(lane_value(&l, 9.0), Some(0.75));
        assert_eq!(lane_value(&[], 1.0), None);
    }

    #[test]
    fn copies_of_a_clip_or_channel_get_units_of_their_own() {
        let mut p = project();
        let cut = p.clip_unit(0).unwrap();
        p.fx_unit_mut(cut).unwrap().volume = lane(&[(0.0, 0.5)]);
        p.fx_unit_mut(cut).unwrap().track.slots[3] = Some(SlotPlugin { id: crate::Id(9), name: "Fx".into(), state: vec![1, 2, 3] });
        // A clip copied by value carries the handle.
        let copy = p.arrangement().unwrap().clips[0];
        p.arrangement_mut().unwrap().clips.push(copy);
        let whole = p.whole_unit(0).unwrap();
        let chan = p.channels[0].clone();
        p.channels.push(Channel { index: 2, ..chan });

        let made = p.separate_fx_units();
        assert_eq!(made.len(), 2, "{made:?}");
        let clips = p.arrangement().unwrap().clips.clone();
        assert_eq!(clips[0].fx, cut, "the first holder keeps it");
        let second = clips[3].fx;
        assert_ne!(second, cut);
        let u = p.fx_unit(second).unwrap();
        assert_eq!(u.volume, lane(&[(0.0, 0.5)]));
        assert_eq!(u.track.index, track_of(second));
        let slot = u.track.slots[3].as_ref().unwrap();
        assert_eq!((slot.name.as_str(), slot.state.as_slice(), slot.id), ("Fx", &[1u8, 2, 3][..], crate::Id::NONE));
        assert_eq!(p.channels[0].whole_fx, whole);
        assert_ne!(p.channels[2].whole_fx, whole);

        // Editing one copy leaves the other alone, and nothing is shared any more.
        p.fx_unit_mut(second).unwrap().volume.clear();
        assert_eq!(p.fx_unit(cut).unwrap().volume.len(), 1);
        assert!(p.separate_fx_units().is_empty());
    }

    #[test]
    fn unit_tracks_are_found_by_number_beside_the_mixers() {
        let mut p = project();
        p.mixer = vec![MixerTrack::new(0), MixerTrack::new(1)];
        let h = p.whole_unit(0).unwrap();
        assert_eq!(p.slot_track(1).map(|m| m.index), Some(1));
        assert_eq!(p.slot_track(track_of(h)).map(|m| m.index), Some(track_of(h)));
        assert_eq!(p.slot_track(track_of(h + 7)).map(|m| m.index), None);
        assert_eq!(handle_of(track_of(h)), Some(h));
        assert_eq!(handle_of(5), None);
        assert_eq!(p.slot_tracks().count(), 3);
    }

    #[test]
    fn slot_lanes_follow_their_plugin() {
        let mut u = FxUnit::new(1);
        *u.lane_mut(Lane::Param { whole: false, slot: 1, param: 0 }) = lane(&[(0.0, 0.1)]);
        *u.lane_mut(Lane::Param { whole: false, slot: 2, param: 0 }) = lane(&[(0.0, 0.2)]);
        *u.lane_mut(Lane::Param { whole: true, slot: 1, param: 0 }) = lane(&[(0.0, 0.3)]);
        u.swap_slots(false, 1, 2);
        assert_eq!(u.lane(Lane::Param { whole: false, slot: 2, param: 0 })[0].value, 0.1);
        assert_eq!(u.lane(Lane::Param { whole: false, slot: 1, param: 0 })[0].value, 0.2);
        assert_eq!(u.lane(Lane::Param { whole: true, slot: 1, param: 0 })[0].value, 0.3, "the whole audio's chain is another chain");
        u.forget_slot(false, 2);
        assert!(u.lane(Lane::Param { whole: false, slot: 2, param: 0 }).is_empty());
        assert!(!u.lane(Lane::Param { whole: true, slot: 1, param: 0 }).is_empty());
    }

    #[test]
    fn a_unit_counts_as_empty_until_it_holds_something() {
        let mut u = FxUnit::new(1);
        assert!(u.is_empty() && !u.has_plugins());
        u.lane_mut(Lane::Param { whole: false, slot: 0, param: 0 });
        assert!(u.is_empty(), "a lane without points is nothing");
        u.volume = lane(&[(0.0, 0.5)]);
        assert!(!u.is_empty() && !u.has_plugins());
        u.volume.clear();
        u.track.slots[0] = Some(SlotPlugin { id: crate::Id::NONE, name: "Fx".into(), state: vec![] });
        assert!(!u.is_empty() && u.has_plugins());
    }
}
