use super::{
    builtin::{
        PAYLOAD_BITS_LEN, SOCKET_ACK_TIMEOUT, SOCKET_FREE_THRESHOLD, SOCKET_MAX_RANGE,
        SOCKET_MAX_RESENDS, SOCKET_RECIEVE_TIMEOUT, SOCKET_SLOT_TIMEOUT,
    },
    frame::{AckFrame, AcsmaFrame, DataFrame, Frame, NonAckFrame},
};
use crate::{
    rather::{signal::Energy, AtherInputStream, AtherOutputStream, AtherStreamConfig},
    raudio::{AsioDevice, AudioInputStream, AudioOutputStream},
};
use anyhow::Result;
use bitvec::prelude::*;
use log;
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
    pub ather_config: AtherStreamConfig,
}

impl AcsmaSocketConfig {
    pub fn new(address: usize, ather_config: AtherStreamConfig) -> Self {
        Self {
            address,
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
    pub async fn read(&mut self, src: usize, buf: &mut BitSlice) -> Result<()> {
        let (mut bucket, mut total_len) = (BTreeMap::new(), 0usize);
        while let Some(frame) = self.read_rx.recv().await {
            let header = frame.header().clone();
            log::info!("Receive frame {}, total {}", header.seq, total_len);
            if src == header.src {
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
        }

        log::info!("Read {} frames, total {}", bucket.len(), total_len);

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
    pub async fn write(&mut self, dest: usize, bits: &BitSlice) -> Result<()> {
        let frames = bits
            .chunks(PAYLOAD_BITS_LEN)
            .enumerate()
            .map(|(index, chunk)| {
                DataFrame::new(dest, self.config.address, index, chunk.to_owned())
            });

        for (index, frame) in frames.enumerate() {
            log::info!("Writing frame {}", index);
            let (tx, rx) = oneshot::channel();
            self.write_tx.send((NonAckFrame::Data(frame), tx))?;
            rx.await??;
            log::info!("Wrote frame (ACK checked) {}", index);
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
    write_monitor: AudioInputStream<f32>,
    read_tx: UnboundedSender<NonAckFrame>,
    mut write_rx: UnboundedReceiver<AcsmaSocketWriteTask>,
) -> Result<()> {
    let mut rng = SmallRng::from_entropy();
    let mut write_state: Option<AcsmaSocketWriteTimer> = None;
    let mut write_monitor = AcsmaSocketWriteMonitor::new(write_monitor);
    loop {
        log::debug!("----------State machine loop----------");
        match &write_state {
            Some(timer) => {
                log::debug!("Timer is has elapsed {}", timer.elapsed().as_millis());
                log::debug!("Expect to elapse {}", timer.duration().as_millis());
            }
            None => {
                log::debug!("Timer is None")
            }
        }
        if let Ok(Some(bits)) = time::timeout(SOCKET_RECIEVE_TIMEOUT, read_ather.next()).await {
            log::debug!("Got frame len: {}", bits.len());
            if let Ok(frame) = AcsmaFrame::try_from(bits) {
                let header = frame.header().clone();
                log::debug!("Recieve raw frame with index {}", header.seq);
                if header.dest == config.address {
                    match frame {
                        AcsmaFrame::Ack(ack) => {
                            log::debug!("Recieve ACK for index {}", header.seq);
                            match write_state {
                                Some(AcsmaSocketWriteTimer::Timeout { start, inner }) => {
                                    let ack_seq = ack.header().seq;
                                    let task_seq = inner.task.0.header().seq;
                                    write_state = if ack_seq == task_seq {
                                        log::debug!("Clear ACK timout {}", ack_seq);
                                        let _ = inner.task.1.send(Ok(()));
                                        let duration = generate_backoff(&mut rng, 0);
                                        Some(AcsmaSocketWriteTimer::backoff(None, 0, duration))
                                    } else {
                                        Some(AcsmaSocketWriteTimer::Timeout { start, inner })
                                    }
                                }
                                Some(AcsmaSocketWriteTimer::Backoff {
                                    inner: Some(inner),
                                    start,
                                    retry,
                                    duration,
                                }) => {
                                    let ack_seq = ack.header().seq;
                                    let task_seq = inner.task.0.header().seq;
                                    write_state = if ack_seq == task_seq {
                                        log::debug!("Clear Backoff timout {}", ack_seq);
                                        let _ = inner.task.1.send(Ok(()));
                                        let duration = generate_backoff(&mut rng, 0);
                                        Some(AcsmaSocketWriteTimer::backoff(None, 0, duration))
                                    } else {
                                        Some(AcsmaSocketWriteTimer::Backoff {
                                            start,
                                            inner: Some(inner),
                                            retry,
                                            duration,
                                        })
                                    }
                                }
                                _ => {}
                            }
                            if let Some(AcsmaSocketWriteTimer::Timeout { start, inner }) =
                                write_state
                            {
                                let ack_seq = ack.header().seq;
                                let task_seq = inner.task.0.header().seq;
                                if ack_seq == task_seq {
                                    log::debug!("Clear ACK timout {}", ack_seq);
                                    let _ = inner.task.1.send(Ok(()));
                                    let duration = generate_backoff(&mut rng, 0);
                                    write_state =
                                        Some(AcsmaSocketWriteTimer::backoff(None, 0, duration));
                                } else {
                                    write_state =
                                        Some(AcsmaSocketWriteTimer::Timeout { start, inner })
                                }
                            }
                        }
                        AcsmaFrame::NonAck(data) => {
                            log::debug!("Receive data for index {}", header.seq);
                            let ack = AckFrame::new(header.src, header.dest, header.seq);
                            log::debug!("Sending ACK for index {}", header.seq);
                            write_ather.write(&Into::<BitVec>::into(ack)).await?;
                            log::debug!("Sent ACK for index {}", header.seq);
                            let _ = read_tx.send(data);
                        }
                    }
                } else {
                    log::debug!("Recieve frame but not for me");
                }
            } else {
                log::debug!("Recieve frame but checksum failed");
            }
        }

        if let Some(timer) = write_state {
            if timer.is_expired() {
                write_state = match timer {
                    AcsmaSocketWriteTimer::Timeout { start: _, inner } => {
                        log::debug!("ACK timer expired for frame {}", inner.task.0.header().seq);
                        let duration = generate_backoff(&mut rng, 0);
                        Some(AcsmaSocketWriteTimer::backoff(
                            Some(inner.task),
                            0,
                            duration,
                        ))
                    }
                    AcsmaSocketWriteTimer::Backoff {
                        start: _,
                        inner,
                        mut retry,
                        duration: _,
                    } => match inner {
                        Some(inner) => {
                            if !is_channel_free(&config, &mut write_monitor).await {
                                log::debug!(
                                    "Backoff timer expired. Medium state: busy. {}",
                                    inner.task.0.header().seq
                                );
                                retry += 1;
                                let duration = generate_backoff(&mut rng, retry);
                                Some(AcsmaSocketWriteTimer::backoff(
                                    Some(inner.task),
                                    retry,
                                    duration,
                                ))
                            } else if inner.resends > SOCKET_MAX_RESENDS {
                                log::debug!(
                                    "Backoff timer expired. Medium state: free. resends exceeded {}",
                                    inner.task.0.header().seq
                                );
                                let _ = inner
                                    .task
                                    .1
                                    .send(Err(AcsmaIoError::LinkError(inner.resends).into()));
                                None
                            } else {
                                log::debug!(
                                    "Backoff timer expired. Medium state: free. Resending {}",
                                    inner.task.0.header().seq
                                );
                                let frame = inner.task.0.clone();
                                write_ather.write(&Into::<BitVec>::into(frame)).await?;
                                log::debug!(
                                    "Backoff timer expired. Medium state: free. Resent {}",
                                    inner.task.0.header().seq
                                );
                                Some(AcsmaSocketWriteTimer::timeout(
                                    inner.task,
                                    inner.resends + 1,
                                ))
                            }
                        }
                        None => {
                            log::debug!("Backoff timer expired. No task");
                            None
                        }
                    },
                }
            } else {
                write_state = Some(timer);
            }
        } else {
            let result = write_rx.try_recv();
            if let Ok(task) = result {
                log::debug!(
                    "Accepted frame from source with index {}",
                    task.0.header().seq
                );
                if !is_channel_free(&config, &mut write_monitor).await {
                    log::debug!("Medium state: busy. set backoff timer");
                    let duration = generate_backoff(&mut rng, 0);
                    write_state = Some(AcsmaSocketWriteTimer::backoff(Some(task), 0, duration));
                } else {
                    log::debug!("Medium state: free. Sending {}", task.0.header().seq);
                    let frame = task.0.clone();
                    write_ather.write(&Into::<BitVec>::into(frame)).await?;
                    log::debug!("Medium state: free. Sent {}", task.0.header().seq);
                    write_state = Some(AcsmaSocketWriteTimer::timeout(task, 0));
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

async fn is_channel_free(
    config: &AcsmaSocketConfig,
    write_monitor: &mut AcsmaSocketWriteMonitor,
) -> bool {
    let sample_rate = config.ather_config.stream_config.sample_rate().0;
    if let Some(sample) = write_monitor.sample().await {
        log::debug!("Energy: {}", sample.energy(sample_rate));
        sample.energy(sample_rate) < SOCKET_FREE_THRESHOLD
    } else {
        log::debug!("No sample");
        true
    }
}

enum AcsmaSocketWriteTimer {
    Timeout {
        start: Instant,
        inner: AcsmaSocketWriteTimerInner,
    },
    Backoff {
        start: Instant,
        inner: Option<AcsmaSocketWriteTimerInner>,
        retry: usize,
        duration: Duration,
    },
}

struct AcsmaSocketWriteTimerInner {
    task: AcsmaSocketWriteTask,
    resends: usize,
}

fn generate_backoff(rng: &mut SmallRng, factor: usize) -> Duration {
    let range = if 1 << factor > SOCKET_MAX_RANGE {
        SOCKET_MAX_RANGE
    } else {
        1 << factor
    };
    let k = rng.gen_range(0..=range as u32);
    log::debug!("Set timer to {} slots by {}", k, range);
    k * SOCKET_SLOT_TIMEOUT
}

impl AcsmaSocketWriteTimer {
    fn timeout(task: AcsmaSocketWriteTask, resends: usize) -> Self {
        Self::Timeout {
            start: Instant::now(),
            inner: AcsmaSocketWriteTimerInner { task, resends },
        }
    }

    fn backoff(task: Option<AcsmaSocketWriteTask>, retry: usize, duration: Duration) -> Self {
        let inner = task.map(|task| AcsmaSocketWriteTimerInner { task, resends: 0 });
        Self::Backoff {
            start: Instant::now(),
            inner,
            retry,
            duration,
        }
    }
}

impl AcsmaSocketWriteTimer {
    fn is_expired(&self) -> bool {
        match self {
            Self::Timeout { start, .. } => start.elapsed() > SOCKET_ACK_TIMEOUT,
            Self::Backoff {
                start, duration, ..
            } => start.elapsed() > *duration,
        }
    }

    fn elapsed(&self) -> Duration {
        match self {
            Self::Timeout { start, .. } => start.elapsed(),
            Self::Backoff { start, .. } => start.elapsed(),
        }
    }

    fn duration(&self) -> Duration {
        match self {
            Self::Timeout { .. } => SOCKET_ACK_TIMEOUT,
            Self::Backoff { duration, .. } => *duration,
        }
    }
}

struct AcsmaSocketWriteMonitor {
    req_tx: UnboundedSender<()>,
    resp_rx: UnboundedReceiver<Option<Box<[f32]>>>,
}

impl AcsmaSocketWriteMonitor {
    fn new(mut write_monitor: AudioInputStream<f32>) -> Self {
        let (req_tx, mut req_rx) = mpsc::unbounded_channel();
        let (resp_tx, resp_rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            let mut sample = None;
            loop {
                tokio::select! {
                    cmd = req_rx.recv() => {
                        if cmd.is_some() && resp_tx.send(sample.clone()).is_ok() {
                            continue;
                        }
                        break;
                    },
                    data = write_monitor.next() => {
                        sample = data
                    }
                }
            }
        });

        Self { req_tx, resp_rx }
    }

    async fn sample(&mut self) -> Option<Box<[f32]>> {
        self.req_tx.send(()).unwrap();
        match self.resp_rx.recv().await {
            Some(inner) => inner,
            None => None,
        }
    }
}
