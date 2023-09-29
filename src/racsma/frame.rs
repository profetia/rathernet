use super::builtin::{
    ADDRESS_BITS_LEN, PARITY_ALGORITHM, PARITY_BITS_LEN, SEQ_BITS_LEN, TYPE_BITS_LEN,
};
use crate::rather::encode::{DecodeToBytes, DecodeToInt};
use anyhow::Error;
use bitflags::bitflags;
use bitvec::prelude::*;
use thiserror::Error;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FrameType: usize {
        const ACK = 0b0000_0001;
        const EOP = 0b0000_0010;
    }
}

#[derive(Debug)]
pub struct Frame {
    pub dest: usize,
    pub src: usize,
    pub seq: usize,
    pub r#type: FrameType,
    pub payload: BitVec,
}

impl Frame {
    pub fn new_data(dest: usize, src: usize, seq: usize, payload: BitVec) -> Self {
        Self {
            dest,
            src,
            seq,
            r#type: FrameType::from_bits_truncate(0b0000_0000),
            payload,
        }
    }

    pub fn new_ack(dest: usize, src: usize, seq: usize) -> Self {
        Self {
            dest,
            src,
            seq,
            r#type: FrameType::ACK,
            payload: bitvec![],
        }
    }
}

impl From<Frame> for BitVec {
    fn from(value: Frame) -> Self {
        let mut frame = bitvec![];
        frame.extend(&value.dest.view_bits::<Lsb0>()[..ADDRESS_BITS_LEN]);
        frame.extend(&value.src.view_bits::<Lsb0>()[..ADDRESS_BITS_LEN]);
        frame.extend(&value.seq.view_bits::<Lsb0>()[..SEQ_BITS_LEN]);
        frame.extend(&value.r#type.bits().view_bits::<Lsb0>()[..TYPE_BITS_LEN]);
        frame.extend(value.payload);

        let bytes = DecodeToBytes::decode(&frame);
        let parity = PARITY_ALGORITHM.checksum(&bytes) as usize;
        frame.extend(&parity.view_bits::<Lsb0>()[..PARITY_BITS_LEN]);

        frame
    }
}

impl TryFrom<BitVec> for Frame {
    type Error = Error;

    fn try_from(value: BitVec) -> Result<Self, Self::Error> {
        if value.len()
            < ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN + PARITY_BITS_LEN
        {
            return Err(FrameError::FrameIsTooShort.into());
        }

        let len = value.len();
        let parity = DecodeToInt::<usize>::decode(&value[len - PARITY_BITS_LEN..]);
        let bytes = DecodeToBytes::decode(&value[..len - PARITY_BITS_LEN]);
        if PARITY_ALGORITHM.checksum(&bytes) as usize != parity {
            return Err(FrameError::ParityCheckFailed.into());
        }

        Ok(Self {
            dest: DecodeToInt::decode(&value[0..ADDRESS_BITS_LEN]),
            src: DecodeToInt::decode(&value[ADDRESS_BITS_LEN..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN]),
            seq: DecodeToInt::decode(
                &value[ADDRESS_BITS_LEN + ADDRESS_BITS_LEN
                    ..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN],
            ),
            r#type: FrameType::from_bits_truncate(DecodeToInt::<usize>::decode(
                &value[ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN
                    ..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN],
            )),
            payload: value[ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN
                ..len - PARITY_BITS_LEN]
                .to_owned(),
        })
    }
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("Frame is too short")]
    FrameIsTooShort,
    #[error("Parity check failed")]
    ParityCheckFailed,
}
