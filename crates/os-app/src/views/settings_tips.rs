// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the settings window's rows say in their hints: a working check, combo, value box, slider or button by its label
//! (`tip`), and the tabs (`TAB_TIPS`). A setting that is stored and shown but changes nothing yet says so ("No effect
//! yet"; `crate::prefs` notes which ones). The rows of Settings > Project > Data folder and the MIDI page's own rows
//! carry their hints where they are drawn (`super::data_folder`, `super::midi_page`).

use super::{
    ANIMATIONS, ASSOCIATE_CHECK, BEATS_PER_BAR, BUFFER_LENGTH, COMBINE_CHECK, CROSSFADES_CHECK, DEFAULT_TEMPLATE, DENOMINATOR, ERRORS_CHECK, INPUT_DEVICE, MANAGE_SHORTCUTS, MODE,
    NATIVE_FIRST_CHECK, NATIVE_HOST_CHECK, NOTE_COLORS_CHECK, NUMERATOR, OUTPUT_DEVICE, PANNING_LAW, REFRESH_MIDI, RESET_WORK_TIME, SAMPLE_RATE, SAVE_PRESET, SET_AS, SHOW_ASIO_PANEL,
    SHOW_INFO_CHECK, STARTUP_CHECK, STEPS_PER_BEAT, THEME_REDO, THEME_UNDO, TIMEBASE, TRUNCATED_CHECK,
};
use crate::prefs as p;

/// The tabs' hints, in `super::TABS` order.
pub(super) const TAB_TIPS: [&str; 10] = [
    "Show the MIDI input and output devices",
    "Show the audio devices and the mixer's settings",
    "Show the general program settings",
    "Show the backup, plugin and search folder settings",
    "Show the plugin manager",
    "Show the skins, themes and colours",
    "Show this project's settings",
    "Show this project's credits",
    "Show the debugging log",
    "Show the version and the licence",
];

/// A row's hint by its label.
const TIPS: &[(&str, &str)] = &[
    // MIDI.
    (p::SYNC_TYPE.label, "What master sync sends to the outputs: MIDI clock or MIDI time code"),
    (p::CONTROLLER_TYPE.label, "How input devices are treated: every device is a generic controller"),
    (p::OMNI_CHANNEL.label, "MIDI channel whose white keys preview Instrument rack channels (wheel changes it)"),
    (p::MARKER_CHANNEL.label, "MIDI channel whose white keys jump to the song's markers (wheel changes it)"),
    (p::MUTING_CHANNEL.label, "MIDI channel whose white keys mute Instrument rack channels (wheel changes it)"),
    (p::PICKUP.label, "A linked controller does nothing until its value reaches the knob's"),
    (p::AUTO_ACCEPT.label, "The Remote control settings box takes the first controller it detects and closes"),
    (p::FOOT_PEDAL.label, "Controller 64 holds released keys until the pedal comes up (off: the instrument gets it)"),
    (p::TOGGLE_ON_RELEASE.label, "The instrument muting keys mute only while they are held"),
    (REFRESH_MIDI, "List the MIDI devices again"),
    // Audio.
    (OUTPUT_DEVICE, "The audio device or ASIO driver Lemon Studio plays through"),
    (INPUT_DEVICE, "The device that mixer track inputs and Quick record take audio from"),
    (MODE, "Shared lets other programs play too; Exclusive takes the device, for smaller buffers"),
    (SAMPLE_RATE, "The rate audio plays at (Device or Driver: its own rate)"),
    (BUFFER_LENGTH, "Audio buffer size: shorter lowers latency, longer avoids dropouts"),
    (p::AUTO_CLOSE.label, "While another program is in front, close the audio output and pause playback"),
    (SHOW_ASIO_PANEL, "Open the ASIO driver's own settings window"),
    (p::MULTITHREADED_GENERATORS.label, "Render instrument channels and sampler voices on several CPU cores"),
    (p::MULTITHREADED_MIXER.label, "Render the mixer's tracks on several CPU cores"),
    (p::SMART_DISABLE.label, "No effect yet: Lemon Studio has no auto-bypass"),
    (p::ALIGN_TICK_LENGTHS.label, "No effect yet: tick lengths do not change with it"),
    (p::RESAMPLING_QUALITY.label, "No effect yet: the sampler keeps its one resampling method"),
    (p::BROWSER_PREVIEW_TRACK.label, "Mixer track the browser's previews play through, ---: the master (wheel changes it)"),
    (p::METRONOME_TRACK.label, "Mixer track the metronome's clicks play through, ---: the master (wheel changes it)"),
    (p::PLAY_TRUNCATED_ON_TRANSPORT.label, "Play and position jumps start notes already under way (off: those notes are skipped)"),
    (p::RESET_PLUGINS_ON_TRANSPORT.label, "Play and position jumps reset every hosted plugin, so effect tails stop"),
    // General: display.
    (p::MAIN_GUI_SCALING.label, "The size of the whole interface (System: the monitor's scale)"),
    (p::POPUPS_SCALING.label, "The size of popup menus and combo lists (Main: as Main GUI scaling)"),
    (p::TOOLBARS_SCALING.label, "The size of the main window's top band (Main: as Main GUI scaling)"),
    (p::TRANSPARENT_WINDOWS.label, "No effect yet: windows are not drawn transparent"),
    (ANIMATIONS, "Playful moves the mixer's cables like real ones, Off hangs them still"),
    (p::SMOOTHING.label, "How slowly editor scrolls ease to where the wheel sends them (far left: they jump)"),
    (p::ANIMATION_REFRESH_RATE.label, "The most frames a second while something moves: 40, 60 or the display's own rate"),
    (p::FORCE_REFRESHES.label, "No effect yet: Lemon Studio redraws its whole window itself each frame"),
    (p::TRANSPARENT_MENUS.label, "No effect yet: menus are not drawn transparent"),
    (p::THICK_LINES.label, "No effect yet: lines are already drawn at the scale's width"),
    (p::HIGH_VISIBILITY.label, "Draw some controls brighter: the record dot, channel names, track numbers, scroll bars"),
    (p::COLORFUL_WAVEFORMS.label, "Colour sample waveforms by their sound: Off, Color map or Multiband"),
    // General: language.
    (p::NOTE_NAMES.label, "How note names read: English, Germanic or Solfege"),
    (p::BLACK_NOTES.label, "Black keys read as sharps of the key below or flats of the key above"),
    // General: input.
    (p::OPTIMIZE_FOR.label, "No effect yet: Lemon Studio has no pen or touch input"),
    (MANAGE_SHORTCUTS, "Choose the keys for the browser, palettes, MIDI import and clip double-clicks"),
    (p::CLICK_AND_HOLD.label, "A held click does more: a new note cycles plain, slide, porta; notes and clips clone"),
    (p::BOTH_MOUSE_BUTTONS.label, "Left click only sets steps, right click only clears them (off: both toggle)"),
    (p::VERTICAL_SCROLLING_SPEED.label, "How far the wheel scrolls the editors"),
    (p::HORIZONTAL_SCROLLING_SPEED.label, "How far a sideways wheel or swipe scrolls the editors"),
    (p::INVERT_PENCIL_BUTTONS.label, "No effect yet: a pen's tip and button arrive as mouse buttons"),
    // General: undo history.
    (p::MAX_UNDO_LEVELS.label, "The most steps the undo history keeps, then the oldest goes (wheel changes it)"),
    (p::UNDO_KNOB_TWEAKS.label, "Knob turns count as undo steps"),
    (p::ALTERNATE_UNDO.label, "Ctrl+Z steps back further, Ctrl+Alt+Z redoes (off: Ctrl+Z toggles the last move)"),
    (p::UNDONE_RECORDINGS_TO_BIN.label, "No effect yet: undone recordings are not moved to the trash"),
    (p::UNDO_HISTORY_IN_BROWSER.label, "Ctrl+Z also opens the undo history in the browser"),
    (p::RECENT_CHANGES_TOP.label, "The browser's undo history lists the newest step first"),
    // General: hints and warnings, system.
    ("Manage messages", "Choose which messages and questions that can be skipped still show"),
    (p::PROJECT_DATA_SIZE_WARNING.label, "After a save, say so when the project file is over this size"),
    (ASSOCIATE_CHECK, "Open .flp files with this copy of Lemon Studio"),
    // General: miscellaneous.
    (p::AUTO_NAME_CHANNELS.label, "A sample or preset loaded into a channel renames the channel after the file"),
    (p::AUTO_NAME_EFFECT_SLOTS.label, "A preset loaded into an effect renames its mixer slot after the preset"),
    (p::AUTO_ZIP_EMPTY.label, "A pattern switch collapses the channels with nothing in the new pattern"),
    (p::AUTO_SELECT_LINKED.label, "Opening or focusing a plugin window selects its mixer track"),
    (p::AUTO_ZOOM_PIANO.label, "Opening the piano roll zooms on the pattern and the channel's notes"),
    (p::SMALL_SCROLLBARS.label, "Thin editor scroll bars without arrow buttons"),
    (p::DETACH_NEW.label, "Plugins added from now on open their windows detached"),
    (p::DETACHED_WITH_MAIN.label, "A detached window first opens on the main window's monitor (off: the primary one)"),
    (p::TYPING_PRIORITY_OFF.label, "No effect yet: native plugins have no typing keyboard option"),
    (p::ALTERNATE_METER_SCALE.label, "Meters go by a second dB scale with more room near 0 dB"),
    (p::SHOW_SPLASH.label, "Show the logo in a small window until the main window opens"),
    (p::RESTORE_AFTER_SOLO.label, "Unsolo puts back the mute states from before the solo (off: everything turns on)"),
    (p::HIDE_PLUGIN_TOOLBAR.label, "No effect yet: plugin windows have no strip to hide"),
    (p::SILENT_STARTUP.label, "No effect yet: Lemon Studio has no startup sound"),
    (p::MONO_PAINTING.label, "A painted note deletes the notes of other keys under it"),
    (p::AUTOMATION_AT_PLAYBACK_POSITION.label, "With no time selection, new automation clips start at the song position (off: bar 1)"),
    (p::SELECT_FIRST_CHANNEL.label, "Switching pattern selects the first channel with notes in it"),
    (p::LIMIT_EDITORS.label, "At most 25 plugin editors stay open: opening another closes the earliest"),
    (DEFAULT_TEMPLATE, "The template FILE > New starts from"),
    (p::STARTUP_PROJECT.label, "What Lemon Studio opens at start when it is not given a project file"),
    // General: advanced.
    (p::DONT_LIMIT_WINDOWS.label, "A detached plugin window drags anywhere (off: its title bar stays on the screens)"),
    (p::FAST_SAMPLE_PREVIEW.label, "No effect yet: browser previews already start as soon as they can"),
    (p::AUTO_KEEP_ON_DISK.label, "A file of 30 s or more loaded into a sampler channel turns its Keep on disk on"),
    (p::READ_TEMPO.label, "A WAV with a beat count loaded into a sampler channel is fitted to those beats"),
    (p::READ_ROOT_NOTE.label, "A WAV loaded into a sampler channel sets the root note from the file"),
    (p::INIT_CONTROLS.label, "Create automation clip also makes the control's value now its start value"),
    (p::THREADED_SAMPLE_LOADING.label, "Decode an opened project's samples on up to 4 threads (off: one file at a time)"),
    (p::CURSORS_SCALING.label, "No effect yet: the system's cursors follow the system's size"),
    (p::LEGACY_SCALING.label, "No effect yet: plugin windows keep the size their plugin gives them"),
    // File.
    (p::AUTOSAVE.label, "How often the project is backed up"),
    (p::BACKUP_MAXIMUM.label, "The most autosaves kept per project name, the oldest go (wheel changes it)"),
    (p::BACKUP_LOCATION.label, "Where backups go: the global folder, or the project's data folder if it has one"),
    ("Manage plugins", "Go to the Plugins tab"),
    // Plugins.
    ("Add folder...", "Add a folder to search for plugins"),
    (STARTUP_CHECK, "Look for new plugins each time Lemon Studio starts"),
    (ERRORS_CHECK, "A scan also tries the plugins that failed last time again, even if they did not change"),
    (COMBINE_CHECK, "List a plugin found in several formats once, in the preferred format"),
    (NATIVE_HOST_CHECK, "Host installed native plugins (experimental), then scan again so the lists follow"),
    (NATIVE_FIRST_CHECK, "Opened projects play native plugins through the installed ones, not Lemon recreations"),
    ("Scan for new plugins", "Look for plugins that are new or have changed"),
    ("Rescan all", "Scan every plugin again"),
    // Theme.
    (NOTE_COLORS_CHECK, "New audio and automation clips take note colours (off: a muted random hue)"),
    ("Load theme...", "Load a theme from a file"),
    ("Reset", "Drop the theme: back to this skin's own colours"),
    (SAVE_PRESET, "Save the applied theme under a name"),
    (THEME_UNDO, "Undo the last theme change"),
    (THEME_REDO, "Redo the theme change just undone"),
    // Project.
    (TIMEBASE, "Ticks per quarter note: how finely this project places notes"),
    (SET_AS, "Give the bar as a time signature or as a time division"),
    (NUMERATOR, "Beats in a bar"),
    (DENOMINATOR, "The note value of a beat: it sets the steps per beat"),
    (BEATS_PER_BAR, "Beats in a bar (wheel changes it)"),
    (STEPS_PER_BEAT, "Steps in a beat (wheel changes it)"),
    (PANNING_LAW, "Circular keeps constant power; Triangular keeps the louder side at its centre level"),
    (TRUNCATED_CHECK, "Notes begun before a clip's start still play from its start, cut at its end"),
    (CROSSFADES_CHECK, "This project's overlapping audio clips crossfade by themselves"),
    (p::AUTO_CROSSFADES_NEW.label, "New projects start with Automatic crossfades on"),
    // Info.
    (SHOW_INFO_CHECK, "Show the song info box when this project opens"),
    (RESET_WORK_TIME, "Set the time spent on this project back to zero"),
];

/// The hint of the row labelled `label`; a row the table does not know says its label.
pub(super) fn tip(label: &'static str) -> &'static str {
    TIPS.iter().find(|t| t.0 == label).map_or(label, |t| t.1)
}
