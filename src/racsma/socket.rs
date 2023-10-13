use super::{
    builtin::{
        PAYLOAD_BITS_LEN, SOCKET_ACK_TIMEOUT, SOCKET_COLISION_THRESHOLD, SOCKET_FREE_THRESHOLD,
        SOCKET_MAX_RANGE, SOCKET_MAX_RESENDS, SOCKET_PERF_INTERVAL, SOCKET_PERF_TIMEOUT,
        SOCKET_PING_INTERVAL, SOCKET_PING_TIMEOUT, SOCKET_RECIEVE_TIMEOUT, SOCKET_SLOT_TIMEOUT,
    },
    frame::{
        AckFrame, AcsmaFrame, DataFrame, Frame, FrameHeader, MacPingReqFrame, MacPingRespFrame,
        NonAckFrame, PacketBeginFrame, PacketEndFrame,
    },
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
    mem,
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
    #[error("Perf timeout after {0} ms")]
    PerfTimeout(usize),
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
                if let NonAckFrame::Data(data) = frame {
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

    pub(crate) async fn read_packet(&mut self, src: usize) -> Result<BitVec> {
        let mut state = AcsmaSocketReadPacketState::Pending;
        while let Some(frame) = self.read_rx.recv().await {
            let header = frame.header().clone();
            if src == header.src {
                match &mut state {
                    AcsmaSocketReadPacketState::Pending => {
                        if let NonAckFrame::PacketBegin(_) = frame {
                            state.begin();
                        }
                    }
                    AcsmaSocketReadPacketState::Reading(ref mut bucket) => {
                        if let NonAckFrame::PacketEnd(_) = frame {
                            break;
                        } else if let NonAckFrame::Data(data) = frame {
                            let payload = data.payload().unwrap();
                            bucket.entry(header.seq).or_insert(payload.to_owned());
                        }
                    }
                }
            }
        }

        if let AcsmaSocketReadPacketState::Reading(bucket) = state {
            return Ok(bucket
                .into_iter()
                .fold(BitVec::new(), |mut acc, (_, payload)| {
                    acc.extend_from_bitslice(&payload);
                    acc
                }));
        }

        unreachable!()
    }

    pub async fn serve(&mut self) -> Result<()> {
        while let Some(frame) = self.read_rx.recv().await {
            let header = frame.header();
            log::info!("Receive frame {} from {}", header.seq, header.src);
        }
        Ok(())
    }
}

enum AcsmaSocketReadPacketState {
    Pending,
    Reading(BTreeMap<usize, BitVec>),
}

impl AcsmaSocketReadPacketState {
    fn begin(&mut self) {
        *self = Self::Pending;
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

    pub(crate) async fn write_packet(&mut self, dest: usize, bits: &BitSlice) -> Result<()> {
        let begin = PacketBeginFrame::new(dest, self.config.address);
        let (tx, rx) = oneshot::channel();
        self.write_tx.send((NonAckFrame::PacketBegin(begin), tx))?;
        rx.await??;

        self.write(dest, bits).await?;

        let end = PacketEndFrame::new(dest, self.config.address);
        let (tx, rx) = oneshot::channel();
        self.write_tx.send((NonAckFrame::PacketEnd(end), tx))?;
        rx.await??;

        Ok(())
    }

    pub async fn perf(&mut self, dest: usize) -> Result<()> {
        let (send_tx, send_rx) = mpsc::unbounded_channel();
        tokio::try_join!(
            perf_main(send_rx),
            perf_daemon(&self.config, &mut self.write_tx, dest, send_tx)
        )?;

        Ok(())
    }

    pub async fn ping(&mut self, dest: usize) -> Result<()> {
        let frame = NonAckFrame::MacPingReq(MacPingReqFrame::new(dest, self.config.address));
        loop {
            time::sleep(SOCKET_PING_INTERVAL).await;
            let (tx, rx) = oneshot::channel();
            self.write_tx.send((frame.clone(), tx))?;
            let start = Instant::now();
            if let Ok(inner) = time::timeout(SOCKET_PING_TIMEOUT, rx).await {
                inner??;
                println!("Ping: {} ms", start.elapsed().as_millis());
            } else {
                println!("Ping: timeout");
            }
        }
    }
}

async fn perf_daemon(
    config: &AcsmaSocketConfig,
    write_tx: &mut UnboundedSender<AcsmaSocketWriteTask>,
    dest: usize,
    send_tx: UnboundedSender<usize>,
) -> Result<()> {
    let bits = bitvec![usize, Lsb0; 0; PAYLOAD_BITS_LEN];
    let frame = DataFrame::new(dest, config.address, 0, bits);

    loop {
        let (tx, rx) = oneshot::channel();
        write_tx.send((NonAckFrame::Data(frame.clone()), tx))?;
        if let Ok(inner) = time::timeout(SOCKET_PERF_TIMEOUT, rx).await {
            inner??;
            let _ = send_tx.send(PAYLOAD_BITS_LEN);
        } else {
            break;
        }
    }

    Ok(())
}

async fn perf_main(mut send_rx: UnboundedReceiver<usize>) -> Result<()> {
    let mut sent = 0;
    let mut epochs = 0usize;
    'a: loop {
        time::sleep(SOCKET_PERF_INTERVAL).await;
        loop {
            match send_rx.try_recv() {
                Ok(len) => sent += len,
                Err(TryRecvError::Disconnected) => {
                    break 'a;
                }
                _ => {
                    break;
                }
            }
        }

        epochs += 1;
        println!(
            "Throughput: {} kbps",
            sent as f32 / (1000. * SOCKET_PERF_INTERVAL.as_secs() as f32 * epochs as f32)
        );
    }

    Err(AcsmaIoError::PerfTimeout(SOCKET_PERF_TIMEOUT.as_millis() as usize).into())
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
                        AcsmaFrame::NonAck(non_ack) => {
                            let bits = create_resp(&header, &non_ack);
                            log::debug!("Sending ACK | MacPingResp for index {}", header.seq);
                            write_ather.write(&bits).await?;
                            log::debug!("Sent ACK | MacPingResp for index {}", header.seq);
                            let _ = read_tx.send(non_ack);
                        }
                        _ => {
                            log::debug!("Recieve ACK | MacPingResp for index {}", header.seq);
                            if let Some(timer) = write_state {
                                write_state = Some(clear_timer(&mut rng, &header, timer));
                            }
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
                        Some(create_backoff(&mut rng, inner.task, 0))
                    }
                    AcsmaSocketWriteTimer::Backoff {
                        inner: Some(inner),
                        retry,
                        ..
                    } => {
                        let header = inner.task.0.header();
                        log::debug!("Backoff timer expired. {}", header.seq);
                        if !is_channel_free(&config, &mut write_monitor).await {
                            log::debug!("Medium state: busy. {}", header.seq);
                            Some(create_backoff(&mut rng, inner.task, retry + 1))
                        } else if inner.resends > SOCKET_MAX_RESENDS {
                            log::debug!("Medium state: free. resends exceeded {}", header.seq);
                            inner.link_error();
                            None
                        } else {
                            log::debug!("Medium state: free. Resending {}", header.seq);
                            let bits = Into::<BitVec>::into(inner.task.0.clone());
                            if !write_bits(&config, &write_ather, &mut write_monitor, &bits).await?
                            {
                                log::debug!("Medium state: free. Colision detected {}", header.seq);
                                Some(create_backoff(&mut rng, inner.task, retry + 1))
                            } else {
                                log::debug!("Medium state: free. Resent {}", header.seq);
                                Some(AcsmaSocketWriteTimer::timeout(
                                    inner.task,
                                    inner.resends + 1,
                                ))
                            }
                        }
                    }
                    _ => {
                        log::debug!("Backoff timer expired. No task");
                        None
                    }
                }
            } else {
                write_state = Some(timer);
            }
        } else {
            let result = write_rx.try_recv();
            if let Ok(task) = result {
                let header = task.0.header();
                log::debug!("Accepted frame from source with index {}", header.seq);
                write_state = if !is_channel_free(&config, &mut write_monitor).await {
                    log::debug!("Medium state: busy. set backoff timer");
                    Some(create_backoff(&mut rng, task, 0))
                } else {
                    log::debug!("Medium state: free. Sending {}", header.seq);
                    let bits = Into::<BitVec>::into(task.0.clone());
                    if !write_bits(&config, &write_ather, &mut write_monitor, &bits).await? {
                        log::debug!("Medium state: free. Colision detected");
                        Some(create_backoff(&mut rng, task, 1))
                    } else {
                        log::debug!("Medium state: free. Sent {}", header.seq);
                        Some(AcsmaSocketWriteTimer::timeout(task, 0))
                    }
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

fn create_backoff(
    rng: &mut SmallRng,
    task: AcsmaSocketWriteTask,
    retry: usize,
) -> AcsmaSocketWriteTimer {
    let duration = generate_backoff(rng, retry);
    AcsmaSocketWriteTimer::backoff(Some(task), retry, duration)
}

fn create_resp(header: &FrameHeader, non_ack: &NonAckFrame) -> BitVec {
    match non_ack {
        NonAckFrame::Data(_) => {
            log::debug!("Receive data for index {}", header.seq);
            Into::<BitVec>::into(AckFrame::new(header.src, header.dest, header.seq))
        }
        NonAckFrame::MacPingReq(_) => {
            log::debug!("Receive MacPingReq for index {}", header.seq);
            Into::<BitVec>::into(MacPingRespFrame::new(header.src, header.dest))
        }
        NonAckFrame::PacketBegin(_) => {
            log::debug!("Receive PacketBegin for index {}", header.seq);
            Into::<BitVec>::into(AckFrame::new(header.src, header.dest, header.seq))
        }
        NonAckFrame::PacketEnd(_) => {
            log::debug!("Receive PacketEnd for index {}", header.seq);
            Into::<BitVec>::into(AckFrame::new(header.src, header.dest, header.seq))
        }
    }
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

fn clear_timer(
    rng: &mut SmallRng,
    header: &FrameHeader,
    mut timer: AcsmaSocketWriteTimer,
) -> AcsmaSocketWriteTimer {
    let inner = match &timer {
        AcsmaSocketWriteTimer::Timeout { inner, .. } => Some(inner),
        AcsmaSocketWriteTimer::Backoff {
            inner: Some(inner), ..
        } => Some(inner),
        _ => None,
    };
    if let Some(inner) = inner {
        let type_ok = inner.task.0.corresponds(header);
        let seq_ok = inner.task.0.header().seq == header.seq;
        if type_ok && seq_ok {
            let duration = generate_backoff(rng, 0);
            match mem::replace(
                &mut timer,
                AcsmaSocketWriteTimer::backoff(None, 0, duration),
            ) {
                AcsmaSocketWriteTimer::Timeout { inner, .. } => {
                    inner.ok();
                    log::debug!("Clear ACK timeout {}", header.seq);
                }
                AcsmaSocketWriteTimer::Backoff { inner, .. } => {
                    inner.unwrap().ok();
                    log::debug!("Clear Backoff timeout {}", header.seq);
                }
            }
            return timer;
        }
    }

    timer
}

async fn write_bits(
    config: &AcsmaSocketConfig,
    write_ather: &AtherOutputStream,
    colision_monitor: &mut AcsmaSocketWriteMonitor,
    bits: &BitSlice,
) -> Result<bool> {
    let sample_rate = config.ather_config.stream_config.sample_rate().0;
    tokio::select! {
        result = write_ather.write(bits) => result.map(|_| true),
        _ = async {
            loop {
                if let Some(sample) = colision_monitor.sample().await {
                    let enery = sample.energy(sample_rate);
                    // log::warn!("Energy {}", enery);
                    if enery > SOCKET_COLISION_THRESHOLD {
                        break;
                    }
                }
            }
        } => Ok(false)
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

impl AcsmaSocketWriteTimerInner {
    fn ok(self) {
        let _ = self.task.1.send(Ok(()));
    }

    fn link_error(self) {
        let _ = self
            .task
            .1
            .send(Err(AcsmaIoError::LinkError(self.resends).into()));
    }
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
        self.clear();
        self.req_tx.send(()).unwrap();
        match self.resp_rx.recv().await {
            Some(inner) => inner,
            None => None,
        }
    }

    fn clear(&mut self) {
        while self.resp_rx.try_recv().is_ok() {}
    }
}
