//! One WebSocket client session. A single loop thread owns the WebSocket: it
//! polls the socket with a short read timeout and drains an mpsc queue fed by
//! channel opener/reader threads, so all WebSocket writes are serialized.
use super::bridge;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, ErrorKind};
use std::net::{Shutdown, TcpStream};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;
use tungstenite::{Message, WebSocket};

const POLL: Duration = Duration::from_millis(40);
pub const SERVER_NAME: &str = "qin-board-service";

enum Event {
    Opened {
        chan: String,
        generation: u64,
        result: Result<bridge::Connected, String>,
    },
    Frame {
        chan: String,
        generation: u64,
        frame: Value,
    },
    Ended {
        chan: String,
        generation: u64,
        error: Option<String>,
    },
}

struct Channel {
    generation: u64,
    writer: UnixStream,
}

struct Session {
    ws: WebSocket<TcpStream>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    open: HashMap<String, Channel>,
    pending: HashMap<String, u64>,
    next_generation: u64,
}

pub fn run(ws: WebSocket<TcpStream>) {
    if ws.get_ref().set_read_timeout(Some(POLL)).is_err() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    let mut session = Session {
        ws,
        tx,
        rx,
        open: HashMap::new(),
        pending: HashMap::new(),
        next_generation: 1,
    };
    let _ = session.serve();
    session.close_all();
    let _ = session.ws.close(None);
    let _ = session.ws.flush();
}

impl Session {
    fn send(&mut self, value: Value) -> Result<(), ()> {
        self.ws
            .send(Message::text(value.to_string()))
            .map_err(|_| ())
    }

    fn serve(&mut self) -> Result<(), ()> {
        self.send(json!({"op":"ready","server":SERVER_NAME}))?;
        loop {
            match self.ws.read() {
                Ok(Message::Text(text)) => self.handle_client(text.as_str())?,
                Ok(Message::Close(_)) => return Err(()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => return Err(()),
            }
            self.drain_events()?;
        }
    }

    fn drain_events(&mut self) -> Result<(), ()> {
        loop {
            match self.rx.try_recv() {
                Ok(event) => self.handle_event(event)?,
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => return Err(()),
            }
        }
    }

    fn handle_client(&mut self, text: &str) -> Result<(), ()> {
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            return Ok(());
        };
        let chan = message["chan"].as_str().map(str::to_owned);
        match (message["op"].as_str(), chan) {
            (Some("ping"), _) => self.send(json!({"op":"pong"})),
            (Some("open"), Some(chan)) => {
                self.drop_channel(&chan);
                self.start_open(chan);
                Ok(())
            }
            (Some("send"), Some(chan)) => {
                let outcome = match self.open.get_mut(&chan) {
                    Some(channel) => bridge::write_frame(&mut channel.writer, &message["frame"]),
                    None => Err("channel not open".into()),
                };
                if let Err(error) = outcome {
                    self.drop_channel(&chan);
                    self.send(json!({"op":"closed","chan":chan,"error":error}))?;
                }
                Ok(())
            }
            (Some("close"), Some(chan)) => {
                self.drop_channel(&chan);
                self.send(json!({"op":"closed","chan":chan}))
            }
            _ => Ok(()),
        }
    }

    fn start_open(&mut self, chan: String) {
        let generation = self.next_generation;
        self.next_generation += 1;
        self.pending.insert(chan.clone(), generation);
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("qin-board-open".into())
            .spawn(move || {
                let result = bridge::open();
                let _ = tx.send(Event::Opened {
                    chan,
                    generation,
                    result,
                });
            });
        if let Err(error) = spawned {
            eprintln!("qin-board-service: could not spawn opener: {error}");
        }
    }

    fn handle_event(&mut self, event: Event) -> Result<(), ()> {
        match event {
            Event::Opened {
                chan,
                generation,
                result,
            } => {
                if self.pending.get(&chan) != Some(&generation) {
                    if let Ok(connected) = result {
                        let _ = connected.writer.shutdown(Shutdown::Both);
                    }
                    return Ok(());
                }
                self.pending.remove(&chan);
                match result {
                    Ok(connected) => {
                        let bridge::Connected {
                            writer,
                            reader,
                            hello,
                        } = connected;
                        self.spawn_reader(chan.clone(), generation, reader);
                        self.open
                            .insert(chan.clone(), Channel { generation, writer });
                        self.send(json!({"op":"opened","chan":chan,"hello":hello}))
                    }
                    Err(error) => self.send(json!({"op":"closed","chan":chan,"error":error})),
                }
            }
            Event::Frame {
                chan,
                generation,
                frame,
            } => {
                if self.is_current(&chan, generation) {
                    self.send(json!({"op":"frame","chan":chan,"frame":frame}))?;
                }
                Ok(())
            }
            Event::Ended {
                chan,
                generation,
                error,
            } => {
                if !self.is_current(&chan, generation) {
                    return Ok(());
                }
                self.drop_channel(&chan);
                let mut closed = json!({"op":"closed","chan":chan});
                if let Some(error) = error {
                    closed["error"] = Value::String(error);
                }
                self.send(closed)
            }
        }
    }

    fn is_current(&self, chan: &str, generation: u64) -> bool {
        self.open
            .get(chan)
            .is_some_and(|channel| channel.generation == generation)
    }

    fn spawn_reader(&self, chan: String, generation: u64, mut reader: BufReader<UnixStream>) {
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("qin-board-reader".into())
            .spawn(move || {
                let mut line = String::new();
                let error = loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => break Some("bridge closed the connection".to_owned()),
                        Ok(_) => {
                            let Ok(frame) = serde_json::from_str::<Value>(line.trim()) else {
                                continue;
                            };
                            let event = Event::Frame {
                                chan: chan.clone(),
                                generation,
                                frame,
                            };
                            if tx.send(event).is_err() {
                                return;
                            }
                        }
                        Err(e) if e.kind() == ErrorKind::Interrupted => {}
                        Err(e) => break Some(e.to_string()),
                    }
                };
                let _ = tx.send(Event::Ended {
                    chan,
                    generation,
                    error,
                });
            });
        if let Err(error) = spawned {
            eprintln!("qin-board-service: could not spawn reader: {error}");
        }
    }

    fn drop_channel(&mut self, chan: &str) {
        self.pending.remove(chan);
        if let Some(channel) = self.open.remove(chan) {
            let _ = channel.writer.shutdown(Shutdown::Both);
        }
    }

    fn close_all(&mut self) {
        self.pending.clear();
        for (_, channel) in self.open.drain() {
            let _ = channel.writer.shutdown(Shutdown::Both);
        }
    }
}
