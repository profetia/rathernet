use bitvec::prelude::*;
use num::traits::PrimInt;

pub trait DecodeToInt<T: PrimInt> {
    fn decode(&self) -> T;
}

fn decode<T: PrimInt>(bits: &BitSlice) -> T {
    let zero = T::zero();
    let one = T::one();
    bits.iter().enumerate().fold(
        zero,
        |acc, (index, bit)| if *bit { acc | (one << index) } else { acc },
    )
}

impl<T: PrimInt> DecodeToInt<T> for BitVec {
    fn decode(&self) -> T {
        decode::<T>(self.as_bitslice())
    }
}

impl<T: PrimInt> DecodeToInt<T> for BitSlice {
    fn decode(&self) -> T {
        decode::<T>(self)
    }
}

pub trait DecodeToBytes {
    fn decode(&self) -> Vec<u8>;
}

impl DecodeToBytes for BitVec {
    fn decode(&self) -> Vec<u8> {
        DecodeToBytes::decode(self.as_bitslice())
    }
}

impl DecodeToBytes for BitSlice {
    fn decode(&self) -> Vec<u8> {
        self.chunks(8).map(DecodeToInt::<u8>::decode).collect()
    }
}

pub trait EncodeFromBytes {
    fn encode(&self) -> BitVec;
}

impl EncodeFromBytes for [u8] {
    fn encode(&self) -> BitVec {
        self.iter()
            .flat_map(|byte| byte.view_bits::<Lsb0>())
            .collect()
    }
}

impl EncodeFromBytes for Vec<u8> {
    fn encode(&self) -> BitVec {
        self.as_slice().encode()
    }
}
