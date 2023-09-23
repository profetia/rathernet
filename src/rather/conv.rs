//! Translated from https://gist.github.com/YairMZ/b88e594047c7b5366053cd7fb375a94f.
//! The corresponding blog can be found in:
//! - https://medium.com/nerd-for-tech/into-to-convolutional-coding-part-i-d63decab56a0
//! - https://medium.com/nerd-for-tech/intro-to-convolutional-coding-part-ii-d289c109ff7a
//! - https://medium.com/nerd-for-tech/intro-to-convolutional-coding-part-iii-5529fdeebdb6

use bitvec::prelude::*;
use std::mem;

#[derive(Debug, Clone)]
struct TrellisPath {
    path_metric: usize,
    path: Vec<usize>,
    last_state: usize,
    bits_input: Vec<u8>,
    len: usize,
}

impl TrellisPath {
    fn new(last_state: usize) -> Self {
        Self {
            path_metric: 0,
            path: vec![last_state],
            last_state,
            bits_input: vec![],
            len: 1,
        }
    }
}

pub struct ConvolutionalCode {
    n: usize,
    k: usize,
    rate: f32,
    constraint_length: usize,
    number_of_states: usize,
    state_space: Vec<usize>,
    generators: Vec<usize>,
    next_states: Vec<Vec<usize>>,
    out_bits: Vec<Vec<Vec<usize>>>,
}

impl ConvolutionalCode {
    pub fn new(generators: Vec<usize>) -> Self {
        let n = generators.len();
        let k = 1usize;
        let rate = k as f32 / n as f32;
        let constraint_length = generators
            .iter()
            .fold(0usize, |acc, item| acc.max(*item))
            .checked_ilog2()
            .unwrap() as usize;
        let num_states = 1usize << constraint_length;
        let state_space = (0..num_states).collect::<Vec<usize>>();

        let possible_inputs = (0..1usize << k).collect::<Vec<usize>>();
        let mut next_states = vec![vec![]; num_states];
        let mut out_bits = vec![vec![]; num_states];
        for &current_state in state_space.iter() {
            next_states[current_state] = vec![0; possible_inputs.len()];
            out_bits[current_state] = vec![vec![]; possible_inputs.len()];

            for &current_input in possible_inputs.iter() {
                let new_state = (current_input << (constraint_length - 1)) + (current_state >> k);
                next_states[current_state][current_input] = new_state;

                let mut tmp = vec![];
                for &fwd in generators.iter() {
                    let bit_reversed_fwd = fwd.reverse_bits()
                        >> (8 * mem::size_of::<usize>() - (constraint_length + 1));
                    let lsr = (current_input << constraint_length) + current_state;
                    let generator_masked_sum_arg = bit_reversed_fwd & lsr;
                    tmp.push((generator_masked_sum_arg.count_ones() % 2) as usize);
                }

                out_bits[current_state][current_input] = tmp;
            }
        }

        Self {
            n,
            k,
            rate,
            constraint_length,
            number_of_states: num_states,
            state_space,
            generators,
            next_states,
            out_bits,
        }
    }
}

impl ConvolutionalCode {
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let mut input_bits = vec![0u8; self.constraint_length + data.len() * 8];
        let mut coded_bits = vec![0u8; (input_bits.len() as f32 / self.rate) as usize];

        for (byte_idx, &byt) in data.iter().enumerate() {
            let bits = byt.view_bits::<Msb0>();
            for (bit_idx, bit) in bits.iter().enumerate() {
                input_bits[byte_idx * 8 + bit_idx] = *bit as u8;
            }
        }

        let mut current_state = 0;
        for (bit_idex, &bit) in input_bits.iter().enumerate() {
            let outputs = self.out_bits[current_state][bit as usize].clone();
            for (output_idx, &output) in outputs.iter().enumerate() {
                coded_bits[bit_idex * self.n + output_idx] = output as u8;
            }
            current_state = self.next_states[current_state][bit as usize];
        }

        coded_bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode() {
        let conv = ConvolutionalCode::new(vec![5, 7]);

        let input_bytes = b"\xFE\xF0\x0A\x01";
        let encoded = conv.encode(input_bytes);
        assert_eq!(
            encoded,
            vec![
                1, 1, 1, 0, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 1,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 1, 1,
            ]
        );

        let conv = ConvolutionalCode::new(vec![3, 7, 13]);
        let input_bytes = b"\x72\x01";
        let encoded = conv.encode(input_bytes);
        assert_eq!(
            encoded,
            vec![
                0, 0, 0, 1, 1, 1, 0, 0, 1, 0, 1, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0, 1, 1, 0, 0, 1, 1, 0,
                0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0,
                1,
            ]
        )
    }
}
