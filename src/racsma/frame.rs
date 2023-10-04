use super::builtin::{
    ADDRESS_BITS_LEN, PARITY_ALGORITHM, PARITY_BITS_LEN, SEQ_BITS_LEN, TYPE_BITS_LEN,
};
use crate::rather::encode::{DecodeToBytes, DecodeToInt};
use anyhow::Error;
use bitvec::prelude::*;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameType {
    Data = 0b0000_0000,
    Ack = 0b0000_0001,
}

impl FrameType {
    pub fn bits(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone)]
pub struct FrameHeader {
    pub dest: usize,
    pub src: usize,
    pub seq: usize,
    pub r#type: usize,
}

impl From<FrameHeader> for BitVec {
    fn from(value: FrameHeader) -> Self {
        let mut header = bitvec![];
        header.extend(&value.dest.view_bits::<Lsb0>()[..ADDRESS_BITS_LEN]);
        header.extend(&value.src.view_bits::<Lsb0>()[..ADDRESS_BITS_LEN]);
        header.extend(&value.seq.view_bits::<Lsb0>()[..SEQ_BITS_LEN]);
        header.extend(&value.r#type.view_bits::<Lsb0>()[..TYPE_BITS_LEN]);

        header
    }
}

impl From<&BitSlice> for FrameHeader {
    fn from(value: &BitSlice) -> Self {
        Self {
            dest: DecodeToInt::decode(&value[0..ADDRESS_BITS_LEN]),
            src: DecodeToInt::decode(&value[ADDRESS_BITS_LEN..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN]),
            seq: DecodeToInt::decode(
                &value[ADDRESS_BITS_LEN + ADDRESS_BITS_LEN
                    ..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN],
            ),
            r#type: DecodeToInt::decode(
                &value[ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN
                    ..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN],
            ),
        }
    }
}

pub trait Frame: From<Self> + TryFrom<BitVec> {
    fn header(&self) -> &FrameHeader;
    fn payload(&self) -> Option<&BitSlice>;
}

#[derive(Debug, Clone)]
pub struct DataFrame {
    header: FrameHeader,
    payload: BitVec,
}

impl DataFrame {
    pub fn new(dest: usize, src: usize, seq: usize, payload: BitVec) -> Self {
        Self {
            header: FrameHeader {
                dest,
                src,
                seq,
                r#type: FrameType::Data.bits(),
            },
            payload,
        }
    }
}

impl Frame for DataFrame {
    fn header(&self) -> &FrameHeader {
        &self.header
    }

    fn payload(&self) -> Option<&BitSlice> {
        Some(&self.payload)
    }
}

impl From<DataFrame> for BitVec {
    fn from(value: DataFrame) -> Self {
        let mut frame = BitVec::from(value.header);
        frame.extend(value.payload);

        let bytes = DecodeToBytes::decode(&frame);
        let parity = PARITY_ALGORITHM.checksum(&bytes) as usize;
        frame.extend(&parity.view_bits::<Lsb0>()[..PARITY_BITS_LEN]);

        frame
    }
}

fn verify(bits: &BitSlice) -> Result<(), Error> {
    if bits.len()
        < ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN + PARITY_BITS_LEN
    {
        return Err(FrameDecodeError::FrameIsTooShort(
            bits.len(),
            ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN + PARITY_BITS_LEN,
        )
        .into());
    }

    let len = bits.len();
    let parity = DecodeToInt::<usize>::decode(&bits[len - PARITY_BITS_LEN..]);
    let bytes = DecodeToBytes::decode(&bits[..len - PARITY_BITS_LEN]);
    if PARITY_ALGORITHM.checksum(&bytes) as usize != parity {
        return Err(FrameDecodeError::ParityCheckFailed(
            parity,
            PARITY_ALGORITHM.checksum(&bytes) as usize,
        )
        .into());
    }

    Ok(())
}

impl TryFrom<BitVec> for DataFrame {
    type Error = Error;

    fn try_from(value: BitVec) -> Result<Self, Self::Error> {
        verify(&value)?;
        let header = FrameHeader::from(
            &value[..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN],
        );
        if header.r#type != FrameType::Data.bits() {
            return Err(FrameDecodeError::UnexpectedFrameType(
                header.r#type,
                FrameType::Data.bits(),
            )
            .into());
        }
        let payload = value[ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN
            ..value.len() - PARITY_BITS_LEN]
            .to_owned();
        Ok(Self { header, payload })
    }
}

#[derive(Debug, Clone)]
pub struct AckFrame {
    header: FrameHeader,
}

impl AckFrame {
    pub fn new(dest: usize, src: usize, seq: usize) -> Self {
        Self {
            header: FrameHeader {
                dest,
                src,
                seq,
                r#type: FrameType::Ack.bits(),
            },
        }
    }
}

impl Frame for AckFrame {
    fn header(&self) -> &FrameHeader {
        &self.header
    }

    fn payload(&self) -> Option<&BitSlice> {
        None
    }
}

impl From<AckFrame> for BitVec {
    fn from(value: AckFrame) -> Self {
        let mut frame = BitVec::from(value.header);

        let bytes = DecodeToBytes::decode(&frame);
        let parity = PARITY_ALGORITHM.checksum(&bytes) as usize;
        frame.extend(&parity.view_bits::<Lsb0>()[..PARITY_BITS_LEN]);

        frame
    }
}

impl TryFrom<BitVec> for AckFrame {
    type Error = Error;

    fn try_from(value: BitVec) -> Result<Self, Self::Error> {
        verify(&value)?;
        let header = FrameHeader::from(
            &value[..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN],
        );
        if header.r#type != FrameType::Ack.bits() {
            return Err(FrameDecodeError::UnexpectedFrameType(
                header.r#type,
                FrameType::Ack.bits(),
            )
            .into());
        }
        Ok(Self { header })
    }
}

pub enum AcsmaFrame {
    Data(DataFrame),
    Ack(AckFrame),
}

impl From<AcsmaFrame> for BitVec {
    fn from(value: AcsmaFrame) -> Self {
        match value {
            AcsmaFrame::Data(data) => data.into(),
            AcsmaFrame::Ack(ack) => ack.into(),
        }
    }
}

impl TryFrom<BitVec> for AcsmaFrame {
    type Error = Error;

    fn try_from(value: BitVec) -> Result<Self, Self::Error> {
        verify(&value)?;
        let header = FrameHeader::from(
            &value[..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN],
        );

        if header.r#type == FrameType::Data.bits() {
            let payload = value[ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN
                ..value.len() - PARITY_BITS_LEN]
                .to_owned();
            Ok(AcsmaFrame::Data(DataFrame { header, payload }))
        } else if header.r#type == FrameType::Ack.bits() {
            Ok(AcsmaFrame::Ack(AckFrame { header }))
        } else {
            Err(FrameDecodeError::UnknownFrameType(header.r#type).into())
        }
    }
}

impl Frame for AcsmaFrame {
    fn header(&self) -> &FrameHeader {
        match self {
            AcsmaFrame::Data(data) => data.header(),
            AcsmaFrame::Ack(ack) => ack.header(),
        }
    }

    fn payload(&self) -> Option<&BitSlice> {
        match self {
            AcsmaFrame::Data(data) => data.payload(),
            AcsmaFrame::Ack(ack) => ack.payload(),
        }
    }
}

pub enum NonAckFrame {
    Data(DataFrame),
}

impl From<NonAckFrame> for BitVec {
    fn from(value: NonAckFrame) -> Self {
        match value {
            NonAckFrame::Data(data) => data.into(),
        }
    }
}

impl TryFrom<BitVec> for NonAckFrame {
    type Error = Error;

    fn try_from(value: BitVec) -> Result<Self, Self::Error> {
        verify(&value)?;
        let header = FrameHeader::from(
            &value[..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN],
        );

        if header.r#type == FrameType::Data.bits() {
            let payload = value[ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN
                ..value.len() - PARITY_BITS_LEN]
                .to_owned();
            Ok(NonAckFrame::Data(DataFrame { header, payload }))
        } else if header.r#type == FrameType::Ack.bits() {
            return Err(FrameDecodeError::UnexpectedFrameType(
                header.r#type,
                FrameType::Data.bits(),
            )
            .into());
        } else {
            Err(FrameDecodeError::UnknownFrameType(header.r#type).into())
        }
    }
}

impl Frame for NonAckFrame {
    fn header(&self) -> &FrameHeader {
        match self {
            NonAckFrame::Data(data) => data.header(),
        }
    }

    fn payload(&self) -> Option<&BitSlice> {
        match self {
            NonAckFrame::Data(data) => data.payload(),
        }
    }
}

#[derive(Debug, Error)]
pub enum FrameDecodeError {
    #[error("Frame is too short (got {0}, expected {1})")]
    FrameIsTooShort(usize, usize),
    #[error("Parity check failed (got {0}, expected {1})")]
    ParityCheckFailed(usize, usize),
    #[error("Unexpected frame type (got {0}, expected {1})")]
    UnexpectedFrameType(usize, usize),
    #[error("Unknown frame type (got {0})")]
    UnknownFrameType(usize),
}
