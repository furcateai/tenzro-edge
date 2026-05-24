// SPDX-License-Identifier: Apache-2.0

//! `WorkSettlement` impl backed by Tenzro nanopayment channels.
//!
//! - [`WorkSettlement::open_channel`] → `nanopayment.open_channel`.
//! - [`WorkSettlement::send_micropayment`] →
//!   `nanopayment.send_nanopayment`. The receipt's `for_work` digest is
//!   carried in the `memo` field (hex-encoded) so the counterparty can
//!   correlate the payment with the receipt it settles.
//! - [`WorkSettlement::close_channel`] → `nanopayment.close_channel`.
//!
//! ## Amount encoding
//!
//! The trait's [`Amount`] carries a decimal string; the SDK takes
//! `u64`. We parse the string as a `u128`, saturate-cast to `u64`, and
//! reject anything that isn't an integer. Sub-unit values (e.g. `"0.001
//! TZN"`) require the caller to convert to the asset's smallest unit
//! before calling — this impl does not infer decimals.

use async_trait::async_trait;
use furcate_inference_core::{
    Amount, ChannelId, ReceiptDigest, SettlementReceipt, WorkSettlement, WorkSettlementError,
};
use tenzro_sdk::error::SdkError;

use crate::client::TenzroHandle;

/// `WorkSettlement` impl backed by Tenzro nanopayments.
pub struct TenzroWorkSettlement {
    handle: TenzroHandle,
    /// Local payer address — the wallet authorising payments.
    payer: String,
}

impl TenzroWorkSettlement {
    /// Construct a Tenzro work settlement gateway. `payer` is the local
    /// node's wallet address (the originator of payments).
    #[must_use]
    pub fn new(handle: TenzroHandle, payer: impl Into<String>) -> Self {
        Self {
            handle,
            payer: payer.into(),
        }
    }
}

fn map_sdk(e: SdkError) -> WorkSettlementError {
    match e {
        SdkError::ConnectionError(s) | SdkError::RpcError(s) => WorkSettlementError::Transient(s),
        SdkError::Timeout => WorkSettlementError::Transient("timeout".into()),
        SdkError::InsufficientFunds {
            required,
            available,
        } => WorkSettlementError::Refused(format!(
            "insufficient funds: required {required}, available {available}"
        )),
        SdkError::SettlementError(s) => WorkSettlementError::Failed(s),
        other => WorkSettlementError::Failed(format!("{other:?}")),
    }
}

/// Parse the trait's decimal-string amount into the SDK's `u64`. We
/// accept only integer values — sub-unit precision is the caller's
/// problem (they have to convert to the asset's smallest unit first).
fn amount_to_u64(a: &Amount) -> Result<u64, WorkSettlementError> {
    if a.value.contains('.') {
        return Err(WorkSettlementError::Refused(format!(
            "sub-unit amounts not supported by tenzro nanopayment ('{}'); pre-convert to smallest unit",
            a.value
        )));
    }
    a.value
        .parse::<u64>()
        .map_err(|e| WorkSettlementError::Refused(format!("invalid amount '{}': {e}", a.value)))
}

#[async_trait]
impl WorkSettlement for TenzroWorkSettlement {
    async fn open_channel(
        &self,
        counterparty: &str,
        deposit: Option<&Amount>,
    ) -> Result<ChannelId, WorkSettlementError> {
        let (deposit_u64, asset) = match deposit {
            Some(a) => (amount_to_u64(a)?, a.asset.clone()),
            // Default asset is TZN; deposit 0 = unfunded channel.
            None => (0, "TZN".to_string()),
        };
        let info = self
            .handle
            .sdk()
            .nanopayment()
            .open_channel(&self.payer, counterparty, deposit_u64, &asset)
            .await
            .map_err(map_sdk)?;
        Ok(ChannelId(info.channel_id))
    }

    async fn send_micropayment(
        &self,
        channel: Option<&ChannelId>,
        _counterparty: &str,
        amount: &Amount,
        for_work: ReceiptDigest,
    ) -> Result<SettlementReceipt, WorkSettlementError> {
        let channel_id = channel.ok_or_else(|| {
            WorkSettlementError::Refused(
                "tenzro nanopayments require a channel — call open_channel first".into(),
            )
        })?;
        let amount_u64 = amount_to_u64(amount)?;
        let memo = hex::encode(for_work);
        let receipt = self
            .handle
            .sdk()
            .nanopayment()
            .send_nanopayment(&channel_id.0, amount_u64, &memo)
            .await
            .map_err(map_sdk)?;
        Ok(SettlementReceipt {
            txid: receipt.payment_id,
            channel: Some(ChannelId(receipt.channel_id)),
            for_work,
        })
    }

    async fn close_channel(&self, channel: &ChannelId) -> Result<(), WorkSettlementError> {
        let _ = self
            .handle
            .sdk()
            .nanopayment()
            .close_channel(&channel.0)
            .await
            .map_err(map_sdk)?;
        Ok(())
    }
}
