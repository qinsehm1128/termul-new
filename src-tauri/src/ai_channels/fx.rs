use std::future::Future;

use tokio_util::sync::CancellationToken;

use super::{FxCapabilityStatus, FxFallback, FxRuntimeKind, FX_CAPABILITY_SCHEMA_VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FxRuntimeCapabilities {
    pub wasm: bool,
    pub jspi: bool,
    pub native: bool,
    pub server: bool,
}

impl FxRuntimeCapabilities {
    pub fn status(self) -> FxCapabilityStatus {
        let selected_runtime = if self.wasm && self.jspi {
            FxRuntimeKind::Wasm
        } else if self.native {
            FxRuntimeKind::Native
        } else if self.server {
            FxRuntimeKind::Server
        } else {
            FxRuntimeKind::Unavailable
        };
        FxCapabilityStatus {
            schema_version: FX_CAPABILITY_SCHEMA_VERSION,
            wasm: self.wasm,
            jspi: self.jspi,
            native: self.native,
            server: self.server,
            selected_runtime,
            fallback: FxFallback::AiRouter,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FxExecutionError {
    Unavailable,
    ProxyFailed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct FxRuntime {
    capabilities: FxRuntimeCapabilities,
}

impl FxRuntime {
    pub fn new(capabilities: FxRuntimeCapabilities) -> Self {
        Self { capabilities }
    }

    pub fn status(&self) -> FxCapabilityStatus {
        self.capabilities.status()
    }

    /// Execute through an optional host-owned proxy. The closure receives no
    /// provider key or MCP credential; callers supply only the bounded request
    /// payload. Any unavailable/proxy failure is intentionally returned so the
    /// caller can fall back to AiRouter without coupling MCP Core to Fx.
    pub async fn execute<T, F, Fut>(
        &self,
        cancellation: CancellationToken,
        proxy: F,
    ) -> Result<T, FxExecutionError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, FxExecutionError>>,
    {
        if matches!(self.status().selected_runtime, FxRuntimeKind::Unavailable) {
            return Err(FxExecutionError::Unavailable);
        }
        tokio::select! {
            _ = cancellation.cancelled() => Err(FxExecutionError::Cancelled),
            result = proxy() => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn selects_wasm_only_when_jspi_is_available() {
        assert_eq!(
            FxRuntimeCapabilities {
                wasm: true,
                jspi: true,
                native: true,
                server: true
            }
            .status()
            .selected_runtime,
            FxRuntimeKind::Wasm
        );
        assert_eq!(
            FxRuntimeCapabilities {
                wasm: true,
                jspi: false,
                native: false,
                server: true
            }
            .status()
            .selected_runtime,
            FxRuntimeKind::Server
        );
    }

    #[tokio::test]
    async fn cancellation_and_proxy_failure_are_fallback_signals() {
        let runtime = FxRuntime::new(FxRuntimeCapabilities {
            wasm: false,
            jspi: false,
            native: true,
            server: false,
        });
        assert_eq!(
            runtime
                .execute(CancellationToken::new(), || async {
                    Err::<(), _>(FxExecutionError::ProxyFailed)
                })
                .await,
            Err(FxExecutionError::ProxyFailed)
        );
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            runtime
                .execute(cancellation, || async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    Ok::<_, FxExecutionError>(())
                })
                .await,
            Err(FxExecutionError::Cancelled)
        );
    }

    #[tokio::test]
    async fn unavailable_runtime_never_invokes_proxy() {
        let runtime = FxRuntime::new(FxRuntimeCapabilities {
            wasm: false,
            jspi: false,
            native: false,
            server: false,
        });
        let invoked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let marker = invoked.clone();
        let result = runtime
            .execute(CancellationToken::new(), move || async move {
                marker.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok::<_, FxExecutionError>(())
            })
            .await;
        assert_eq!(result, Err(FxExecutionError::Unavailable));
        assert!(!invoked.load(std::sync::atomic::Ordering::SeqCst));
    }
}
