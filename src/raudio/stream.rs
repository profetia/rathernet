use std::{
    future::Future,
    sync::{
        mpsc::{self, Sender},
        Arc, Mutex,
    },
    task::Waker,
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
    pub fn try_from_stream(stream: AsioOutputStream) -> Result<Self> {
        let sink = Arc::new(Sink::try_new(&stream.handle)?);
        let (sender, receiver) = mpsc::channel::<AudioOutputTask<S>>();
        thread::spawn({
            let sink = Arc::clone(&sink);
            move || {
                while let Ok(task) = receiver.recv() {
                    let source = task.source;

                    {
                        let mut guard = task.inner.lock().unwrap();
                        if let AudioOutputTaskState::Completed = guard.state {
                            if let Some(waker) = guard.waker.take() {
                                waker.wake()
                            }
                            continue;
                        } else {
                            guard.state = AudioOutputTaskState::Running;
                        }
                    }

                    sink.append(source);
                    sink.sleep_until_end();

                    let mut guard = task.inner.lock().unwrap();
                    if let Some(waker) = guard.waker.take() {
                        waker.wake();
                        guard.state = AudioOutputTaskState::Completed;
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
}

impl<S> AudioOutputStream<S>
where
    S: Source + Send + 'static,
    f32: FromSample<S::Item>,
    S::Item: Sample + Send,
{
    pub fn write(&self, source: S) -> AudioOutputFuture {
        let state = Arc::new(Mutex::new(AudioOutputTaskInner {
            waker: None,
            state: AudioOutputTaskState::Ready,
        }));
        let task = AudioOutputTask {
            source,
            inner: Arc::clone(&state),
        };
        self.sender.send(task).unwrap();
        AudioOutputFuture {
            sink: self.sink.clone(),
            state,
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

struct AudioOutputTask<S>
where
    S: Source + Send + 'static,
    f32: FromSample<S::Item>,
    S::Item: Sample + Send,
{
    source: S,
    inner: Arc<Mutex<AudioOutputTaskInner>>,
}

struct AudioOutputTaskInner {
    waker: Option<Waker>,
    state: AudioOutputTaskState,
}

enum AudioOutputTaskState {
    Ready,
    Running,
    Completed,
}

pub struct AudioOutputFuture {
    sink: Arc<Sink>,
    state: Arc<Mutex<AudioOutputTaskInner>>,
}

impl Future for AudioOutputFuture {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut guard = self.state.lock().unwrap();
        if let AudioOutputTaskState::Completed = guard.state {
            return std::task::Poll::Ready(());
        }
        guard.waker = Some(cx.waker().clone());
        std::task::Poll::Pending
    }
}

impl Drop for AudioOutputFuture {
    fn drop(&mut self) {
        let mut guard = self.state.lock().unwrap();
        if let AudioOutputTaskState::Running = guard.state {
            self.sink.clear();
        }
        guard.state = AudioOutputTaskState::Completed;
    }
}
