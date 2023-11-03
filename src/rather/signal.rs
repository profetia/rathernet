use realfft::{num_complex::Complex, RealFftPlanner};
use std::f32::consts::PI;
extern crate fixed;
use fixed::types::I32F32;

pub fn rfft(source: &[f32], len: usize) -> Box<[Complex<f32>]> {
    let mut real_planner = RealFftPlanner::<f32>::new();
    let fft = real_planner.plan_fft_forward(len);

    let mut buffer = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let len = buffer.len();
    if source.len() < len {
        buffer[..source.len()].copy_from_slice(source);
    } else {
        buffer.copy_from_slice(&source[..len]);
    }

    fft.process(&mut buffer, &mut spectrum).unwrap();
    spectrum.into()
}

fn irfft(source: &[Complex<f32>], len: usize) -> Box<[f32]> {
    let mut real_planner = RealFftPlanner::<f32>::new();
    let fft = real_planner.plan_fft_inverse(len);

    let mut spectrum = fft.make_input_vec();
    let mut buffer = fft.make_output_vec();
    let len = spectrum.len();
    if source.len() < len {
        spectrum[..source.len()].copy_from_slice(source);
    } else {
        spectrum.copy_from_slice(&source[..len]);
    }

    fft.process(&mut spectrum, &mut buffer).unwrap();
    buffer.into()
}

pub fn correlate(volume: &[f32], kernel: &[f32]) -> Box<[f32]> {
    let full = volume.len() + kernel.len() - 1;

    let volume = volume.to_vec().normalize();
    let mut kernel = kernel.to_vec().normalize();
    kernel.reverse();

    let volume_fft = rfft(&volume, full);
    let kernel_fft = rfft(&kernel, full);

    let corr_ifft = volume_fft
        .iter()
        .zip(kernel_fft.iter())
        .map(|(a, b)| a * b)
        .collect::<Vec<_>>();

    irfft(&corr_ifft, full)
        .iter()
        .copied()
        .map(|item| item / full as f32)
        .collect()
}

pub fn correlate_naive(volume: &[f32], kernel: &[f32]) -> Box<[f32]> {
    let n = volume.len();
    let m = kernel.len();
    let full = n + m - 1;

    // Initialize the result vector with zeros
    let mut result = vec![0f32; full];

    // Loop through each position in the volume
    for i in 0..n {
        // Loop through each position in the kernel
        for j in 0..m {
            result[i + j] += volume[i] * kernel[m - 1 - j];
        }
    }
    result.into_boxed_slice()
}

pub fn correlate_naive_fixpoint(volume: &[f32], kernel: &[f32]) -> Box<[f32]> {
    let n = volume.len();
    let m = kernel.len();
    let full = n + m - 1;

    // Convert the input slices to fixed-point
    let volume_fixed: Vec<I32F32> = volume.iter().map(|&v| I32F32::from_num(v)).collect();
    let kernel_fixed: Vec<I32F32> = kernel.iter().map(|&k| I32F32::from_num(k)).collect();

    // Initialize the result vector with zeros
    let mut result = vec![I32F32::from_bits(0); full];

    // Loop through each position in the volume
    for i in 0..n {
        // Loop through each position in the kernel
        for j in 0..m {
            result[i + j] = result[i + j] + (volume_fixed[i] * kernel_fixed[m - 1 - j]);
        }
    }

    // Convert the fixed-point result back to f32
    let result_f32: Vec<f32> = result.iter().map(|&res| res.to_num::<f32>()).collect();

    result_f32.into_boxed_slice()
}

pub trait ArgMax
where
    Self: AsRef<[f32]>,
{
    fn argmax(&self) -> (usize, f32);
}

impl ArgMax for Box<[f32]> {
    fn argmax(&self) -> (usize, f32) {
        let (mut index, mut max) = (0, 0.);
        for (i, item) in self.iter().enumerate() {
            if *item > max {
                (index, max) = (i, *item);
            }
        }
        (index, max)
    }
}

impl ArgMax for Vec<f32> {
    fn argmax(&self) -> (usize, f32) {
        let (mut index, mut max) = (0, 0.);
        for (i, item) in self.iter().enumerate() {
            if *item > max {
                (index, max) = (i, *item);
            }
        }
        (index, max)
    }
}

pub trait Normalize
where
    Self: AsRef<[f32]>,
{
    fn normalize(&self) -> Self;
}

impl Normalize for Box<[f32]> {
    fn normalize(&self) -> Box<[f32]> {
        let norm = self.iter().fold(0., |acc, item| acc + item * item).sqrt();
        self.iter()
            .map(|item| item / norm)
            .collect::<Vec<_>>()
            .into()
    }
}

impl Normalize for Vec<f32> {
    fn normalize(&self) -> Vec<f32> {
        let norm = self.iter().fold(0., |acc, item| acc + item * item).sqrt();
        self.iter().map(|item| item / norm).collect::<Vec<_>>()
    }
}

pub fn synchronize(volume: &[f32], kernel: &[f32]) -> (isize, f32) {
    let corr = correlate_naive_fixpoint(volume, kernel);
    let (index, max) = corr.argmax();
    (kernel.len() as isize - 1 - index as isize, max)
}

pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).fold(0., |acc, (a, b)| acc + a * b)
}

pub fn dot_product_fixpoint(a: &[f32], b: &[f32]) -> f32 {
    let a_fixed: Vec<I32F32> = a.iter().map(|&v| I32F32::from_num(v)).collect();
    let b_fixed: Vec<I32F32> = b.iter().map(|&k| I32F32::from_num(k)).collect();
    a_fixed
        .iter()
        .zip(b_fixed.iter())
        .fold(I32F32::from_bits(0), |acc, (a, b)| acc + (a * b))
        .to_num::<f32>()
}

pub trait LowPass
where
    Self: AsMut<[f32]>,
{
    fn low_pass(&mut self, sample_rate: f32, cutoff: f32);
}

impl LowPass for Box<[f32]> {
    fn low_pass(&mut self, sample_rate: f32, cutoff: f32) {
        let rc = 1. / (cutoff * 2. * PI);
        let dt = 1. / sample_rate;
        let alpha = dt / (rc + dt);

        self[0] *= alpha;
        for i in 1..self.len() {
            self[i] = self[i - 1] + alpha * (self[i] - self[i - 1]);
        }
    }
}

impl LowPass for Vec<f32> {
    fn low_pass(&mut self, sample_rate: f32, cutoff: f32) {
        let rc = 1. / (cutoff * 2. * PI);
        let dt = 1. / sample_rate;
        let alpha = dt / (rc + dt);

        self[0] *= alpha;
        for i in 1..self.len() {
            self[i] = self[i - 1] + alpha * (self[i] - self[i - 1]);
        }
    }
}

pub trait HighPass
where
    Self: AsMut<[f32]>,
{
    fn high_pass(&mut self, sample_rate: f32, cutoff: f32);
}

impl HighPass for Box<[f32]> {
    fn high_pass(&mut self, sample_rate: f32, cutoff: f32) {
        let rc = 1. / (cutoff * 2. * PI);
        let dt = 1. / sample_rate;
        let alpha = rc / (rc + dt);

        let mut last = self[0];
        for i in 1..self.len() {
            let cur = self[i];
            self[i] = alpha * (self[i - 1] + self[i] - last);
            last = cur;
        }
    }
}

impl HighPass for Vec<f32> {
    fn high_pass(&mut self, sample_rate: f32, cutoff: f32) {
        let rc = 1. / (cutoff * 2. * PI);
        let dt = 1. / sample_rate;
        let alpha = rc / (rc + dt);

        let mut last = self[0];
        for i in 1..self.len() {
            let cur = self[i];
            self[i] = alpha * (self[i - 1] + self[i] - last);
            last = cur;
        }
    }
}

pub trait BandPass
where
    Self: AsMut<[f32]>,
{
    fn band_pass(&mut self, sample_rate: f32, band: (f32, f32));
}

impl BandPass for Box<[f32]> {
    fn band_pass(&mut self, sample_rate: f32, band: (f32, f32)) {
        self.low_pass(sample_rate, band.1);
        self.high_pass(sample_rate, band.0);
    }
}

impl BandPass for Vec<f32> {
    fn band_pass(&mut self, sample_rate: f32, band: (f32, f32)) {
        self.low_pass(sample_rate, band.1);
        self.high_pass(sample_rate, band.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correlate() {
        let a: Vec<f32> = vec![0., 0., 0., 0., 0., 1., 2., 3., 4., 3., 2., 1., 0.];
        let b: Vec<f32> = vec![1., 2., 3., 4., 3., 2., 1.];

        let (index, value) = synchronize(&b, &a);
        assert_eq!(index, 5);
        assert!((-1. ..=1.).contains(&value));

        let (index, value) = synchronize(&a, &b);
        assert_eq!(index, -5);
        assert!((-1. ..=1.).contains(&value));

        let a: Vec<f32> = vec![2., 3., 4., 3., 2., 1., 0., 3., 6.];
        let b: Vec<f32> = vec![1., 2., 3., 4., 3., 2., 1.];
        let (index, value) = synchronize(&b, &a);
        assert_eq!(index, -1);
        assert!((-1. ..=1.).contains(&value));
        assert_eq!(&a[(b.len() as isize + index) as usize..], [0., 3., 6.]);

        let a: Vec<f32> = vec![0., 0., 0., 3., 4., 3., 2., 1., 0., 3., 6.];
        let b: Vec<f32> = vec![1., 2., 3., 4., 3., 2., 1.];
        let (index, value) = synchronize(&b, &a);
        assert_eq!(index, 1);
        assert!((-1. ..=1.).contains(&value));
        assert_eq!(&a[(b.len() as isize + index) as usize..], [0., 3., 6.]);
    }
}
