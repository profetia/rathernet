use super::{
    builtin::{PAYLOAD_BITS_LEN, SOCKET_ACK_TIMEOUT, SOCKET_MAX_RESENDS},
    frame::{AckFrame, DataFrame, Frame, FrameFlag},
    AcsmaIoError,
};
use crate::rather::{AtherInputStream, AtherOutputStream};
use anyhow::Result;
use bitvec::prelude::*;
use log;
use std::collections::BTreeMap;
use tokio::time;
use tokio_stream::StreamExt;

#[derive(Clone)]
pub struct AcsmaStreamConfig {
    pub address: usize,
}

impl AcsmaStreamConfig {
    pub fn new(address: usize) -> Self {
        Self { address }
    }
}

pub struct AcsmaIoStream {
    config: AcsmaStreamConfig,
    istream: AtherInputStream,
    ostream: AtherOutputStream,
}

impl AcsmaIoStream {
    pub fn new(
        config: AcsmaStreamConfig,
        istream: AtherInputStream,
        ostream: AtherOutputStream,
    ) -> Self {
        Self {
            config,
            istream,
            ostream,
        }
    }
}

impl AcsmaIoStream {
    pub async fn write(&mut self, dest: usize, bits: &BitSlice) -> Result<()> {
        let frames = bits.chunks(PAYLOAD_BITS_LEN);
        let len = frames.len();
        let frames = frames.enumerate().map(|(index, chunk)| {
            let flag = if index == len - 1 {
                FrameFlag::EOP
            } else {
                FrameFlag::empty()
            };
            Into::<BitVec>::into(DataFrame::new(
                dest,
                self.config.address,
                index,
                flag,
                chunk.to_owned(),
            ))
        });

        for (index, frame) in frames.enumerate() {
            let mut retry = 0usize;
            log::info!("Writing frame {}", index);
            loop {
                // log::debug!("Sending frame {} for the {} time", index, retry);
                self.ostream.write(&frame).await?;
                // log::debug!("Sent frame {} for the {} time", index, retry);
                let ack_future = async {
                    while let Some(bits) = self.istream.next().await {
                        if let Ok(frame) = AckFrame::try_from(bits) {
                            let header = frame.header();
                            if header.src == dest
                                && header.dest == self.config.address
                                && header.seq == index
                            {
                                // log::debug!("Recieve ACK for index {}", header.seq);
                                break;
                            }
                        }
                    }
                };
                if time::timeout(SOCKET_ACK_TIMEOUT, ack_future).await.is_ok() {
                    break;
                } else {
                    // log::debug!("Timeout ACK for index");
                    retry += 1;
                    if retry >= SOCKET_MAX_RESENDS {
                        return Err(AcsmaIoError::LinkError(retry).into());
                    }
                }
            }
            log::info!("Wrote frame {}", index);
        }

        Ok(())
    }

    pub async fn read(&mut self, src: usize) -> Result<BitVec> {
        let mut bucket = BTreeMap::new();
        while let Some(bits) = self.istream.next().await {
            // log::debug!("Got frame {}", bits.len());
            if let Ok(frame) = DataFrame::try_from(bits) {
                let header = frame.header();
                if header.src == src && header.dest == self.config.address {
                    // log::debug!("Recieve frame with index {}", header.seq);
                    let ack = AckFrame::new(header.src, header.dest, header.seq);
                    // log::debug!("Sending ACK for index {}", header.seq);
                    self.ostream.write(&Into::<BitVec>::into(ack)).await?;

                    let payload = frame.payload().unwrap();
                    bucket.entry(header.seq).or_insert(payload.to_owned());

                    if header.flag.contains(FrameFlag::EOP) {
                        break;
                    }
                }
            }
        }

        let result = bucket.iter().fold(bitvec![], |mut acc, (_, payload)| {
            acc.extend_from_bitslice(payload);
            acc
        });

        log::info!("Read {} frames, total {}", bucket.len(), result.len());

        Ok(result)
    }
}
