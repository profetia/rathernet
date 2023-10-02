use super::{
    builtin::{ACK_LINK_ERROR_THRESHOLD, ACK_RECIEVE_TIMEOUT, PAYLOAD_BITS_LEN},
    frame::{AckFrame, DataFrame, Frame},
};
use crate::rather::{AtherInputStream, AtherOutputStream};
use anyhow::Result;
use bitvec::prelude::*;
use std::collections::BTreeMap;
use thiserror::Error;
use tokio::time;
use tokio_stream::StreamExt;

#[derive(Debug, Clone)]
pub struct AcsmaIoConfig {
    pub address: usize,
}

impl AcsmaIoConfig {
    pub fn new(address: usize) -> Self {
        Self { address }
    }
}

pub struct AcsmaIoStream {
    config: AcsmaIoConfig,
    istream: AtherInputStream,
    ostream: AtherOutputStream,
}

impl AcsmaIoStream {
    pub fn new(
        config: AcsmaIoConfig,
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
            loop {
                println!("Sending frame {}", index);
                self.ostream.write(&frame).await;
                println!("Sent frame {}", index);
                let ack_future = async {
                    while let Some(bits) = self.istream.next().await {
                        if let Ok(frame) = AckFrame::try_from(bits) {
                            let header = frame.header();
                            if header.src == dest
                                && header.dest == self.config.address
                                && header.seq == index
                            {
                                println!("Recieve ACK for index {}", header.seq);
                                break;
                            }
                        }
                    }
                };
                if time::timeout(ACK_RECIEVE_TIMEOUT, ack_future).await.is_ok() {
                    break;
                } else {
                    println!("Timeout ACK for index");
                    retry += 1;
                    if retry >= ACK_LINK_ERROR_THRESHOLD {
                        return Err(AcsmaIoError::LinkError(retry).into());
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn read(&mut self, src: usize, buf: &mut BitSlice) -> Result<()> {
        let (mut bucket, mut total_len) = (BTreeMap::new(), 0usize);
        while let Some(bits) = self.istream.next().await {
            println!("Got frame {}", bits.len());
            if let Ok(frame) = DataFrame::try_from(bits) {
                let header = frame.header();
                if header.src == src && header.dest == self.config.address {
                    println!("Recieve frame with index {}", header.seq);
                    let ack = AckFrame::new(header.dest, header.src, header.seq);
                    println!("Sending ACK for index {}", header.seq);
                    self.ostream.write(&Into::<BitVec>::into(ack)).await;
                    println!(
                        "Sent ACK for index {}, total recieved {}",
                        header.seq, total_len
                    );

                    let payload = frame.payload().unwrap();
                    bucket.entry(header.seq).or_insert_with(|| {
                        total_len += payload.len();
                        payload.to_owned()
                    });

                    if total_len >= buf.len() {
                        break;
                    }
                }
            }
        }

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

#[derive(Debug, Error)]
pub enum AcsmaIoError {
    #[error("Link error after {0} retries")]
    LinkError(usize),
}
