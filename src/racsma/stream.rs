use super::{
    builtin::{PAYLOAD_BITS_LEN, SOCKET_ACK_TIMEOUT, SOCKET_MAX_RESENDS},
    frame::{AckFrame, DataFrame, Frame},
    socket::AcsmaIoError,
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
        let frames = bits
            .chunks(PAYLOAD_BITS_LEN)
            .enumerate()
            .map(|(index, chunk)| {
                Into::<BitVec>::into(DataFrame::new(
                    dest,
                    self.config.address,
                    index,
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

    pub async fn read(&mut self, src: usize, buf: &mut BitSlice) -> Result<()> {
        let (mut bucket, mut total_len) = (BTreeMap::new(), 0usize);
        while let Some(bits) = self.istream.next().await {
            // log::debug!("Got frame {}", bits.len());
            if let Ok(frame) = DataFrame::try_from(bits) {
                let header = frame.header();
                if header.src == src && header.dest == self.config.address {
                    // log::debug!("Recieve frame with index {}", header.seq);
                    let ack = AckFrame::new(header.src, header.dest, header.seq);
                    // log::debug!("Sending ACK for index {}", header.seq);
                    self.ostream.write(&Into::<BitVec>::into(ack)).await?;
                    // log::debug!(
                    //     "Sent ACK for index {}, total recieved {}",
                    //     header.seq,
                    //     total_len
                    // );

                    let payload = frame.payload().unwrap();
                    bucket.entry(header.seq).or_insert_with(|| {
                        log::info!("Read frame with index {}", header.seq);
                        total_len += payload.len();
                        payload.to_owned()
                    });

                    if total_len >= buf.len() {
                        break;
                    }
                }
            }
        }

        log::info!("Read {} bits", total_len);

        buf.copy_from_bitslice(
            &bucket
                .into_iter()
                .fold(BitVec::new(), |mut acc, (_, payload)| {
                    acc.extend_from_bitslice(&payload);
                    acc
                })[..buf.len()],
        );

        Ok(())
    }
}
