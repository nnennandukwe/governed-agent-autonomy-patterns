//! Serialize host probes and bound the caller's wait without granting late results authority.
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gaap::contracts::ProtectedEffectRequest;
use tokio_util::sync::CancellationToken;

use crate::{McpError, SubjectObservation, SubjectObserver};

struct Probe {
    effect: ProtectedEffectRequest,
    reply: mpsc::Sender<Result<SubjectObservation, McpError>>,
}

#[derive(Clone)]
pub(crate) struct ObserverPort {
    sender: mpsc::SyncSender<Probe>,
    cancellation: CancellationToken,
    timeout: Duration,
}

impl ObserverPort {
    pub fn start(
        mut observer: impl SubjectObserver + 'static,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> Result<Self, McpError> {
        let (sender, receiver) = mpsc::sync_channel::<Probe>(1);
        // A timed-out callback can't be killed safely. Its thread stays quarantined; no new
        // callback is started, and its eventual reply has no live receiver.
        std::thread::Builder::new()
            .name("gaap-mcp-observer".into())
            .spawn(move || {
                while let Ok(probe) = receiver.recv() {
                    let result = observer.observe(&probe.effect);
                    let _ = probe.reply.send(result);
                }
            })
            .map_err(|error| McpError::new("mcp.observer_start", error.to_string()))?;
        Ok(Self {
            sender,
            cancellation,
            timeout,
        })
    }

    pub fn observe(
        &self,
        effect: &ProtectedEffectRequest,
        run_deadline: Instant,
        after_dispatch: bool,
    ) -> Result<SubjectObservation, McpError> {
        let cancelled = self.cancellation.is_cancelled();
        if cancelled && !after_dispatch {
            return Err(McpError::new(
                "mcp.observer_cancelled",
                "session is terminal",
            ));
        }
        let remaining = run_deadline.saturating_duration_since(Instant::now());
        let budget = if after_dispatch && (cancelled || remaining.is_zero()) {
            self.timeout.min(Duration::from_millis(250))
        } else {
            self.timeout.min(remaining)
        };
        if budget.is_zero() {
            self.cancellation.cancel();
            return Err(McpError::new(
                "mcp.observer_timeout",
                "host probe deadline expired",
            ));
        }
        let (reply, receiver) = mpsc::channel();
        self.sender
            .try_send(Probe {
                effect: effect.clone(),
                reply,
            })
            .map_err(|_| {
                self.cancellation.cancel();
                McpError::new(
                    "mcp.observer_unavailable",
                    "host probe worker is unavailable",
                )
            })?;
        let mut deadline = Instant::now() + budget;
        let mut reconciliation_cancelled = cancelled;
        loop {
            if after_dispatch && !reconciliation_cancelled && self.cancellation.is_cancelled() {
                reconciliation_cancelled = true;
                deadline = deadline.min(Instant::now() + Duration::from_millis(250));
            }
            if !after_dispatch && !cancelled && self.cancellation.is_cancelled() {
                return Err(McpError::new(
                    "mcp.observer_cancelled",
                    "host probe wait cancelled",
                ));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.cancellation.cancel();
                return Err(McpError::new(
                    "mcp.observer_timeout",
                    "host probe exceeded its deadline; late results are ignored",
                ));
            }
            match receiver.recv_timeout(remaining.min(Duration::from_millis(10))) {
                Ok(result) => {
                    if Instant::now() >= deadline {
                        self.cancellation.cancel();
                        return Err(McpError::new(
                            "mcp.observer_timeout",
                            "late host probe result ignored",
                        ));
                    }
                    if !after_dispatch && !cancelled && self.cancellation.is_cancelled() {
                        return Err(McpError::new(
                            "mcp.observer_cancelled",
                            "host probe wait cancelled",
                        ));
                    }
                    if result.is_err() {
                        self.cancellation.cancel();
                    }
                    return result;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.cancellation.cancel();
                    return Err(McpError::new(
                        "mcp.observer_unavailable",
                        "host probe worker exited",
                    ));
                }
            }
        }
    }
}
