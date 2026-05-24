// SPDX-License-Identifier: Apache-2.0

//! `AgentRegistry` impl backed by Tenzro agent + skill + identity.
//!
//! - [`AgentRegistry::list`] → `agent.list_agents()`, mapping each
//!   `AgentIdentity` to an [`AgentDescriptor`].
//! - [`AgentRegistry::search`] → `skill.search(query)`, mapping each
//!   `SkillInfo` to a descriptor keyed on `creator_did`. The skill name +
//!   tags become the descriptor's `skills` field.
//! - [`AgentRegistry::resolve`] → `identity.resolve(did)`.
//!
//! The Tenzro registry exposes more metadata than this trait surface
//! consumes; the bits the trait doesn't model (versions, on-chain
//! addresses) are stashed under `AgentDescriptor::meta` so consumers can
//! pull them out without us teaching the trait about Tenzro shapes.

use async_trait::async_trait;
use furcate_inference_core::{AgentDescriptor, AgentDid, AgentRegistry, AgentRegistryError};
use serde_json::Value;
use tenzro_sdk::error::SdkError;

use crate::client::TenzroHandle;

/// `AgentRegistry` impl backed by Tenzro.
pub struct TenzroAgentRegistry {
    handle: TenzroHandle,
}

impl TenzroAgentRegistry {
    /// Construct a Tenzro agent registry.
    #[must_use]
    pub const fn new(handle: TenzroHandle) -> Self {
        Self { handle }
    }
}

fn map_sdk(e: SdkError) -> AgentRegistryError {
    match e {
        SdkError::ConnectionError(s) | SdkError::RpcError(s) => AgentRegistryError::Transient(s),
        SdkError::Timeout => AgentRegistryError::Transient("timeout".into()),
        other => AgentRegistryError::Lookup(format!("{other:?}")),
    }
}

/// Pull a string out of a `serde_json::Value` that might be either a
/// string or wrapped in `{ "0x...": ... }` — Tenzro's address fields use
/// the latter shape in some RPC responses.
fn json_to_did(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    v.to_string()
}

#[async_trait]
impl AgentRegistry for TenzroAgentRegistry {
    async fn list(&self) -> Result<Vec<AgentDescriptor>, AgentRegistryError> {
        let agents = self
            .handle
            .sdk()
            .agent()
            .list_agents()
            .await
            .map_err(map_sdk)?;
        Ok(agents
            .into_iter()
            .map(|a| {
                let mut meta = serde_json::Map::new();
                if !a.version.is_empty() {
                    meta.insert("version".into(), Value::String(a.version));
                }
                meta.insert("address".into(), a.address);
                meta.insert("creator".into(), a.creator);
                AgentDescriptor {
                    did: AgentDid(a.agent_id),
                    name: a.name,
                    skills: Vec::new(),
                    endpoint: None,
                    meta,
                }
            })
            .collect())
    }

    async fn search(&self, query: &str) -> Result<Vec<AgentDescriptor>, AgentRegistryError> {
        let skills = self
            .handle
            .sdk()
            .skill()
            .search(query)
            .await
            .map_err(map_sdk)?;
        Ok(skills
            .into_iter()
            .map(|s| {
                let mut tags = s.tags;
                if !s.name.is_empty() {
                    tags.push(s.name.clone());
                }
                let mut meta = serde_json::Map::new();
                meta.insert("skill_id".into(), Value::String(s.skill_id));
                if !s.version.is_empty() {
                    meta.insert("version".into(), Value::String(s.version));
                }
                if !s.description.is_empty() {
                    meta.insert("description".into(), Value::String(s.description));
                }
                AgentDescriptor {
                    did: AgentDid(s.creator_did),
                    name: s.name,
                    skills: tags,
                    endpoint: None,
                    meta,
                }
            })
            .collect())
    }

    async fn resolve(&self, did: &AgentDid) -> Result<AgentDescriptor, AgentRegistryError> {
        let info = self
            .handle
            .sdk()
            .identity()
            .resolve(&did.0)
            .await
            .map_err(map_sdk)?;
        let mut meta = serde_json::Map::new();
        if !info.status.is_empty() {
            meta.insert("status".into(), Value::String(info.status));
        }
        meta.insert("is_human".into(), Value::Bool(info.is_human));
        meta.insert("is_machine".into(), Value::Bool(info.is_machine));
        meta.insert("key_count".into(), Value::Number(info.key_count.into()));
        Ok(AgentDescriptor {
            did: AgentDid(json_to_did(&Value::String(info.did))),
            name: info.display_name,
            skills: Vec::new(),
            endpoint: None,
            meta,
        })
    }
}
