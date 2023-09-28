//! Translated from <https://gist.github.com/YairMZ/b88e594047c7b5366053cd7fb375a94f>.
//!
//! The corresponding blog can be found in:
//! - <https://medium.com/nerd-for-tech/into-to-convolutional-coding-part-i-d63decab56a0>
//! - <https://medium.com/nerd-for-tech/intro-to-convolutional-coding-part-ii-d289c109ff7a>
//! - <https://medium.com/nerd-for-tech/intro-to-convolutional-coding-part-iii-5529fdeebdb6>

use bitvec::prelude::*;
use std::mem;

type StateId = usize;

#[derive(Debug, Clone)]
struct Path {
    score: u32,
    path: Vec<StateId>,
    last: StateId,
    bits: BitVec,
}

impl Path {
    fn new(last: StateId) -> Self {
        Self {
            score: 0,
            path: vec![last],
            last,
            bits: bitvec![],
        }
    }
}

impl Path {
    fn forward(&mut self, state: StateId, score: u32, bit: bool) {
        self.last = state;
        self.path.push(state);
        self.score += score;
        self.bits.push(bit);
    }
}

pub struct ConvCode {
    factor: usize,
    order: usize,
    states: Vec<StateId>,
    transitions: Vec<Vec<StateId>>,
    outputs: Vec<Vec<BitVec>>,
}

impl ConvCode {
    pub fn new(generators: &[usize]) -> Self {
        let factor = generators.len();
        let order = generators
            .iter()
            .fold(0usize, |acc, item| acc.max(*item))
            .checked_ilog2()
            .unwrap() as usize;
        let num_states = 1usize << order;
        let states = (0..num_states).collect::<Vec<usize>>();

        let mut transitions = vec![vec![]; num_states];
        let mut outputs = vec![vec![]; num_states];
        for &now in states.iter() {
            transitions[now] = vec![0; 2];
            outputs[now] = vec![bitvec![]; 2];

            for &bit in &[0, 1] {
                let next = (bit << (order - 1)) + (now >> 1);
                transitions[now][bit] = next;

                let mut output = bitvec![];
                for &polygon in generators.iter() {
                    let polygon =
                        polygon.reverse_bits() >> (8 * mem::size_of::<usize>() - (order + 1));
                    output.push((polygon & ((bit << order) + now)).count_ones() % 2 == 1);
                }
                outputs[now][bit] = output;
            }
        }

        Self {
            factor,
            order,
            states,
            transitions,
            outputs,
        }
    }
}

type Transition<'a> = (usize, u32, u32, &'a Path, bool);

impl ConvCode {
    pub fn encode(&self, bits: &BitSlice) -> BitVec {
        let mut bits = bits.to_owned();
        let mut outputs = bitvec![];
        outputs.reserve(bits.len() * self.factor + self.order);
        bits.resize(bits.len() + self.order, false);

        let mut now = 0;
        for bit in bits.iter() {
            outputs.extend(&self.outputs[now][*bit as usize]);
            now = self.transitions[now][*bit as usize];
        }

        outputs
    }

    pub fn decode(&self, bits: &BitSlice) -> (BitVec, u32) {
        if bits.is_empty() {
            return (bitvec![], 0);
        }

        let words = bits.chunks(self.factor);
        let mut paths = vec![Path::new(0)];

        for word in words {
            let mut transitions: Vec<Transition> = vec![];
            for path in paths.iter() {
                for bit in [0, 1] {
                    let next = self.transitions[path.last][bit];
                    let output = &self.outputs[path.last][bit];
                    let score = output
                        .iter()
                        .zip(word.iter())
                        .map(|(x, y)| (*x ^ *y) as u32)
                        .sum::<u32>();

                    let transition: Transition = (next, score + path.score, score, path, bit != 0);
                    transitions.push(transition);
                }
            }

            let mut new_paths = vec![];
            for &state in self.states.iter() {
                let target = transitions
                    .iter()
                    .filter(|x| x.0 == state)
                    .collect::<Vec<_>>();
                if !target.is_empty() {
                    let transition =
                        target
                            .iter()
                            .fold(target[0], |acc, x| if acc.1 < x.1 { acc } else { x });
                    let mut path = transition.3.clone();
                    path.forward(state, transition.2, transition.4);
                    new_paths.push(path);
                }
            }
            paths = new_paths
        }

        let Path { bits, score, .. } = paths
            .into_iter()
            .find_map(|path| if path.last == 0 { Some(path) } else { None })
            .unwrap();

        (bits[..bits.len() - self.order].to_owned(), score)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_bytes(bytes: &[u8]) -> BitVec {
        let mut result = bitvec![];
        for &byte in bytes.iter() {
            result.extend(byte.view_bits::<Msb0>());
        }
        result
    }

    fn decode_bytes(bits: &BitSlice) -> Vec<u8> {
        bits.chunks(8)
            .map(|bits| {
                bits.iter()
                    .rev()
                    .enumerate()
                    .fold(
                        0u8,
                        |acc, (index, bit)| if *bit { acc | (1 << index) as u8 } else { acc },
                    )
            })
            .collect()
    }

    #[test]
    fn test_encode() {
        let conv = ConvCode::new(&[5, 7]);
        let input_bytes = b"\xFE\xF0\x0A\x01";
        let encoded = conv.encode(&encode_bytes(input_bytes));
        assert_eq!(
            encoded,
            vec![
                1, 1, 1, 0, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 1,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 1, 1,
            ]
            .into_iter()
            .map(|x| x == 1)
            .collect::<BitVec>()
        );

        let conv = ConvCode::new(&[3, 7, 13]);
        let input_bytes = b"\x72\x01";
        let encoded = conv.encode(&encode_bytes(input_bytes));
        assert_eq!(
            encoded,
            vec![
                0, 0, 0, 1, 1, 1, 0, 0, 1, 0, 1, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0, 1, 1, 0, 0, 1, 1, 0,
                0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0,
                1,
            ]
            .into_iter()
            .map(|x| x == 1)
            .collect::<BitVec>()
        )
    }

    #[test]
    fn test_decode() {
        let conv = ConvCode::new(&[5, 7]);
        let input_bytes = b"\xFE\xF0\x0A\x01";
        let encoded = conv.encode(&encode_bytes(input_bytes));
        let (decoded, _) = conv.decode(&encoded);
        assert_eq!(decode_bytes(&decoded[..]), input_bytes);

        let conv = ConvCode::new(&[3, 7, 13]);
        let input_bytes = b"\x72\x01";
        let encoded = conv.encode(&encode_bytes(input_bytes));
        let (decoded, _) = conv.decode(&encoded);
        assert_eq!(decode_bytes(&decoded[..]), input_bytes);
    }

    #[test]
    fn test_correction() {
        let conv = ConvCode::new(&[5, 7]);
        let input_bytes = b"\xFE\xF0\x0A\x01";
        let mut encoded = conv.encode(&encode_bytes(input_bytes));
        let (_, corrected_errors) = conv.decode(&encoded);
        assert_eq!(corrected_errors, 0);

        for index in [1, 4, 9, 17, 23] as [usize; 5] {
            let mut bit = encoded.get_mut(index).unwrap();
            *bit ^= true;
        }
        let (_, corrected_errors) = conv.decode(&encoded);
        assert_eq!(corrected_errors, 5);

        let conv = ConvCode::new(&[3, 7, 13]);
        let input_bytes = b"\x72\x01";
        let mut encoded = conv.encode(&encode_bytes(input_bytes));
        let (_, corrected_errors) = conv.decode(&encoded);
        assert_eq!(corrected_errors, 0);

        for index in [1, 4, 9, 17, 23] as [usize; 5] {
            let mut bit = encoded.get_mut(index).unwrap();
            *bit ^= true;
        }
        let (_, corrected_errors) = conv.decode(&encoded);
        assert_eq!(corrected_errors, 5);
    }
}
