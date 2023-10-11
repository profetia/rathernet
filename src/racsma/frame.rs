use super::builtin::{
    ADDRESS_BITS_LEN, PARITY_ALGORITHM, PARITY_BITS_LEN, SEQ_BITS_LEN, TYPE_BITS_LEN,
};
use crate::rather::encode::{DecodeToBytes, DecodeToInt};
use anyhow::{Error, Result};
use bitvec::prelude::*;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameType {
    Data = 0b0000_0000,
    Ack = 0b0000_0001,
    MacPingReq = 0b0000_0010,
    MacPingResp = 0b0000_0011,
}

impl FrameType {
    pub const fn bits(self) -> usize {
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

#[derive(Debug, Clone)]
pub struct MacPingReqFrame {
    header: FrameHeader,
}

impl MacPingReqFrame {
    pub fn new(dest: usize, src: usize) -> Self {
        Self {
            header: FrameHeader {
                dest,
                src,
                seq: 0,
                r#type: FrameType::MacPingReq.bits(),
            },
        }
    }
}

impl Frame for MacPingReqFrame {
    fn header(&self) -> &FrameHeader {
        &self.header
    }

    fn payload(&self) -> Option<&BitSlice> {
        None
    }
}

impl From<MacPingReqFrame> for BitVec {
    fn from(value: MacPingReqFrame) -> Self {
        let mut frame = BitVec::from(value.header);

        let bytes = DecodeToBytes::decode(&frame);
        let parity = PARITY_ALGORITHM.checksum(&bytes) as usize;
        frame.extend(&parity.view_bits::<Lsb0>()[..PARITY_BITS_LEN]);

        frame
    }
}

impl TryFrom<BitVec> for MacPingReqFrame {
    type Error = Error;

    fn try_from(value: BitVec) -> Result<Self, Self::Error> {
        verify(&value)?;
        let header = FrameHeader::from(
            &value[..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN],
        );
        if header.r#type != FrameType::MacPingReq.bits() {
            return Err(FrameDecodeError::UnexpectedFrameType(
                header.r#type,
                FrameType::MacPingReq.bits(),
            )
            .into());
        }
        Ok(Self { header })
    }
}

#[derive(Debug, Clone)]
pub struct MacPingRespFrame {
    header: FrameHeader,
}

impl MacPingRespFrame {
    pub fn new(dest: usize, src: usize) -> Self {
        Self {
            header: FrameHeader {
                dest,
                src,
                seq: 0,
                r#type: FrameType::MacPingResp.bits(),
            },
        }
    }
}

impl Frame for MacPingRespFrame {
    fn header(&self) -> &FrameHeader {
        &self.header
    }

    fn payload(&self) -> Option<&BitSlice> {
        None
    }
}

impl From<MacPingRespFrame> for BitVec {
    fn from(value: MacPingRespFrame) -> Self {
        let mut frame = BitVec::from(value.header);

        let bytes = DecodeToBytes::decode(&frame);
        let parity = PARITY_ALGORITHM.checksum(&bytes) as usize;
        frame.extend(&parity.view_bits::<Lsb0>()[..PARITY_BITS_LEN]);

        frame
    }
}

impl TryFrom<BitVec> for MacPingRespFrame {
    type Error = Error;

    fn try_from(value: BitVec) -> Result<Self, Self::Error> {
        verify(&value)?;
        let header = FrameHeader::from(
            &value[..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN],
        );
        if header.r#type != FrameType::MacPingResp.bits() {
            return Err(FrameDecodeError::UnexpectedFrameType(
                header.r#type,
                FrameType::MacPingResp.bits(),
            )
            .into());
        }
        Ok(Self { header })
    }
}

#[derive(Debug, Clone)]
pub enum AcsmaFrame {
    NonAck(NonAckFrame),
    Ack(AckFrame),
    MacPingResp(MacPingRespFrame),
}

impl From<AcsmaFrame> for BitVec {
    fn from(value: AcsmaFrame) -> Self {
        match value {
            AcsmaFrame::NonAck(data) => data.into(),
            AcsmaFrame::Ack(ack) => ack.into(),
            AcsmaFrame::MacPingResp(ping) => ping.into(),
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

        if header.r#type == FrameType::Ack.bits() {
            Ok(AcsmaFrame::Ack(AckFrame { header }))
        } else if header.r#type == FrameType::MacPingResp.bits() {
            Ok(AcsmaFrame::MacPingResp(MacPingRespFrame { header }))
        } else {
            Ok(AcsmaFrame::NonAck(NonAckFrame::try_from_bitvec_unchecked(
                value,
            )?))
        }
    }
}

impl Frame for AcsmaFrame {
    fn header(&self) -> &FrameHeader {
        match self {
            AcsmaFrame::NonAck(data) => data.header(),
            AcsmaFrame::Ack(ack) => ack.header(),
            AcsmaFrame::MacPingResp(ping) => ping.header(),
        }
    }

    fn payload(&self) -> Option<&BitSlice> {
        match self {
            AcsmaFrame::NonAck(data) => data.payload(),
            AcsmaFrame::Ack(ack) => ack.payload(),
            AcsmaFrame::MacPingResp(ping) => ping.payload(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum NonAckFrame {
    Data(DataFrame),
    MacPingReq(MacPingReqFrame),
}

impl NonAckFrame {
    fn try_from_bitvec_unchecked(value: BitVec) -> Result<Self> {
        let header = FrameHeader::from(
            &value[..ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN],
        );

        if header.r#type == FrameType::Data.bits() {
            let payload = value[ADDRESS_BITS_LEN + ADDRESS_BITS_LEN + SEQ_BITS_LEN + TYPE_BITS_LEN
                ..value.len() - PARITY_BITS_LEN]
                .to_owned();
            Ok(NonAckFrame::Data(DataFrame { header, payload }))
        } else if header.r#type == FrameType::MacPingReq.bits() {
            Ok(NonAckFrame::MacPingReq(MacPingReqFrame { header }))
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

impl From<NonAckFrame> for BitVec {
    fn from(value: NonAckFrame) -> Self {
        match value {
            NonAckFrame::Data(data) => data.into(),
            NonAckFrame::MacPingReq(ping) => ping.into(),
        }
    }
}

impl TryFrom<BitVec> for NonAckFrame {
    type Error = Error;

    fn try_from(value: BitVec) -> Result<Self, Self::Error> {
        verify(&value)?;
        Self::try_from_bitvec_unchecked(value)
    }
}

impl Frame for NonAckFrame {
    fn header(&self) -> &FrameHeader {
        match self {
            NonAckFrame::Data(data) => data.header(),
            NonAckFrame::MacPingReq(ping) => ping.header(),
        }
    }

    fn payload(&self) -> Option<&BitSlice> {
        match self {
            NonAckFrame::Data(data) => data.payload(),
            NonAckFrame::MacPingReq(ping) => ping.payload(),
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
