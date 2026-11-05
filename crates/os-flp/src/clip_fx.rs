// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The project's clip and whole-audio units (`os_model::clip_fx`) in a file. FL's event stream has no place for them, so
//! they go in a chunk of their own after `FLdt`, `LSfx`; the events stay as they are, and a project that uses none of
//! this has no such chunk. A clip is named by its arrangement, track, place, length and channel, a whole audio by its
//! rack place: a file stores no id that lasts.
//!
//! The chunk, little-endian: a version (u16); the units; the whole audios (rack place u16, handle u32); the cuts
//! (arrangement u16, track u16, position u32, length u32, channel u16, handle u32), each list after its count (u32). A
//! unit: its handle (u32), flags (u8, bit 0: the chain's slots are enabled), ten slots (on u8, mix f32, present u8 and,
//! when present, its name and state as length-prefixed bytes), the volume and pan lanes, then its parameter lanes (count
//! u16; whole u8, slot u8, parameter u16, points). A lane: its count (u32), then per point its time and value (f64),
//! tension (f32), mode and flag (u8).

use os_model::clip_fx::{track_of, FxUnit, ParamLane};
use os_model::{AutoMode, AutoPoint, ClipSource, Id, Project, SlotPlugin};

use crate::container::{Chunk, ChunkBody, FlpFile, Tag};

/// The chunk's tag.
pub const TAG: Tag = Tag(*b"LSfx");
const VERSION: u16 = 1;

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn lane(&mut self, points: &[AutoPoint]) {
        self.u32(points.len() as u32);
        for p in points {
            self.f64(p.beat);
            self.f64(p.value);
            self.f32(p.tension);
            self.u8(p.mode.byte());
            self.u8(p.flag);
        }
    }
    fn unit(&mut self, u: &FxUnit) {
        self.u32(u.handle);
        self.u8(u8::from(u.track.slots_enabled));
        for k in 0..10 {
            self.u8(u8::from(u.track.slot_on[k]));
            self.f32(u.track.slot_mix[k] as f32);
            match &u.track.slots[k] {
                Some(s) => {
                    self.u8(1);
                    self.bytes(s.name.as_bytes());
                    self.bytes(&s.state);
                }
                None => self.u8(0),
            }
        }
        self.lane(&u.volume);
        self.lane(&u.pan);
        let lanes: Vec<&ParamLane> = u.params.iter().filter(|l| !l.points.is_empty()).collect();
        self.u16(lanes.len() as u16);
        for l in lanes {
            self.u8(u8::from(l.whole));
            self.u8(l.slot);
            self.u16(l.param);
            self.lane(&l.points);
        }
    }
}

/// A reader that stops (`None`) at the end of the bytes: a damaged chunk is left out, never a crash.
struct R<'a>(&'a [u8]);

impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let (a, b) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
    fn f64(&mut self) -> Option<f64> {
        let b: [u8; 8] = self.take(8)?.try_into().ok()?;
        Some(f64::from_le_bytes(b))
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.u32()? as usize;
        self.take(n).map(<[u8]>::to_vec)
    }
    fn lane(&mut self) -> Option<Vec<AutoPoint>> {
        let n = self.u32()? as usize;
        // A point is at least 22 bytes: a count the chunk cannot hold is damage.
        if n > self.0.len() / 22 {
            return None;
        }
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let (beat, value, tension, mode, flag) = (self.f64()?, self.f64()?, self.f32()?, self.u8()?, self.u8()?);
            if !(beat.is_finite() && value.is_finite() && tension.is_finite()) {
                return None;
            }
            v.push(AutoPoint { beat: beat.max(0.0), value: value.clamp(0.0, 1.0), tension, mode: AutoMode::from_byte(mode), flag });
        }
        v.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        Some(v)
    }
    fn unit(&mut self) -> Option<FxUnit> {
        let handle = self.u32()?;
        let mut u = FxUnit::new(handle);
        u.track.index = track_of(handle);
        u.track.slots_enabled = self.u8()? & 1 != 0;
        for k in 0..10 {
            u.track.slot_on[k] = self.u8()? != 0;
            u.track.slot_mix[k] = f64::from(self.f32()?).clamp(0.0, 1.0);
            if self.u8()? != 0 {
                let name = String::from_utf8(self.bytes()?).ok()?;
                let state = self.bytes()?;
                u.track.slots[k] = Some(SlotPlugin { id: Id::NONE, name, state });
            }
        }
        u.volume = self.lane()?;
        u.pan = self.lane()?;
        for _ in 0..self.u16()? {
            let (whole, slot, param) = (self.u8()? != 0, self.u8()?, self.u16()?);
            let points = self.lane()?;
            if slot < 10 {
                u.params.push(ParamLane { whole, slot, param, points });
            }
        }
        Some(u)
    }
}

/// The chunk's bytes for `p`, `None` when no clip or channel holds anything: units nothing points at are left out.
pub fn encode(p: &Project) -> Option<Vec<u8>> {
    let live = |h: u32| p.fx_unit(h).filter(|u| !u.is_empty());
    let wholes: Vec<(u16, u32)> = p.channels.iter().enumerate().filter(|(_, c)| live(c.whole_fx).is_some()).map(|(row, c)| (row as u16, c.whole_fx)).collect();
    let mut cuts = Vec::new();
    for a in &p.arrangements {
        for c in &a.clips {
            let ClipSource::Channel(ch) = c.source else { continue };
            if live(c.fx).is_some() {
                cuts.push((a.number, c.track, c.position, c.length, ch, c.fx));
            }
        }
    }
    if wholes.is_empty() && cuts.is_empty() {
        return None;
    }
    let mut handles: Vec<u32> = wholes.iter().map(|w| w.1).chain(cuts.iter().map(|c| c.5)).collect();
    handles.sort_unstable();
    handles.dedup();
    let mut w = W(Vec::new());
    w.u16(VERSION);
    w.u32(handles.len() as u32);
    for &h in &handles {
        if let Some(u) = p.fx_unit(h) {
            w.unit(u);
        }
    }
    w.u32(wholes.len() as u32);
    for (row, h) in wholes {
        w.u16(row);
        w.u32(h);
    }
    w.u32(cuts.len() as u32);
    for (arr, track, position, length, ch, h) in cuts {
        w.u16(arr);
        w.u16(track);
        w.u32(position);
        w.u32(length);
        w.u16(ch);
        w.u32(h);
    }
    Some(w.0)
}

/// Reads the chunk's bytes into `p`; `None` (and nothing changed) when they are damaged or of a later version.
pub fn decode(bytes: &[u8], p: &mut Project) -> Option<()> {
    let mut r = R(bytes);
    if r.u16()? != VERSION {
        return None;
    }
    let mut units = Vec::new();
    for _ in 0..r.u32()? {
        units.push(r.unit()?);
    }
    let mut wholes = Vec::new();
    for _ in 0..r.u32()? {
        wholes.push((r.u16()?, r.u32()?));
    }
    let mut cuts = Vec::new();
    for _ in 0..r.u32()? {
        cuts.push((r.u16()?, r.u16()?, r.u32()?, r.u32()?, r.u16()?, r.u32()?));
    }
    p.fx_units = units;
    for (row, h) in wholes {
        if let Some(c) = p.channels.get_mut(usize::from(row)) {
            c.whole_fx = h;
        }
    }
    for (arr, track, position, length, ch, h) in cuts {
        let clips = p.arrangements.iter_mut().filter(|a| a.number == arr).flat_map(|a| a.clips.iter_mut());
        // Two clips alike, one on the other: each takes the next unit.
        let mut clips = clips.filter(|c| c.fx == 0 && c.track == track && c.position == position && c.length == length && c.source == ClipSource::Channel(ch));
        if let Some(c) = clips.next() {
            c.fx = h;
        }
    }
    Some(())
}

/// Reads `file`'s chunk into `p`, if it has one.
pub fn read(file: &FlpFile, p: &mut Project) {
    let body = file.chunks.iter().find(|c| c.tag == TAG).and_then(|c| match &c.body {
        ChunkBody::Raw(b) => Some(b.as_slice()),
        _ => None,
    });
    if let Some(b) = body {
        let _ = decode(b, p);
    }
}

/// Puts `p`'s units in `file`: its chunk is replaced, added after the others, or dropped when there is nothing to store.
pub fn write(p: &Project, file: &mut FlpFile) {
    file.chunks.retain(|c| c.tag != TAG);
    if let Some(body) = encode(p) {
        file.chunks.push(Chunk { tag: TAG, body: ChunkBody::Raw(body) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{apply, create, load};
    use os_model::clip_fx::{point, Lane};
    use os_model::{Arrangement, Channel, ChannelKind, Clip, MixerTrack};

    fn channel(index: u16, path: &str) -> Channel {
        Channel {
            id: Id::NONE,
            index,
            kind: ChannelKind::AudioClip,
            name: Some(format!("Audio {index}")),
            plugin: None,
            plugin_state: None,
            color: None,
            enabled: true,
            volume: 0.78125,
            pan: 0.0,
            pitch_cents: 0,
            mixer_track: 0,
            sample_path: Some(path.into()),
            layer_children: Vec::new(),
            whole_fx: 0,
            envelopes: Default::default(),
            rack: Default::default(),
            misc: Default::default(),
            sound: Default::default(),
        }
    }

    /// A project using all of it: a cut with a plugin and lanes, a whole audio with lanes, an overriding parameter lane.
    fn project() -> Project {
        let mut p = Project { ppq: 96, tempo: Some(120.0), ..Project::default() };
        p.channels = vec![channel(0, "a.wav"), channel(1, "b.wav")];
        p.mixer = vec![MixerTrack::new(0), MixerTrack::new(1)];
        let clip = |pos: u32, c: u16| Clip { window: Some((f64::from(pos), f64::from(pos) + 500.0)), ..Clip::new(pos, 96, 0, ClipSource::Channel(c)) };
        p.arrangements = vec![Arrangement { number: 1, clips: vec![clip(0, 0), clip(96, 0), clip(192, 1)], ..Default::default() }];
        p.current_arrangement = Some(1);
        let whole = p.whole_unit(0).unwrap();
        {
            let u = p.fx_unit_mut(whole).unwrap();
            u.volume = vec![point(0.0, 0.25), point(3.5, 0.75)];
            u.track.slots[9] = Some(SlotPlugin { id: Id::NONE, name: "Whole fx".into(), state: vec![9, 8, 7] });
            u.track.slot_mix[9] = 0.5;
        }
        let cut = p.clip_unit(1).unwrap();
        {
            let u = p.fx_unit_mut(cut).unwrap();
            u.pan = vec![point(1.0, 0.1)];
            u.track.slots[0] = Some(SlotPlugin { id: Id::NONE, name: "Cut fx".into(), state: (0..=255).collect() });
            u.track.slot_on[0] = false;
            u.track.slots_enabled = false;
            *u.lane_mut(Lane::Param { whole: false, slot: 0, param: 3 }) = vec![point(0.0, 1.0)];
            *u.lane_mut(Lane::Param { whole: true, slot: 9, param: 300 }) = vec![point(2.0, 0.0), point(4.0, 1.0)];
        }
        p
    }

    /// The units as a test compares them: ids are the session's.
    fn clean(mut u: FxUnit) -> FxUnit {
        for s in u.track.slots.iter_mut().flatten() {
            s.id = Id::NONE;
        }
        u
    }

    #[test]
    fn units_survive_their_bytes() {
        let p = project();
        let bytes = encode(&p).expect("something to store");
        let mut q = project();
        q.fx_units.clear();
        for c in &mut q.channels {
            c.whole_fx = 0;
        }
        for c in &mut q.arrangements[0].clips {
            c.fx = 0;
        }
        decode(&bytes, &mut q).expect("reads back");
        assert_eq!(q.channels[0].whole_fx, p.channels[0].whole_fx);
        assert_eq!(q.channels[1].whole_fx, 0);
        assert_eq!(q.arrangements[0].clips.iter().map(|c| c.fx).collect::<Vec<_>>(), p.arrangements[0].clips.iter().map(|c| c.fx).collect::<Vec<_>>());
        assert_eq!(q.fx_units.len(), 2);
        for u in &q.fx_units {
            assert_eq!(clean(u.clone()), clean(p.fx_unit(u.handle).unwrap().clone()));
        }
    }

    #[test]
    fn a_project_that_uses_none_of_it_stores_nothing() {
        let mut p = project();
        p.fx_units.clear();
        assert_eq!(encode(&p), None, "handles that lead nowhere");
        for c in &mut p.channels {
            c.whole_fx = 0;
        }
        for c in &mut p.arrangements[0].clips {
            c.fx = 0;
        }
        assert_eq!(encode(&p), None);
        // Units nothing points at, or that hold nothing, are not stored either.
        p.add_fx_unit();
        assert_eq!(encode(&p), None);
        let h = p.clip_unit(0).unwrap();
        assert_eq!(p.fx_unit(h).map(FxUnit::is_empty), Some(true));
        assert_eq!(encode(&p), None);
    }

    #[test]
    fn units_nothing_points_at_are_left_out() {
        let mut p = project();
        let orphan = p.add_fx_unit();
        p.fx_unit_mut(orphan).unwrap().volume = vec![point(0.0, 0.5)];
        let mut q = project();
        decode(&encode(&p).unwrap(), &mut q).unwrap();
        assert!(q.fx_unit(orphan).is_none());
        assert_eq!(q.fx_units.len(), 2);
    }

    #[test]
    fn damaged_bytes_change_nothing_and_never_panic() {
        let bytes = encode(&project()).unwrap();
        for n in 0..bytes.len() {
            let mut q = project();
            let before = q.clone();
            assert!(decode(&bytes[..n], &mut q).is_none(), "cut at {n}");
            assert_eq!(q, before, "cut at {n}");
        }
        // A later version is left alone.
        let mut later = bytes.clone();
        later[0] = 99;
        let mut q = project();
        let before = q.clone();
        assert!(decode(&later, &mut q).is_none());
        assert_eq!(q, before);
        // Noise of any kind reads as damage or as something harmless.
        let mut x = 0x2545_f491u32;
        for _ in 0..2000 {
            let mut noisy = bytes.clone();
            for _ in 0..4 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let at = x as usize % noisy.len();
                noisy[at] = (x >> 8) as u8;
            }
            let mut q = project();
            let _ = decode(&noisy, &mut q);
        }
    }

    #[test]
    fn two_clips_alike_keep_a_unit_each() {
        let mut p = project();
        let same = p.arrangements[0].clips[0];
        p.arrangements[0].clips.push(same);
        let (a, b) = (p.clip_unit(0).unwrap(), p.clip_unit(3).unwrap());
        p.fx_unit_mut(a).unwrap().volume = vec![point(0.0, 0.1)];
        p.fx_unit_mut(b).unwrap().volume = vec![point(0.0, 0.2)];
        let bytes = encode(&p).unwrap();
        let mut q = p.clone();
        for c in &mut q.arrangements[0].clips {
            c.fx = 0;
        }
        decode(&bytes, &mut q).unwrap();
        let v = |q: &Project, i: usize| q.fx_unit(q.arrangements[0].clips[i].fx).map(|u| u.volume[0].value);
        assert_eq!((v(&q, 0), v(&q, 3)), (Some(0.1), Some(0.2)));
    }

    /// Through a real file: `create`, `apply`, the bytes, `parse`, `load`; the events are the same with and without the
    /// chunk, and taking the units away takes the chunk away.
    #[test]
    fn a_file_keeps_them_and_stays_the_same_without_them() {
        let p = project();
        let mut plain = p.clone();
        plain.fx_units.clear();
        for c in &mut plain.channels {
            c.whole_fx = 0;
        }
        for c in &mut plain.arrangements[0].clips {
            c.fx = 0;
        }
        let mut with = create(&plain).unwrap();
        let mut without = with.clone();
        apply(&p, &mut with).unwrap();
        apply(&plain, &mut without).unwrap();
        assert!(without.chunks.iter().all(|c| c.tag != TAG), "no chunk without units");
        assert_eq!(with.chunks.iter().filter(|c| c.tag == TAG).count(), 1);
        // Everything but the chunk is what it would have been.
        let events = |f: &FlpFile| f.chunks.iter().filter(|c| c.tag != TAG).cloned().collect::<Vec<_>>();
        assert_eq!(events(&with), events(&without));

        let parsed = FlpFile::parse(&with.to_bytes().unwrap()).unwrap();
        let back = load(&parsed).unwrap();
        assert_eq!(back.fx_units.len(), 2);
        for u in &back.fx_units {
            assert_eq!(clean(u.clone()), clean(p.fx_unit(u.handle).unwrap().clone()));
        }
        assert_eq!(back.channels[0].whole_fx, p.channels[0].whole_fx);

        // Saving again changes nothing; saving without them removes the chunk.
        let mut again = parsed.clone();
        apply(&p, &mut again).unwrap();
        assert_eq!(again.chunks.iter().filter(|c| c.tag == TAG).count(), 1);
        apply(&plain, &mut again).unwrap();
        assert!(again.chunks.iter().all(|c| c.tag != TAG));
    }
}
