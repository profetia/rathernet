use super::{
    builtin::{
        PAYLOAD_BITS_LEN, SOCKET_ACK_TIMEOUT, SOCKET_FREE_THRESHOLD, SOCKET_MAX_BACKOFF,
        SOCKET_MAX_RESENDS, SOCKET_RECIEVE_TIMEOUT, SOCKET_SLOT_TIMEOUT,
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
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
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

pub struct AcsmaSocketReader {
    read_rx: UnboundedReceiver<NonAckFrame>,
}

impl AcsmaSocketReader {
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

pub struct AcsmaSocketWriter {
    config: AcsmaSocketConfig,
    write_tx: UnboundedSender<AcsmaSocketWriteTask>,
}

impl AcsmaSocketWriter {
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
            println!("Sending frame {}", index);
            let (tx, rx) = oneshot::channel();
            self.write_tx.send((NonAckFrame::Data(frame.clone()), tx))?;
            rx.await??;
            println!("Sent frame {}", index);
        }

        Ok(())
    }
}

type AcsmaSocketWriteTask = (NonAckFrame, Sender<Result<()>>);

pub struct AcsmaIoSocket;

impl AcsmaIoSocket {
    pub fn try_from_device(
        config: AcsmaSocketConfig,
        device: &AsioDevice,
    ) -> Result<(AcsmaSocketWriter, AcsmaSocketReader)> {
        let (read_tx, read_rx) = mpsc::unbounded_channel();
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
            write_rx,
        ));

        Ok((
            AcsmaSocketWriter { config, write_tx },
            AcsmaSocketReader { read_rx },
        ))
    }

    pub fn try_default(
        config: AcsmaSocketConfig,
    ) -> Result<(AcsmaSocketWriter, AcsmaSocketReader)> {
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
    mut write_rx: UnboundedReceiver<AcsmaSocketWriteTask>,
) -> Result<()> {
    let mut rng = SmallRng::from_entropy();
    let mut write_state: Option<AcsmaSocketWriteState> = None;
    loop {
        if let Ok(Some(bits)) = time::timeout(SOCKET_RECIEVE_TIMEOUT, read_ather.next()).await {
            println!("Got frame {}", bits.len());
            if let Ok(frame) = AcsmaFrame::try_from(bits) {
                let header = frame.header().clone();
                println!("Recieve frame with index {}", header.seq);
                if header.src == config.opponent && header.dest == config.address {
                    match frame {
                        AcsmaFrame::Ack(ack) => {
                            if let Some(state) = &write_state {
                                let header = ack.header();
                                if header.seq == state.task.0.header().seq {
                                    let state = write_state.take().unwrap();
                                    let (_, sender) = state.task;
                                    let _ = sender.send(Ok(()));
                                }
                            }
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
        } else if let Some(state) = &mut write_state {
            if state.timer.is_expired() {
                if state.timer.is_timeout() {
                    state.timer = AcsmaSocketWriteTimer::backoff(&mut rng, 0);
                } else if !is_channel_free(&mut write_monitor).await {
                    let result = state.timer.retry(&mut rng);
                    if result.is_err() {
                        let state = write_state.take().unwrap();
                        let (_, sender) = state.task;
                        let _ = sender.send(result);
                    }
                } else if state.resends > SOCKET_MAX_RESENDS {
                    let state = write_state.take().unwrap();
                    let (_, sender) = state.task;
                    let _ = sender.send(Err(AcsmaIoError::LinkError(state.resends).into()));
                } else {
                    let frame = state.task.0.clone();
                    write_ather.write(&Into::<BitVec>::into(frame)).await?;
                    state.resends += 1;
                    state.timer = AcsmaSocketWriteTimer::timeout();
                }
            }
        } else if write_state.is_none() {
            let result = write_rx.try_recv();
            if let Ok(task) = result {
                if !is_channel_free(&mut write_monitor).await {
                    write_state = Some(AcsmaSocketWriteState {
                        task,
                        resends: 0,
                        timer: AcsmaSocketWriteTimer::backoff(&mut rng, 0),
                    });
                } else {
                    let frame = task.0.clone();
                    write_ather.write(&Into::<BitVec>::into(frame)).await?;
                    write_state = Some(AcsmaSocketWriteState {
                        task,
                        resends: 0,
                        timer: AcsmaSocketWriteTimer::timeout(),
                    });
                }
            } else if let Err(TryRecvError::Disconnected) = result {
                if read_tx.is_closed() {
                    break;
                }
            }
        }
    }
    Ok(())
}

async fn is_channel_free(write_monitor: &mut AudioInputStream<f32>) -> bool {
    write_monitor.resume();
    if let Some(sample) = write_monitor.next().await {
        if sample
            .iter()
            .all(|&sample| sample.abs() < SOCKET_FREE_THRESHOLD)
        {
            write_monitor.suspend();
            return true;
        }
    }
    write_monitor.suspend();
    false
}

struct AcsmaSocketWriteState {
    task: AcsmaSocketWriteTask,
    resends: usize,
    timer: AcsmaSocketWriteTimer,
}

struct AcsmaSocketWriteTimer {
    start: Instant,
    r#type: AcsmaSocketWriteTimerType,
}

enum AcsmaSocketWriteTimerType {
    Timeout,
    Backoff(usize, Duration),
}

fn generate_backoff(rng: &mut SmallRng, factor: usize) -> Duration {
    rng.gen_range(0..=(1 << (factor - 1))) * SOCKET_SLOT_TIMEOUT
}

impl AcsmaSocketWriteTimer {
    fn timeout() -> Self {
        Self {
            start: Instant::now(),
            r#type: AcsmaSocketWriteTimerType::Timeout,
        }
    }

    fn backoff(rng: &mut SmallRng, retry: usize) -> Self {
        let duration = generate_backoff(rng, retry);
        Self {
            start: Instant::now(),
            r#type: AcsmaSocketWriteTimerType::Backoff(retry, duration),
        }
    }
}

impl AcsmaSocketWriteTimer {
    fn is_expired(&self) -> bool {
        match self.r#type {
            AcsmaSocketWriteTimerType::Timeout => self.start.elapsed() > SOCKET_ACK_TIMEOUT,
            AcsmaSocketWriteTimerType::Backoff(_, duration) => self.start.elapsed() > duration,
        }
    }

    fn retry(&mut self, rng: &mut SmallRng) -> Result<()> {
        match &mut self.r#type {
            AcsmaSocketWriteTimerType::Timeout => {}
            AcsmaSocketWriteTimerType::Backoff(retry, duration) => {
                *retry += 1;
                if *retry > SOCKET_MAX_BACKOFF {
                    return Err(AcsmaIoError::LinkError(*retry).into());
                }
                *duration = generate_backoff(rng, *retry);
            }
        }
        Ok(())
    }

    fn is_timeout(&self) -> bool {
        matches!(self.r#type, AcsmaSocketWriteTimerType::Timeout)
    }
}
