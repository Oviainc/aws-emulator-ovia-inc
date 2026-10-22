// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol service.
//!
//! The agent-runtime resource (`/runtimes/`) is persisted here: one
//! [`AgentRuntimeRecord`] per `agentRuntimeId`, each holding every version that
//! has ever been published for it, so `GetAgentRuntime?version=`,
//! `ListAgentRuntimes` and `ListAgentRuntimeVersions` all read one store.
//!
//! Helpers the agent-runtime handlers share (the region a request was signed
//! for, the canonical ARNs, id minting, the clock-owed status transition) live
//! here rather than in any one `ops/` file, so create/get/list/update/delete
//! cannot drift from each other.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AgentRuntime, AgentRuntimeArtifact, AuthorizerConfiguration, CapacityProviderConfiguration,
    FilesystemConfiguration, LifecycleConfiguration, NetworkConfiguration, ProtocolConfiguration,
    RequestHeaderConfiguration, RuntimeMetadataConfiguration, WorkloadIdentityDetails,
};

/// The ARN namespace (and sigv4 signing name) this service stamps on its
/// resources — `OPERATIONS.md`'s "ARN namespace: bedrock-agentcore".
pub const ARN_SERVICE: &str = "bedrock-agentcore";

/// The region used when a request carries no SigV4 credential scope to read one
/// from (an anonymous call); the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a freshly created or updated runtime stays in its transient status
/// (`CREATING` / `UPDATING`) before the clock owes it `READY`.
///
/// AWS documents no duration for this transition, so the span is chosen: it is
/// short enough to keep a client script quick and long enough that a caller
/// acting straight after the call still observes the transient status.
pub const RUNTIME_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// `AgentRuntimeStatus` members this service drives. The enum's full value list
/// is `CREATING | CREATE_FAILED | UPDATING | UPDATE_FAILED | READY | DELETING |
/// DELETE_FAILED` (`smithy_query shape=AgentRuntimeStatus`).
pub const STATUS_CREATING: &str = "CREATING";
pub const STATUS_UPDATING: &str = "UPDATING";
pub const STATUS_READY: &str = "READY";
pub const STATUS_DELETING: &str = "DELETING";

/// `LifecycleConfiguration.idleRuntimeSessionTimeout` when the caller supplies
/// none. The model documents it: "Timeout in seconds for idle runtime sessions.
/// … Default: 900 seconds (15 minutes)."
pub const DEFAULT_IDLE_RUNTIME_SESSION_TIMEOUT: i32 = 900;

/// `LifecycleConfiguration.maxLifetime` when the caller supplies none. The model
/// documents it: "Maximum lifetime for the instance in seconds. … Default:
/// 28800 seconds (8 hours)."
pub const DEFAULT_MAX_LIFETIME: i32 = 28_800;

/// How many agent runtimes this emulator will hold.
///
/// The real service caps runtimes per account per Region with a service quota,
/// and `CreateAgentRuntime` models `ServiceQuotaExceededException` (HTTP 402)
/// for exceeding it. The number here is NOT a fidelity claim about the published
/// quota (which is far smaller and adjustable): it is the emulator's own
/// ceiling, set generously so no realistic workflow meets it, so that a caller
/// looping on create cannot grow this process's memory without bound.
pub const MAX_AGENT_RUNTIMES: usize = 1_000;

/// How many versions one runtime will hold. Same reasoning: every update
/// publishes another version record carrying the whole configuration, and
/// `UpdateAgentRuntime` models the same `ServiceQuotaExceededException`.
pub const MAX_VERSIONS_PER_RUNTIME: usize = 1_000;

/// The highest version number the wire can carry: `AgentRuntimeVersion` is
/// `^([1-9][0-9]{0,4})$`, so five digits. A runtime that has published this many
/// versions can publish no more, whatever has since been deleted.
pub const MAX_AGENT_RUNTIME_VERSION: u32 = 99_999;

/// How many `clientToken` -> result records are kept for idempotent replay.
///
/// Tokens are caller-supplied, so the map is caller-driven growth that holds no
/// resource of its own. Records naming a deleted runtime are dropped with it
/// (`forget_client_tokens_for`), and beyond this many live records the oldest is
/// evicted.
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = 10_000;

/// One published version of an agent runtime: everything the caller configured,
/// plus the status the clock drives.
#[derive(Debug, Clone)]
pub struct AgentRuntimeVersionRecord {
    pub version: u32,
    /// The status until [`settles_at`](Self::settles_at) passes.
    pub transient_status: String,
    /// The status from [`settles_at`](Self::settles_at) onwards.
    pub settled_status: String,
    /// The instant the transient status is owed its transition. Stored, never
    /// slept on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub role_arn: String,
    pub description: Option<String>,
    pub agent_runtime_artifact: AgentRuntimeArtifact,
    pub network_configuration: Option<NetworkConfiguration>,
    pub protocol_configuration: Option<ProtocolConfiguration>,
    pub authorizer_configuration: Option<AuthorizerConfiguration>,
    pub request_header_configuration: Option<RequestHeaderConfiguration>,
    pub metadata_configuration: Option<RuntimeMetadataConfiguration>,
    pub lifecycle_configuration: LifecycleConfiguration,
    pub environment_variables: BTreeMap<String, String>,
    pub filesystem_configurations: Vec<FilesystemConfiguration>,
    pub capacity_provider_configuration: Option<CapacityProviderConfiguration>,
    pub platform_version: Option<String>,
    pub failure_reason: Option<String>,
}

impl AgentRuntimeVersionRecord {
    /// The status this version reports at `now`: the transient one until the
    /// transition the clock owes comes due, the settled one afterwards.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// The version number as the wire renders it (`AgentRuntimeVersion`,
    /// pattern `^([1-9][0-9]{0,4})$`).
    pub fn version_string(&self) -> String {
        self.version.to_string()
    }
}

/// One agent runtime, with every version published for it.
#[derive(Debug, Clone)]
pub struct AgentRuntimeRecord {
    pub agent_runtime_id: String,
    pub agent_runtime_arn: String,
    pub agent_runtime_name: String,
    pub created_at: SystemTime,
    /// The version a read without a `version` resolves to.
    pub latest_version: u32,
    /// The highest version number ever published for this runtime. It never
    /// decreases, so deleting a version does not let the next update re-use its
    /// number for different configuration.
    pub highest_version_published: u32,
    pub versions: BTreeMap<u32, AgentRuntimeVersionRecord>,
    pub tags: BTreeMap<String, String>,
    pub workload_identity_details: Option<WorkloadIdentityDetails>,
}

impl AgentRuntimeRecord {
    /// The latest published version, or `None` once every version is deleted.
    pub fn latest(&self) -> Option<&AgentRuntimeVersionRecord> {
        self.versions.get(&self.latest_version)
    }

    /// Resolve a `version` query value against the stored versions. `None` asks
    /// for the latest. An unparseable or unknown version resolves to `None`,
    /// which the caller answers with `ResourceNotFoundException`.
    pub fn version_for(&self, requested: Option<&str>) -> Option<&AgentRuntimeVersionRecord> {
        match requested {
            None => self.latest(),
            Some(raw) => raw.parse::<u32>().ok().and_then(|v| self.versions.get(&v)),
        }
    }

    /// Recompute [`latest_version`](Self::latest_version) from the versions that
    /// remain, after one was deleted.
    pub fn resync_latest_version(&mut self) {
        if let Some(highest) = self.versions.keys().next_back().copied() {
            self.latest_version = highest;
        }
    }

    /// This runtime's `AgentRuntime` list summary, at its latest version.
    pub fn summary(&self, now: SystemTime) -> Option<AgentRuntime> {
        let version = self.latest()?;
        Some(AgentRuntime {
            agent_runtime_arn: self.agent_runtime_arn.clone(),
            agent_runtime_id: self.agent_runtime_id.clone(),
            agent_runtime_version: version.version_string(),
            agent_runtime_name: self.agent_runtime_name.clone(),
            // `@required` on the summary, so a runtime created without a
            // description reports the empty string rather than omitting it.
            description: version.description.clone().unwrap_or_default(),
            last_updated_at: timestamp(version.last_updated_at),
            status: version.status_at(now).to_string(),
        })
    }
}

/// Per-service state for com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol. A handler reaches it through [`service_state`].
///
/// Every map here is caller-driven growth, so every one is bounded:
/// [`MAX_AGENT_RUNTIMES`] caps the runtimes (and with them the 1:1 name index),
/// [`MAX_VERSIONS_PER_RUNTIME`] caps the versions one runtime accumulates, and
/// the idempotency records are capped at [`MAX_REMEMBERED_CLIENT_TOKENS`] with
/// oldest-first eviction plus removal when the runtime they name is deleted.
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState {
    /// Every agent runtime, keyed by `agentRuntimeId`. The key order is the
    /// deterministic list order `ListAgentRuntimes` pages over.
    pub agent_runtimes: BTreeMap<String, AgentRuntimeRecord>,
    /// `agentRuntimeName` -> `agentRuntimeId`, so a duplicate name at create
    /// time is a `ConflictException`. One entry per stored runtime.
    pub runtime_names: BTreeMap<String, String>,
    /// `<operation>/<clientToken>` -> the (runtime, version) that token first
    /// produced, so a replayed Create/Update answers with its first result
    /// instead of minting a second runtime or a second version. The key carries
    /// the operation, so a token already used for a create cannot short-circuit
    /// an update (and silently discard its configuration). Reached through
    /// [`Self::replayed_result`] / [`Self::remember_client_token`], which keep it
    /// bounded.
    idempotency: BTreeMap<String, (String, u32)>,
    /// The keys of [`Self::idempotency`] in the order they were first recorded,
    /// so the oldest record is the one evicted at the ceiling.
    idempotency_order: VecDeque<String>,
    /// Bumped on every mint, so two runtimes created in the same instant with
    /// the same name still get distinct ids.
    mint_counter: u64,
}

impl ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState {
    /// Mint an `agentRuntimeId` for `name` that no stored runtime holds:
    /// `name` + `-` + 10 alphanumerics, per `AgentRuntimeId`'s pattern
    /// `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$`.
    pub fn mint_runtime_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.agent_runtimes.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The (runtime, version) a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and the runtime still
    /// exists.
    pub fn replayed_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<(String, u32)> {
        let (id, version) = self.idempotency.get(&token_key(operation, client_token?))?;
        let record = self.agent_runtimes.get(id)?;
        record
            .versions
            .contains_key(version)
            .then(|| (id.clone(), *version))
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        agent_runtime_id: &str,
        version: u32,
    ) {
        let Some(token) = client_token else {
            return;
        };
        let key = token_key(operation, &token);
        if self
            .idempotency
            .insert(key.clone(), (agent_runtime_id.to_string(), version))
            .is_none()
        {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_REMEMBERED_CLIENT_TOKENS {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// Drop the idempotency records naming a runtime that no longer exists.
    ///
    /// Called from the delete path: such a record can never replay anything
    /// again, and nothing else would ever reclaim it.
    pub fn forget_client_tokens_for(&mut self, agent_runtime_id: &str) {
        self.idempotency.retain(|_, (id, _)| id != agent_runtime_id);
        let live = &self.idempotency;
        self.idempotency_order.retain(|token| live.contains_key(token));
    }
}

/// One idempotency key: the operation the token was presented to, then the
/// token itself, so the two operations' tokens cannot collide.
fn token_key(operation: &str, client_token: &str) -> String {
    format!("{operation}/{client_token}")
}

/// The 10-character id suffix: SHA-256 over the name, the mint counter and the
/// creation instant, mapped into `[a-z0-9]` (a subset of the `[a-zA-Z0-9]{10}`
/// the model's pattern allows).
fn suffix_for(name: &str, counter: u64, at: SystemTime) -> String {
    use sha2::{Digest, Sha256};

    // Lowercase alphanumerics only: the pattern accepts upper case too, but a
    // single-case alphabet keeps the suffix unambiguous when a human reads it.
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let nanos = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    hasher
        .finalize()
        .iter()
        .take(10)
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect()
}

/// A stored instant as the wire timestamp (`DateTimestamp`, `date-time`).
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// An ARN segment must match `[a-z0-9-]+` (every `*Arn` pattern in this model
/// says so), so a scope region outside that grammar is not stamped into an ARN.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:bedrock-agentcore:{region}:{account}:runtime/{agentRuntimeId}` —
/// `AgentRuntimeArn`'s pattern.
pub fn runtime_arn(region: &str, agent_runtime_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("runtime/{agent_runtime_id}"))
}

/// The workload identity minted alongside a runtime, in the token vault's
/// `default` directory.
pub fn workload_identity_arn(region: &str, agent_runtime_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("workload-identity-directory/default/workload-identity/{agent_runtime_id}"),
    )
}

/// The lifecycle configuration to store for a request: the caller's, with each
/// member the caller left out filled from the default the model documents.
pub fn effective_lifecycle_configuration(
    requested: Option<LifecycleConfiguration>,
) -> LifecycleConfiguration {
    let requested = requested.unwrap_or_default();
    LifecycleConfiguration {
        idle_runtime_session_timeout: Some(
            requested
                .idle_runtime_session_timeout
                .unwrap_or(DEFAULT_IDLE_RUNTIME_SESSION_TIMEOUT),
        ),
        max_lifetime: Some(requested.max_lifetime.unwrap_or(DEFAULT_MAX_LIFETIME)),
    }
}

/// A `HashMap` request member as the `BTreeMap` the store keeps, so a read-back
/// is ordered deterministically.
pub fn sorted_map(
    map: Option<std::collections::HashMap<String, String>>,
) -> BTreeMap<String, String> {
    map.unwrap_or_default().into_iter().collect()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState>)
}
