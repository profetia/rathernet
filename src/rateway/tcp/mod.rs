//! TCP implementation for RateWay.
//! Based on <https://www.youtube.com/watch?v=_OnJ9z-e_TY&t=0s>.

mod conn;

use crate::{
    racsma::{AcsmaIoSocket, AcsmaSocketConfig, AcsmaSocketReader, AcsmaSocketWriter},
    rateway::builtin::TCP_BUFFER_LEN,
    rather::encode::DecodeToBytes,
    raudio::AsioDevice,
};
use anyhow::Result;
use etherparse::{Ipv4HeaderSlice, TcpHeaderSlice};
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    io::{self, Read, Write},
    net::Ipv4Addr,
    sync::Arc,
};
use tokio::{
    runtime::Handle,
    sync::{Mutex, Notify},
    task::{self, JoinHandle},
};

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
struct Quad {
    src: (Ipv4Addr, u16),
    dst: (Ipv4Addr, u16),
}

#[derive(Default)]
struct InterfaceHandleInner {
    manager: Mutex<ConnectionManager>,
    pending_var: Notify,
    rcv_var: Notify,
}

type InterfaceHandle = Arc<InterfaceHandleInner>;

pub struct Interface {
    ih: Option<InterfaceHandle>,
    jh: Option<JoinHandle<Result<()>>>,
}

impl Drop for Interface {
    fn drop(&mut self) {
        self.ih.as_mut().unwrap().manager.blocking_lock().terminate = true;

        drop(self.ih.take());
        if let Some(inner) = self.jh.take() {
            let _ = task::block_in_place(move || Handle::current().block_on(inner));
        }
    }
}

#[derive(Default)]
struct ConnectionManager {
    terminate: bool,
    connections: HashMap<Quad, conn::Connection>,
    pending: HashMap<u16, VecDeque<Quad>>,
}

async fn packet_loop(
    mut write_socket: AcsmaSocketWriter,
    mut read_socket: AcsmaSocketReader,
    ih: InterfaceHandle,
) -> Result<()> {
    loop {
        // we want to read from nic, but we want to make sure that we'll wake up when the next
        // timer has to be triggered!
        let buf = read_socket.read_unchecked().await?.decode();
        let nbytes = buf.len();

        // TODO: if self.terminate && Arc::get_strong_refs(ih) == 1; then tear down all connections and return.

        // if s/without_packet_info/new/:
        //
        // let _eth_flags = u16::from_be_bytes([buf[0], buf[1]]);
        // let eth_proto = u16::from_be_bytes([buf[2], buf[3]]);
        // if eth_proto != 0x0800 {
        //     // not ipv4
        //     continue;
        // }
        //
        // and also include on send

        match Ipv4HeaderSlice::from_slice(&buf[..nbytes]) {
            Ok(iph) => {
                let src = iph.source_addr();
                let dst = iph.destination_addr();
                if iph.protocol() != 0x06 {
                    eprintln!("BAD PROTOCOL");
                    // not tcp
                    continue;
                }

                match TcpHeaderSlice::from_slice(&buf[iph.slice().len()..nbytes]) {
                    Ok(tcph) => {
                        let datai = iph.slice().len() + tcph.slice().len();
                        let mut cmg = ih.manager.lock().await;
                        let cm = &mut *cmg;
                        let q = Quad {
                            src: (src, tcph.source_port()),
                            dst: (dst, tcph.destination_port()),
                        };

                        match cm.connections.entry(q) {
                            Entry::Occupied(mut c) => {
                                eprintln!("got packet for known quad {:?}", q);
                                let a = c
                                    .get_mut()
                                    .on_packet(&mut write_socket, iph, tcph, &buf[datai..nbytes])
                                    .await?;

                                // TODO: compare before/after
                                drop(cmg);
                                if a.contains(conn::Available::READ) {
                                    ih.rcv_var.notify_waiters()
                                }
                                if a.contains(conn::Available::WRITE) {
                                    // TODO: ih.snd_var.notify_all()
                                }
                            }
                            Entry::Vacant(e) => {
                                eprintln!("got packet for unknown quad {:?}", q);
                                if let Some(pending) = cm.pending.get_mut(&tcph.destination_port())
                                {
                                    eprintln!("listening, so accepting");
                                    if let Some(c) = conn::Connection::accept(
                                        &mut write_socket,
                                        iph,
                                        tcph,
                                        &buf[datai..nbytes],
                                    )
                                    .await?
                                    {
                                        e.insert(c);
                                        pending.push_back(q);
                                        drop(cmg);
                                        ih.pending_var.notify_waiters()
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("ignoring weird tcp packet {:?}", e);
                    }
                }
            }
            Err(_e) => {
                // eprintln!("ignoring weird packet {:?}", e);
            }
        }
    }
}

impl Interface {
    pub fn new(config: AcsmaSocketConfig, device: &AsioDevice) -> Result<Self> {
        let (write_socket, read_socket) = AcsmaIoSocket::try_from_device(config, device)?;

        let ih: InterfaceHandle = Arc::default();
        let jh = {
            let ih = ih.clone();
            tokio::spawn(packet_loop(write_socket, read_socket, ih))
        };

        Ok(Interface {
            ih: Some(ih),
            jh: Some(jh),
        })
    }

    pub async fn bind(&mut self, port: u16) -> Result<TcpListener> {
        let mut cm = self.ih.as_mut().unwrap().manager.lock().await;
        match cm.pending.entry(port) {
            Entry::Vacant(v) => {
                v.insert(VecDeque::new());
            }
            Entry::Occupied(_) => {
                return Err(io::Error::new(io::ErrorKind::AddrInUse, "port already bound").into());
            }
        };
        drop(cm);
        Ok(TcpListener {
            port,
            h: self.ih.as_mut().unwrap().clone(),
        })
    }
}

pub struct TcpListener {
    port: u16,
    h: InterfaceHandle,
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        let mut cm = self.h.manager.blocking_lock();

        let pending = cm
            .pending
            .remove(&self.port)
            .expect("port closed while listener still active");

        for _quad in pending {
            // TODO: terminate cm.connections[quad]
            unimplemented!();
        }
    }
}

impl TcpListener {
    pub async fn accept(&mut self) -> Result<TcpStream> {
        loop {
            if let Some(quad) = self
                .h
                .manager
                .lock()
                .await
                .pending
                .get_mut(&self.port)
                .expect("port closed while listener still active")
                .pop_front()
            {
                return Ok(TcpStream {
                    quad,
                    h: self.h.clone(),
                });
            }

            self.h.pending_var.notified().await;
        }
    }
}

pub struct TcpStream {
    quad: Quad,
    h: InterfaceHandle,
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        let _cm = self.h.manager.blocking_lock();
        // TODO: send FIN on cm.connections[quad]
        // TODO: _eventually_ remove self.quad from cm.connections
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut cm = self.h.manager.blocking_lock();
            let c = cm.connections.get_mut(&self.quad).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "stream was terminated unexpectedly",
                )
            })?;

            if c.is_rcv_closed() && c.incoming.is_empty() {
                // no more data to read, and no need to block, because there won't be any more
                return Ok(0);
            }

            if !c.incoming.is_empty() {
                let mut nread = 0;
                let (head, tail) = c.incoming.as_slices();
                let hread = buf.len().min(head.len());
                buf[..hread].copy_from_slice(&head[..hread]);
                nread += hread;
                let tread = (buf.len() - nread).min(tail.len());
                buf[hread..(hread + tread)].copy_from_slice(&tail[..tread]);
                nread += tread;
                drop(c.incoming.drain(..nread));
                return Ok(nread);
            }
            drop(cm);

            self.h.rcv_var.notified();
        }
    }
}

impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut cm = self.h.manager.blocking_lock();
        let c = cm.connections.get_mut(&self.quad).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "stream was terminated unexpectedly",
            )
        })?;

        if c.unacked.len() >= TCP_BUFFER_LEN as usize {
            // TODO: block
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "too many bytes buffered",
            ));
        }

        let nwrite = std::cmp::min(buf.len(), TCP_BUFFER_LEN as usize - c.unacked.len());
        c.unacked.extend(buf[..nwrite].iter());

        Ok(nwrite)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut cm = self.h.manager.blocking_lock();
        let c = cm.connections.get_mut(&self.quad).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "stream was terminated unexpectedly",
            )
        })?;

        if c.unacked.is_empty() {
            Ok(())
        } else {
            // TODO: block
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "too many bytes buffered",
            ))
        }
    }
}

impl TcpStream {
    pub async fn shutdown(&self, _how: std::net::Shutdown) -> Result<()> {
        let mut cm = self.h.manager.lock().await;
        let c = cm.connections.get_mut(&self.quad).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "stream was terminated unexpectedly",
            )
        })?;

        c.close()
    }
}
