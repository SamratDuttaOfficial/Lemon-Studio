// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Voice's AI denoise: DeepFilterNet 3 (Schröter et al., MIT / Apache-2.0), a small neural network trained to
//! keep speech and singing and take away everything else (hum, hiss, fans, traffic, keyboards, the room's noise). Its
//! model is built into the plugin and runs on the CPU (tract), nothing to download.
//!
//! How:
//! - The model hears 48 kHz in hops of 10 ms. At another rate the sound is resampled to 48 kHz and back (rubato's FFT
//!   resampler) in chunks of whole hops: 441 samples at 44.1 kHz are 480 at 48 kHz.
//! - Each instance builds its own model (a moment): a new instance with the AI on at once (outside the audio thread);
//!   turning the AI on while playing, on a thread of its own, the sound going on untouched until it is there.
//! - The amount is how deep it may cut: the model's attenuation limit, 0.6 dB a percent, none at 100%.
//! - The delay is always the same, the AI on or off (the untouched sound is delayed alike), so the host compensates
//!   it once; it fades in and out over 30 ms, after a few hops that fill the model's look-ahead.

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::OnceLock;

use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::{ArrayView2, ArrayViewMut2};
use rubato::{FftFixedInOut, Resampler};

/// The model's rate and hop.
const SR: usize = 48000;
const HOP: usize = 480;
/// Its delay at 48 kHz: the STFT's (one hop) and its look-ahead (two hops).
const MODEL_DELAY: usize = 1440;
/// Hops it runs before it is heard (its look-ahead and history fill), and how long it fades in or out (s).
const WARM: usize = 5;
const FADE_S: f32 = 0.03;
/// The longest chunk at 48 kHz (100 ms): a rate that needs longer ones has no AI.
const LONGEST: usize = SR / 10;

/// DeepFilterNet's model, one instance's own.
struct Model(DfTract);

// SAFETY: tract keeps some of a model's tensors behind `Rc`, so `DfTract` is not `Send`. A `Model` is built on its own
// (`build`) and never cloned, so none of those `Rc` is shared with anything outside it: it moves between threads whole
// and only the thread holding it uses it.
unsafe impl Send for Model {}

/// The model's files, unpacked from the plugin once.
fn params() -> Option<DfParams> {
    static PARAMS: OnceLock<Option<DfParams>> = OnceLock::new();
    PARAMS.get_or_init(|| catch_unwind(DfParams::default).ok()).clone()
}

/// A model ready to run, for two channels; `None` when it cannot be built.
fn build() -> Option<Box<Model>> {
    let p = params()?;
    let make = move || DfTract::new(p, &RuntimeParams::default_with_ch(2)).ok();
    catch_unwind(AssertUnwindSafe(make)).ok().flatten().map(|m| Box::new(Model(m)))
}

/// To 48 kHz and back (rubato's FFT resamplers, a chunk at a time).
type Resamplers = (FftFixedInOut<f32>, FftFixedInOut<f32>);

/// The chunk lengths for `rate`: host samples and 48 kHz samples (whole hops), `None` when they would be too long.
fn chunks(rate: usize) -> Option<(usize, usize)> {
    let gcd = |mut a: usize, mut b: usize| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    let unit = SR / gcd(rate, SR);
    let big = unit / gcd(unit, HOP) * HOP;
    (rate > 0 && big <= LONGEST).then(|| (big * rate / SR, big))
}

fn resamplers(rate: usize, chunk: usize, big: usize) -> Option<Resamplers> {
    let up = FftFixedInOut::new(rate, SR, big, 2).ok()?;
    let down = FftFixedInOut::new(SR, rate, chunk, 2).ok()?;
    let fits = (up.input_frames_next(), up.output_frames_next(), down.input_frames_next(), down.output_frames_next()) == (chunk, big, big, chunk);
    fits.then_some((up, down))
}

pub struct Denoiser {
    rate: usize,
    /// Host samples a chunk, and 48 kHz samples (whole hops); 0 where the rate has no AI.
    chunk: usize,
    big: usize,
    /// To 48 kHz and back (`None` at 48 kHz).
    resample: Option<Resamplers>,
    model: Option<Box<Model>>,
    /// A model being built on a thread of its own, and whether building it failed.
    coming: Option<Receiver<Option<Box<Model>>>>,
    failed: bool,
    /// The amount (0 to 1) and the attenuation limit last given the model (dB).
    amount: f32,
    limit: f32,
    /// The chunk being gathered (per channel) and how far.
    gather: [Vec<f32>; 2],
    fill: usize,
    /// The chunk at 48 kHz, cleaned at 48 kHz, cleaned at the host rate; a hop in and out (channel after channel).
    hi: [Vec<f32>; 2],
    clean_hi: [Vec<f32>; 2],
    clean: [Vec<f32>; 2],
    hop_in: Vec<f32>,
    hop_out: Vec<f32>,
    /// The untouched sound, delayed as the cleaned one is; what is played next.
    dry: VecDeque<[f32; 2]>,
    out: VecDeque<[f32; 2]>,
    /// How much of the cleaned sound is heard (0 to 1) and hops run since the model started on this sound.
    fade: f32,
    warm: usize,
    /// The whole delay (samples at the host rate).
    latency: usize,
}

impl Denoiser {
    /// For host rate `rate`; `amount` (0 to 100) above zero builds the model now.
    pub fn new(rate: u32, amount: i32) -> Denoiser {
        let rate = rate.max(1) as usize;
        let (mut chunk, mut big) = chunks(rate).unwrap_or((0, 0));
        let resample = if chunk > 0 && rate != SR { resamplers(rate, chunk, big) } else { None };
        if rate != SR && resample.is_none() {
            (chunk, big) = (0, 0);
        }
        // The chunk is gathered (chunk - 1), then the resamplers (half a chunk each) and the model delay it.
        let latency = if chunk == 0 {
            0
        } else {
            let hi = if rate == SR { MODEL_DELAY } else { big / 2 + MODEL_DELAY };
            let lo = if rate == SR { 0.0 } else { chunk as f64 / 2.0 };
            chunk - 1 + (hi as f64 * rate as f64 / SR as f64 + lo).round() as usize
        };
        let inner = latency.saturating_sub(chunk.saturating_sub(1));
        let mut d = Denoiser {
            rate,
            chunk,
            big,
            resample,
            model: None,
            coming: None,
            failed: false,
            amount: 0.0,
            limit: -1.0,
            gather: [vec![0.0; chunk], vec![0.0; chunk]],
            fill: 0,
            hi: [vec![0.0; big], vec![0.0; big]],
            clean_hi: [vec![0.0; big], vec![0.0; big]],
            clean: [vec![0.0; chunk], vec![0.0; chunk]],
            hop_in: vec![0.0; 2 * HOP],
            hop_out: vec![0.0; 2 * HOP],
            dry: VecDeque::from(vec![[0.0; 2]; inner]),
            out: VecDeque::from(vec![[0.0; 2]; chunk.saturating_sub(1)]),
            fade: 0.0,
            warm: 0,
            latency,
        };
        if amount > 0 && chunk > 0 {
            d.model = build();
            d.failed = d.model.is_none();
        }
        d.set(amount);
        d
    }

    /// The same model (built or coming) for host rate `rate`, starting on a new sound.
    pub fn renew(&mut self, rate: u32) -> Denoiser {
        let mut d = Denoiser::new(rate, 0);
        (d.model, d.coming, d.failed) = (self.model.take(), self.coming.take(), self.failed);
        d.set((self.amount * 100.0).round() as i32);
        d
    }

    /// The delay, in samples at the host rate (the same whether the AI is on or not).
    pub fn latency(&self) -> usize {
        self.latency
    }

    /// Whether the AI is heard (or fading in).
    pub fn active(&self) -> bool {
        self.fade > 0.0
    }

    /// The amount, 0 to 100: above zero, a model is built when there is none (on a thread of its own).
    pub fn set(&mut self, amount: i32) {
        self.amount = amount.clamp(0, 100) as f32 / 100.0;
        if self.amount > 0.0 && self.chunk > 0 && self.model.is_none() && self.coming.is_none() && !self.failed {
            let (tx, rx) = sync_channel(1);
            let spawned = std::thread::Builder::new().name("Lemon Voice AI".into()).spawn(move || {
                let _ = tx.send(build());
            });
            match spawned {
                Ok(_) => self.coming = Some(rx),
                Err(_) => self.failed = true,
            }
        }
    }

    /// Cleans `left` / `right` in place (delayed by `latency`).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.chunk == 0 {
            return;
        }
        if let Some(rx) = &self.coming {
            match rx.try_recv() {
                Ok(m) => {
                    self.failed = m.is_none();
                    (self.model, self.coming, self.warm) = (m, None, 0);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => (self.coming, self.failed) = (None, true),
            }
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.gather[0][self.fill] = *l;
            self.gather[1][self.fill] = *r;
            self.fill += 1;
            if self.fill == self.chunk {
                self.fill = 0;
                self.run();
            }
            [*l, *r] = self.out.pop_front().unwrap_or([0.0; 2]);
        }
    }

    /// One chunk: cleaned while the AI is on or fading, mixed with the untouched sound as the fade says.
    fn run(&mut self) {
        let want = self.amount > 0.0 && self.model.is_some();
        let running = want || self.fade > 0.0;
        if running && self.clean_chunk() {
            self.warm += self.big / HOP;
        } else {
            self.warm = 0;
        }
        let target = if want && self.warm >= WARM { 1.0 } else { 0.0 };
        let step = 1.0 / (FADE_S * self.rate as f32);
        for i in 0..self.chunk {
            let [a, b] = self.dry.pop_front().unwrap_or([0.0; 2]);
            self.dry.push_back([self.gather[0][i], self.gather[1][i]]);
            self.fade = if target > self.fade { (self.fade + step).min(target) } else { (self.fade - step).max(target) };
            let f = self.fade;
            let out = if f > 0.0 { [a + (self.clean[0][i] - a) * f, b + (self.clean[1][i] - b) * f] } else { [a, b] };
            self.out.push_back(out);
        }
    }

    /// The gathered chunk through the model into `clean`; false when there is no model to run.
    fn clean_chunk(&mut self) -> bool {
        let Some(model) = self.model.as_mut() else { return false };
        let limit = if self.amount >= 1.0 { 100.0 } else { 60.0 * self.amount };
        if limit != self.limit {
            model.0.set_atten_lim(limit);
            self.limit = limit;
        }
        match &mut self.resample {
            Some((up, _)) => {
                if up.process_into_buffer(&self.gather, &mut self.hi, None).is_err() {
                    return false;
                }
            }
            None => {
                for ch in 0..2 {
                    self.hi[ch].copy_from_slice(&self.gather[ch]);
                }
            }
        }
        for h in 0..self.big / HOP {
            for ch in 0..2 {
                self.hop_in[ch * HOP..(ch + 1) * HOP].copy_from_slice(&self.hi[ch][h * HOP..(h + 1) * HOP]);
            }
            let (Ok(noisy), Ok(enh)) = (ArrayView2::from_shape((2, HOP), &self.hop_in[..]), ArrayViewMut2::from_shape((2, HOP), &mut self.hop_out[..]))
            else {
                return false;
            };
            if model.0.process(noisy, enh).is_err() {
                return false;
            }
            for ch in 0..2 {
                self.clean_hi[ch][h * HOP..(h + 1) * HOP].copy_from_slice(&self.hop_out[ch * HOP..(ch + 1) * HOP]);
            }
        }
        match &mut self.resample {
            Some((_, down)) => down.process_into_buffer(&self.clean_hi, &mut self.clean, None).is_ok(),
            None => {
                for ch in 0..2 {
                    self.clean[ch].copy_from_slice(&self.clean_hi[ch]);
                }
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_are_whole_hops() {
        assert_eq!(chunks(48000), Some((480, 480)));
        assert_eq!(chunks(44100), Some((441, 480)));
        assert_eq!(chunks(96000), Some((960, 480)));
        assert_eq!(chunks(22050), Some((441, 960)));
        assert_eq!(chunks(32000), Some((320, 480)));
        assert_eq!(chunks(44101), None);
        for rate in [44100, 22050, 96000, 32000, 88200, 192000, 8000] {
            let (c, b) = chunks(rate).unwrap();
            assert!(resamplers(rate, c, b).is_some(), "{rate}");
        }
    }

    /// To 48 kHz and back delays a click by what `latency` counts for the resamplers.
    #[test]
    fn the_resamplers_delay_as_counted() {
        for rate in [44100usize, 22050, 96000] {
            let (chunk, big) = chunks(rate).unwrap();
            let (mut up, mut down) = resamplers(rate, chunk, big).unwrap();
            let len = 8 * chunk;
            let mut x = vec![0.0f32; len];
            x[3 * chunk + 17] = 1.0;
            let mut y = Vec::new();
            let (mut hi, mut lo) = ([vec![0.0; big], vec![0.0; big]], [vec![0.0; chunk], vec![0.0; chunk]]);
            for c in x.chunks(chunk) {
                up.process_into_buffer(&[c.to_vec(), c.to_vec()], &mut hi, None).unwrap();
                down.process_into_buffer(&hi, &mut lo, None).unwrap();
                y.extend_from_slice(&lo[0]);
            }
            let peak = y.iter().enumerate().fold((0, 0.0f32), |m, (i, &v)| if v > m.1 { (i, v) } else { m }).0;
            let want = 3 * chunk + 17 + ((big / 2) as f64 * rate as f64 / SR as f64 + chunk as f64 / 2.0).round() as usize;
            assert!(peak.abs_diff(want) <= 1, "{rate}: peak at {peak}, counted {want}");
        }
    }
}

#[cfg(test)]
mod timing {
    /// How long a model takes to build and to run (`cargo test -p lemon-small-fx --release -- --ignored build_time
    /// --nocapture`).
    #[test]
    #[ignore]
    fn build_time() {
        let t = std::time::Instant::now();
        let _ = super::params();
        println!("unpacked in {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let m = super::build().expect("builds");
        println!("built in {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let _again = super::build().expect("builds");
        println!("built again in {:?}", t.elapsed());
        let mut d = super::Denoiser::new(48000, 100);
        let (mut l, mut r) = (vec![0.0f32; 48000], vec![0.0f32; 48000]);
        let mut seed = 1u32;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *a = ((seed >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.1;
            *b = *a;
        }
        let t = std::time::Instant::now();
        d.process(&mut l, &mut r);
        println!("one second of stereo in {:?}", t.elapsed());
        drop(m);
    }
}
