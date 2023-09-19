use realfft::{num_complex::Complex, RealFftPlanner};

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
}

pub trait ArgMax<T>
where
    T: AsRef<[f32]>,
{
    fn argmax(&self) -> (usize, f32);
}

impl ArgMax<Box<[f32]>> for Box<[f32]> {
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

pub trait Normalize<T>
where
    T: AsRef<[f32]>,
{
    fn normalize(&self) -> T;
}

impl Normalize<Box<[f32]>> for Box<[f32]> {
    fn normalize(&self) -> Box<[f32]> {
        let norm = self.iter().fold(0., |acc, item| acc + item * item).sqrt();
        self.iter()
            .map(|item| item / norm)
            .collect::<Vec<_>>()
            .into()
    }
}

impl Normalize<Vec<f32>> for Vec<f32> {
    fn normalize(&self) -> Vec<f32> {
        let norm = self.iter().fold(0., |acc, item| acc + item * item).sqrt();
        self.iter().map(|item| item / norm).collect::<Vec<_>>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correlate() {
        let a: Vec<f32> = vec![0., 0., 0., 0., 0., 1., 2., 3., 4., 3., 2., 1., 0.];
        let b: Vec<f32> = vec![1., 2., 3., 4., 3., 2., 1.];

        let c = correlate(&b, &a);
        let (index, _) = c.argmax();
        assert_eq!(a.len() as isize - 1 - index as isize, 5);

        let c = correlate(&a, &b);
        let (index, _) = c.argmax();
        assert_eq!(b.len() as isize - 1 - index as isize, -5);
    }
}
