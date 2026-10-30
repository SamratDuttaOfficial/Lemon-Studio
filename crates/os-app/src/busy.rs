// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Slow edits (the vocal timing tools, consolidating, exporting tracks) worked out on a thread of their own behind a
//! progress panel, so the window keeps painting and playback keeps playing. Like a project loading, the window takes
//! no input meanwhile (`App::loading`): the project the work started from is the one its result is put into, as one
//! undo step, on the UI thread.

use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use crate::app::App;

/// What a job hands back: run on the UI thread when it is done, it puts the result into the project and returns the
/// status line.
pub type Finish = Box<dyn FnOnce(&mut App) -> String + Send>;

/// A job's progress, which it sets as it goes and the panel shows: how far (0..=1) and what it is doing.
#[derive(Clone, Default)]
pub struct Progress(Arc<Mutex<(f32, String)>>);

impl Progress {
    pub fn set(&self, done: f32, what: impl Into<String>) {
        if let Ok(mut p) = self.0.lock() {
            *p = (done.clamp(0.0, 1.0), what.into());
        }
    }

    /// Step `i` of `n` begins, doing `what`.
    pub fn step(&self, i: usize, n: usize, what: impl Into<String>) {
        self.set(i as f32 / n.max(1) as f32, what);
    }

    fn get(&self) -> (f32, String) {
        self.0.lock().map(|p| p.clone()).unwrap_or_default()
    }
}

/// A job under way.
pub struct Busy {
    title: &'static str,
    progress: Progress,
    rx: Receiver<Finish>,
}

/// Starts `job` on a thread of its own behind the panel titled `title`; its `Finish` runs when it is done. One job at
/// a time: while one runs, another is refused (returns false).
pub fn run(app: &mut App, title: &'static str, job: impl FnOnce(&Progress) -> Finish + Send + 'static) -> bool {
    if app.busy.is_some() {
        app.state.message = Some(format!("{title}: wait for the work under way to finish"));
        return false;
    }
    let (tx, rx) = channel();
    let progress = Progress::default();
    progress.set(0.0, "Starting");
    let p = progress.clone();
    let spawned = std::thread::Builder::new().name("lemon-busy".into()).spawn(move || {
        let finish = job(&p);
        let _ = tx.send(finish);
        os_clap::wake();
    });
    if let Err(e) = spawned {
        app.state.message = Some(format!("{title} failed: {e}"));
        return false;
    }
    app.busy = Some(Busy { title, progress, rx });
    app.invalidate();
    true
}

/// Each UI tick: a finished job's result goes into the project; one still working repaints its panel.
pub fn tick(app: &mut App) {
    let Some(b) = &app.busy else { return };
    let finish = match b.rx.try_recv() {
        Ok(f) => f,
        Err(TryRecvError::Empty) => {
            app.damage_rect(crate::loading::panel_rect(app.state.screen).outset(4.0));
            return;
        }
        Err(TryRecvError::Disconnected) => {
            let title = b.title;
            app.busy = None;
            app.state.message = Some(format!("{title} stopped with an error"));
            app.invalidate();
            return;
        }
    };
    app.busy = None;
    let msg = finish(app);
    app.state.message = Some(msg);
    app.invalidate();
}

/// The progress panel.
pub fn paint(c: &mut os_ui::Canvas, t: &os_theme::Palette, b: &Busy) {
    let (done, what) = b.progress.get();
    crate::loading::panel(c, t, b.title, &format!("{what}   {:.0} %", done * 100.0), done);
}

/// Waits for the job under way and puts its result in (tests): the status line.
#[cfg(test)]
pub fn wait(app: &mut App) -> Option<String> {
    let b = app.busy.as_ref()?;
    let finish = b.rx.recv_timeout(std::time::Duration::from_secs(120)).ok()?;
    app.busy = None;
    let msg = finish(app);
    app.state.message = Some(msg.clone());
    Some(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A job runs off the UI thread: `run` returns at once, the panel shows its progress, the window takes no input,
    /// and its result goes in when it is done.
    #[test]
    fn a_slow_job_leaves_the_window_free() {
        let mut app = App::new(None);
        let (go, wait_go) = channel::<()>();
        let t = std::time::Instant::now();
        assert!(run(&mut app, "Testing", move |p| {
            p.set(0.5, "Halfway");
            let _ = wait_go.recv();
            Box::new(|app: &mut App| {
                app.state.project.tempo = Some(99.0);
                "Done".to_string()
            })
        }));
        assert!(t.elapsed() < std::time::Duration::from_secs(1), "run does not wait for the job");
        assert!(app.loading(), "no input meanwhile");
        assert!(!run(&mut app, "Another", |_| Box::new(|_: &mut App| String::new())), "one at a time");
        let shown = std::time::Instant::now();
        while app.busy.as_ref().is_some_and(|b| b.progress.get().0 < 0.5) && shown.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::yield_now();
        }
        assert_eq!(app.busy.as_ref().unwrap().progress.get(), (0.5, "Halfway".to_string()));
        tick(&mut app);
        assert!(app.busy.is_some(), "still working");
        go.send(()).unwrap();
        assert_eq!(wait(&mut app).as_deref(), Some("Done"));
        assert_eq!(app.state.project.tempo, Some(99.0));
        assert!(!app.loading());
    }
}
