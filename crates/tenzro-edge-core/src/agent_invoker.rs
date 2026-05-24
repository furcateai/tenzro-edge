// SPDX-License-Identifier: Apache-2.0

//! `AgentInvoker` impl backed by Tenzro agent messaging.
//!
//! Each [`AgentCall`] is dispatched via `agent.send_message` (unsigned
//! variant). The SDK exposes a signed variant
//! (`send_message_signed`) that requires the caller to pre-compute both
//! an Ed25519 and an ML-DSA-65 signature; that's outside the scope of
//! this thin adapter — when production routers enforce signing, layer a
//! signing wrapper above this impl rather than burying key handling here.
//!
//! ## Payment tokens
//!
//! [`AgentCall::payment_token`] is forwarded verbatim into the message
//! body under `payment_token`. Tenzro routers that don't enforce
//! payment ignore it; routers that do can validate the token through
//! their own AP2 / nanopayment plumbing.

use async_trait::async_trait;
use furcate_inference_core::{AgentCall, AgentDid, AgentInvoker, AgentInvokerError, AgentReply};
use tenzro_sdk::error::SdkError;

use crate::client::TenzroHandle;

/// `AgentInvoker` impl backed by Tenzro agent messaging.
pub struct TenzroAgentInvoker {
    handle: TenzroHandle,
}

impl TenzroAgentInvoker {
    /// Construct a Tenzro agent invoker.
    #[must_use]
    pub const fn new(handle: TenzroHandle) -> Self {
        Self { handle }
    }
}

fn map_sdk(e: SdkError) -> AgentInvokerError {
    match e {
        SdkError::ConnectionError(s) | SdkError::RpcError(s) => AgentInvokerError::Transient(s),
        SdkError::Timeout => AgentInvokerError::Transient("timeout".into()),
        SdkError::NotFound(s) => AgentInvokerError::Unreachable(s),
        SdkError::AuthenticationError(s) => AgentInvokerError::PaymentRefused(format!("auth: {s}")),
        SdkError::InsufficientFunds {
            required,
            available,
        } => AgentInvokerError::PaymentRefused(format!(
            "insufficient funds: required {required}, available {available}"
        )),
        other => AgentInvokerError::Remote(format!("{other:?}")),
    }
}

#[async_trait]
impl AgentInvoker for TenzroAgentInvoker {
    async fn invoke(&self, call: AgentCall) -> Result<AgentReply, AgentInvokerError> {
        // Bundle the request + optional payment token into a single JSON
        // body. Tenzro `send_message` takes a string, so we serialise.
        let body = serde_json::json!({
            "request": call.request,
            "payment_token": call.payment_token,
            "timeout_secs": call.timeout_secs,
        });
        let body_str = serde_json::to_string(&body)
            .map_err(|e| AgentInvokerError::Remote(format!("encode body: {e}")))?;

        let resp = self
            .handle
            .sdk()
            .agent()
            .send_message(&call.from.0, &call.to.0, &body_str)
            .await
            .map_err(map_sdk)?;

        // Tenzro's `AgentMessageResponse` is delivery confirmation, not
        // the callee's reply payload — the actual reply arrives via the
        // SDK's message router. Without a synchronous reply RPC, we
        // package the delivery receipt as the "response" so callers can
        // observe `message_id` + `status`. Layers that want sync
        // request/response should poll for reply messages above this.
        let response = serde_json::json!({
            "delivery": {
                "message_id": resp.message_id,
                "status": resp.status,
                "timestamp": resp.timestamp,
                "signed": resp.signed,
            },
        });

        Ok(AgentReply {
            from: AgentDid(resp.from),
            response,
        })
    }
}
