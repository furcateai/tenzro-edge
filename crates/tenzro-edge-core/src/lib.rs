// SPDX-License-Identifier: Apache-2.0

//! # `tenzro-edge-core`
//!
//! Pi-class Tenzro participation runtime. Implements the Tier-1 trait surface
//! defined in `furcate-inference-core` and `furcate-mesh-core` against the
//! Tenzro Network via `tenzro-sdk-rust`.
//!
//! ## Trait impls
//!
//! From `furcate-inference-core`:
//! - [`TenzroAttester`] — `Attester` via Tenzro crypto + identity (TDIP)
//! - [`TenzroReceiptSink`] — `ReceiptSink` via `tenzro_signMessage` (DPoP)
//! - [`TenzroRemoteInference`] — `RemoteInferenceProvider` via Tenzro inference
//! - [`TenzroModelSource`] — `ModelSource` for `tenzro://` URIs via iroh
//! - [`TenzroStorage`] — `StorageBackend` via iroh blobs
//! - [`TenzroAgentRegistry`] — `AgentRegistry` via Tenzro agent + skill
//! - [`TenzroAgentInvoker`] — `AgentInvoker` via Tenzro agent + AP2
//! - [`TenzroWorkSettlement`] — `WorkSettlement` via Tenzro nanopayment
//!
//! From `furcate-mesh-core`:
//! - [`TenzroDiscovery`] — `DiscoveryBackend` via Tenzro providers
//! - [`TenzroBroker`] — `WorkBroker` via Tenzro task marketplace
//!
//! ## Fail-soft posture
//!
//! Every impl maps unreachable-Tenzro into the `Transient`/`Unreachable`
//! error variants of its respective trait — never propagates a hard failure
//! to the agent loop.

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms, unreachable_pub)]

mod agent_invoker;
mod agent_registry;
mod attester;
mod auth;
mod broker;
mod client;
mod discovery;
mod dpop_rpc;
mod model_source;
mod receipt_sink;
mod remote_inference;
mod storage_backend;
mod work_settlement;

pub use agent_invoker::TenzroAgentInvoker;
pub use agent_registry::TenzroAgentRegistry;
pub use attester::TenzroAttester;
pub use auth::DpopSigner;
pub use broker::TenzroBroker;
pub use client::{TenzroEdgeConfig, TenzroEdgeError, TenzroHandle};
pub use discovery::TenzroDiscovery;
pub use model_source::TenzroModelSource;
pub use receipt_sink::TenzroReceiptSink;
pub use remote_inference::TenzroRemoteInference;
pub use storage_backend::TenzroStorage;
pub use work_settlement::TenzroWorkSettlement;
