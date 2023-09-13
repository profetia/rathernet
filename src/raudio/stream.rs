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
                    sink.append(source);
                    sink.sleep_until_end();

                    let mut guard = task.state.lock().unwrap();
                    if let Some(waker) = guard.waker.take() {
                        waker.wake();
                        guard.result = Some(Ok(()));
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
    pub fn write(&self, source: S) -> impl Future<Output = Result<()>> {
        let state = Arc::new(Mutex::new(AudioOutputTaskState {
            waker: None,
            result: None,
        }));
        let task = AudioOutputTask {
            source,
            state: Arc::clone(&state),
        };
        self.sender.send(task).unwrap();
        AudioOutputFuture {
            sink: self.sink.clone(),
            state,
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
    state: Arc<Mutex<AudioOutputTaskState>>,
}

struct AudioOutputTaskState {
    waker: Option<Waker>,
    result: Option<Result<()>>,
}

struct AudioOutputFuture {
    sink: Arc<Sink>,
    state: Arc<Mutex<AudioOutputTaskState>>,
}

impl Future for AudioOutputFuture {
    type Output = Result<()>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut guard = self.state.lock().unwrap();
        if let Some(result) = guard.result.take() {
            return std::task::Poll::Ready(result);
        }
        guard.waker = Some(cx.waker().clone());
        std::task::Poll::Pending
    }
}

impl Drop for AudioOutputFuture {
    fn drop(&mut self) {
        let guard = self.state.lock().unwrap();
        if guard.result.is_none() {
            self.sink.clear();
        }
    }
}
