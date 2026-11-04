//! Native TCP calls happen on one worker started by create, never in callbacks.
use se_plugin_abi::{Host, SeBrokerCallFn, SeHostApiV1, SeOutput, SeSlice};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Copy)]
struct Waker {
    context: usize,
    call: SeBrokerCallFn,
}
thread_local! { static WAKER: Cell<Option<Waker>> = const { Cell::new(None) }; }
pub fn capture_waker(api: &SeHostApiV1) {
    WAKER.set(api.broker_call.map(|call| Waker {
        context: api.host_context as usize,
        call,
    }));
}
impl Waker {
    fn request(self) {
        let input = br#"{"schemaVersion":1,"service":"se.host.wake","version":1,"method":"request","input":{"reason":"studio"}}"#;
        let mut bytes = [0u8; 1024];
        let mut output = SeOutput {
            ptr: bytes.as_mut_ptr(),
            capacity: bytes.len(),
            len: 0,
        };
        // SAFETY: the copied context/callback is used ONLY for se.host.wake.request,
        // explicitly permitted out of callback. Studio::drop joins this worker
        // before destroy returns, so neither context nor library can outlive host.
        unsafe {
            (self.call)(
                self.context as *mut _,
                SeSlice::from_bytes(input),
                &mut output,
            );
        }
    }
}
#[derive(Default)]
struct Shared {
    port: Option<u16>,
    result: Value,
}
pub struct Studio {
    shared: Arc<Mutex<Shared>>,
    tx: mpsc::SyncSender<()>,
    worker: Option<JoinHandle<()>>,
    pub wakes: u64,
}
impl Studio {
    pub fn new() -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = mpsc::sync_channel(1);
        let worker = WAKER.take().map(|waker| {
            let state = shared.clone();
            thread::spawn(move || loop {
                let port = state.lock().ok().and_then(|s| s.port);
                if let Some(port) = port {
                    let result = ping(port)
                        .unwrap_or_else(|error| json!({"port":port,"ok":false,"error":error}));
                    if let Ok(mut s) = state.lock() {
                        s.result = result;
                    }
                }
                waker.request();
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    _ => continue,
                }
            })
        });
        Self {
            shared,
            tx,
            worker,
            wakes: 0,
        }
    }
    pub fn ping(&self) {
        let _ = self.tx.try_send(());
    }
    pub fn service(&self) -> Value {
        self.shared
            .lock()
            .map(|s| s.result.clone())
            .unwrap_or(Value::Null)
    }
    pub fn wake(&mut self, host: &Host) {
        self.wakes += 1;
        if let Ok(logs) = call(
            host,
            "se.host.process",
            "logs",
            json!({"id":"activity-service","tail":20}),
        ) {
            if let Some(entries) = logs["lines"].as_array() {
                for entry in entries {
                    if let Some(line) = entry["text"].as_str() {
                        if let Some(port) = line
                            .strip_prefix("SE_ACTIVITY_PORT=")
                            .and_then(|s| s.trim().parse::<u16>().ok())
                        {
                            if let Ok(mut s) = self.shared.lock() {
                                s.port = Some(port);
                            }
                        }
                    }
                }
            }
        }
        let _ = call(
            host,
            "se.host.status",
            "set",
            json!({"id":"studio-service","text":if self.service()["ok"] == true {"Studio service online"} else {"Studio service connecting"},"tone":"default","visible":true}),
        );
        let _ = call(
            host,
            "se.host.ui",
            "invalidate",
            json!({"viewIds":["example.counter","activity-studio"]}),
        );
    }
    pub fn post(&self, host: &Host, snapshot: Value) {
        let _ = call(
            host,
            "se.host.ui",
            "postMessage",
            json!({"viewId":"activity-studio","payload":snapshot}),
        );
    }
}
impl Drop for Studio {
    fn drop(&mut self) {
        let (dummy, _) = mpsc::sync_channel(1);
        drop(std::mem::replace(&mut self.tx, dummy));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn call(host: &Host, service: &str, method: &str, input: Value) -> Result<Value, ()> {
    let bytes = serde_json::to_vec(
        &json!({"schemaVersion":1,"service":service,"version":1,"method":method,"input":input}),
    )
    .map_err(|_| ())?;
    let mut output = vec![0; 256 * 1024];
    let n = host.broker_call(&bytes, &mut output).map_err(|_| ())?;
    let reply: Value = serde_json::from_slice(&output[..n]).map_err(|_| ())?;
    if reply["result"]["ok"] == true {
        Ok(reply["result"]["value"].clone())
    } else {
        Err(())
    }
}
fn ping(port: u16) -> Result<Value, String> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(200))
        .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(b"GET /ping HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .map_err(|e| e.to_string())?;
    let mut bytes = String::new();
    stream
        .take(8192)
        .read_to_string(&mut bytes)
        .map_err(|e| e.to_string())?;
    let (_, body) = bytes
        .split_once("\r\n\r\n")
        .ok_or("invalid HTTP response")?;
    let mut value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    value["port"] = json!(port);
    Ok(value)
}
