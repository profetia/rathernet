use super::{
    asio::{AsioInputStream, AsioOutputStream},
    AsioDevice, AudioSamples,
};
use anyhow::Result;
use cpal::{
    traits::{DeviceTrait, StreamTrait},
    FromSample, SampleFormat, SizedSample, SupportedStreamConfig, SupportedStreamConfigsError,
};
use crossbeam::{
    channel::{self, Receiver, Sender},
    sync::ShardedLock,
};
use rodio::{Sample, Sink, Source};
use std::{
    future::Future,
    mem,
    sync::{Arc, Mutex},
    task::{Poll, Waker},
    thread,
    time::Duration,
};
use tokio_stream::{Stream, StreamExt};

pub struct AudioOutputStream<S>
where
    S: Source + Send + 'static,
    f32: FromSample<S::Item>,
    S::Item: Sample + Send,
{
    _stream: AsioOutputStream,
    sink: Arc<Sink>,
    sender: Sender<AudioOutputTask<S>>,
}

impl<S> AudioOutputStream<S>
where
    S: Source + Send + 'static,
    f32: FromSample<S::Item>,
    S::Item: Sample + Send,
{
    fn try_from_stream(stream: AsioOutputStream) -> Result<Self> {
        let sink = Arc::new(Sink::try_new(&stream.handle)?);
        let (sender, receiver) = channel::unbounded::<AudioOutputTask<S>>();
        thread::spawn({
            let sink = Arc::clone(&sink);
            move || {
                while let Ok(task) = receiver.recv() {
                    {
                        let mut guard = task.lock().unwrap();
                        match guard.take() {
                            AudioOutputTaskState::Ready(source, waker) => {
                                sink.append(source);
                                *guard = AudioOutputTaskState::Running(waker);
                            }
                            state => {
                                *guard = state;
                                continue;
                            }
                        }
                    }

                    sink.sleep_until_end();

                    let mut guard = task.lock().unwrap();
                    match guard.take() {
                        AudioOutputTaskState::Running(waker) => {
                            waker.wake();
                            *guard = AudioOutputTaskState::Completed;
                        }
                        state => *guard = state,
                    }
                }
            }
        });

        Ok(Self {
            _stream: stream,
            sink,
            sender,
        })
    }

    pub fn try_from_device_config(
        device: &AsioDevice,
        config: SupportedStreamConfig,
    ) -> Result<Self> {
        Self::try_from_stream(AsioOutputStream::try_from_device_config(device, config)?)
    }

    pub fn try_from_device(device: &AsioDevice) -> Result<Self> {
        Self::try_from_stream(AsioOutputStream::try_from_device(device)?)
    }

    pub fn try_default() -> Result<Self> {
        Self::try_from_stream(AsioOutputStream::try_default()?)
    }
}

impl<S> AudioOutputStream<S>
where
    S: Source + Send + 'static,
    f32: FromSample<S::Item>,
    S::Item: Sample + Send,
{
    pub fn write(&self, source: S) -> AudioOutputFuture<S> {
        let task = Arc::new(Mutex::new(AudioOutputTaskState::Pending(source)));
        AudioOutputFuture {
            sink: self.sink.clone(),
            sender: self.sender.clone(),
            task,
        }
    }

    pub async fn write_timeout(&self, source: S, timeout: Duration) {
        let write_future = self.write(source);
        let timeout_future = tokio::time::sleep(timeout);
        tokio::select! {
            _ = write_future => {},
            _ = timeout_future => {},
        }
    }
}

type AudioOutputTask<S> = Arc<Mutex<AudioOutputTaskState<S>>>;

enum AudioOutputTaskState<S>
where
    S: Source + Send + 'static,
    f32: FromSample<S::Item>,
    S::Item: Sample + Send,
{
    Pending(S),
    Ready(S, Waker),
    Running(Waker),
    Completed,
}

impl<S> AudioOutputTaskState<S>
where
    S: Source + Send + 'static,
    f32: FromSample<S::Item>,
    S::Item: Sample + Send,
{
    fn take(&mut self) -> Self {
        mem::replace(self, AudioOutputTaskState::Completed)
    }
}

pub struct AudioOutputFuture<S>
where
    S: Source + Send + 'static,
    f32: FromSample<S::Item>,
    S::Item: Sample + Send,
{
    sink: Arc<Sink>,
    sender: Sender<AudioOutputTask<S>>,
    task: AudioOutputTask<S>,
}

impl<S> Future for AudioOutputFuture<S>
where
    S: Source + Send + 'static,
    f32: FromSample<S::Item>,
    S::Item: Sample + Send,
{
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut guard = self.task.lock().unwrap();
        match guard.take() {
            AudioOutputTaskState::Pending(source) => {
                *guard = AudioOutputTaskState::Ready(source, cx.waker().clone());
                self.sender.send(self.task.clone()).unwrap();
                Poll::Pending
            }
            AudioOutputTaskState::Ready(source, _) => {
                *guard = AudioOutputTaskState::Ready(source, cx.waker().clone());
                Poll::Pending
            }
            AudioOutputTaskState::Running(_) => {
                *guard = AudioOutputTaskState::Running(cx.waker().clone());
                Poll::Pending
            }
            _ => Poll::Ready(()),
        }
    }
}

impl<S> Drop for AudioOutputFuture<S>
where
    S: Source + Send + 'static,
    f32: FromSample<S::Item>,
    S::Item: Sample + Send,
{
    fn drop(&mut self) {
        let mut guard = self.task.lock().unwrap();
        match guard.take() {
            AudioOutputTaskState::Pending(_) => {
                *guard = AudioOutputTaskState::Completed;
            }
            AudioOutputTaskState::Ready(_, waker) => {
                waker.wake();
                *guard = AudioOutputTaskState::Completed;
            }
            AudioOutputTaskState::Running(waker) => {
                self.sink.clear();
                waker.wake();
                *guard = AudioOutputTaskState::Completed;
            }
            _ => {}
        }
    }
}

pub struct AudioInputStream<S: Sample> {
    stream: AsioInputStream,
    task: AudioInputTask,
    reciever: Receiver<AudioSamples<S>>,
}

impl<S> AudioInputStream<S>
where
    S: Send
        + Sample
        + FromSample<i8>
        + FromSample<i16>
        + FromSample<i32>
        + FromSample<f32>
        + 'static,
{
    pub fn try_from_device_config(
        device: &AsioDevice,
        config: SupportedStreamConfig,
    ) -> Result<Self> {
        let (sender, reciever) = channel::unbounded::<AudioSamples<S>>();
        let task = Arc::new(ShardedLock::new(AudioInputTaskState::Pending));

        let stream = match config.sample_format() {
            SampleFormat::I8 => {
                build_input_stream::<i8, S>(device, config.clone(), sender.clone(), task.clone())
            }
            SampleFormat::I16 => {
                build_input_stream::<i16, S>(device, config.clone(), sender.clone(), task.clone())
            }
            SampleFormat::I32 => {
                build_input_stream::<i32, S>(device, config.clone(), sender.clone(), task.clone())
            }
            SampleFormat::F32 => {
                build_input_stream::<f32, S>(device, config.clone(), sender.clone(), task.clone())
            }
            _ => return Err(SupportedStreamConfigsError::InvalidArgument.into()),
        }?;

        stream.0.pause().unwrap();

        Ok(AudioInputStream {
            stream,

            task,
            reciever,
        })
    }

    pub fn try_from_device(device: &AsioDevice) -> Result<Self> {
        let config = device.0.default_input_config()?;
        Self::try_from_device_config(device, config)
    }

    pub fn try_default() -> Result<Self> {
        let device = AsioDevice::try_default()?;
        Self::try_from_device(&device)
    }
}

fn build_input_stream<T, S>(
    device: &AsioDevice,
    config: SupportedStreamConfig,
    sender: Sender<AudioSamples<S>>,
    task: AudioInputTask,
) -> Result<AsioInputStream>
where
    T: SizedSample,
    S: Send + Sample + FromSample<T> + 'static,
{
    Ok(AsioInputStream::new(device.0.build_input_stream(
        &config.into(),
        {
            move |data: &[T], _: _| {
                let data = data
                    .iter()
                    .map(|sample| S::from_sample(*sample))
                    .collect::<AudioSamples<S>>();

                let guard = task.read().unwrap();
                if let AudioInputTaskState::Running(ref waker) = *guard {
                    sender.send(data).unwrap();
                    waker.wake_by_ref();
                }
            }
        },
        |error| eprintln!("an error occurred on input stream: {}", error),
        None,
    )?))
}

impl<S> AudioInputStream<S>
where
    S: Send
        + Sample
        + FromSample<i8>
        + FromSample<i16>
        + FromSample<i32>
        + FromSample<f32>
        + 'static,
{
    pub async fn read(&mut self) -> AudioSamples<S> {
        let mut result = vec![];
        while let Some(data) = self.next().await {
            result.extend(data.iter());
        }
        result.into_boxed_slice()
    }

    pub async fn read_timeout(&mut self, timeout: Duration) -> AudioSamples<S> {
        let mut result = vec![];
        tokio::select! {
            _ = async {
                while let Some(data) = self.next().await {
                    result.extend(data.iter());
                }
            } => {},
            _ = tokio::time::sleep(timeout) => {},
        };
        result.into_boxed_slice()
    }
}

type AudioInputTask = Arc<ShardedLock<AudioInputTaskState>>;

enum AudioInputTaskState {
    Pending,
    Running(Waker),
    Suspended,
}

impl AudioInputTaskState {
    fn take(&mut self) -> AudioInputTaskState {
        mem::replace(self, AudioInputTaskState::Suspended)
    }
}

impl<S> Stream for AudioInputStream<S>
where
    S: Send
        + Sample
        + FromSample<i8>
        + FromSample<i16>
        + FromSample<i32>
        + FromSample<f32>
        + 'static,
{
    type Item = AudioSamples<S>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut guard = self.task.write().unwrap();
        match guard.take() {
            AudioInputTaskState::Pending => {
                *guard = AudioInputTaskState::Running(cx.waker().clone());
                self.stream.0.play().unwrap();
                Poll::Pending
            }
            AudioInputTaskState::Running(_) => {
                *guard = AudioInputTaskState::Running(cx.waker().clone());
                let mut samples: Vec<S> = vec![];
                let iter = self.reciever.try_iter();
                for data in iter {
                    samples.extend(data.iter());
                }

                if samples.is_empty() {
                    Poll::Pending
                } else {
                    Poll::Ready(Some(samples.into_boxed_slice()))
                }
            }
            AudioInputTaskState::Suspended => Poll::Ready(None),
        }
    }
}

/// Continuous Stream is a stream that is only lazy when it is polled for the first time.
/// After that, it will keep running until it is suspended. Once it is suspended, it will
/// yield `None` until it is resumed. Resuming the stream will reset the stream to its
/// initial state, and it will be lazy again until it is polled for the first time.
pub trait ContinuousStream: Stream
where
    Self: Sized,
{
    fn suspend(&self);

    fn resume(&self);
}

impl<S> ContinuousStream for AudioInputStream<S>
where
    S: Send
        + Sample
        + FromSample<i8>
        + FromSample<i16>
        + FromSample<i32>
        + FromSample<f32>
        + 'static,
{
    fn suspend(&self) {
        let mut guard = self.task.write().unwrap();
        match guard.take() {
            AudioInputTaskState::Running(waker) => {
                self.stream.0.pause().unwrap();
                *guard = AudioInputTaskState::Suspended;
                waker.wake();
            }
            AudioInputTaskState::Pending => *guard = AudioInputTaskState::Suspended,
            content => *guard = content,
        }
    }

    fn resume(&self) {
        let mut guard = self.task.write().unwrap();
        match guard.take() {
            AudioInputTaskState::Suspended => *guard = AudioInputTaskState::Pending,
            content => *guard = content,
        }
    }
}
