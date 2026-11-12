//! Strict schema-v1 manifests, data-only rendering and installed-snapshot normalization.
use super::{
    resources::{validate_id, validate_relative, AdapterResourceReader, RESERVED_IDS},
    HookKind,
};
use regex::Regex;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterOrigin {
    Builtin,
    Custom,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterManifest {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub version: Version,
    pub cli: CliSpec,
    pub config: Vec<ConfigSpec>,
    pub payload: PayloadSpec,
    pub native_events: Vec<NativeEventSpec>,
    pub files: Vec<FileSpec>,
    pub resume: Option<ResumeSpec>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CliSpec {
    pub binaries: Vec<String>,
    pub version_args: Vec<String>,
    pub version_parser: VersionParserSpec,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionParserSpec {
    pub kind: String,
    pub pattern: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSpec {
    pub id: String,
    pub path: String,
    pub format: String,
    pub ops: Option<Vec<ConfigOp>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigOp {
    pub op: String,
    pub key: Vec<String>,
    pub value_json: String,
    pub event: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadSpec {
    pub native_event: String,
    pub session_id: String,
    pub transcript_path: Option<String>,
    pub tool_name: Option<String>,
    pub ok: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeEventSpec {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub hook_kind: Option<HookKind>,
    pub cases: Option<Vec<EventCase>>,
    pub enabled_by_default: bool,
    pub min_schema_version: Option<Version>,
    pub min_delivery_version: Option<Version>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventCase {
    pub pointer: String,
    pub equals: Value,
    pub hook_kind: HookKind,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSpec {
    pub id: String,
    pub source: String,
    pub target: String,
    pub mode: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeSpec {
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default = "home_root")]
    pub path_root: String,
    pub variants: Vec<ResumeVariant>,
}
fn home_root() -> String {
    "home".into()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeVariant {
    pub id: String,
    pub binary: String,
    pub argv: Vec<String>,
    pub requires: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadedAdapter {
    pub manifest: AdapterManifest,
    pub files: BTreeMap<String, String>,
    pub origin: AdapterOrigin,
    pub source_digest: String,
}
#[derive(Clone, Debug)]
pub struct RenderContext {
    pub script_path: String,
    pub endpoint_file: String,
    pub agent_id: String,
}
#[derive(Clone, Debug)]
pub struct NormalizedPayload {
    pub native_event: String,
    pub session_id: String,
    pub transcript_path: Option<String>,
    pub tool_name: Option<String>,
    pub ok: Option<bool>,
    pub kind: HookKind,
}

pub fn load_adapter(
    reader: &dyn AdapterResourceReader,
    id: &str,
    origin: AdapterOrigin,
) -> Result<LoadedAdapter, String> {
    validate_id(id)?;
    if origin == AdapterOrigin::Custom && RESERVED_IDS.contains(&id) {
        return Err("reserved builtin adapter id".into());
    }
    let bytes = reader.read(id, "adapter.toml")?;
    if bytes.len() > 256 * 1024 {
        return Err("manifest exceeds 256 KiB".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "manifest is not UTF-8")?;
    let manifest: AdapterManifest = toml::from_str(text).map_err(|_| {
        "invalid adapter schema (unknown field, type, or missing required value)".to_string()
    })?;
    manifest.validate(id, &origin)?;
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(&bytes);
    let mut files = BTreeMap::new();
    let mut total = 0;
    for file in &manifest.files {
        let bytes = reader.read(id, &file.source)?;
        total += bytes.len();
        if bytes.len() > 1024 * 1024 || total > 4 * 1024 * 1024 {
            return Err("template size limit exceeded".into());
        }
        digest.update((file.id.len() as u64).to_be_bytes());
        digest.update(file.id.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(&bytes);
        let text = String::from_utf8(bytes).map_err(|_| "template is not UTF-8")?;
        validate_placeholders(&text, &["script_path", "endpoint_file", "agent_id"], false)?;
        files.insert(file.id.clone(), text);
    }
    Ok(LoadedAdapter {
        manifest,
        files,
        origin,
        source_digest: format!("{:x}", digest.finalize()),
    })
}

#[derive(Clone, Debug)]
pub struct AdapterEntry {
    pub id: String,
    pub adapter: Option<LoadedAdapter>,
    pub diagnostics: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct RenderedFile {
    pub path: std::path::PathBuf,
    pub bytes: Vec<u8>,
    pub mode: u32,
}

pub fn load_registry(home: &std::path::Path) -> Vec<AdapterEntry> {
    let builtin = super::resources::builtin_reader();
    load_registry_with_reader(home, builtin.as_ref())
}
pub fn load_registry_with_reader(
    home: &std::path::Path,
    builtin: &dyn AdapterResourceReader,
) -> Vec<AdapterEntry> {
    let mut entries = Vec::new();
    for id in RESERVED_IDS {
        let result = load_adapter(builtin, id, AdapterOrigin::Builtin);
        entries.push(match result {
            Ok(adapter) => AdapterEntry {
                id: id.into(),
                adapter: Some(adapter),
                diagnostics: Vec::new(),
            },
            Err(error) => AdapterEntry {
                id: id.into(),
                adapter: None,
                diagnostics: vec![error],
            },
        });
    }
    let custom = super::resources::FilesystemAdapterReader::new(
        home.join(".se-manager/agent-hooks/adapters"),
    );
    match custom.enumerate() {
        Ok(ids) => {
            for id in ids {
                match load_adapter(&custom, &id, AdapterOrigin::Custom) {
                    Ok(adapter) => entries.push(AdapterEntry {
                        id,
                        adapter: Some(adapter),
                        diagnostics: Vec::new(),
                    }),
                    Err(error) if RESERVED_IDS.contains(&id.as_str()) => {
                        // Preserve the valid builtin row; display collision on that row, never shadow it.
                        if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                            entry.diagnostics.push(error);
                        }
                    }
                    Err(error) => entries.push(AdapterEntry {
                        id,
                        adapter: None,
                        diagnostics: vec![error],
                    }),
                }
            }
        }
        Err(error) => entries.push(AdapterEntry {
            id: "custom-root".into(),
            adapter: None,
            diagnostics: vec![error],
        }),
    }
    entries
}

impl LoadedAdapter {
    pub fn resolve_config_path(
        &self,
        config: &ConfigSpec,
        home: &std::path::Path,
    ) -> Result<std::path::PathBuf, String> {
        let (root, relative) = config.path.split_once(':').ok_or("invalid config root")?;
        validate_relative(relative)?;
        if root != "home" {
            return Err("agent root is not registered by this host".into());
        }
        let path = home.join(relative);
        super::resources::reject_symlink_components(&path)?;
        Ok(path)
    }
    pub fn resolve_file_path(
        &self,
        file: &FileSpec,
        home: &std::path::Path,
    ) -> Result<std::path::PathBuf, String> {
        let (root, relative) = file.target.split_once(':').ok_or("invalid file target")?;
        let path = match root {
            "install" => {
                validate_relative(relative)?;
                let install_root = home.join(".se-manager/agent-hooks");
                if self.origin == AdapterOrigin::Custom {
                    validate_id(&self.manifest.id)?;
                    install_root
                        .join("installed")
                        .join(&self.manifest.id)
                        .join("files")
                        .join(relative)
                } else {
                    install_root.join(relative)
                }
            }
            "config" => self.resolve_config_path(
                self.manifest
                    .config
                    .iter()
                    .find(|c| c.id == relative)
                    .ok_or("missing config target")?,
                home,
            )?,
            _ => return Err("invalid file target".into()),
        };
        super::resources::reject_symlink_components(&path)?;
        Ok(path)
    }
    pub fn render_context(&self, home: &std::path::Path) -> Result<RenderContext, String> {
        let hook = self
            .manifest
            .files
            .iter()
            .find(|f| f.id == "hook")
            .ok_or("missing hook file")?;
        Ok(RenderContext {
            script_path: self
                .resolve_file_path(hook, home)?
                .to_str()
                .ok_or("non UTF-8 script path")?
                .into(),
            endpoint_file: home
                .join(".se-manager/agent-hooks/instances/{instance}/endpoint.env")
                .to_str()
                .ok_or("non UTF-8 endpoint path")?
                .into(),
            agent_id: self.manifest.id.clone(),
        })
    }
    pub fn render_files(
        &self,
        context: &RenderContext,
        events: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, String>, String> {
        self.files
            .iter()
            .map(|(id, text)| {
                let mut filtered = String::new();
                for line in text.split_inclusive('\n') {
                    if let Some((_, event)) = line.rsplit_once("// se-event:") {
                        let event = event.trim();
                        if !self.manifest.native_events.iter().any(|e| e.name == event) {
                            return Err("template references unknown native event".into());
                        }
                        if !events.contains(event) {
                            continue;
                        }
                    }
                    filtered.push_str(line);
                }
                Ok((id.clone(), render_template(&filtered, context)?))
            })
            .collect()
    }
    /// Pure renderer: caller supplies the original/baseline bytes. No filesystem writes or probes.
    pub fn render(
        &self,
        home: &std::path::Path,
        events: &BTreeSet<String>,
        originals: &BTreeMap<std::path::PathBuf, Option<Vec<u8>>>,
    ) -> Result<Vec<RenderedFile>, String> {
        let context = self.render_context(home)?;
        let templates = self.render_files(&context, events)?;
        let mut rendered = BTreeMap::new();
        for file in &self.manifest.files {
            let path = self.resolve_file_path(file, home)?;
            rendered.insert(
                path.clone(),
                RenderedFile {
                    path,
                    bytes: templates
                        .get(&file.id)
                        .ok_or("missing template")?
                        .as_bytes()
                        .to_vec(),
                    mode: if file.mode == "0700" { 0o700 } else { 0o600 },
                },
            );
        }
        for config in self
            .filtered_ops(events)
            .iter()
            .filter(|c| c.format != "owned-file")
        {
            let path = self.resolve_config_path(config, home)?;
            if rendered.contains_key(&path) {
                return Err("colliding rendered targets".into());
            }
            let original = originals
                .get(&path)
                .ok_or("missing config baseline")?
                .as_deref();
            let mut value = match (config.format.as_str(), original) {
                (_, None) => serde_json::json!({}),
                ("json-merge", Some(bytes)) => {
                    serde_json::from_slice(bytes).map_err(|_| "invalid original JSON")?
                }
                ("toml-merge", Some(bytes)) => {
                    let text =
                        std::str::from_utf8(bytes).map_err(|_| "invalid original TOML encoding")?;
                    let value: toml::Value =
                        toml::from_str(text).map_err(|_| "invalid original TOML")?;
                    serde_json::to_value(value).map_err(|_| "invalid original TOML value")?
                }
                _ => return Err("unsupported config format".into()),
            };
            for op in config.ops.as_ref().ok_or("missing merge operations")? {
                let data: Value =
                    serde_json::from_str(&op.value_json).map_err(|_| "invalid operation JSON")?;
                apply_merge_op(
                    &mut value,
                    &op.key,
                    &op.op,
                    render_json_value(&data, &context)?,
                )?;
            }
            let mut bytes = if config.format == "json-merge" {
                serde_json::to_vec_pretty(&value).map_err(|_| "JSON serialization failed")?
            } else {
                let value = toml::Value::try_from(&value).map_err(|_| "unsupported TOML value")?;
                toml::to_string_pretty(&value)
                    .map_err(|_| "TOML serialization failed")?
                    .into_bytes()
            };
            if !bytes.ends_with(b"\n") {
                bytes.push(b'\n');
            }
            rendered.insert(
                path.clone(),
                RenderedFile {
                    path,
                    bytes,
                    mode: 0o600,
                },
            );
        }
        Ok(rendered.into_values().collect())
    }
}
pub fn apply_merge_op(
    root: &mut Value,
    key: &[String],
    operation: &str,
    data: Value,
) -> Result<(), String> {
    let (leaf, parents) = key.split_last().ok_or("empty merge path")?;
    let mut cursor = root;
    for segment in parents {
        let object = cursor
            .as_object_mut()
            .ok_or("merge parent is not a table")?;
        cursor = object
            .entry(segment.clone())
            .or_insert_with(|| serde_json::json!({}));
    }
    let object = cursor
        .as_object_mut()
        .ok_or("merge target parent is not a table")?;
    match operation {
        "set" => {
            object.insert(leaf.clone(), data);
        }
        "append-unique" => {
            let array = object
                .entry(leaf.clone())
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .ok_or("append target is not an array")?;
            if !array.contains(&data) {
                array.push(data);
            }
        }
        _ => return Err("unknown merge operation".into()),
    }
    Ok(())
}

fn nonempty(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}
fn unique<'a>(values: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = BTreeSet::new();
    values.into_iter().all(|v| seen.insert(v))
}
pub fn validate_pointer(pointer: &str) -> Result<(), String> {
    if (!pointer.is_empty() && !pointer.starts_with('/')) || pointer.chars().any(char::is_control) {
        return Err("invalid JSON pointer".into());
    }
    let mut chars = pointer.chars();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return Err("invalid JSON pointer escape".into());
        }
    }
    Ok(())
}
fn validate_requirements(req: &[String]) -> Result<(), String> {
    if !unique(req.iter().map(String::as_str))
        || req
            .iter()
            .any(|r| !matches!(r.as_str(), "session_id" | "transcript_path"))
    {
        return Err("invalid resume requirements".into());
    }
    Ok(())
}
impl AdapterManifest {
    pub fn validate(&self, directory_id: &str, origin: &AdapterOrigin) -> Result<(), String> {
        validate_id(&self.id)?;
        if self.schema_version != 1 || self.id != directory_id {
            return Err("schema version or directory id mismatch".into());
        }
        if !nonempty(&self.display_name) || self.display_name.chars().count() > 80 {
            return Err("invalid display name".into());
        }
        if !(1..=8).contains(&self.cli.binaries.len())
            || !unique(self.cli.binaries.iter().map(String::as_str))
            || self
                .cli
                .binaries
                .iter()
                .any(|b| !nonempty(b) || b.contains(['/', '\\', ':']) || b == "." || b == "..")
        {
            return Err("invalid CLI basenames".into());
        }
        if !(1..=8).contains(&self.cli.version_args.len())
            || self.cli.version_args.iter().map(String::len).sum::<usize>() > 4096
            || self.cli.version_args.iter().any(|s| !nonempty(s))
        {
            return Err("invalid version argv".into());
        }
        let parser = &self.cli.version_parser;
        if parser.kind != "anchored-regex"
            || parser.pattern.len() > 512
            || !parser.pattern.starts_with('^')
            || !parser.pattern.ends_with('$')
            || parser.pattern.contains(['\n', '\r'])
            || parser.pattern.contains("(?")
                && ["(?m", "(?s"].iter().any(|s| parser.pattern.contains(s))
        {
            return Err("version parser must be anchored single-line regex".into());
        }
        let regex = Regex::new(&parser.pattern).map_err(|_| "invalid version regex")?;
        if !regex.capture_names().flatten().any(|n| n == "version") {
            return Err("missing version capture".into());
        }
        if !(1..=16).contains(&self.config.len())
            || !unique(self.config.iter().map(|c| c.id.as_str()))
        {
            return Err("invalid config ids/count".into());
        }
        if self.native_events.is_empty() || self.native_events.len() > 128 {
            return Err("invalid event count".into());
        }
        let mut names = BTreeSet::new();
        for event in &self.native_events {
            for name in std::iter::once(&event.name).chain(&event.aliases) {
                if !nonempty(name) || !names.insert(name.clone()) {
                    return Err("duplicate or invalid native event/alias".into());
                }
            }
            match (&event.hook_kind, &event.cases) {
                (Some(kind), None) => {
                    if matches!(kind, HookKind::ToolStart | HookKind::ToolEnd)
                        && event.enabled_by_default
                    {
                        return Err("tool events cannot default on".into());
                    }
                }
                (None, Some(cases)) if !cases.is_empty() => {
                    for case in cases {
                        validate_pointer(&case.pointer)?;
                        if case.equals.is_array() || case.equals.is_object() {
                            return Err("event case must compare scalar".into());
                        }
                        if matches!(case.hook_kind, HookKind::ToolStart | HookKind::ToolEnd)
                            && event.enabled_by_default
                        {
                            return Err("tool events cannot default on".into());
                        }
                    }
                    for (i, case) in cases.iter().enumerate() {
                        if cases[..i]
                            .iter()
                            .any(|c| c.pointer == case.pointer && c.equals == case.equals)
                        {
                            return Err("duplicate event case".into());
                        }
                    }
                }
                _ => return Err("event requires exactly one hook_kind or cases".into()),
            }
        }
        for pointer in std::iter::once(&self.payload.native_event)
            .chain(std::iter::once(&self.payload.session_id))
            .chain(self.payload.transcript_path.iter())
            .chain(self.payload.tool_name.iter())
            .chain(self.payload.ok.iter())
        {
            validate_pointer(pointer)?;
        }
        for config in &self.config {
            validate_id(&config.id)?;
            let (root, path) = config
                .path
                .split_once(':')
                .ok_or("config path needs root")?;
            if root != "home" && !(root == "agent" && *origin == AdapterOrigin::Builtin) {
                return Err("unregistered config root".into());
            }
            validate_relative(path)?;
            if root == "home"
                && path
                    .to_ascii_lowercase()
                    .split('/')
                    .take(2)
                    .eq([".se-manager", "agent-hooks"])
            {
                return Err("configuration cannot target the host hook control directory".into());
            }
            match config.format.as_str() {
                "owned-file" if config.ops.is_none() => {}
                "json-merge" | "toml-merge"
                    if config.ops.as_ref().is_some_and(|ops| !ops.is_empty()) =>
                {
                    for op in config.ops.as_ref().unwrap() {
                        if !matches!(op.op.as_str(), "set" | "append-unique")
                            || op.key.is_empty()
                            || op.key.iter().any(|k| !nonempty(k))
                        {
                            return Err("invalid merge operation".into());
                        }
                        if let Some(event) = &op.event {
                            if !self.native_events.iter().any(|e| &e.name == event) {
                                return Err("unknown operation event".into());
                            }
                        }
                        let value: Value = serde_json::from_str(&op.value_json)
                            .map_err(|_| "invalid operation JSON")?;
                        if op.event.is_none() && op.value_json.contains("{{script_path}}") {
                            return Err("hook registration operation requires event binding".into());
                        }
                        validate_json_templates(&value)?;
                        if config.format == "toml-merge" {
                            validate_toml_value(&value)?;
                        }
                    }
                }
                _ => return Err("invalid config format or operations".into()),
            }
        }
        if !(1..=32).contains(&self.files.len())
            || !unique(self.files.iter().map(|f| f.id.as_str()))
            || !unique(self.files.iter().map(|f| f.target.as_str()))
        {
            return Err("invalid file count or duplicate target".into());
        }
        for file in &self.files {
            validate_id(&file.id)?;
            validate_relative(&file.source)?;
            if !matches!(file.mode.as_str(), "0600" | "0700") {
                return Err("invalid file mode".into());
            }
            let (root, path) = file.target.split_once(':').ok_or("invalid file target")?;
            match root {
                "install" => validate_relative(path)?,
                "config"
                    if self
                        .config
                        .iter()
                        .any(|c| c.id == path && c.format == "owned-file") => {}
                _ => return Err("file target must be install or owned config".into()),
            }
        }
        if !self.files.iter().any(|f| f.id == "hook") {
            return Err("missing hook file".into());
        }
        if !unique(self.config.iter().map(|c| c.path.as_str())) {
            return Err("duplicate config target".into());
        }
        for config in self.config.iter().filter(|c| c.format == "owned-file") {
            if self
                .files
                .iter()
                .filter(|f| f.target == format!("config:{}", config.id))
                .count()
                != 1
            {
                return Err("owned-file needs exactly one template".into());
            }
        }
        if let Some(resume) = &self.resume {
            validate_requirements(&resume.requires)?;
            if !matches!(resume.path_root.as_str(), "home" | "pi-sessions")
                || (*origin == AdapterOrigin::Custom && resume.path_root != "home")
            {
                return Err("invalid resume root".into());
            }
            if self.id == "pi"
                && (resume.path_root != "pi-sessions"
                    || !resume.requires.iter().any(|r| r == "transcript_path"))
            {
                return Err("pi requires exact session file".into());
            }
            if resume.variants.is_empty() || !unique(resume.variants.iter().map(|v| v.id.as_str()))
            {
                return Err("invalid resume variants".into());
            }
            for variant in &resume.variants {
                validate_id(&variant.id)?;
                validate_requirements(&variant.requires)?;
                if !self.cli.binaries.contains(&variant.binary) || variant.argv.len() > 63 {
                    return Err("invalid resume executable/argv".into());
                }
                for arg in &variant.argv {
                    if arg.contains('\0') || arg.chars().any(char::is_control) {
                        return Err("invalid resume argument".into());
                    }
                    validate_placeholders(arg, &["session_id", "transcript_path"], true)?;
                    if arg.contains('$') || arg.contains('`') {
                        return Err("resume shell expansion prohibited".into());
                    }
                    for key in ["session_id", "transcript_path"] {
                        if arg == &format!("{{{{{key}}}}}")
                            && !resume
                                .requires
                                .iter()
                                .chain(&variant.requires)
                                .any(|r| r == key)
                        {
                            return Err("resume token missing requirement".into());
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn parse_version(&self, output: &str) -> Option<Version> {
        // Never scrape a first SemVer from arbitrary wrapper output.
        let line = output
            .strip_suffix("\n")
            .unwrap_or(output)
            .strip_suffix('\r')
            .unwrap_or(output.trim_end_matches('\n'));
        if line.contains(['\n', '\r']) {
            return None;
        }
        let re = Regex::new(&self.cli.version_parser.pattern).ok()?;
        let captures = re.captures(line)?;
        let matched = captures.get(0)?;
        if matched.start() != 0 || matched.end() != line.len() {
            return None;
        }
        Version::parse(captures.name("version")?.as_str()).ok()
    }
}
fn validate_toml_value(value: &Value) -> Result<(), String> {
    match value {
        Value::Null => Err("TOML has no null".into()),
        Value::Number(n) if n.as_u64().is_some_and(|n| n > i64::MAX as u64) => {
            Err("TOML integer overflow".into())
        }
        Value::Array(a) => {
            for v in a {
                validate_toml_value(v)?;
            }
            Ok(())
        }
        Value::Object(o) => {
            for v in o.values() {
                validate_toml_value(v)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
fn validate_json_templates(value: &Value) -> Result<(), String> {
    match value {
        Value::String(s) => {
            validate_placeholders(s, &["script_path", "endpoint_file", "agent_id"], false)
        }
        Value::Array(a) => {
            for v in a {
                validate_json_templates(v)?;
            }
            Ok(())
        }
        Value::Object(o) => {
            for (k, v) in o {
                if k.contains("{{") || k.contains("}}") {
                    return Err("placeholder in JSON key".into());
                }
                validate_json_templates(v)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
pub fn validate_placeholders(text: &str, allowed: &[&str], whole: bool) -> Result<(), String> {
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        if rest[..start].contains("}}") {
            return Err("unbalanced placeholder".into());
        }
        let end = rest[start + 2..]
            .find("}}")
            .ok_or("unbalanced placeholder")?
            + start
            + 2;
        let key = &rest[start + 2..end];
        if !allowed.contains(&key) || (whole && (start != 0 || end + 2 != text.len())) {
            return Err("unsafe template placeholder".into());
        }
        rest = &rest[end + 2..];
    }
    if rest.contains("}}") {
        return Err("unbalanced placeholder".into());
    }
    Ok(())
}
pub fn render_template(text: &str, context: &RenderContext) -> Result<String, String> {
    validate_placeholders(text, &["script_path", "endpoint_file", "agent_id"], false)?;
    validate_id(&context.agent_id)?;
    if !std::path::Path::new(&context.script_path).is_absolute()
        || !std::path::Path::new(&context.endpoint_file).is_absolute()
        || !context
            .endpoint_file
            .ends_with("/instances/{instance}/endpoint.env")
    {
        return Err("template context requires absolute script and instance pattern paths".into());
    }
    // Paths are inserted into quoted script literals, not interpreted as code.
    if [&context.script_path, &context.endpoint_file]
        .iter()
        .any(|v| v.chars().any(char::is_control) || v.contains(['\'', '"', '`', '$', '\\']))
    {
        return Err("unsafe template path literal".into());
    }
    Ok(text
        .replace("{{script_path}}", &context.script_path)
        .replace("{{endpoint_file}}", &context.endpoint_file)
        .replace("{{agent_id}}", &context.agent_id))
}
pub fn render_json_value(value: &Value, context: &RenderContext) -> Result<Value, String> {
    match value {
        Value::String(s) => Ok(Value::String(render_template(s, context)?)),
        Value::Array(a) => Ok(Value::Array(
            a.iter()
                .map(|v| render_json_value(v, context))
                .collect::<Result<_, _>>()?,
        )),
        Value::Object(o) => Ok(Value::Object(
            o.iter()
                .map(|(k, v)| Ok((k.clone(), render_json_value(v, context)?)))
                .collect::<Result<_, String>>()?,
        )),
        _ => Ok(value.clone()),
    }
}
impl LoadedAdapter {
    pub fn effective_events(
        &self,
        version: Option<&Version>,
        options: &BTreeMap<String, bool>,
    ) -> Result<BTreeSet<String>, String> {
        if options
            .keys()
            .any(|k| !self.manifest.native_events.iter().any(|e| &e.name == k))
        {
            return Err("unknown event option".into());
        }
        let mut enabled = BTreeSet::new();
        for event in &self.manifest.native_events {
            let meets = [&event.min_schema_version, &event.min_delivery_version]
                .iter()
                .all(|floor| {
                    floor
                        .as_ref()
                        .is_none_or(|floor| version.is_some_and(|v| v >= floor))
                });
            let requested = options
                .get(&event.name)
                .copied()
                .unwrap_or(event.enabled_by_default);
            if meets && requested {
                enabled.insert(event.name.clone());
            }
            if requested && !meets && event.hook_kind == Some(HookKind::SessionStart) {
                return Err("session identity event unavailable for CLI version".into());
            }
        }
        Ok(enabled)
    }
    pub fn filtered_ops(&self, events: &BTreeSet<String>) -> Vec<ConfigSpec> {
        self.manifest
            .config
            .iter()
            .cloned()
            .map(|mut c| {
                if let Some(ops) = &mut c.ops {
                    ops.retain(|op| op.event.as_ref().is_none_or(|e| events.contains(e)));
                }
                c
            })
            .collect()
    }
    pub fn normalize(&self, payload: &Value) -> Result<NormalizedPayload, String> {
        let p = &self.manifest.payload;
        let event_name = payload
            .pointer(&p.native_event)
            .and_then(Value::as_str)
            .filter(|s| nonempty(s))
            .ok_or("missing native event")?;
        let event = self
            .manifest
            .native_events
            .iter()
            .find(|e| e.name == event_name || e.aliases.iter().any(|a| a == event_name))
            .ok_or("unknown native event")?;
        let kind = if let Some(kind) = event.hook_kind {
            kind
        } else {
            let matched: Vec<_> = event
                .cases
                .as_ref()
                .ok_or("missing event mapping")?
                .iter()
                .filter(|c| payload.pointer(&c.pointer) == Some(&c.equals))
                .collect();
            if matched.len() != 1 {
                return Err("native event case has no unique match".into());
            }
            matched[0].hook_kind
        };
        let session_id = payload
            .pointer(&p.session_id)
            .and_then(Value::as_str)
            .filter(|s| nonempty(s))
            .ok_or("missing session identity")?
            .to_owned();
        let optional_string = |pointer: &Option<String>| -> Result<Option<String>, String> {
            match pointer.as_ref().and_then(|p| payload.pointer(p)) {
                None => Ok(None),
                Some(Value::String(s)) if nonempty(s) => Ok(Some(s.clone())),
                _ => Err("invalid payload string".into()),
            }
        };
        let ok = match p.ok.as_ref().and_then(|p| payload.pointer(p)) {
            None => None,
            Some(Value::Bool(b)) => Some(*b),
            _ => return Err("invalid payload ok boolean".into()),
        };
        Ok(NormalizedPayload {
            native_event: event.name.clone(),
            session_id,
            transcript_path: optional_string(&p.transcript_path)?,
            tool_name: optional_string(&p.tool_name)?,
            ok,
            kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::resources::{EmbeddedAdapterReader, FilesystemAdapterReader};
    use super::*;
    fn builtin(id: &str) -> LoadedAdapter {
        load_adapter(&EmbeddedAdapterReader, id, AdapterOrigin::Builtin).unwrap()
    }
    fn invalid(mutator: impl FnOnce(&mut AdapterManifest)) {
        let mut m = builtin("qin-code").manifest;
        mutator(&mut m);
        assert!(m.validate("qin-code", &AdapterOrigin::Builtin).is_err());
    }
    #[test]
    fn builtin_resources_are_strict_and_shared_between_backends() {
        let fs = FilesystemAdapterReader::new(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/agent-hook-adapters")
                .canonicalize()
                .unwrap(),
        );
        for id in RESERVED_IDS {
            let embedded = builtin(id);
            let disk = load_adapter(&fs, id, AdapterOrigin::Builtin).unwrap();
            assert_eq!(embedded.source_digest, disk.source_digest);
            assert_eq!(embedded.files, disk.files);
        }
    }
    #[test]
    fn unknown_fields_rejected_at_every_table() {
        let claude = serde_json::to_value(builtin("claude-code").manifest).unwrap();
        for pointer in [
            "",
            "/cli",
            "/cli/version_parser",
            "/config/0",
            "/config/0/ops/0",
            "/payload",
            "/native_events/0",
            "/native_events/4/cases/0",
            "/files/0",
            "/resume",
            "/resume/variants/0",
        ] {
            let mut value = claude.clone();
            value
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), serde_json::json!(true));
            assert!(
                serde_json::from_value::<AdapterManifest>(value).is_err(),
                "accepted unknown at {pointer}"
            );
        }
    }
    #[test]
    fn missing_required_fields_and_wrong_types_rejected() {
        let valid = serde_json::to_value(builtin("qin-code").manifest).unwrap();
        for key in [
            "schema_version",
            "id",
            "version",
            "cli",
            "config",
            "payload",
            "native_events",
            "files",
        ] {
            let mut v = valid.clone();
            v.as_object_mut().unwrap().remove(key);
            assert!(
                serde_json::from_value::<AdapterManifest>(v).is_err(),
                "missing {key}"
            );
        }
        let mut v = valid;
        v["schema_version"] = serde_json::json!("1");
        assert!(serde_json::from_value::<AdapterManifest>(v).is_err());
    }
    #[test]
    fn all_ten_existing_hook_kinds_are_accepted_unknown_rejected() {
        for value in [
            "session_start",
            "session_end",
            "prompt_submitted",
            "tool_start",
            "tool_end",
            "needs_approval",
            "blocked",
            "idle",
            "turn_done",
            "error",
        ] {
            let kind: HookKind = serde_json::from_value(serde_json::json!(value)).unwrap();
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::json!(value)
            );
        }
        for bad in [
            "turn_start",
            "turn_end",
            "notification",
            "permission_request",
            "future",
        ] {
            assert!(serde_json::from_value::<HookKind>(serde_json::json!(bad)).is_err());
        }
    }
    #[test]
    fn rejects_manifest_security_violations() {
        invalid(|m| m.schema_version = 2);
        invalid(|m| m.id = "other".into());
        invalid(|m| m.display_name = "bad\nname".into());
        invalid(|m| m.cli.binaries = vec!["/usr/bin/qin-code".into()]);
        invalid(|m| m.cli.version_args = vec![]);
        invalid(|m| m.cli.version_parser.pattern = "(?P<version>.*)".into());
        invalid(|m| m.cli.version_parser.pattern = "^(?P<other>.*)$".into());
        invalid(|m| m.config[0].path = "home:../escape".into());
        invalid(|m| m.config[0].path = "home:a//b".into());
        invalid(|m| m.config[0].path = "home:C:\\escape".into());
        invalid(|m| m.config[0].path = "home:/absolute".into());
        invalid(|m| m.config[0].ops.as_mut().unwrap()[0].event = Some("missing".into()));
        invalid(|m| m.config[0].ops.as_mut().unwrap()[0].event = None);
        invalid(|m| m.config[0].ops.as_mut().unwrap()[0].value_json = "null".into());
        invalid(|m| m.config[0].ops.as_mut().unwrap()[0].key = vec![]);
        invalid(|m| m.files[0].source = "../outside".into());
        invalid(|m| m.files[0].mode = "0755".into());
        invalid(|m| m.files[0].target = "config:settings".into());
        invalid(|m| m.files.push(m.files[0].clone()));
        invalid(|m| m.native_events[1].aliases = vec!["session_start".into()]);
        invalid(|m| m.native_events[4].enabled_by_default = true);
        invalid(|m| m.native_events[0].cases = Some(vec![]));
        invalid(|m| m.payload.native_event = "/bad~2escape".into());
        invalid(|m| {
            m.resume.as_mut().unwrap().variants[0].argv = vec!["--resume={{session_id}}".into()]
        });
        invalid(|m| m.resume.as_mut().unwrap().variants[0].binary = "sh".into());
        invalid(|m| m.resume.as_mut().unwrap().variants[0].argv = vec!["$(touch x)".into()]);
    }
    #[test]
    fn notification_cases_are_exact_and_fail_closed() {
        let a = builtin("claude-code");
        for (value, expected) in [
            ("permission_prompt", HookKind::NeedsApproval),
            ("idle_prompt", HookKind::Idle),
        ] {
            let p = serde_json::json!({"hook_event_name":"Notification","session_id":"s","notification_type":value});
            assert_eq!(a.normalize(&p).unwrap().kind, expected);
        }
        for p in [
            serde_json::json!({"hook_event_name":"Notification","session_id":"s","notification_type":"other"}),
            serde_json::json!({"hook_event_name":"SessionStart"}),
            serde_json::json!({"hook_event_name":"other","session_id":"s"}),
        ] {
            assert!(a.normalize(&p).is_err());
        }
        let mut ambiguous = a;
        let event = ambiguous
            .manifest
            .native_events
            .iter_mut()
            .find(|e| e.name == "Notification")
            .unwrap();
        event.cases.as_mut().unwrap().push(EventCase {
            pointer: "/also".into(),
            equals: serde_json::json!(true),
            hook_kind: HookKind::Blocked,
        });
        assert!(ambiguous.normalize(&serde_json::json!({"hook_event_name":"Notification","session_id":"s","notification_type":"idle_prompt","also":true})).is_err());
    }
    #[test]
    fn payload_types_and_aliases_are_strict() {
        let a = builtin("pi");
        for (name, kind) in [
            ("turn_start", HookKind::PromptSubmitted),
            ("turn_end", HookKind::TurnDone),
            ("session_end", HookKind::SessionEnd),
        ] {
            assert_eq!(
                a.normalize(&serde_json::json!({"event":name,"session_id":"s"}))
                    .unwrap()
                    .kind,
                kind
            );
        }
        for p in [
            serde_json::json!({"event":"agent_start","session_id":""}),
            serde_json::json!({"event":"agent_start","session_id":"s","ok":"yes"}),
            serde_json::json!({"event":"agent_start","session_id":"s","transcript_path":42}),
            serde_json::json!({"event":"agent_start","session_id":"s","tool_name":null}),
        ] {
            assert!(a.normalize(&p).is_err());
        }
    }
    #[test]
    fn version_floors_unknown_version_and_registration_filter() {
        let a = builtin("claude-code");
        assert_eq!(
            a.manifest.parse_version("2.1.261 (Claude Code)\n").unwrap(),
            Version::new(2, 1, 261)
        );
        assert!(a.manifest.parse_version("wrapper 2.1.261").is_none());
        assert!(a
            .manifest
            .parse_version("2.1.261 (Claude Code)\nnoise")
            .is_none());
        assert!(a.effective_events(None, &BTreeMap::new()).is_err());
        let events = a
            .effective_events(Some(&Version::new(2, 1, 260)), &BTreeMap::new())
            .unwrap();
        assert!(!events.contains("SessionEnd"));
        assert!(!events.contains("PreToolUse"));
        for op in a.filtered_ops(&events)[0].ops.as_ref().unwrap() {
            assert!(events.contains(op.event.as_ref().unwrap()));
        }
        let p = builtin("pi");
        let events = p.effective_events(None, &BTreeMap::new()).unwrap();
        let ctx = RenderContext {
            script_path: "/home/me/.pi/extension.ts".into(),
            endpoint_file: "/home/me/.se-manager/agent-hooks/instances/{instance}/endpoint.env"
                .into(),
            agent_id: "pi".into(),
        };
        let files = p.render_files(&ctx, &events).unwrap();
        let text = &files["hook"];
        assert!(text.contains("pi.on(\"agent_start\""));
        assert!(!text.contains("pi.on(\"tool_execution_start\""));
    }
    #[test]
    fn placeholders_are_data_only_and_merge_conflicts_fail() {
        for token in ["{{unknown}}", "{{script_path}", "{{{script_path}}}", "x}}"] {
            assert!(validate_placeholders(token, &["script_path"], false).is_err());
        }
        let mut root = serde_json::json!({"x":[]});
        let key = vec!["x".into()];
        apply_merge_op(&mut root, &key, "append-unique", serde_json::json!({"a":1})).unwrap();
        apply_merge_op(&mut root, &key, "append-unique", serde_json::json!({"a":1})).unwrap();
        assert_eq!(root["x"].as_array().unwrap().len(), 1);
        assert!(apply_merge_op(
            &mut serde_json::json!({"x":1}),
            &key,
            "append-unique",
            Value::Null
        )
        .is_err());
    }
    #[test]
    fn reserved_ids_and_missing_builtin_remain_visible() {
        assert!(load_adapter(&EmbeddedAdapterReader, "pi", AdapterOrigin::Custom).is_err());
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let missing = FilesystemAdapterReader::new(home.join("missing"));
        let registry = load_registry_with_reader(&home, &missing);
        assert_eq!(registry.len(), 3);
        assert!(registry
            .iter()
            .all(|e| e.adapter.is_none() && !e.diagnostics.is_empty()));
        assert!(!home.join(".se-manager").exists());
    }
    #[test]
    fn observed_builtin_version_fixtures_and_rfc6901() {
        for (id, output, expected) in [
            ("claude-code", "2.1.284 (Claude Code)\n", "2.1.284"),
            ("pi", "1.2.0\n", "1.2.0"),
            (
                "qin-code",
                "qin-code v0.81.2165-dev (6585f0000)\n",
                "0.81.2165-dev",
            ),
        ] {
            assert_eq!(
                builtin(id)
                    .manifest
                    .parse_version(output)
                    .unwrap()
                    .to_string(),
                expected
            );
        }
        for pointer in ["", "/", "/a~0b~1c", "/array/0"] {
            validate_pointer(pointer).unwrap();
        }
        for pointer in ["relative", "/bad~", "/bad~3"] {
            assert!(validate_pointer(pointer).is_err());
        }
    }
    #[test]
    fn resource_size_limits_and_utf8_are_enforced() {
        struct Memory {
            manifest: Vec<u8>,
            template: Vec<u8>,
        }
        impl AdapterResourceReader for Memory {
            fn enumerate(&self) -> Result<Vec<String>, String> {
                Ok(vec!["qin-code".into()])
            }
            fn read(&self, _: &str, path: &str) -> Result<Vec<u8>, String> {
                Ok(if path == "adapter.toml" {
                    self.manifest.clone()
                } else {
                    self.template.clone()
                })
            }
        }
        let manifest = EmbeddedAdapterReader
            .read("qin-code", "adapter.toml")
            .unwrap();
        let oversized = Memory {
            manifest: vec![b' '; 256 * 1024 + 1],
            template: vec![],
        };
        assert!(load_adapter(&oversized, "qin-code", AdapterOrigin::Builtin).is_err());
        let oversized = Memory {
            manifest: manifest.clone(),
            template: vec![b'x'; 1024 * 1024 + 1],
        };
        assert!(load_adapter(&oversized, "qin-code", AdapterOrigin::Builtin).is_err());
        let invalid_utf8 = Memory {
            manifest: manifest.clone(),
            template: vec![0xff],
        };
        assert!(load_adapter(&invalid_utf8, "qin-code", AdapterOrigin::Builtin).is_err());
        let mut m = builtin("qin-code").manifest;
        for i in 0..5 {
            let mut file = m.files[0].clone();
            file.id = format!("extra-{i}");
            file.target = format!("install:extra-{i}");
            m.files.push(file);
        }
        let oversized = Memory {
            manifest: toml::to_string(&m).unwrap().into_bytes(),
            template: vec![b'x'; 1024 * 1024],
        };
        assert!(load_adapter(&oversized, "qin-code", AdapterOrigin::Builtin).is_err());
    }
    #[test]
    fn custom_install_targets_cannot_overwrite_other_adapter_or_host_files() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let mut a = builtin("qin-code");
        a.origin = AdapterOrigin::Custom;
        a.manifest.id = "custom-agent".into();
        a.manifest.files[0].target = "install:instances/other/endpoint.env".into();
        let path = a.resolve_file_path(&a.manifest.files[0], &home).unwrap();
        assert!(path.starts_with(home.join(".se-manager/agent-hooks/installed/custom-agent/files")));
    }
    #[test]
    fn rendered_configs_have_only_effective_events_and_original_data() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        for id in RESERVED_IDS {
            let a = builtin(id);
            let events = a
                .effective_events(Some(&Version::new(9, 0, 0)), &BTreeMap::new())
                .unwrap();
            let mut baseline = BTreeMap::new();
            for c in &a.manifest.config {
                if c.format != "owned-file" {
                    baseline.insert(a.resolve_config_path(c, &home).unwrap(), None);
                }
            }
            let rendered = a.render(&home, &events, &baseline).unwrap();
            let config = &a.manifest.config[0];
            let path = a.resolve_config_path(config, &home).unwrap();
            let bytes = &rendered.iter().find(|f| f.path == path).unwrap().bytes;
            if id == "claude-code" {
                let v: Value = serde_json::from_slice(bytes).unwrap();
                let hooks = v["hooks"].as_object().unwrap();
                assert_eq!(hooks.len(), events.len());
                assert!(!hooks.contains_key("PreToolUse"));
                assert!(!hooks.contains_key("PostToolUse"));
            } else if id == "qin-code" {
                let v: toml::Value = toml::from_str(std::str::from_utf8(bytes).unwrap()).unwrap();
                let rules = v["hooks"]["rules"].as_array().unwrap();
                assert_eq!(rules.len(), 4);
                assert!(rules
                    .iter()
                    .all(|r| r["event"].as_str() != Some("post_tool")));
            } else {
                let text = std::str::from_utf8(bytes).unwrap();
                assert!(!text.contains("pi.on(\"tool_execution_"));
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn v3_shell_sender_prefers_complete_env_rejects_partial_and_never_sources_global() {
        use std::{
            fs,
            io::Write,
            os::unix::fs::PermissionsExt,
            process::{Command, Stdio},
        };
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let root = home.join(".se-manager/agent-hooks");
        let bin = home.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let instance = "11111111-1111-1111-1111-111111111111";
        let install = "22222222-2222-2222-2222-222222222222";
        let endpoint = root.join("instances").join(instance).join("endpoint.env");
        fs::create_dir_all(endpoint.parent().unwrap()).unwrap();
        fs::write(&endpoint,format!("SE_AGENT_HOOK_INSTANCE={instance}\nSE_AGENT_HOOK_ENDPOINT=http://127.0.0.1:7777\nSE_AGENT_HOOK_TOKEN=filetoken\n")).unwrap();
        let install_path = root.join("installed/claude-code/install.env");
        fs::create_dir_all(install_path.parent().unwrap()).unwrap();
        fs::write(
            &install_path,
            format!("SE_AGENT_HOOK_INSTALL_ID={install}\n"),
        )
        .unwrap();
        // If a script ever sources either file, it would create the marker.
        let marker = home.join("executed");
        fs::write(
            root.join("endpoint.env"),
            format!("touch '{}'\n", marker.display()),
        )
        .unwrap();
        let capture = home.join("capture");
        let curl = bin.join("curl");
        fs::write(&curl,"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CAPTURE.argv\"\ncat > \"$CAPTURE.body\"\ncat <&3 > \"$CAPTURE.config\"\n").unwrap();
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o700)).unwrap();
        let a = builtin("claude-code");
        let context = a.render_context(&home).unwrap();
        let script = home.join("hook.sh");
        fs::write(
            &script,
            render_template(&a.files["hook"], &context).unwrap(),
        )
        .unwrap();
        let run = |tuple: u8, explicit: &str| {
            for ext in ["argv", "body", "config"] {
                let _ = fs::remove_file(capture.with_extension(ext));
            }
            let mut cmd = Command::new("/bin/sh");
            cmd.arg(&script)
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("CAPTURE", &capture)
                .env("SE_AGENT_HOOK_INSTANCE", explicit)
                .env(
                    "SE_AGENT_HOOK_PTY_ID",
                    "terminal-1794455097514-5",
                )
                .env("SE_AGENT_HOOK_GENERATION", "4")
                .env_remove("SE_AGENT_HOOK_ENDPOINT")
                .env_remove("SE_AGENT_HOOK_TOKEN")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if tuple > 0 {
                cmd.env("SE_AGENT_HOOK_ENDPOINT", "http://127.0.0.1:8888");
            }
            if tuple == 2 {
                cmd.env("SE_AGENT_HOOK_TOKEN", "envtoken");
            }
            let mut child = cmd.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"{\"session_id\":\"s\",\"hook_event_name\":\"SessionStart\"}")
                .unwrap();
            assert!(child.wait().unwrap().success());
        };
        run(2, instance);
        let argv = fs::read_to_string(capture.with_extension("argv")).unwrap();
        assert!(argv.contains("http://127.0.0.1:8888/hooks/claude-code"));
        assert!(!argv.contains("envtoken"));
        assert!(argv.contains("0.05"));
        assert!(argv.contains("0.15"));
        assert!(fs::read_to_string(capture.with_extension("config"))
            .unwrap()
            .contains("Bearer envtoken"));
        run(1, instance);
        assert!(!capture.with_extension("argv").exists());
        run(0, instance);
        assert!(fs::read_to_string(capture.with_extension("config"))
            .unwrap()
            .contains("Bearer filetoken"));
        run(2, "../bad");
        assert!(!capture.with_extension("argv").exists());
        fs::write(
            &install_path,
            format!(
                "SE_AGENT_HOOK_INSTALL_ID={install}\ntouch '{}'\n",
                marker.display()
            ),
        )
        .unwrap();
        run(2, instance);
        assert!(!capture.with_extension("argv").exists());
        assert!(!marker.exists());
    }
    #[cfg(unix)]
    #[test]
    fn qin_sender_preserves_native_bytes_without_encoder_and_rejects_header_injection() {
        use std::{
            fs,
            os::unix::fs::PermissionsExt,
            process::{Command, Stdio},
        };
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let install_file = home.join(".se-manager/agent-hooks/installed/qin-code/install.env");
        fs::create_dir_all(install_file.parent().unwrap()).unwrap();
        fs::write(
            &install_file,
            "SE_AGENT_HOOK_INSTALL_ID=22222222-2222-2222-2222-222222222222\n",
        )
        .unwrap();
        let adapter = builtin("qin-code");
        let script = home.join("hook.sh");
        let rendered = render_template(
            &adapter.files["hook"],
            &adapter.render_context(&home).unwrap(),
        )
        .unwrap();
        for encoder in ["python", "node", "jq"] {
            // Inspect source, not the rendered random HOME path (which can
            // itself contain e.g. "jq"). Runtime spies below still catch calls.
            assert!(!adapter.files["hook"].contains(encoder));
        }
        fs::write(&script, rendered).unwrap();
        let capture = home.join("capture");
        let fakecurl = home.join("fakecurl");
        fs::write(&fakecurl, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CAPTURE.argv\"\ncat > \"$CAPTURE.body\"\ncat <&3 > \"$CAPTURE.config\"\n").unwrap();
        fs::set_permissions(&fakecurl, fs::Permissions::from_mode(0o700)).unwrap();
        let spy = home.join("encoder-invoked");
        // Override curl without adding a non-system directory to PATH. Any
        // accidental encoder invocation is caught before a macOS stub can run.
        let wrapper = r#"curl() { "$FAKE_CURL" "$@"; }
python3() { printf invoked > "$ENCODER_SPY"; return 127; }
python() { printf invoked > "$ENCODER_SPY"; return 127; }
node() { printf invoked > "$ENCODER_SPY"; return 127; }
jq() { printf invoked > "$ENCODER_SPY"; return 127; }
. "$HOOK_SCRIPT"
"#;
        let payload = " {\"tool_name\":\"a\\\"b\\\\c\",\"raw\":\"\\r\\n\",\"n\":42}\r\n\n";
        let session = "session\"\\literal\\n$(not-executed)`not-executed`";
        let run = |event: &str, identity: &str| {
            for extension in ["argv", "body", "config"] {
                let _ = fs::remove_file(capture.with_extension(extension));
            }
            let output = Command::new("/bin/sh")
                .args(["-c", wrapper])
                .env("PATH", "/bin:/usr/bin")
                .env("FAKE_CURL", &fakecurl)
                .env("ENCODER_SPY", &spy)
                .env("HOOK_SCRIPT", &script)
                .env("CAPTURE", &capture)
                .env(
                    "SE_AGENT_HOOK_INSTANCE",
                    "11111111-1111-1111-1111-111111111111",
                )
                .env("SE_AGENT_HOOK_ENDPOINT", "http://127.0.0.1:8888")
                .env("SE_AGENT_HOOK_TOKEN", "envtoken")
                .env(
                    "SE_AGENT_HOOK_PTY_ID",
                    "terminal-1794455097514-5",
                )
                .env("SE_AGENT_HOOK_GENERATION", "4")
                .env("QIN_CODE_HOOK_EVENT", event)
                .env("QIN_CODE_HOOK_SESSION_ID", identity)
                .env("QIN_CODE_SESSION_ID", "wrong-tool-call-variable")
                .env("QIN_CODE_HOOK_PAYLOAD", payload)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(output.status.success());
            assert!(output.stdout.is_empty());
            assert!(!spy.exists());
        };
        run("session_start", session);
        assert_eq!(
            fs::read(capture.with_extension("body")).unwrap(),
            payload.as_bytes()
        );
        let config = fs::read_to_string(capture.with_extension("config")).unwrap();
        let escaped = session.replace('\\', "\\\\").replace('"', "\\\"");
        assert_eq!(config, format!("header = \"Authorization: Bearer envtoken\"\nheader = \"x-se-hook-event: session_start\"\nheader = \"x-se-hook-session-id: {escaped}\"\n"));
        let argv = fs::read_to_string(capture.with_extension("argv")).unwrap();
        assert!(!argv.contains("envtoken"));
        assert!(!argv.contains(session));
        assert!(argv.contains("http://127.0.0.1:8888/hooks/qin-code"));
        for (event, identity) in [
            ("session_start", "bad\rheader"),
            ("session_start", "bad\nheader"),
            ("bad\rheader", session),
            ("bad\nheader", session),
            ("", session),
            ("session_start", ""),
        ] {
            run(event, identity);
            assert!(!capture.with_extension("argv").exists());
        }
        run(&"a".repeat(65), session);
        assert!(!capture.with_extension("argv").exists());
        run("session_start", &"a".repeat(513));
        assert!(!capture.with_extension("argv").exists());
        run("session_start", &"a".repeat(512));
        assert!(capture.with_extension("argv").exists());
        assert!(!home.join("not-executed").exists());
    }
    #[cfg(unix)]
    #[test]
    fn filesystem_rejects_symlink_root_manifest_and_template() {
        use std::os::unix::fs::symlink;
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources/agent-hook-adapters")
            .canonicalize()
            .unwrap();
        symlink(&source, root.join("linked")).unwrap();
        assert!(load_adapter(
            &FilesystemAdapterReader::new(root.join("linked")),
            "pi",
            AdapterOrigin::Builtin
        )
        .is_err());
        std::fs::create_dir_all(root.join("pi/files")).unwrap();
        symlink(source.join("pi/adapter.toml"), root.join("pi/adapter.toml")).unwrap();
        assert!(load_adapter(
            &FilesystemAdapterReader::new(&root),
            "pi",
            AdapterOrigin::Builtin
        )
        .is_err());
        std::fs::remove_file(root.join("pi/adapter.toml")).unwrap();
        std::fs::copy(source.join("pi/adapter.toml"), root.join("pi/adapter.toml")).unwrap();
        symlink(
            source.join("pi/files/extension.ts.tmpl"),
            root.join("pi/files/extension.ts.tmpl"),
        )
        .unwrap();
        assert!(load_adapter(
            &FilesystemAdapterReader::new(&root),
            "pi",
            AdapterOrigin::Builtin
        )
        .is_err());
    }
}
