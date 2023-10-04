use super::{
    builtin::{
        ACK_LINK_ERROR_THRESHOLD, ACK_RECIEVE_TIMEOUT, PAYLOAD_BITS_LEN, SOCKET_FRAMING_TIMEOUT,
    },
    frame::{AckFrame, AcsmaFrame, DataFrame, Frame, NonAckFrame},
};
use crate::{
    rather::{AtherInputStream, AtherOutputStream, AtherStreamConfig},
    raudio::{AsioDevice, AudioInputStream, AudioOutputStream},
};
use anyhow::Result;
use bitvec::prelude::*;
use std::collections::BTreeMap;
use thiserror::Error;
use tokio::{
    sync::{
        mpsc::{self, error::TryRecvError, UnboundedReceiver, UnboundedSender},
        oneshot::{self, Sender},
    },
    time,
};
use tokio_stream::StreamExt;

#[derive(Clone)]
pub struct AcsmaSocketConfig {
    pub address: usize,
    pub opponent: usize,
    pub ather_config: AtherStreamConfig,
}

impl AcsmaSocketConfig {
    pub fn new(address: usize, opponent: usize, ather_config: AtherStreamConfig) -> Self {
        Self {
            address,
            opponent,
            ather_config,
        }
    }
}

#[derive(Debug, Error)]
pub enum AcsmaIoError {
    #[error("Link error after {0} retries")]
    LinkError(usize),
}

pub struct AcsmaIoSocketReader {
    read_rx: UnboundedReceiver<NonAckFrame>,
}

impl AcsmaIoSocketReader {
    pub async fn read(&mut self, buf: &mut BitSlice) -> Result<()> {
        let (mut bucket, mut total_len) = (BTreeMap::new(), 0usize);
        while let Some(frame) = self.read_rx.recv().await {
            let header = frame.header().clone();
            println!("Receive frame {}", header.r#type);
            match frame {
                NonAckFrame::Data(data) => {
                    let payload = data.payload().unwrap();
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

pub struct AcsmaIoSocketWriter {
    config: AcsmaSocketConfig,
    ack_rx: UnboundedReceiver<AckFrame>,
    write_tx: UnboundedSender<(NonAckFrame, Sender<()>)>,
}

impl AcsmaIoSocketWriter {
    pub async fn write(&mut self, bits: &BitSlice) -> Result<()> {
        let frames = bits
            .chunks(PAYLOAD_BITS_LEN)
            .enumerate()
            .map(|(index, chunk)| {
                DataFrame::new(
                    self.config.opponent,
                    self.config.address,
                    index,
                    chunk.to_owned(),
                )
            });

        for (index, frame) in frames.enumerate() {
            let mut retry = 0usize;
            loop {
                println!("Sending frame {}", index);
                let (tx, rx) = oneshot::channel();
                self.write_tx.send((NonAckFrame::Data(frame.clone()), tx))?;
                rx.await?;
                println!("Sent frame {}", index);

                let ack_future = async {
                    if let Some(ack) = self.ack_rx.recv().await {
                        let header = ack.header();
                        println!("Recieve ACK for index {}", header.seq);
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
}

pub struct AcsmaIoSocket;

impl AcsmaIoSocket {
    pub fn try_from_device(
        config: AcsmaSocketConfig,
        device: &AsioDevice,
    ) -> Result<(AcsmaIoSocketWriter, AcsmaIoSocketReader)> {
        let (read_tx, read_rx) = mpsc::unbounded_channel();
        let (ack_tx, ack_rx) = mpsc::unbounded_channel();
        let (write_tx, write_rx) = mpsc::unbounded_channel();

        tokio::spawn(socket_daemon(
            config.clone(),
            AtherInputStream::new(
                config.ather_config.clone(),
                AudioInputStream::try_from_device_config(
                    device,
                    config.ather_config.stream_config.clone(),
                )?,
            ),
            AtherOutputStream::new(
                config.ather_config.clone(),
                AudioOutputStream::try_from_device_config(
                    device,
                    config.ather_config.stream_config.clone(),
                )?,
            ),
            AudioInputStream::try_from_device_config(
                device,
                config.ather_config.stream_config.clone(),
            )?,
            read_tx,
            ack_tx,
            write_rx,
        ));

        Ok((
            AcsmaIoSocketWriter {
                config,
                ack_rx,
                write_tx,
            },
            AcsmaIoSocketReader { read_rx },
        ))
    }

    pub fn try_default(
        config: AcsmaSocketConfig,
    ) -> Result<(AcsmaIoSocketWriter, AcsmaIoSocketReader)> {
        let device = AsioDevice::try_default()?;
        Self::try_from_device(config, &device)
    }
}

async fn socket_daemon(
    config: AcsmaSocketConfig,
    mut read_ather: AtherInputStream,
    write_ather: AtherOutputStream,
    write_monitor: AudioInputStream<f32>,
    read_tx: UnboundedSender<NonAckFrame>,
    ack_tx: UnboundedSender<AckFrame>,
    mut write_rx: UnboundedReceiver<(NonAckFrame, Sender<()>)>,
) -> Result<()> {
    loop {
        match write_rx.try_recv() {
            Ok((frame, sender)) => {
                let bits = Into::<BitVec>::into(frame);
                write_ather.write(&bits).await?;
                let _ = sender.send(());
            }
            Err(TryRecvError::Disconnected) if ack_tx.is_closed() && read_tx.is_closed() => {
                break;
            }
            _ => match time::timeout(SOCKET_FRAMING_TIMEOUT, read_ather.next()).await {
                Ok(Some(bits)) => {
                    println!("Got frame {}", bits.len());
                    if let Ok(frame) = AcsmaFrame::try_from(bits) {
                        let header = frame.header().clone();
                        println!("Recieve frame with index {}", header.seq);
                        if header.src == config.opponent && header.dest == config.address {
                            match frame {
                                AcsmaFrame::Ack(ack) => {
                                    let _ = ack_tx.send(ack);
                                }
                                AcsmaFrame::Data(data) => {
                                    let ack = AckFrame::new(header.dest, header.src, header.seq);
                                    println!("Sending ACK for index {}", header.seq);
                                    write_ather.write(&Into::<BitVec>::into(ack)).await?;
                                    println!("Sent ACK for index {}", header.seq);
                                    let _ = read_tx.send(NonAckFrame::Data(data));
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
        }
    }
    Ok(())
}
