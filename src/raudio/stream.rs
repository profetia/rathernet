use std::{
    future::Future,
    mem,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    task::{Poll, Waker},
    thread,
    time::Duration,
};

use tokio_stream::{Stream, StreamExt};

use anyhow::Result;
use cpal::{
    traits::{DeviceTrait, StreamTrait},
    FromSample, SampleFormat, SupportedStreamConfig, SupportedStreamConfigsError,
};
use rodio::{Sample, Sink, Source};

use super::{asio::AsioOutputStream, AsioDevice};

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
        let (sender, receiver) = mpsc::channel::<AudioOutputTask<S>>();
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

    pub fn try_from_device(device: &AsioDevice) -> Result<Self> {
        Self::try_from_stream(AsioOutputStream::try_from_device(device)?)
    }

    pub fn try_default() -> Result<Self> {
        Self::try_from_stream(AsioOutputStream::try_default()?)
    }

    pub fn try_from_name(name: &str) -> Result<Self> {
        Self::try_from_stream(AsioOutputStream::try_from_name(name)?)
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

    pub async fn write_timeout(&self, source: S, timeout: std::time::Duration) {
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

pub struct AudioInputStream<S>
where
    S: Sample + hound::Sample,
{
    stream: cpal::Stream,
    _config: SupportedStreamConfig,
    task: AudioInputTask,
    reciever: Receiver<Vec<S>>,
}

impl<S> AudioInputStream<S>
where
    S: Send
        + Sample
        + hound::Sample
        + FromSample<i8>
        + FromSample<i16>
        + FromSample<i32>
        + FromSample<f32>
        + 'static,
{
    pub fn try_from_device(device: &AsioDevice) -> Result<Self> {
        let config = device.inner.default_input_config()?;
        let (sender, reciever) = mpsc::channel::<Vec<S>>();
        let task = Arc::new(Mutex::new(AudioInputTaskState::Pending));
        macro_rules! build_input_stream {
            ($sample_type:ty) => {
                device.inner.build_input_stream(
                    &config.clone().into(),
                    {
                        let sender = sender.clone();
                        let task = task.clone();
                        move |data: &[$sample_type], _: _| {
                            let guard = task.lock().unwrap();
                            if let AudioInputTaskState::Running(ref waker) = *guard {
                                let data = data
                                    .iter()
                                    .map(|sample| S::from_sample(*sample))
                                    .collect::<Vec<S>>();
                                sender.send(data).unwrap();
                                waker.wake_by_ref();
                            }
                        }
                    },
                    |error| eprintln!("an error occurred on input stream: {}", error),
                    None,
                )
            };
        }
        let stream = match config.sample_format() {
            SampleFormat::I8 => build_input_stream!(i8),
            SampleFormat::I16 => build_input_stream!(i16),
            SampleFormat::I32 => build_input_stream!(i32),
            SampleFormat::F32 => build_input_stream!(f32),
            _ => return Err(SupportedStreamConfigsError::InvalidArgument.into()),
        }?;

        stream.pause().unwrap();

        Ok(AudioInputStream {
            stream,
            _config: config,
            task,
            reciever,
        })
    }

    pub fn try_default() -> Result<Self> {
        let device = AsioDevice::try_default()?;
        Self::try_from_device(&device)
    }

    pub fn try_from_name(name: &str) -> Result<Self> {
        let device = AsioDevice::try_from_name(name)?;
        Self::try_from_device(&device)
    }
}

impl<S> AudioInputStream<S>
where
    S: Send
        + Sample
        + hound::Sample
        + FromSample<i8>
        + FromSample<i16>
        + FromSample<i32>
        + FromSample<f32>
        + 'static,
{
    pub fn config(&self) -> &SupportedStreamConfig {
        &self._config
    }

    pub fn suspend(&self) {
        let mut guard = self.task.lock().unwrap();
        match guard.take() {
            AudioInputTaskState::Running(waker) => {
                self.stream.pause().unwrap();
                *guard = AudioInputTaskState::Suspended;
                waker.wake();
            }
            content => *guard = content,
        }
    }

    pub fn resume(&self) {
        let mut guard = self.task.lock().unwrap();
        match guard.take() {
            AudioInputTaskState::Suspended => *guard = AudioInputTaskState::Pending,
            content => *guard = content,
        }
    }

    pub async fn read(&mut self) -> Vec<S> {
        let mut result = vec![];
        while let Some(data) = self.next().await {
            result.extend(data.into_iter());
        }
        result
    }

    pub async fn read_timeout(&mut self, timeout: Duration) -> Vec<S> {
        let mut result = vec![];
        tokio::select! {
            _ = async {
                while let Some(data) = self.next().await {
                    result.extend(data.into_iter());
                }
            } => {},
            _ = tokio::time::sleep(timeout) => {},
        };
        result
    }
}

type AudioInputTask = Arc<Mutex<AudioInputTaskState>>;

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
        + hound::Sample
        + FromSample<i8>
        + FromSample<i16>
        + FromSample<i32>
        + FromSample<f32>
        + 'static,
{
    type Item = Vec<S>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut guard = self.task.lock().unwrap();
        match guard.take() {
            AudioInputTaskState::Pending => {
                *guard = AudioInputTaskState::Running(cx.waker().clone());
                self.stream.play().unwrap();
                Poll::Pending
            }
            AudioInputTaskState::Running(_) => {
                *guard = AudioInputTaskState::Running(cx.waker().clone());
                match self.reciever.try_recv() {
                    Ok(data) => Poll::Ready(Some(data)),
                    Err(_) => Poll::Pending,
                }
            }
            AudioInputTaskState::Suspended => Poll::Ready(None),
        }
    }
}
