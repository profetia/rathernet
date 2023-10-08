use super::{
    builtin::{
        PAYLOAD_BITS_LEN, SOCKET_ACK_TIMEOUT, SOCKET_FREE_THRESHOLD, SOCKET_MAX_BACKOFF,
        SOCKET_MAX_RESENDS, SOCKET_RECIEVE_TIMEOUT, SOCKET_SLOT_TIMEOUT,
    },
    frame::{AckFrame, AcsmaFrame, DataFrame, Frame, NonAckFrame},
};
use crate::{
    rather::{AtherInputStream, AtherOutputStream, AtherStreamConfig, signal::Energy},
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
            println!("Begin frame {}", index);
            let (tx, rx) = oneshot::channel();
            self.write_tx.send((NonAckFrame::Data(frame), tx))?;
            rx.await??;
            println!("End frame (ACK checked) {}", index);
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
        println!("----------State machine loop----------");
        match &write_state {
            Some(state) => {
                println!(
                    "Timer is has elapsed {}",
                    state.timer.start.elapsed().as_millis()
                );
                println!(
                    "Expect to elapse {}",
                    match &state.timer.r#type {
                        AcsmaSocketWriteTimerType::Timeout => SOCKET_ACK_TIMEOUT.as_millis(),
                        AcsmaSocketWriteTimerType::Backoff(_, duration) => duration.as_millis(),
                    }
                );
            }
            None => {
                println!("Timer is None")
            }
        }
        if let Ok(Some(bits)) = time::timeout(SOCKET_RECIEVE_TIMEOUT, read_ather.next()).await {
            println!("Got frame len: {}", bits.len());
            if let Ok(frame) = AcsmaFrame::try_from(bits) {
                let header = frame.header().clone();
                println!("Recieve raw frame with index {}", header.seq);
                if header.src == config.opponent && header.dest == config.address {
                    match frame {
                        AcsmaFrame::Ack(ack) => {
                            println!("Recieve ACK for index {}", header.seq);
                            if let Some(state) = &write_state {
                                let header = ack.header();
                                if header.seq == state.task.0.header().seq {
                                    println!("Clear ACK timout {}", header.seq);
                                    let state = write_state.take().unwrap();
                                    let (_, sender) = state.task;
                                    let _ = sender.send(Ok(()));
                                }
                            }
                        }
                        AcsmaFrame::NonAck(data) => {
                            println!("Receive data for index {}", header.seq);
                            let ack = AckFrame::new(header.src, header.dest, header.seq);
                            println!("Sending ACK for index {}", header.seq);
                            write_ather.write(&Into::<BitVec>::into(ack)).await?;
                            println!("Sent ACK for index {}", header.seq);
                            let _ = read_tx.send(data);
                        }
                    }
                } else {
                    println!("Recieve frame but not for me");
                }
            } else {
                println!("Recieve frame but checksum failed");
            }
        }

        if let Some(state) = &mut write_state {
            if state.timer.is_expired() {
                if state.timer.is_timeout() {
                    println!("ACK timer expired for frame {}", state.task.0.header().seq);
                    state.timer = AcsmaSocketWriteTimer::backoff(&mut rng, 0);
                } else if !is_channel_free(&config, &mut write_monitor).await {
                    println!(
                        "Backoff timer expired. Medium state: busy. {}",
                        state.task.0.header().seq
                    );
                    state.timer.retry(&mut rng);
                } else if state.resends > SOCKET_MAX_RESENDS {
                    println!(
                        "Backoff timer expired. Medium state: free. resends exceeded {}",
                        state.task.0.header().seq
                    );
                    let state = write_state.take().unwrap();
                    let (_, sender) = state.task;
                    let _ = sender.send(Err(AcsmaIoError::LinkError(state.resends).into()));
                } else {
                    println!(
                        "Backoff timer expired. Medium state: free. Resending {}",
                        state.task.0.header().seq
                    );
                    let frame = state.task.0.clone();
                    write_ather.write(&Into::<BitVec>::into(frame)).await?;
                    println!(
                        "Backoff timer expired. Medium state: free. Resent {}",
                        state.task.0.header().seq
                    );
                    state.resends += 1;
                    state.timer = AcsmaSocketWriteTimer::timeout();
                }
            }
        } else {
            let result = write_rx.try_recv();
            if let Ok(task) = result {
                println!(
                    "Accepted frame from source with index {}",
                    task.0.header().seq
                );
                if !is_channel_free(&config, &mut write_monitor).await {
                    println!("Medium state: busy. set backoff timer");
                    write_state = Some(AcsmaSocketWriteState {
                        task,
                        resends: 0,
                        timer: AcsmaSocketWriteTimer::backoff(&mut rng, 0),
                    });
                } else {
                    println!("Medium state: free. Sending {}", task.0.header().seq);
                    let frame = task.0.clone();
                    write_ather.write(&Into::<BitVec>::into(frame)).await?;
                    println!("Medium state: free. Sent {}", task.0.header().seq);
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

async fn is_channel_free(config: &AcsmaSocketConfig, write_monitor: &mut AudioInputStream<f32>) -> bool {
    let sample_rate = config.ather_config.stream_config.sample_rate().0;
    write_monitor.resume();
    if let Some(sample) = write_monitor.next().await {
        println!("Energy: {}", sample.energy(sample_rate));
        if sample.energy(sample_rate) < SOCKET_FREE_THRESHOLD
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
    let k = rng.gen_range(0..(1 << factor) as u32);
    println!("Set timer to {} slots by {}", k, factor);
    k * SOCKET_SLOT_TIMEOUT
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

    fn retry(&mut self, rng: &mut SmallRng) {
        self.start = Instant::now();
        match &mut self.r#type {
            AcsmaSocketWriteTimerType::Timeout => {}
            AcsmaSocketWriteTimerType::Backoff(retry, duration) => {
                *retry += 1;
                if *retry > SOCKET_MAX_BACKOFF {
                    *duration = generate_backoff(rng, SOCKET_MAX_BACKOFF);
                } else {
                    *duration = generate_backoff(rng, *retry);
                }
            }
        }
    }

    fn is_timeout(&self) -> bool {
        matches!(self.r#type, AcsmaSocketWriteTimerType::Timeout)
    }
}
