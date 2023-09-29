use super::{
    builtin::{FRAME_DETECT_TIMEOUT, PAYLOAD_BITS_LEN},
    frame::{Frame, FrameType},
};
use crate::{
    racsma::builtin::ACK_RECIEVE_TIMEOUT,
    rather::{encode::DecodeToInt, AtherInputStream, AtherOutputStream},
};
use bitvec::prelude::*;
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
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
}

impl AcsmaIoStream {
    pub fn new(
        config: AcsmaIoConfig,
        mut istream: AtherInputStream,
        ostream: AtherOutputStream,
    ) -> Self {
        let (tx_sender, mut tx_reciever) = mpsc::unbounded_channel::<BitVec>();
        let (rx_sender, rx_reciever) = mpsc::unbounded_channel::<BitVec>();
        tokio::spawn(async move {
            loop {
                if let Ok(bits) = tx_reciever.try_recv() {
                    ostream.write(&bits).await;
                } else if let Ok(Some(bits)) =
                    time::timeout(FRAME_DETECT_TIMEOUT, istream.next()).await
                {
                    rx_sender.send(bits).unwrap();
                } else {
                    println!("Timeout");
                }
            }
        });

        Self {
            config,
            sender: tx_sender,
            reciever: rx_reciever,
        }
    }
}

impl AcsmaIoStream {
    pub async fn write(&mut self, dest: usize, bits: &BitSlice) {
        let chunks = bits.chunks(PAYLOAD_BITS_LEN);
        let frame = Frame::new_meta(dest, self.config.src, chunks.len());
        println!("Meta len {}", chunks.len());
        let bits: BitVec = frame.into();
        loop {
            self.sender.send(bits.clone()).unwrap();
            println!("Send meta");
            let timeout_future = async {
                while let Some(bits) = self.reciever.recv().await {
                    if let Ok(frame) = Frame::try_from(bits) {
                        if frame.dest == self.config.src && frame.r#type == FrameType::ACK {
                            println!("Recieve ACK for meta");
                            break;
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
        }

        for (index, chunk) in chunks.enumerate() {
            let frame = Frame::new_data(dest, self.config.src, index, chunk.to_owned());
            let bits: BitVec = frame.into();
            loop {
                self.sender.send(bits.clone()).unwrap();
                println!("Send data at index {}", index);
                let timeout_future = async {
                    while let Some(bits) = self.reciever.recv().await {
                        if let Ok(frame) = Frame::try_from(bits) {
                            if frame.dest == self.config.src
                                && frame.seq == index
                                && frame.r#type.contains(FrameType::ACK)
                            {
                                println!("Recieve ACK for index {}", index);
                                break;
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
            }
        }
    }

    pub async fn read(&mut self, src: usize) -> BitVec {
        let mut buf = bitvec![];
        loop {
            if let Some(bits) = self.reciever.recv().await {
                println!("Got bits len {}", bits.len());
                if let Ok(frame) = Frame::try_from(bits) {
                    println!(
                        "Got frame, type {:?},  len {}",
                        frame.r#type,
                        frame.payload.len()
                    );
                    if frame.src == src
                        && frame.dest == self.config.src
                        && frame.r#type == FrameType::META
                    {
                        println!("Got meta");
                        buf.extend(frame.payload);
                        let bits = Frame::new_ack(src, self.config.src, frame.seq).into();
                        self.sender.send(bits).unwrap();
                        println!("Send ACK");
                        break;
                    }
                }
            }
        }

        let length = DecodeToInt::<usize>::decode(&buf);
        println!("Meta len {}", length);
        let mut buf: Vec<(usize, BitVec)> = vec![];
        loop {
            if let Some(bits) = self.reciever.recv().await {
                println!("Got bits len {}", bits.len());
                if let Ok(frame) = Frame::try_from(bits) {
                    println!(
                        "Got frame, type {:?},  len {}",
                        frame.r#type,
                        frame.payload.len()
                    );
                    if frame.src == src && frame.dest == self.config.src {
                        if frame.r#type == FrameType::DATA {
                            println!("Got data at index {}", frame.seq);
                            if !buf.iter().any(|item| item.0 == frame.seq) {
                                buf.push((frame.seq, frame.payload));
                            }

                            let bits = Frame::new_ack(src, self.config.src, frame.seq).into();
                            self.sender.send(bits).unwrap();
                            println!("Send ACK");

                            if length == buf.len() {
                                break;
                            }
                        } else if frame.r#type == FrameType::META {
                            println!("Got meta");
                            let bits = Frame::new_ack(src, self.config.src, frame.seq).into();
                            self.sender.send(bits).unwrap();
                            println!("Send ACK");
                        }
                    }
                }
            }
        }

        buf.into_iter()
            .map(|item| item.1)
            .fold(bitvec![], |mut acc, mut item| {
                acc.append(&mut item);
                acc
            })
    }
}
