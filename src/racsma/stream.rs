use super::{
    builtin::{ACK_RECIEVE_TIMEOUT, ACK_WINDOW_SIZE, FRAME_DETECT_TIMEOUT, PAYLOAD_BITS_LEN},
    frame::{AckFrame, DataFrame, Frame},
};
use crate::rather::{AtherInputStream, AtherOutputStream};
use bitvec::prelude::*;
use std::collections::VecDeque;
use tokio::{
    sync::mpsc::{self, error::TryRecvError, UnboundedReceiver, UnboundedSender},
    task::{JoinError, JoinHandle},
    time,
};
use tokio_stream::StreamExt;

#[derive(Debug, Clone)]
pub struct AcsmaIoConfig {
    pub src: usize,
}

impl AcsmaIoConfig {
    pub fn new(src: usize) -> Self {
        Self { src }
    }
}

pub struct AcsmaIoStream {
    config: AcsmaIoConfig,
    sender: UnboundedSender<BitVec>,
    reciever: UnboundedReceiver<BitVec>,
    handle: JoinHandle<()>,
}

impl AcsmaIoStream {
    pub fn new(
        config: AcsmaIoConfig,
        mut istream: AtherInputStream,
        ostream: AtherOutputStream,
    ) -> Self {
        let (tx_sender, mut tx_reciever) = mpsc::unbounded_channel::<BitVec>();
        let (rx_sender, rx_reciever) = mpsc::unbounded_channel::<BitVec>();
        let handle = tokio::spawn(async move {
            loop {
                let bits = tx_reciever.try_recv();
                if let Ok(bits) = bits {
                    ostream.write(&bits).await;
                } else if let Err(TryRecvError::Disconnected) = bits {
                    break;
                } else if let Ok(Some(bits)) =
                    time::timeout(FRAME_DETECT_TIMEOUT, istream.next()).await
                {
                    if rx_sender.send(bits).is_err() {
                        break;
                    }
                } else {
                    println!("Timeout");
                }
            }
        });

        Self {
            config,
            sender: tx_sender,
            reciever: rx_reciever,
            handle,
        }
    }
}

impl AcsmaIoStream {
    pub async fn write(&mut self, dest: usize, bits: &BitSlice) {
        let mut chunks = bits.chunks(PAYLOAD_BITS_LEN).enumerate();
        let mut window = VecDeque::with_capacity(ACK_WINDOW_SIZE);
        for (index, chunk) in chunks.by_ref() {
            let bits: BitVec =
                DataFrame::new(dest, self.config.src, index, chunk.to_owned()).into();
            self.sender.send(bits.clone()).unwrap();
            window.push_back((index, bits));
            println!("Send frame at index {}", index);
            if window.len() >= ACK_WINDOW_SIZE {
                break;
            }
        }

        while !window.is_empty() {
            let front_index = window.front().unwrap().0;
            let back_index = window.back().unwrap().0;
            loop {
                let timeout_future = async {
                    while let Some(bits) = self.reciever.recv().await {
                        if let Ok(frame) = AckFrame::try_from(bits) {
                            let header = frame.header();
                            if header.dest == self.config.src {
                                println!("Recieve ACK for index {}", header.seq);
                                if front_index <= header.seq && header.seq <= back_index {
                                    window.retain(|(index, _)| *index > header.seq);
                                    println!("Move window to index {}", header.seq + 1);
                                    break;
                                }
                            }
                        }
                    }
                };
                if time::timeout(ACK_RECIEVE_TIMEOUT, timeout_future)
                    .await
                    .is_ok()
                {
                    break;
                }
                window.iter().for_each(|(index, bits)| {
                    self.sender.send(bits.clone()).unwrap();
                    println!("Send frame at index {}", index);
                });
            }
            for (index, chunk) in chunks.by_ref() {
                let bits: BitVec =
                    DataFrame::new(dest, self.config.src, index, chunk.to_owned()).into();
                self.sender.send(bits.clone()).unwrap();
                window.push_back((index, bits));
                println!("Send frame at index {}", index);
                if window.len() >= ACK_WINDOW_SIZE {
                    break;
                }
            }
        }
    }

    pub async fn read(&mut self, src: usize, buf: &mut BitSlice) {
        let mut bucket = bitvec![];
        let mut window: VecDeque<(usize, Option<BitVec>)> =
            VecDeque::with_capacity(ACK_WINDOW_SIZE);
        for index in 0..ACK_WINDOW_SIZE {
            window.push_back((index, None));
        }

        while let Some(bits) = self.reciever.recv().await {
            println!("Got bits len {}", bits.len());
            let front_index = window.front().unwrap().0;
            let back_index = window.back().unwrap().0;
            if let Ok(frame) = DataFrame::try_from(bits) {
                let header = frame.header();
                if header.src == src && header.dest == self.config.src {
                    println!("Recieve DATA for index {}", header.seq);
                    if header.seq < front_index {
                        let ack = AckFrame::new(header.src, self.config.src, front_index - 1);
                        self.sender.send(ack.into()).unwrap();
                        println!("Send ACK at index {}", front_index - 1);
                    } else if header.seq <= back_index {
                        let item = window.get_mut(header.seq - front_index).unwrap();
                        if item.1.is_none() {
                            let index = item.0;
                            let payload = frame.payload().unwrap().to_owned();
                            *item = (index, Some(payload));
                        }

                        if header.seq == front_index {
                            window = window
                                .into_iter()
                                .skip_while(|(_, payload)| {
                                    if let Some(bits) = payload {
                                        bucket.extend(bits);
                                        return true;
                                    }
                                    false
                                })
                                .collect();
                            for index in back_index + 1..back_index + ACK_WINDOW_SIZE + 1 {
                                window.push_back((index, None));
                            }
                            let front_index = window.front().unwrap().0;
                            println!("Move window to index {}", front_index);
                            let ack = AckFrame::new(header.src, self.config.src, front_index - 1);
                            self.sender.send(ack.into()).unwrap();
                            println!("Send ACK at index {}", front_index - 1);

                            if bucket.len() >= buf.len() {
                                break;
                            }
                        } else if front_index > 0 {
                            let ack = AckFrame::new(header.src, self.config.src, front_index - 1);
                            self.sender.send(ack.into()).unwrap();
                            println!("Send ACK at index {}", front_index - 1);
                        }
                    } else if front_index > 0 {
                        let ack = AckFrame::new(header.src, self.config.src, front_index - 1);
                        self.sender.send(ack.into()).unwrap();
                        println!("Send ACK at index {}", front_index - 1);
                    }
                }
            }
        }

        let front_index = window.front().unwrap().0;
        for _ in 0..3 {
            let ack = AckFrame::new(src, self.config.src, front_index - 1);
            self.sender.send(ack.into()).unwrap();
            println!("Send ACK at index {}", front_index - 1);
        }

        buf.copy_from_bitslice(&bucket[..buf.len()]);
    }
}

impl AcsmaIoStream {
    pub async fn close(self) -> Result<(), JoinError> {
        drop(self.sender);
        drop(self.reciever);
        self.handle.await?;
        Ok(())
    }
}
