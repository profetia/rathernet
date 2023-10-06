use super::{
    builtin::{
        ACK_LINK_ERROR_THRESHOLD, ACK_RECIEVE_TIMEOUT, PAYLOAD_BITS_LEN,
        SOCKET_BACKOFF_ERROR_THRESHOLD, SOCKET_BACKOFF_WAIT_THRESHOLD, SOCKET_FREE_THRESHOLD,
        SOCKET_RECIEVE_TIMEOUT, SOCKET_SLOT_TIMEOUT,
    },
    frame::{AckFrame, AcsmaFrame, DataFrame, Frame, NonAckFrame},
};
use crate::{
    rather::{AtherInputStream, AtherOutputStream, AtherStreamConfig},
    raudio::{AsioDevice, AudioInputStream, AudioOutputStream, ContinuousStream},
};
use anyhow::Result;
use bitvec::prelude::*;
use rand::{rngs::SmallRng, Rng, SeedableRng};
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
            println!("Receive frame {}, total {}", header.seq, total_len);
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
    write_tx: UnboundedSender<AcsmaIoSocketWriteTask>,
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
                rx.await??;
                println!("Sent frame {}", index);

                let ack_future = async {
                    while let Some(ack) = self.ack_rx.recv().await {
                        let header = ack.header();
                        if header.seq == index {
                            println!("Recieve ACK for index {}", header.seq);
                            break;
                        }
                    }
                };
                if time::timeout(ACK_RECIEVE_TIMEOUT, ack_future).await.is_ok() {
                    break;
                } else {
                    println!("Timeout ACK for index {}", index);
                    retry += 1;
                    if retry >= ACK_LINK_ERROR_THRESHOLD {
                        println!("Link error:  ACK FAILED at {}", index);
                        return Err(AcsmaIoError::LinkError(retry).into());
                    }
                }
            }
        }

        Ok(())
    }
}

type AcsmaIoSocketWriteTask = (NonAckFrame, Sender<Result<()>>);

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
    mut write_monitor: AudioInputStream<f32>,
    read_tx: UnboundedSender<NonAckFrame>,
    ack_tx: UnboundedSender<AckFrame>,
    mut write_rx: UnboundedReceiver<AcsmaIoSocketWriteTask>,
) -> Result<()> {
    let mut rng = SmallRng::from_entropy();
    loop {
        match write_rx.try_recv() {
            Ok(task) => {
                write_frame(&mut rng, &write_ather, &mut write_monitor, task).await?;
            }
            Err(TryRecvError::Disconnected) if ack_tx.is_closed() && read_tx.is_closed() => {
                break;
            }
            _ => {
                read_ather.resume();
                if let Ok(Some(bits)) =
                    time::timeout(SOCKET_RECIEVE_TIMEOUT, read_ather.next()).await
                {
                    println!("Got frame {}", bits.len());
                    if let Ok(frame) = AcsmaFrame::try_from(bits) {
                        let header = frame.header().clone();
                        println!("Recieve frame with index {}", header.seq);
                        if header.src == config.opponent && header.dest == config.address {
                            match frame {
                                AcsmaFrame::Ack(ack) => {
                                    let _ = ack_tx.send(ack);
                                }
                                AcsmaFrame::NonAck(data) => {
                                    let ack = AckFrame::new(header.src, header.dest, header.seq);
                                    println!("Sending ACK for index {}", header.seq);
                                    write_ather.write(&Into::<BitVec>::into(ack)).await?;
                                    println!("Sent ACK for index {}", header.seq);
                                    let _ = read_tx.send(data);
                                }
                            }
                        }
                    }
                }
                read_ather.suspend();
            }
        }
    }
    Ok(())
}

async fn write_frame(
    rng: &mut SmallRng,
    write_ather: &AtherOutputStream,
    write_monitor: &mut AudioInputStream<f32>,
    (frame, sender): AcsmaIoSocketWriteTask,
) -> Result<()> {
    let bits = Into::<BitVec>::into(frame);
    // println!("Waiting for the channel to be free");
    let mut m = 0;
    loop {
        write_monitor.resume();
        if let Some(sample) = write_monitor.next().await {
            if sample
                .iter()
                .all(|&sample| sample.abs() < SOCKET_FREE_THRESHOLD)
            {
                break;
            } else {
                // println!("Ooops, not free");
                write_monitor.suspend();
                m += 1;
                if m >= SOCKET_BACKOFF_ERROR_THRESHOLD {
                    let _ = sender.send(Err(AcsmaIoError::LinkError(m).into()));
                    return Ok(());
                } else {
                    let upper_bound = if m > SOCKET_BACKOFF_WAIT_THRESHOLD {
                        1024
                    } else {
                        1 << (m - 1)
                    };
                    let k = rng.gen_range(0..=upper_bound);
                    // println!("Waiting for {} slots", k);
                    time::sleep(SOCKET_SLOT_TIMEOUT * k).await;
                }
            }
        }
    }

    println!("The channel is free");
    write_ather.write(&bits).await?;
    let _ = sender.send(Ok(()));

    Ok(())
}
