// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's DSP building blocks, shared by the engine and Lemon Studio's own plugins: the arithmetic the engine
//! pages specify, at the precision they give, so that a project plays the way it was made.
//!
//! - [`level`]: the level curve, knob positions as products with a reciprocal, the constant-power pan split.
//! - [`glide`]: the per-block gain glide.
//! - [`mixer`]: a mixer track's fader matrix and its three-band equaliser.
//! - [`resample`]: the sampler's windowed-sinc resampler (table or exact weights).
//! - [`stretch`]: time stretching and pitch shifting (Signalsmith Stretch).
//! - [`articulator`]: a channel's envelopes and LFOs, stepped once per tick for every voice.
//! - [`channel_filter`]: the channel filter a voice runs through.
//! - [`tone`]: the spectral make-up of a sound's stretches, for colourful waveforms.
//! - [`curve`]: Image-Line's curves (envelopes, LFOs, maps) as Citrus and Pulp Filter walk them.
//! - [`align`]: one recording lined up with another by how they sound (dynamic time warping).
//! - [`comp`]: comping takes, phrase by phrase the best of several takes of one part, in time with each other.
//! - [`flow`]: Fit to tempo, a take's syllables moved onto the beat grid as its flow reads.
//! - [`syllables`]: where a voice's syllables, attacks, silences and voiced stretches are, from the sound alone.
//! - [`retime`]: a voice's time map from anchors, the time taken where it is least heard.
//! - [`warp`]: audio played through a time map (WSOLA), for the retiming tools.
//! - [`num`]: the plugin pages' arithmetic (`single( )`, Round, flushes) and a little-endian state reader.
//!
// spec: audio/conventions.md, audio/mixer.md, audio/sampler.md, audio/time-stretch.md, audio/envelopes-and-lfos.md,
// audio/channel-filter.md, plugins/image-line-curves.md

pub mod align;
pub mod articulator;
pub mod channel_filter;
pub mod comp;
pub mod curve;
pub mod flow;
pub mod glide;
pub mod level;
pub mod mixer;
pub mod num;
pub mod resample;
pub mod retime;
pub mod stretch;
pub mod syllables;
pub mod tone;
pub mod warp;
