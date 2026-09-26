//! Process shutdown for every `saxo-rust` mode.
//!
//! Each mode runs as PID 1 in its container, and PID 1 gets no default action
//! for SIGTERM. Without a handler, Kubernetes' SIGTERM is ignored and the pod
//! keeps working until the grace period ends in SIGKILL -- at an arbitrary
//! instant, possibly after Saxo rotated the refresh token and before the
//! rotation became durable, which leaves every other pod holding a consumed
//! token. Handling SIGTERM lets a process stop starting rotations and finish
//! the one in flight before it exits.

use std::time::Duration;

use tracing::{info, warn};

use crate::state::AppState;

/// Well inside the default 30-second termination grace period.
const SAXO_REFRESH_DRAIN_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a server keeps accepting after SIGTERM, so traffic stops arriving
/// through the Service before the listener closes.
pub const ENDPOINT_REMOVAL_GRACE: Duration = Duration::from_secs(5);

/// SIGTERM/SIGINT listeners, registered when constructed.
pub struct ShutdownSignal {
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    interrupt: Option<tokio::signal::unix::Signal>,
}

impl ShutdownSignal {
    /// Registers the handlers now: a SIGTERM that reaches PID 1 before any
    /// handler exists is discarded, not queued.
    pub fn listen() -> Self {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let register = |kind: SignalKind, name: &str| match signal(kind) {
                Ok(stream) => Some(stream),
                Err(err) => {
                    warn!("{name} handler unavailable: {err}");
                    None
                }
            };
            Self {
                terminate: register(SignalKind::terminate(), "SIGTERM"),
                interrupt: register(SignalKind::interrupt(), "SIGINT"),
            }
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }

    /// Resolves on the first SIGTERM or SIGINT.
    pub async fn received(self) {
        #[cfg(unix)]
        {
            let Self {
                terminate,
                interrupt,
            } = self;
            let wait = |stream: Option<tokio::signal::unix::Signal>| async move {
                match stream {
                    Some(mut stream) => {
                        stream.recv().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                _ = wait(terminate) => info!("SIGTERM received; shutting down"),
                _ = wait(interrupt) => info!("SIGINT received; shutting down"),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            info!("Ctrl-C received; shutting down");
        }
    }
}

/// Stops this process from starting Saxo token rotations and waits for one
/// already in flight to become durable.
pub async fn drain_saxo_session_refreshes(state: &AppState) {
    if state
        .drain_saxo_session_refreshes(SAXO_REFRESH_DRAIN_TIMEOUT)
        .await
    {
        info!("no Saxo token rotation in flight; shutdown can proceed");
    }
}

/// The graceful-shutdown future for an HTTP server: on the signal, stop
/// rotating at once, let an in-flight rotation land, then give the Service
/// time to stop routing here before the listener closes.
pub async fn server_shutdown(signal: ShutdownSignal, state: std::sync::Arc<AppState>) {
    signal.received().await;
    drain_saxo_session_refreshes(&state).await;
    tokio::time::sleep(ENDPOINT_REMOVAL_GRACE).await;
}
