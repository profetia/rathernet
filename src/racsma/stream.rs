use super::{
    builtin::{ACK_RECIEVE_TIMEOUT, FRAME_DETECT_TIMEOUT, PAYLOAD_BITS_LEN},
    frame::{AckFrame, DataFrame, Frame},
};
use crate::rather::{AtherInputStream, AtherOutputStream};
use bitvec::prelude::*;
use std::collections::BTreeMap;
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
        }
    }
}

impl AcsmaIoStream {
    pub async fn write(&mut self, dest: usize, bits: &BitSlice) {
        for (index, chunk) in bits.chunks(PAYLOAD_BITS_LEN).enumerate() {
            let frame = DataFrame::new(dest, self.config.src, index, chunk.to_bitvec());
            let bits: BitVec = frame.into();
            loop {
                self.sender.send(bits.clone()).unwrap();
                println!("Send data at index {}", index);
                let timeout_future = async {
                    while let Some(bits) = self.reciever.recv().await {
                        if let Ok(frame) = AckFrame::try_from(bits) {
                            let header = frame.header();
                            if header.dest == self.config.src && header.seq == index {
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

    pub async fn read(&mut self, src: usize, buf: &mut BitSlice) {
        let (mut bucket, mut len) = (BTreeMap::new(), 0usize);
        loop {
            if let Some(bits) = self.reciever.recv().await {
                println!("Got bits len {}", bits.len());
                if let Ok(frame) = DataFrame::try_from(bits) {
                    let header = frame.header();
                    if header.src == src && header.dest == self.config.src {
                        println!("Got data at index {}", header.seq);
                        bucket.entry(header.seq).or_insert_with(|| {
                            let payload = frame.payload().unwrap();
                            len += payload.len();
                            payload.to_owned()
                        });

                        let ack = AckFrame::new(header.src, self.config.src, header.seq);
                        self.sender.send(ack.into()).unwrap();
                        println!("Send ACK");

                        if len == buf.len() {
                            break;
                        }
                    }
                }
            }
        }

        buf.copy_from_bitslice(&bucket.iter().fold(bitvec![], |mut acc, (_, bits)| {
            acc.extend(bits);
            acc
        }));
    }
}
