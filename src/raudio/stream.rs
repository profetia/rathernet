use std::{
    future::Future,
    sync::{
        mpsc::{self, Sender},
        Arc, Mutex,
    },
    task::{Poll, Waker},
    thread,
};

use anyhow::Result;
use cpal::FromSample;
use rodio::{Sample, Sink, Source};

use super::asio::AsioOutputStream;

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
                            if let Some(waker) = waker {
                                waker.wake()
                            }
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
    Ready(S, Option<Waker>),
    Running(Option<Waker>),
    Completed,
}

impl<S> AudioOutputTaskState<S>
where
    S: Source + Send + 'static,
    f32: FromSample<S::Item>,
    S::Item: Sample + Send,
{
    fn take(&mut self) -> Self {
        std::mem::replace(self, AudioOutputTaskState::Completed)
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
                *guard = AudioOutputTaskState::Ready(source, Some(cx.waker().clone()));
                self.sender.send(self.task.clone()).unwrap();
                Poll::Pending
            }
            AudioOutputTaskState::Ready(source, _) => {
                *guard = AudioOutputTaskState::Ready(source, Some(cx.waker().clone()));
                Poll::Pending
            }
            AudioOutputTaskState::Running(_) => {
                *guard = AudioOutputTaskState::Running(Some(cx.waker().clone()));
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
                if let Some(waker) = waker {
                    waker.wake()
                }
                *guard = AudioOutputTaskState::Completed;
            }
            AudioOutputTaskState::Running(waker) => {
                self.sink.clear();
                if let Some(waker) = waker {
                    waker.wake()
                }
                *guard = AudioOutputTaskState::Completed;
            }
            _ => {}
        }
    }
}
