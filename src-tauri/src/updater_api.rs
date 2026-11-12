//! Desktop-side channel manifest fetch for the insider/nightly updater path.
//!
//! The renderer's `fetchChannelManifest` previously did a `fetch()` to
//! `https://github.com/.../releases/download/...`, which is blocked by CSP
//! (`connect-src` allows only `https://api.github.com`) AND CORS (GitHub
//! release-download assets return no `Access-Control-Allow-Origin`). This
//! module performs the same fetch server-side via reqwest — where CSP/CORS do
//! not apply — and returns the parsed manifest JSON as `IpcResult<Value>`.
//!
//! URL source of truth: [`server_update::UpdateChannel::manifest_url`] —
//! reused (not duplicated) so the desktop command, the standalone server, and
//! the renderer error messages never drift.

use std::sync::OnceLock;
use std::time::Duration;

use crate::commands::IpcResult;
use crate::server_update::UpdateChannel;

/// Desktop-specific User-Agent so GitHub's anonymous request gate doesn't 403.
/// Distinct from `server_update.rs`'s `se-server-updater` so access logs
/// can attribute requests to the desktop manager vs. the standalone server.
const USER_AGENT: &str = "se-manager-updater";

/// Total request timeout — caps a hung manifest endpoint so the periodic
/// check cannot stall indefinitely. Mirrors `server_update.rs`'s 120s cap.
const REQUEST_TIMEOUT_SECS: u64 = 120;

/// Shared reqwest client (constructed once): sets a `User-Agent` so GitHub's
/// anonymous request gate doesn't 403, and a total request timeout so a hung
/// manifest endpoint can't stall the check indefinitely. Mirrors the
/// `embedded_public_key()` `OnceLock<Result<T>>` pattern in `server_update.rs`.
///
/// Returns `Result` so a `ClientBuilder::build()` failure (TLS init, resolver
/// config) propagates as an `Err` that `fetch_channel_manifest` maps to
/// `NETWORK_ERROR` — NOT a panic. (`reqwest::Client::new()` internally
/// `expect`s the build result, so the previous `unwrap_or_else` fallback could
/// unwind the periodic check on a TLS-init failure instead of surfacing an
/// error.)
fn http_client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
                .build()
                .map_err(|e| format!("network error: failed to build updater HTTP client: {e}"))
        })
        .as_ref()
        .map_err(|e| e.to_string())
}

/// Fetch + parse the per-channel manifest.
///
/// Validates `channel` via [`UpdateChannel::parse`] (the single source of
/// truth for the accepted values + URLs), then GETs the manifest with a
/// `User-Agent` + 120s timeout. Returns the parsed manifest JSON, or an
/// `Err(String)` describing the failure (unknown channel, network error,
/// non-2xx, or non-object body).
///
/// No live network is exercised for an unknown channel — the parse fails first.
pub async fn fetch_channel_manifest(channel: &str) -> Result<serde_json::Value, String> {
    let parsed = UpdateChannel::parse(channel)
        .ok_or_else(|| format!("unknown update channel: {channel}"))?;
    let url = parsed.manifest_url();
    log::info!(
        "[updater] fetching channel manifest channel={} url={}",
        channel,
        url
    );

    let response = http_client()?
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("network error fetching channel manifest from {url}: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        log::warn!(
            "[updater] channel manifest fetch failed channel={} url={} status={}",
            channel,
            url,
            status
        );
        return Err(format!("channel manifest {url} returned HTTP {status}"));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("failed to decode channel manifest from {url}: {e}"))?;

    if !body.is_object() {
        log::warn!(
            "[updater] channel manifest is not a JSON object channel={} url={}",
            channel,
            url
        );
        return Err(format!("channel manifest {url} is not a JSON object"));
    }

    log::info!(
        "[updater] channel manifest fetch ok channel={} url={} status={}",
        channel,
        url,
        status
    );
    Ok(body)
}

/// Desktop command: fetch the per-channel update manifest server-side (where
/// CSP/CORS do not apply) and return it as `IpcResult<Value>`. The renderer
/// facade `fetchChannelManifest` invokes this instead of a raw `fetch()` to
/// `github.com`.
///
/// Error codes:
/// - `INVALID_UPDATE_CHANNEL` — `channel` is not `stable`/`insider`/`nightly`
///   (no network call made).
/// - `NETWORK_ERROR` — reqwest failed to send or receive (DNS, TLS, timeout,
///   connection reset).
/// - `UPDATE_CHECK_FAILED` — the manifest endpoint returned a non-2xx status
///   (e.g. 404 for an unreleased insider channel).
/// - `INVALID_UPDATE_INFO` — the response body is not a JSON object.
#[tauri::command]
pub async fn updater_fetch_channel_manifest(
    channel: String,
) -> Result<IpcResult<serde_json::Value>, String> {
    match fetch_channel_manifest(&channel).await {
        Ok(value) => Ok(IpcResult::success(value)),
        Err(message) => {
            let code = if message.starts_with("unknown update channel") {
                "INVALID_UPDATE_CHANNEL"
            } else if message.contains("network error") {
                "NETWORK_ERROR"
            } else if message.contains("returned HTTP") {
                "UPDATE_CHECK_FAILED"
            } else {
                "INVALID_UPDATE_INFO"
            };
            Ok(IpcResult::error(message, code))
        }
    }
}

// ---------------------------------------------------------------------------
// Signed stable update: download and install on the Rust side.
//
// The JS plugin API streams one `Channel` message per network chunk. Each one
// is a `webview.eval` on the main thread plus a store update in the renderer;
// a ~55 MB package arrives in thousands of chunks, which kept the window
// unresponsive until the download finished. Here progress is coalesced to a
// few messages per second, the bytes stay in Rust, and the install (archive
// extraction and bundle swap) runs on a blocking worker.
// ---------------------------------------------------------------------------

/// Minimum spacing between progress messages sent to the renderer.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, serde::Serialize, PartialEq)]
#[serde(tag = "event", content = "data")]
pub enum UpdaterDownloadEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        content_length: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        downloaded: u64,
        content_length: Option<u64>,
    },
    Finished,
}

/// Lets a progress message through at most once per `interval`.
struct ProgressThrottle {
    interval: Duration,
    last: Option<std::time::Instant>,
}

impl ProgressThrottle {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    fn should_emit(&mut self, now: std::time::Instant) -> bool {
        match self.last {
            Some(last) if now.duration_since(last) < self.interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

struct DownloadedPackage {
    update: tauri_plugin_updater::Update,
    bytes: Vec<u8>,
}

fn downloaded_package() -> &'static std::sync::Mutex<Option<DownloadedPackage>> {
    static PACKAGE: OnceLock<std::sync::Mutex<Option<DownloadedPackage>>> = OnceLock::new();
    PACKAGE.get_or_init(|| std::sync::Mutex::new(None))
}

static DOWNLOAD_IN_FLIGHT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

struct DownloadSlot;
impl DownloadSlot {
    fn claim() -> Option<Self> {
        DOWNLOAD_IN_FLIGHT
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .ok()
            .map(|_| Self)
    }
}
impl Drop for DownloadSlot {
    fn drop(&mut self) {
        DOWNLOAD_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Desktop command: download the signed stable update for `version`.
///
/// Error codes: `UPDATE_NOT_AVAILABLE` (no update, or a different version is
/// now published), `DOWNLOAD_FAILED` (check, network or signature failure, or
/// a download already in progress).
#[tauri::command]
pub async fn updater_download(
    app: tauri::AppHandle,
    version: String,
    on_event: tauri::ipc::Channel<UpdaterDownloadEvent>,
) -> Result<IpcResult<()>, String> {
    use tauri_plugin_updater::UpdaterExt;

    let Some(_slot) = DownloadSlot::claim() else {
        return Ok(IpcResult::error(
            "An update download is already in progress",
            "DOWNLOAD_FAILED",
        ));
    };
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(e) => return Ok(IpcResult::error(e.to_string(), "DOWNLOAD_FAILED")),
    };
    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => {
            return Ok(IpcResult::error(
                "No update available to download",
                "UPDATE_NOT_AVAILABLE",
            ))
        }
        Err(e) => {
            log::warn!("[updater] download check failed stable_code=CHECK_FAILED: {e}");
            return Ok(IpcResult::error(e.to_string(), "DOWNLOAD_FAILED"));
        }
    };
    if update.version != version {
        log::warn!(
            "[updater] download version moved stable_code=VERSION_CHANGED requested={version} available={}",
            update.version
        );
        return Ok(IpcResult::error(
            format!("Update {version} is no longer the published version"),
            "UPDATE_NOT_AVAILABLE",
        ));
    }

    log::info!("[updater] download start version={version}");
    let started_at = std::time::Instant::now();
    let mut throttle = ProgressThrottle::new(PROGRESS_INTERVAL);
    let mut downloaded: u64 = 0;
    let mut started = false;
    let result = update
        .download(
            |chunk, content_length| {
                if !started {
                    started = true;
                    let _ = on_event.send(UpdaterDownloadEvent::Started { content_length });
                }
                downloaded += chunk as u64;
                if throttle.should_emit(std::time::Instant::now()) {
                    let _ = on_event.send(UpdaterDownloadEvent::Progress {
                        downloaded,
                        content_length,
                    });
                }
            },
            || {
                let _ = on_event.send(UpdaterDownloadEvent::Finished);
            },
        )
        .await;
    match result {
        Ok(bytes) => {
            log::info!(
                "[updater] download ok version={version} bytes={} elapsed_ms={}",
                bytes.len(),
                started_at.elapsed().as_millis()
            );
            if let Ok(mut slot) = downloaded_package().lock() {
                *slot = Some(DownloadedPackage { update, bytes });
            }
            Ok(IpcResult::success(()))
        }
        Err(e) => {
            log::warn!(
                "[updater] download failed version={version} stable_code=DOWNLOAD_FAILED: {e}"
            );
            Ok(IpcResult::error(e.to_string(), "DOWNLOAD_FAILED"))
        }
    }
}

/// Desktop command: install the package `updater_download` kept for `version`.
/// Extraction and the bundle swap run off the async runtime and the main
/// thread; a failed install keeps the package so the user can retry.
#[tauri::command]
pub async fn updater_install(version: String) -> Result<IpcResult<()>, String> {
    let package = match downloaded_package().lock() {
        Ok(mut slot) => match slot.take() {
            Some(package) if package.update.version == version => package,
            other => {
                *slot = other;
                return Ok(IpcResult::error(
                    "No downloaded update ready to install",
                    "UPDATE_NOT_AVAILABLE",
                ));
            }
        },
        Err(_) => {
            return Ok(IpcResult::error(
                "Update package state is unavailable",
                "INSTALL_FAILED",
            ))
        }
    };
    log::info!("[updater] install start version={version}");
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let result = package.update.install(&package.bytes);
        (package, result)
    })
    .await;
    match joined {
        Ok((_, Ok(()))) => {
            log::info!("[updater] install ok version={version}");
            Ok(IpcResult::success(()))
        }
        Ok((package, Err(e))) => {
            log::warn!(
                "[updater] install failed version={version} stable_code=INSTALL_FAILED: {e}"
            );
            if let Ok(mut slot) = downloaded_package().lock() {
                *slot = Some(package);
            }
            Ok(IpcResult::error(e.to_string(), "INSTALL_FAILED"))
        }
        Err(e) => {
            log::warn!(
                "[updater] install task failed version={version} stable_code=INSTALL_FAILED: {e}"
            );
            Ok(IpcResult::error(e.to_string(), "INSTALL_FAILED"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server_update::UpdateChannel;

    /// An unknown channel must fail WITHOUT any network call — the parse is
    /// the gate, so a bogus value never reaches reqwest.
    #[tokio::test]
    async fn fetch_channel_manifest_rejects_unknown_channel_without_network() {
        let result = fetch_channel_manifest("bogus").await;
        let err = result.expect_err("bogus channel must error");
        assert!(
            err.contains("unknown update channel"),
            "unexpected error: {err}"
        );
    }

    /// Parity guard: the three accepted channels must map to the same URLs
    /// `server_update::UpdateChannel::manifest_url()` returns. If someone
    /// changes the Rust constant, this breaks — prompting a check that the
    /// renderer `CHANNEL_MANIFEST_URLS` constant (the error-message source)
    /// is updated in lockstep so the surfaced error keeps naming the right URL.
    #[test]
    fn channel_manifest_urls_match_server_update() {
        assert_eq!(
            UpdateChannel::Stable.manifest_url(),
            "https://github.com/qinsehm1128/termul-new/releases/latest/download/latest-stable.json"
        );
        assert_eq!(
            UpdateChannel::Insider.manifest_url(),
            "https://github.com/qinsehm1128/termul-new/releases/download/insider/latest-insider.json"
        );
        assert_eq!(
            UpdateChannel::Nightly.manifest_url(),
            "https://github.com/qinsehm1128/termul-new/releases/download/nightly/latest-nightly.json"
        );
    }

    /// Thousands of network chunks must collapse to a handful of IPC
    /// messages: each message is a main-thread `webview.eval`.
    #[test]
    fn progress_throttle_lets_one_message_through_per_interval() {
        let start = std::time::Instant::now();
        let mut throttle = ProgressThrottle::new(Duration::from_millis(200));
        let emitted = (0..5_000u64)
            .filter(|i| throttle.should_emit(start + Duration::from_micros(i * 200)))
            .count();
        // 5000 chunks over one second: the first plus one per 200 ms.
        assert_eq!(emitted, 5);
    }

    #[test]
    fn download_events_match_the_renderer_contract() {
        let progress = serde_json::to_value(UpdaterDownloadEvent::Progress {
            downloaded: 40,
            content_length: Some(100),
        })
        .unwrap();
        assert_eq!(
            progress,
            serde_json::json!({"event": "Progress", "data": {"downloaded": 40, "contentLength": 100}})
        );
        let started = serde_json::to_value(UpdaterDownloadEvent::Started {
            content_length: None,
        })
        .unwrap();
        assert_eq!(
            started,
            serde_json::json!({"event": "Started", "data": {"contentLength": null}})
        );
        assert_eq!(
            serde_json::to_value(UpdaterDownloadEvent::Finished).unwrap(),
            serde_json::json!({"event": "Finished"})
        );
    }

    #[tokio::test]
    async fn install_without_a_downloaded_package_is_not_available() {
        let result = updater_install("9.9.9".into()).await.unwrap();
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["success"], false);
        assert_eq!(value["code"], "UPDATE_NOT_AVAILABLE");
    }
}
