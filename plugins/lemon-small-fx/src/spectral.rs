// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Tools the analysing effects share: a radix-2 complex FFT, the periodic Hann window, linear prediction (Levinson)
//! and a median.

use std::f32::consts::TAU;

/// A complex FFT of one power-of-two length, unscaled both ways.
pub struct Fft {
    n: usize,
    rev: Vec<u32>,
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two() && n >= 2, "FFT length {n}");
        let bits = n.trailing_zeros();
        let rev = (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect();
        let (sin, cos) = (0..n / 2).map(|k| (TAU * k as f32 / n as f32).sin_cos()).unzip();
        Fft { n, rev, cos, sin }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    /// In place: forward (`e^-i`), or with `inverse` the inverse (`e^+i`); neither is scaled.
    pub fn run(&self, re: &mut [f32], im: &mut [f32], inverse: bool) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let sign = if inverse { 1.0 } else { -1.0 };
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (c, s) = (self.cos[k * step], sign * self.sin[k * step]);
                    let (a, b) = (start + k, start + k + len / 2);
                    let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }
}

/// The periodic Hann window of length `n` (its copies `n / 4` apart add up to 2).
pub fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (TAU * i as f32 / n as f32).cos()).collect()
}

/// The prediction coefficients `a[1..=order]` of `x` (autocorrelation method, Levinson-Durbin), so that
/// `x[t] + sum a[k] x[t-k]` is the prediction error; `a[0]` is 1. A silent `x` gives no prediction (all 0).
pub fn lpc(x: &[f32], order: usize, a: &mut Vec<f32>) {
    a.clear();
    a.resize(order + 1, 0.0);
    a[0] = 1.0;
    let r: Vec<f64> = (0..=order).map(|k| x[k..].iter().zip(x).map(|(p, q)| f64::from(*p) * f64::from(*q)).sum()).collect();
    if r[0] <= 1e-12 {
        return;
    }
    // A little white-noise floor keeps the model stable on pure tones.
    let mut e = r[0] * (1.0 + 1e-6);
    let mut c = vec![0.0f64; order + 1];
    c[0] = 1.0;
    let mut prev = c.clone();
    for i in 1..=order {
        let acc: f64 = (1..i).map(|j| c[j] * r[i - j]).sum::<f64>() + r[i];
        let k = -acc / e;
        prev.copy_from_slice(&c);
        for j in 1..i {
            c[j] = prev[j] + k * prev[i - j];
        }
        c[i] = k;
        e *= 1.0 - k * k;
        if e <= 0.0 {
            break;
        }
    }
    for (o, v) in a.iter_mut().zip(&c) {
        *o = *v as f32;
    }
}

/// The median of `v` (reorders it); 0 for an empty one.
pub fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let m = v.len() / 2;
    *v.select_nth_unstable_by(m, |a, b| a.total_cmp(b)).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fft_finds_a_tone_and_comes_back() {
        let n = 64;
        let f = Fft::new(n);
        let x: Vec<f32> = (0..n).map(|i| (TAU * 5.0 * i as f32 / n as f32).cos()).collect();
        let (mut re, mut im) = (x.clone(), vec![0.0; n]);
        f.run(&mut re, &mut im, false);
        assert!((re[5] - n as f32 / 2.0).abs() < 1e-3 && (re[n - 5] - n as f32 / 2.0).abs() < 1e-3);
        assert!(re[6].abs() < 1e-3 && im[5].abs() < 1e-3);
        f.run(&mut re, &mut im, true);
        assert!(re.iter().zip(&x).all(|(a, b)| (a / n as f32 - b).abs() < 1e-5));
    }

    #[test]
    fn hann_copies_a_quarter_apart_add_to_two() {
        let w = hann(16);
        for i in 0..4 {
            let s: f32 = (0..4).map(|k| w[i + 4 * k]).sum();
            assert!((s - 2.0).abs() < 1e-6);
        }
    }

    /// A second-order resonance is predicted almost perfectly by its own model.
    #[test]
    fn linear_prediction_models_a_resonance() {
        let mut x = vec![0.0f32; 512];
        (x[0], x[1]) = (1.0, 1.8);
        for t in 2..512 {
            x[t] += 1.8 * x[t - 1] - 0.9 * x[t - 2];
        }
        let mut a = Vec::new();
        lpc(&x, 2, &mut a);
        assert!((a[1] + 1.8).abs() < 0.02 && (a[2] - 0.9).abs() < 0.02, "{a:?}");
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
    }
}
