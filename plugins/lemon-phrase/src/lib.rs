// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Phrase: Lemon Studio's note-by-note pitch correction: each note moved onto the scale as a whole, its vibrato and expression kept as a VST3 / CLAP plugin module. The plugin, its parameters and its editor
//! are `lemon_small_fx::phrase` (shared with the other small effects); this crate gives it its own module file.
#![allow(semicolon_in_expressions_from_non_local_macros)]

#[cfg(not(feature = "linked"))]
use nih_plug::prelude::*;

pub use lemon_small_fx::phrase::LemonPhrase;

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonPhrase);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonPhrase);
os_plugkit::linked_clap_entry!(LemonPhrase);
