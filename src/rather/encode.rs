use bitvec::prelude::*;
use num::traits::PrimInt;

pub trait BitDecoding<T: PrimInt> {
    fn decode(&self) -> T;
}

fn bit_decode<T: PrimInt>(bits: &BitSlice) -> T {
    let zero = T::zero();
    let one = T::one();
    bits.iter().enumerate().fold(
        zero,
        |acc, (index, bit)| if *bit { acc | (one << index) } else { acc },
    )
}

impl<T: PrimInt> BitDecoding<T> for BitVec {
    fn decode(&self) -> T {
        bit_decode::<T>(self.as_bitslice())
    }
}

impl<T: PrimInt> BitDecoding<T> for &BitSlice {
    fn decode(&self) -> T {
        bit_decode::<T>(self)
    }
}

pub trait ByteDecoding {
    fn decode(&self) -> Vec<u8>;
}
