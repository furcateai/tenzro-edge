// SPDX-License-Identifier: Apache-2.0

//! `WorkBroker` impl backed by Tenzro's task marketplace.
//!
//! [`WorkBroker::offer`] posts a task via `task.post_task`, then polls
//! `task.get_task` until the task reaches a terminal status. The
//! `task_type` field is forwarded from [`WorkOffer::work_type`]
//! verbatim — Tenzro's marketplace already speaks free-form work types,
//! so no translation is needed.
//!
//! ## Polling
//!
//! Tenzro does not currently expose a task-completion subscription, so
//! we poll on a fixed cadence (default 2s). Cadence is configurable via
//! [`TenzroBroker::with_poll_interval`]; the per-offer
//! [`WorkOffer::deadline_secs`] caps total wait time and surfaces as a
//! [`WorkOutcome::Failed`] when exceeded.
//!
//! ## Payment + poster
//!
//! `max_price` is forwarded verbatim into the SDK call (Tenzro takes
//! `u128`). `poster` is the constructor-supplied address — typically
//! the local node's wallet — and is used as the payer the marketplace
//! charges at settlement.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use furcate_mesh_core::PeerId;
use furcate_mesh_core::extensions::{WorkBroker, WorkBrokerError, WorkOffer, WorkOutcome};
use tenzro_sdk::error::SdkError;
use tenzro_sdk::types::Address;

use crate::client::TenzroHandle;

/// `WorkBroker` impl backed by Tenzro's task marketplace.
pub struct TenzroBroker {
    handle: TenzroHandle,
    /// Address that posts tasks (and is charged on settlement).
    poster: Address,
    poll_interval: Duration,
    /// Default deadline when [`WorkOffer::deadline_secs`] is `None`.
    default_deadline: Duration,
}

impl TenzroBroker {
    /// Construct a Tenzro broker. `poster_hex` may be `0x`-prefixed or
    /// bare; 20-byte EVM addresses are left-padded to 32 bytes.
    ///
    /// # Errors
    /// Returns [`WorkBrokerError::Failed`] when `poster_hex` is not a
    /// valid 20- or 32-byte hex address.
    pub fn new(handle: TenzroHandle, poster_hex: &str) -> Result<Self, WorkBrokerError> {
        let poster = Address::from_hex(poster_hex).ok_or_else(|| {
            WorkBrokerError::Failed(format!("invalid poster address: {poster_hex}"))
        })?;
        Ok(Self {
            handle,
            poster,
            poll_interval: Duration::from_secs(2),
            default_deadline: Duration::from_secs(300),
        })
    }

    /// Override the completion-poll cadence (default 2s).
    #[must_use]
    pub const fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Override the default deadline applied when [`WorkOffer::deadline_secs`]
    /// is `None` (default 5 minutes).
    #[must_use]
    pub const fn with_default_deadline(mut self, deadline: Duration) -> Self {
        self.default_deadline = deadline;
        self
    }
}

fn map_sdk(e: SdkError) -> WorkBrokerError {
    match e {
        SdkError::ConnectionError(s) | SdkError::RpcError(s) => WorkBrokerError::Unreachable(s),
        SdkError::Timeout => WorkBrokerError::Transient("timeout".into()),
        SdkError::InsufficientFunds {
            required,
            available,
        } => WorkBrokerError::Failed(format!(
            "insufficient funds: required {required}, available {available}"
        )),
        other => WorkBrokerError::Failed(format!("{other:?}")),
    }
}

/// Tenzro's `TaskInfo::status` is `serde_json::Value`. Pull out the
/// string form, lowercasing for uniform matching.
fn status_str(v: &serde_json::Value) -> String {
    v.as_str().map(str::to_lowercase).unwrap_or_default()
}

#[async_trait]
impl WorkBroker for TenzroBroker {
    async fn offer(&self, offer: WorkOffer) -> Result<WorkOutcome, WorkBrokerError> {
        // Tenzro's `post_task` takes the input as a UTF-8 string. We
        // base64 the bytes so binary requests survive — the executor is
        // expected to base64-decode on its side. The mesh-core trait is
        // already deliberately opaque about wire format, so this is a
        // transport-layer detail.
        use base64::Engine as _;
        let input_b64 = base64::engine::general_purpose::STANDARD.encode(&offer.request);
        let max_price = offer.max_price.unwrap_or(u128::MAX);
        let title = format!("furcate:{}", offer.work_type);
        let description = "Posted by furcate WorkBroker (tenzro-edge)".to_string();

        let posted = self
            .handle
            .sdk()
            .task()
            .post_task(
                &title,
                &description,
                &offer.work_type,
                max_price,
                &input_b64,
                &self.poster,
            )
            .await
            .map_err(map_sdk)?;

        let task_id = posted.task_id;
        let deadline = offer
            .deadline_secs
            .map_or(self.default_deadline, |s| Duration::from_secs(u64::from(s)));
        let started = Instant::now();

        loop {
            if started.elapsed() > deadline {
                return Ok(WorkOutcome::Failed {
                    reason: format!("deadline {deadline:?} exceeded for task {task_id}"),
                });
            }

            let info = self
                .handle
                .sdk()
                .task()
                .get_task(&task_id)
                .await
                .map_err(map_sdk)?;

            match status_str(&info.status).as_str() {
                "completed" => {
                    // Output is base64-encoded bytes from the executor.
                    let output = info.output.unwrap_or_default();
                    let response_bytes = base64::engine::general_purpose::STANDARD
                        .decode(output.as_bytes())
                        .map_err(|e| {
                            WorkBrokerError::Failed(format!(
                                "task {task_id} output is not valid base64: {e}"
                            ))
                        })?;
                    let executor = info
                        .assigned_agent
                        .as_deref()
                        .map_or(PeerId([0u8; 32]), |a| {
                            PeerId(*blake3::hash(a.as_bytes()).as_bytes())
                        });
                    return Ok(WorkOutcome::Completed {
                        response: Bytes::from(response_bytes),
                        executor,
                    });
                }
                "failed" => {
                    return Ok(WorkOutcome::Failed {
                        reason: format!("task {task_id} failed"),
                    });
                }
                "cancelled" => {
                    return Ok(WorkOutcome::Refused {
                        reason: format!("task {task_id} cancelled"),
                    });
                }
                _ => {
                    // Still pending / running — wait and re-poll.
                    tokio::time::sleep(self.poll_interval).await;
                }
            }
        }
    }
}
