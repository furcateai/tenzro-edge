// SPDX-License-Identifier: Apache-2.0

//! `RemoteInferenceProvider` impl backed by Tenzro inference.
//!
//! Translates a [`InferRequest::TextCompletion`] into a Tenzro
//! `inference.request` call and packages the result back into an
//! [`InferResponse::TextCompletion`].
//!
//! Tensor inference is currently declined — Tenzro's inference RPC
//! accepts only text prompts. The trait's `can_serve` filters tensor
//! requests so the caller falls back to a local Engine.

use async_trait::async_trait;
use furcate_inference_core::{
    InferRequest, InferResponse, RemoteInferenceError, RemoteInferenceProvider, RemoteProviderId,
};
use tenzro_sdk::error::SdkError;

use crate::client::TenzroHandle;

/// `RemoteInferenceProvider` impl that delegates text completions to a
/// configured Tenzro model.
pub struct TenzroRemoteInference {
    id: RemoteProviderId,
    handle: TenzroHandle,
    /// Default `model_id` to call when the request does not name one.
    /// Tenzro's model registry is the source of valid ids; on a Pi this
    /// is typically a small text model the operator pre-selected.
    default_model_id: String,
}

impl TenzroRemoteInference {
    /// Construct a remote inference provider with `default_model_id`.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        handle: TenzroHandle,
        default_model_id: impl Into<String>,
    ) -> Self {
        Self {
            id: RemoteProviderId(id.into()),
            handle,
            default_model_id: default_model_id.into(),
        }
    }
}

fn map_sdk(e: SdkError) -> RemoteInferenceError {
    match e {
        SdkError::ConnectionError(s) | SdkError::RpcError(s) => RemoteInferenceError::Transient(s),
        SdkError::Timeout => RemoteInferenceError::Transient("timeout".into()),
        SdkError::NotFound(s) => RemoteInferenceError::Declined(format!("not found: {s}")),
        SdkError::InvalidParameter(s) => RemoteInferenceError::Declined(format!("invalid: {s}")),
        SdkError::InferenceError(s) => RemoteInferenceError::Failed(s),
        SdkError::InsufficientFunds {
            required,
            available,
        } => RemoteInferenceError::Declined(format!(
            "insufficient funds: required {required}, available {available}"
        )),
        other => RemoteInferenceError::Failed(format!("{other:?}")),
    }
}

#[async_trait]
impl RemoteInferenceProvider for TenzroRemoteInference {
    fn id(&self) -> RemoteProviderId {
        self.id.clone()
    }

    async fn can_serve(&self, req: &InferRequest) -> bool {
        matches!(req, InferRequest::TextCompletion { .. })
    }

    async fn infer(&self, req: InferRequest) -> Result<InferResponse, RemoteInferenceError> {
        let InferRequest::TextCompletion {
            prompt, max_tokens, ..
        } = req
        else {
            return Err(RemoteInferenceError::Declined(
                "tenzro inference only supports text completion".into(),
            ));
        };

        let result = self
            .handle
            .sdk()
            .inference()
            .request(&self.default_model_id, &prompt, max_tokens)
            .await
            .map_err(map_sdk)?;

        // The SDK's `InferenceResult` carries `tokens_used` (combined). We
        // do not have a separate prompt/completion split from the RPC, so
        // attribute everything to `completion_tokens` and set
        // `prompt_tokens` to 0 — accurate accounting requires either an
        // SDK upgrade or local tokenisation, neither of which we want to
        // bake into this thin adapter.
        let completion_tokens = u32::try_from(result.tokens_used).unwrap_or(u32::MAX);

        Ok(InferResponse::TextCompletion {
            text: result.output,
            completion_tokens,
            prompt_tokens: 0,
            finish_reason: "tenzro".into(),
        })
    }
}
