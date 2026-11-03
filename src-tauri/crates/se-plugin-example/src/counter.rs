use se_plugin_abi::{
    Host, OutputWriter, PluginV1, SeStatus, SEP_INTERNAL, SEP_INVALID_ARGUMENT, SEP_INVALID_JSON,
    SEP_LOG_INFO, SEP_UNAVAILABLE,
};
use serde::Deserialize;
use serde_json::{json, Value};

const VIEW: &str = "example.counter";

#[cfg(feature = "test-hooks")]
struct LateHost {
    host: Host,
    result_file: std::path::PathBuf,
}

#[cfg(feature = "test-hooks")]
impl Drop for LateHost {
    fn drop(&mut self) {
        // A complete protocol ServiceRequest: malformed JSON must not mask an
        // invalid host-context lifetime with SEP_INVALID_JSON.
        let request = br#"{"schemaVersion":1,"service":"org.termul.example.counter","version":1,"method":"get","input":null}"#;
        let mut output = [0; 1024];
        let status = match self.host.broker_call(request, &mut output) {
            Ok(_) => se_plugin_abi::SEP_OK,
            Err(status) => status,
        };
        // TLS destruction must not panic, even if the test removed its directory.
        let _ = std::fs::write(&self.result_file, status.to_string());
    }
}

#[cfg(feature = "test-hooks")]
thread_local! {
    static LATE_HOST: std::cell::RefCell<Option<LateHost>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(feature = "test-hooks")]
fn escape_host(host: Host, result_file: &str) {
    LATE_HOST.with(|slot| {
        *slot.borrow_mut() = Some(LateHost {
            host,
            result_file: result_file.into(),
        });
    });
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    greeting: String,
    step: f64,
    show_count: bool,
    tone: String,
}

impl Config {
    fn parse(value: Value) -> Result<Self, SeStatus> {
        let config: Self = serde_json::from_value(value).map_err(|_| SEP_INVALID_JSON)?;
        if !(1.0..=100.0).contains(&config.step)
            || !matches!(config.tone.as_str(), "normal" | "muted")
        {
            return Err(SEP_INVALID_ARGUMENT);
        }
        Ok(config)
    }
}

pub struct CounterPlugin {
    host: Option<Host>,
    count: f64,
    config: Config,
}

fn parse(bytes: &[u8]) -> Result<Value, SeStatus> {
    serde_json::from_slice(bytes).map_err(|_| SEP_INVALID_JSON)
}

fn write(out: &mut OutputWriter<'_>, value: Value) -> Result<(), SeStatus> {
    let bytes = serde_json::to_vec(&value).map_err(|_| SEP_INTERNAL)?;
    out.write(&bytes)
}

impl CounterPlugin {
    fn tree(&self) -> Value {
        // Heading has no tone property in UI v1. The adjacent greeting text
        // carries the configured tone, rather than emitting an invalid heading.
        let title = if cfg!(feature = "upgrade-fixture") {
            // Reserve the suffix's five characters within UI v1's 8192 limit.
            format!(
                "{} — v2",
                self.config.greeting.chars().take(8187).collect::<String>()
            )
        } else {
            self.config.greeting.clone()
        };
        let tone = if self.config.tone == "muted" {
            "muted"
        } else {
            "default"
        };
        let mut children = vec![
            json!({"type":"heading", "id":"heading", "props":{"text":title,"level":2}}),
            json!({"type":"text", "id":"greeting", "props":{"text":self.config.greeting,"tone":tone}}),
        ];
        if self.config.show_count {
            children.push(json!({"type":"text", "id":"count", "props":{"text":self.count.to_string(),"tone":tone}}));
        }
        children.extend([
            json!({"type":"row", "id":"actions", "props":{"gap":"sm"}, "children":[
                {"type":"button", "id":"increment", "props":{"label":format!("+{}",self.config.step),"actionId":"increment"}},
                {"type":"button", "id":"reset", "props":{"label":"Reset","actionId":"reset","variant":"secondary"}}
            ]}),
            // Config strings allow 8192 characters, whereas input values allow
            // at most 4096. Keep the complete heading/text and bound only input.
            json!({"type":"input", "id":"greeting-input", "props":{"label":"Local greeting","value":self.config.greeting.chars().take(4096).collect::<String>(),"actionId":"greeting","maxLength":4096}}),
            json!({"type":"toggle", "id":"show-count", "props":{"label":"Show count","checked":self.config.show_count,"actionId":"showCount"}}),
            json!({"type":"select", "id":"tone-select", "props":{"label":"Tone","value":self.config.tone,"actionId":"tone","options":[{"value":"normal","label":"Normal"},{"value":"muted","label":"Muted"}]}}),
            json!({"type":"divider", "id":"divider", "props":{}}),
            json!({"type":"list", "id":"help", "props":{"items":[
                {"id":"local","text":"Panel controls change local state","description":"Configuration changes apply live; local edits are not saved as configuration."},
                {"id":"reset","text":"Reset is also available as a command","description":"example.reset"}
            ]}}),
        ]);
        json!({"schemaVersion":1,"viewId":VIEW,"root":{"type":"column","id":"counter","props":{"gap":"md"},"children":children}})
    }
}

impl PluginV1 for CounterPlugin {
    fn manifest() -> &'static [u8] {
        #[cfg(feature = "test-hooks")]
        {
            // Only fault builds publish the intentionally dangerous commands.
            static MANIFEST: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
            MANIFEST.get_or_init(|| {
                let mut manifest: Value =
                    serde_json::from_slice(if cfg!(feature = "upgrade-fixture") {
                        include_bytes!("manifest-v2.json").as_slice()
                    } else {
                        include_bytes!("manifest.json").as_slice()
                    })
                    .expect("embedded manifest is valid JSON");
                let commands = manifest["contributes"]["commands"].as_array_mut().unwrap();
                for (id, title) in [
                    ("example.panic", "Test panic"),
                    ("example.timeout", "Test timeout"),
                    ("example.abort", "Test process abort"),
                    ("example.escape-host", "Test escaped host lifetime"),
                ] {
                    commands.push(json!({"id":id,"title":title}));
                }
                serde_json::to_vec(&manifest).expect("embedded manifest serializes")
            })
        }
        #[cfg(all(not(feature = "test-hooks"), feature = "upgrade-fixture"))]
        {
            include_bytes!("manifest-v2.json")
        }
        #[cfg(all(not(feature = "test-hooks"), not(feature = "upgrade-fixture")))]
        {
            include_bytes!("manifest.json")
        }
    }

    fn create(host: Host, activation_json: &[u8]) -> Result<Self, SeStatus> {
        let activation = parse(activation_json)?;
        let config = Config::parse(activation["config"]["values"].clone())?;
        host.log(SEP_LOG_INFO, "Configured Counter activated")?;
        Ok(Self {
            host: Some(host),
            count: 0.0,
            config,
        })
    }

    fn render_view(&mut self, bytes: &[u8], out: &mut OutputWriter<'_>) -> Result<(), SeStatus> {
        if parse(bytes)?["viewId"] != VIEW {
            return Err(SEP_UNAVAILABLE);
        }
        write(out, self.tree())
    }

    fn on_action(&mut self, bytes: &[u8], out: &mut OutputWriter<'_>) -> Result<(), SeStatus> {
        let event = parse(bytes)?;
        if event["viewId"] != VIEW {
            return Err(SEP_UNAVAILABLE);
        }
        match event["actionId"].as_str() {
            Some("increment") => self.count += self.config.step,
            Some("reset") => self.count = 0.0,
            Some("greeting") => {
                self.config.greeting = event["value"]
                    .as_str()
                    .ok_or(SEP_INVALID_ARGUMENT)?
                    .to_owned()
            }
            Some("showCount") => {
                self.config.show_count = event["value"].as_bool().ok_or(SEP_INVALID_ARGUMENT)?
            }
            Some("tone") => {
                let tone = event["value"].as_str().ok_or(SEP_INVALID_ARGUMENT)?;
                if !matches!(tone, "normal" | "muted") {
                    return Err(SEP_INVALID_ARGUMENT);
                }
                self.config.tone = tone.to_owned();
            }
            _ => return Err(SEP_UNAVAILABLE),
        }
        write(out, self.tree())
    }

    fn on_command(&mut self, bytes: &[u8], out: &mut OutputWriter<'_>) -> Result<(), SeStatus> {
        let request = parse(bytes)?;
        match request["commandId"].as_str() {
            Some("example.reset") => self.count = 0.0,
            #[cfg(feature = "test-hooks")]
            Some("example.panic") => panic!("intentional Configured Counter test panic"),
            #[cfg(feature = "test-hooks")]
            Some("example.timeout") => std::thread::sleep(std::time::Duration::from_secs(3)),
            #[cfg(feature = "test-hooks")]
            Some("example.abort") => std::process::abort(),
            #[cfg(feature = "test-hooks")]
            Some("example.escape-host") => {
                let result_file = request["args"]["resultFile"]
                    .as_str()
                    .ok_or(SEP_INVALID_ARGUMENT)?;
                let host = self.host.take().ok_or(SEP_UNAVAILABLE)?;
                escape_host(host, result_file);
            }
            _ => return Err(SEP_UNAVAILABLE),
        }
        write(
            out,
            json!({"schemaVersion":1,"result":{"count":self.count},"invalidateViews":[VIEW]}),
        )
    }

    fn on_config_changed(&mut self, bytes: &[u8]) -> Result<(), SeStatus> {
        self.config = Config::parse(parse(bytes)?["values"].clone())?;
        if let Some(host) = &self.host {
            host.log(SEP_LOG_INFO, "Configured Counter configuration applied")?;
        }
        Ok(())
    }

    fn on_service_call(
        &mut self,
        bytes: &[u8],
        out: &mut OutputWriter<'_>,
    ) -> Result<(), SeStatus> {
        let request = parse(bytes)?;
        if request["service"] != "org.termul.example.counter"
            || request["version"] != 1
            || request["method"] != "get"
        {
            return Err(SEP_UNAVAILABLE);
        }
        write(
            out,
            json!({"schemaVersion":1,"result":{"count":self.count}}),
        )
    }

    fn destroy(self) -> Result<(), SeStatus> {
        if let Some(host) = self.host {
            host.log(SEP_LOG_INFO, "Configured Counter destroyed")?;
        }
        Ok(())
    }
}
