//! Native plugin: answers the web view's `endpoint`, `restart-service` and
//! `open-board` commands. All sidecar state lives in the sidecar process; the
//! native side only reads its announcement line through `se.host.process`.
use se_plugin_abi::{
    Host, OutputWriter, PluginV1, SeStatus, SEP_INTERNAL, SEP_INVALID_JSON, SEP_LOG_INFO,
    SEP_UNAVAILABLE,
};
use serde_json::{json, Value};

const PROCESS_ID: &str = "board-service";

pub struct BoardPlugin {
    host: Option<Host>,
}

fn parse(bytes: &[u8]) -> Result<Value, SeStatus> {
    serde_json::from_slice(bytes).map_err(|_| SEP_INVALID_JSON)
}

fn write(out: &mut OutputWriter<'_>, value: Value) -> Result<(), SeStatus> {
    let bytes = serde_json::to_vec(&value).map_err(|_| SEP_INTERNAL)?;
    out.write(&bytes)
}

fn call(host: &Host, service: &str, method: &str, input: Value) -> Result<Value, String> {
    let bytes = serde_json::to_vec(
        &json!({"schemaVersion":1,"service":service,"version":1,"method":method,"input":input}),
    )
    .map_err(|e| e.to_string())?;
    let mut output = vec![0; 256 * 1024];
    let n = host
        .broker_call(&bytes, &mut output)
        .map_err(|status| format!("broker call failed ({status})"))?;
    let reply: Value = serde_json::from_slice(&output[..n]).map_err(|e| e.to_string())?;
    if reply["result"]["ok"] == true {
        Ok(reply["result"]["value"].clone())
    } else {
        let message = reply["result"]["error"]["message"]
            .as_str()
            .or_else(|| reply["result"]["error"].as_str())
            .unwrap_or("host service error");
        Err(message.to_owned())
    }
}

impl BoardPlugin {
    fn endpoint(&self) -> Value {
        let Some(host) = self.host.as_ref() else {
            return json!({"ready":false,"error":"host unavailable"});
        };
        match call(
            host,
            "se.host.process",
            "logs",
            json!({"id":PROCESS_ID,"tail":50}),
        ) {
            Ok(logs) => match crate::endpoint::latest_from_logs(&logs) {
                Some(ep) => json!({"ready":true,"port":ep.port,"token":ep.token}),
                None => {
                    json!({"ready":false,"error":"board service has not announced its endpoint yet"})
                }
            },
            Err(error) => json!({"ready":false,"error":error}),
        }
    }

    fn restart(&self) -> Value {
        let Some(host) = self.host.as_ref() else {
            return json!({"ok":false,"error":"host unavailable"});
        };
        match call(host, "se.host.process", "restart", json!({"id":PROCESS_ID})) {
            Ok(_) => json!({"ok":true}),
            Err(error) => json!({"ok":false,"error":error}),
        }
    }
}

impl PluginV1 for BoardPlugin {
    fn manifest() -> &'static [u8] {
        crate::MANIFEST
    }

    fn create(host: Host, _activation_json: &[u8]) -> Result<Self, SeStatus> {
        host.log(SEP_LOG_INFO, "qin-code board activated")?;
        Ok(Self { host: Some(host) })
    }

    fn on_command(&mut self, bytes: &[u8], out: &mut OutputWriter<'_>) -> Result<(), SeStatus> {
        let request = parse(bytes)?;
        let result = match request["commandId"].as_str() {
            Some("endpoint") => self.endpoint(),
            Some("restart-service") => self.restart(),
            Some("open-board") => json!({"ok":true}),
            _ => return Err(SEP_UNAVAILABLE),
        };
        write(out, json!({"schemaVersion":1,"result":result}))
    }

    fn destroy(self) -> Result<(), SeStatus> {
        if let Some(host) = self.host {
            host.log(SEP_LOG_INFO, "qin-code board destroyed")?;
        }
        Ok(())
    }
}
