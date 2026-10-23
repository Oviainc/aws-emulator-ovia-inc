// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol service.
//!
//! Two resources are persisted here:
//!
//! * the agent runtime (`/runtimes/`): one [`AgentRuntimeRecord`] per
//!   `agentRuntimeId`, each holding every version that has ever been published
//!   for it, so `GetAgentRuntime?version=`, `ListAgentRuntimes` and
//!   `ListAgentRuntimeVersions` all read one store;
//! * the gateway (`/gateways/`): one [`GatewayRecord`] per `gatewayId`, each
//!   holding the targets configured on it ([`GatewayTargetRecord`]), because a
//!   target is addressed only through its gateway.
//!
//! Helpers the handlers share (the region a request was signed for, the
//! canonical ARNs, id minting, the clock-owed status transition, the response
//! envelopes, the cross-service Lambda check a gateway target needs) live here
//! rather than in any one `ops/` file, so create/get/list/update/delete cannot
//! drift from each other.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AgentRuntime, AgentRuntimeArtifact, AuthorizerConfiguration, CapacityProviderConfiguration,
    CertificateConfiguration, CreateGatewayOutput, CreateGatewayTargetOutput,
    CredentialProviderConfiguration, FilesystemConfiguration, GatewayInterceptorConfiguration,
    GatewayPolicyEngineConfiguration, GatewayProtocolConfiguration, GetGatewayOutput,
    GetGatewayTargetOutput, LifecycleConfiguration, McpTargetConfiguration, MetadataConfiguration,
    NetworkConfiguration, PrivateEndpoint, ProtocolConfiguration, RequestHeaderConfiguration,
    RuntimeMetadataConfiguration, TargetConfiguration, ValidationException,
    ValidationExceptionField, WorkloadIdentityDetails,
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

/// `CredentialProviderType` for "the gateway's own IAM role", the credential a
/// gateway target carries when the caller configures none. The enum's full value
/// list is `API_KEY | CALLER_IAM_CREDENTIALS | GATEWAY_IAM_ROLE |
/// JWT_PASSTHROUGH | OAUTH` (`shared_types::CREDENTIAL_PROVIDER_TYPE_VALUES`,
/// from `smithy_query shape=CredentialProviderType`).
///
/// `credentialProviderConfigurations` is OPTIONAL on `CreateGatewayTarget` and
/// `@required` (min 1 entry) on every response that reports a target, so a
/// target created without one still reports the credential it runs with.
pub const CREDENTIAL_PROVIDER_GATEWAY_IAM_ROLE: &str = "GATEWAY_IAM_ROLE";

/// The `authorizerType` whose `authorizerConfiguration` the model makes
/// mandatory: "If you specify CUSTOM_JWT as the authorizerType, you must provide
/// an authorizerConfiguration" (`smithy_query doc shape=CreateGateway`).
pub const AUTHORIZER_TYPE_CUSTOM_JWT: &str = "CUSTOM_JWT";

/// `ValidationExceptionReason` for a member of the request that failed
/// validation. The enum's full value list is `CannotParse |
/// FieldValidationFailed | IdempotentParameterMismatchException |
/// EventInOtherSession | ResourceConflict`
/// (`smithy_query shape=ValidationExceptionReason`).
pub const VALIDATION_REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// The modelled validation error for ONE request member this service refuses:
/// the member's name and why, in the `fieldList` the shape declares for exactly
/// that, with `reason` saying a field failed validation.
///
/// Shared by the handlers rather than written per operation, so every member
/// this service refuses is reported in one shape.
pub fn field_validation_failed(member: &str, why: &str) -> ValidationException {
    ValidationException {
        message: format!("{member}: {why}"),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: why.to_string(),
        }]),
    }
}

/// How many gateways this emulator will hold, and how many targets one gateway
/// will hold.
///
/// The real service caps both with service quotas, and `CreateGateway` /
/// `CreateGatewayTarget` model `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding one. Neither number is a fidelity claim about the published quota:
/// each is the emulator's own ceiling, set generously so no realistic workflow
/// meets it, so that a caller looping on create cannot grow this process's
/// memory without bound.
pub const MAX_GATEWAYS: usize = 1_000;
pub const MAX_TARGETS_PER_GATEWAY: usize = 1_000;

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

/// One gateway, with every target configured on it.
///
/// `GetGateway` reports exactly what `CreateGateway` stored, so the two cannot
/// drift; the targets live here rather than in a map of their own, because a
/// target is addressed only through its gateway
/// (`/gateways/{gatewayIdentifier}/targets/{targetId}/`) and is gone when the
/// gateway is.
#[derive(Debug, Clone)]
pub struct GatewayRecord {
    pub gateway_id: String,
    pub gateway_arn: String,
    pub gateway_url: String,
    pub name: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status until [`settles_at`](Self::settles_at) passes.
    pub transient_status: String,
    /// The status from [`settles_at`](Self::settles_at) onwards.
    pub settled_status: String,
    /// The instant the transient status is owed its transition. Stored, never
    /// slept on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    pub description: Option<String>,
    pub role_arn: String,
    pub protocol_type: Option<String>,
    pub protocol_configuration: Option<GatewayProtocolConfiguration>,
    pub authorizer_type: String,
    pub authorizer_configuration: Option<AuthorizerConfiguration>,
    pub kms_key_arn: Option<String>,
    pub interceptor_configurations: Option<Vec<GatewayInterceptorConfiguration>>,
    pub policy_engine_configuration: Option<GatewayPolicyEngineConfiguration>,
    pub exception_level: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub workload_identity_details: Option<WorkloadIdentityDetails>,
    /// Every target on this gateway, keyed by `targetId`. The key order is the
    /// deterministic list order a target listing pages over.
    pub targets: BTreeMap<String, GatewayTargetRecord>,
}

impl GatewayRecord {
    /// The status this gateway reports at `now`: the transient one until the
    /// transition the clock owes comes due, the settled one afterwards.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// This gateway as `CreateGateway`'s response envelope.
    ///
    /// `CreateGatewayResponse` and `GetGatewayResponse` declare the SAME members,
    /// so both envelopes are rendered from the one stored record here rather than
    /// in either handler: a member the create reports and the read does not (or
    /// reports differently) would be a difference no stored state justifies.
    pub fn create_output(&self, now: SystemTime) -> CreateGatewayOutput {
        CreateGatewayOutput {
            gateway_arn: self.gateway_arn.clone(),
            gateway_id: self.gateway_id.clone(),
            gateway_url: Some(self.gateway_url.clone()),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            status: self.status_at(now).to_string(),
            // Reported only for a status that HAS a reason; a gateway this
            // emulator drives reaches READY, which has none.
            status_reasons: None,
            name: self.name.clone(),
            description: self.description.clone(),
            role_arn: Some(self.role_arn.clone()),
            protocol_type: self.protocol_type.clone(),
            protocol_configuration: self.protocol_configuration.clone(),
            authorizer_type: self.authorizer_type.clone(),
            authorizer_configuration: self.authorizer_configuration.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
            // No operation of this service sets a custom transform, a web ACL or
            // a WAF configuration on a gateway, so a gateway carries none.
            custom_transform_configuration: None,
            interceptor_configurations: self.interceptor_configurations.clone(),
            policy_engine_configuration: self.policy_engine_configuration.clone(),
            workload_identity_details: self.workload_identity_details.clone(),
            exception_level: self.exception_level.clone(),
            web_acl_arn: None,
            waf_configuration: None,
        }
    }

    /// This gateway as `GetGateway`'s response envelope — the same members
    /// [`Self::create_output`] renders, read off the same record.
    pub fn get_output(&self, now: SystemTime) -> GetGatewayOutput {
        let created = self.create_output(now);
        GetGatewayOutput {
            gateway_arn: created.gateway_arn,
            gateway_id: created.gateway_id,
            gateway_url: created.gateway_url,
            created_at: created.created_at,
            updated_at: created.updated_at,
            status: created.status,
            status_reasons: created.status_reasons,
            name: created.name,
            description: created.description,
            role_arn: created.role_arn,
            protocol_type: created.protocol_type,
            protocol_configuration: created.protocol_configuration,
            authorizer_type: created.authorizer_type,
            authorizer_configuration: created.authorizer_configuration,
            kms_key_arn: created.kms_key_arn,
            custom_transform_configuration: created.custom_transform_configuration,
            interceptor_configurations: created.interceptor_configurations,
            policy_engine_configuration: created.policy_engine_configuration,
            workload_identity_details: created.workload_identity_details,
            exception_level: created.exception_level,
            web_acl_arn: created.web_acl_arn,
            waf_configuration: created.waf_configuration,
        }
    }
}

/// One target configured on a gateway.
#[derive(Debug, Clone)]
pub struct GatewayTargetRecord {
    pub target_id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub transient_status: String,
    pub settled_status: String,
    pub settles_at: SystemTime,
    pub target_configuration: TargetConfiguration,
    pub credential_provider_configurations: Vec<CredentialProviderConfiguration>,
    pub metadata_configuration: Option<MetadataConfiguration>,
    pub private_endpoint: Option<PrivateEndpoint>,
    pub certificate_configurations: Option<Vec<CertificateConfiguration>>,
}

impl GatewayTargetRecord {
    /// The status this target reports at `now`, on the same clock-owed rule a
    /// gateway's status follows.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// This target as `CreateGatewayTarget`'s response envelope. `gateway_arn` is
    /// the owning gateway's, which the response reports and the target itself
    /// does not store twice.
    pub fn create_output(
        &self,
        gateway_arn: &str,
        now: SystemTime,
    ) -> CreateGatewayTargetOutput {
        CreateGatewayTargetOutput {
            gateway_arn: gateway_arn.to_string(),
            target_id: self.target_id.clone(),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            status: self.status_at(now).to_string(),
            status_reasons: None,
            name: self.name.clone(),
            description: self.description.clone(),
            target_configuration: self.target_configuration.clone(),
            credential_provider_configurations: self.credential_provider_configurations.clone(),
            // Nothing has synchronized this target: `SynchronizeGatewayTargets`
            // is the operation that would, and it is not implemented.
            last_synchronized_at: None,
            metadata_configuration: self.metadata_configuration.clone(),
            private_endpoint: self.private_endpoint.clone(),
            // Reported for a target served through a private endpoint, which
            // this emulator creates no managed resources for.
            private_endpoint_managed_resources: None,
            // Carried only by a credential provider with an authorization-code
            // grant that needs user federation; none of the providers a target
            // can be created with here does.
            authorization_data: None,
            protocol_type: None,
            certificate_configurations: self.certificate_configurations.clone(),
        }
    }

    /// This target as `GetGatewayTarget`'s response envelope — the same members
    /// [`Self::create_output`] renders, read off the same record.
    pub fn get_output(&self, gateway_arn: &str, now: SystemTime) -> GetGatewayTargetOutput {
        let created = self.create_output(gateway_arn, now);
        GetGatewayTargetOutput {
            gateway_arn: created.gateway_arn,
            target_id: created.target_id,
            created_at: created.created_at,
            updated_at: created.updated_at,
            status: created.status,
            status_reasons: created.status_reasons,
            name: created.name,
            description: created.description,
            target_configuration: created.target_configuration,
            credential_provider_configurations: created.credential_provider_configurations,
            last_synchronized_at: created.last_synchronized_at,
            metadata_configuration: created.metadata_configuration,
            private_endpoint: created.private_endpoint,
            private_endpoint_managed_resources: created.private_endpoint_managed_resources,
            authorization_data: created.authorization_data,
            protocol_type: created.protocol_type,
            certificate_configurations: created.certificate_configurations,
        }
    }
}

/// Per-service state for com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol. A handler reaches it through [`service_state`].
///
/// Every map here is caller-driven growth, so every one is bounded:
/// [`MAX_AGENT_RUNTIMES`] caps the runtimes (and with them the 1:1 name index),
/// [`MAX_VERSIONS_PER_RUNTIME`] caps the versions one runtime accumulates,
/// [`MAX_GATEWAYS`] caps the gateways (and with them their 1:1 name index) and
/// [`MAX_TARGETS_PER_GATEWAY`] the targets one gateway holds, and the
/// idempotency records are capped at [`MAX_REMEMBERED_CLIENT_TOKENS`] with
/// oldest-first eviction plus removal when the resource they name is deleted.
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState {
    /// Every agent runtime, keyed by `agentRuntimeId`. The key order is the
    /// deterministic list order `ListAgentRuntimes` pages over.
    pub agent_runtimes: BTreeMap<String, AgentRuntimeRecord>,
    /// `agentRuntimeName` -> `agentRuntimeId`, so a duplicate name at create
    /// time is a `ConflictException`. One entry per stored runtime.
    pub runtime_names: BTreeMap<String, String>,
    /// Every gateway, keyed by `gatewayId` — the identifier every gateway and
    /// gateway-target route addresses it by. The key order is the deterministic
    /// list order a gateway listing pages over.
    pub gateways: BTreeMap<String, GatewayRecord>,
    /// `name` -> `gatewayId`, so a duplicate name at create time is a
    /// `ConflictException` ("The name must be unique within your account").
    /// One entry per stored gateway.
    pub gateway_names: BTreeMap<String, String>,
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
    /// `<operation>/<clientToken>` -> the (gateway, target) that token first
    /// produced — `None` for a gateway create, which produced no target — so a
    /// replayed `CreateGateway` / `CreateGatewayTarget` answers with its first
    /// result instead of minting a second resource. Separate from
    /// [`Self::idempotency`] because the record is validated against a different
    /// store; the key carries the operation for the same reason as there.
    gateway_idempotency: BTreeMap<String, (String, Option<String>)>,
    /// The keys of [`Self::gateway_idempotency`] in the order they were first
    /// recorded, so the oldest record is the one evicted at the ceiling.
    gateway_idempotency_order: VecDeque<String>,
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

    /// Mint a `gatewayId` for `name` that no stored gateway holds: the name,
    /// lower-cased, + `-` + 10 lower-case alphanumerics, per `GatewayId`'s
    /// pattern `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$`.
    ///
    /// Two things the pattern requires and `GatewayName`
    /// (`^([0-9a-zA-Z][-]?){1,48}$`) does not: the id is lower case, and no two
    /// dashes are adjacent — so a name ENDING in a dash has it trimmed before the
    /// suffix's own dash is appended, rather than minting an id outside the
    /// pattern a client validates the response against.
    pub fn mint_gateway_id(&mut self, name: &str, at: SystemTime) -> String {
        let lowercased = name.to_ascii_lowercase();
        let stem = lowercased.trim_end_matches('-');
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{stem}-{}", suffix_for(stem, self.mint_counter, at));
            if !self.gateways.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint a `targetId` no target of `gateway_id` holds: 10 alphanumerics, per
    /// `TargetId`'s pattern `^[0-9a-zA-Z]{10}$`, which \u2014 unlike a gateway id \u2014
    /// carries no name.
    pub fn mint_target_id(&mut self, gateway_id: &str, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = suffix_for(&format!("{gateway_id}/{name}"), self.mint_counter, at);
            let taken = self
                .gateways
                .get(gateway_id)
                .is_some_and(|gateway| gateway.targets.contains_key(&candidate));
            if !taken {
                return candidate;
            }
        }
    }

    /// The (gateway, target) a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and what it names
    /// still exists.
    pub fn replayed_gateway_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<(String, Option<String>)> {
        let (gateway_id, target_id) = self
            .gateway_idempotency
            .get(&token_key(operation, client_token?))?;
        let gateway = self.gateways.get(gateway_id)?;
        match target_id {
            None => Some((gateway_id.clone(), None)),
            Some(target_id) => gateway
                .targets
                .contains_key(target_id)
                .then(|| (gateway_id.clone(), Some(target_id.clone()))),
        }
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_gateway_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        gateway_id: &str,
        target_id: Option<&str>,
    ) {
        let Some(token) = client_token else {
            return;
        };
        let key = token_key(operation, &token);
        let record = (gateway_id.to_string(), target_id.map(str::to_string));
        if self.gateway_idempotency.insert(key.clone(), record).is_none() {
            self.gateway_idempotency_order.push_back(key);
        }
        while self.gateway_idempotency_order.len() > MAX_REMEMBERED_CLIENT_TOKENS {
            if let Some(oldest) = self.gateway_idempotency_order.pop_front() {
                self.gateway_idempotency.remove(&oldest);
            }
        }
    }

    /// Drop the idempotency records naming a gateway that no longer exists, for
    /// the same reason [`Self::forget_client_tokens_for`] does.
    pub fn forget_gateway_client_tokens_for(&mut self, gateway_id: &str) {
        self.gateway_idempotency.retain(|_, (id, _)| id != gateway_id);
        let live = &self.gateway_idempotency;
        self.gateway_idempotency_order
            .retain(|token| live.contains_key(token));
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
pub fn workload_identity_arn(region: &str, resource_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("workload-identity-directory/default/workload-identity/{resource_id}"),
    )
}

/// `arn:aws:bedrock-agentcore:{region}:{account}:gateway/{gatewayId}` —
/// `GatewayArn`'s pattern.
pub fn gateway_arn(region: &str, gateway_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("gateway/{gateway_id}"))
}

/// The gateway's endpoint, in the form AWS returns for a Bedrock AgentCore
/// gateway: `https://{gatewayId}.gateway.bedrock-agentcore.{region}.amazonaws.com`.
///
/// HOST ONLY, with no path: the recorded `CreateGateway`/`GetGateway` responses
/// report exactly this origin, and the protocol's own path (`/mcp` for MCP) is
/// the client's to append — a URL that already carried it sent an MCP client to
/// `/mcp/mcp`.
///
/// Derived from the gateway id and the request's Region, so a caller that reads
/// one gateway twice sees one URL. `GatewayUrl` is only constrained in length
/// (1..1024), which this form satisfies for every id the pattern admits.
pub fn gateway_url(region: &str, gateway_id: &str) -> String {
    format!("https://{gateway_id}.gateway.{ARN_SERVICE}.{region}.amazonaws.com")
}

/// How long a freshly created gateway or gateway target stays in its transient
/// status (`CREATING`) before the clock owes it `READY`.
///
/// AWS documents no duration for either transition, so the span is chosen: short
/// enough to keep a client script quick, long enough that a caller acting
/// straight after the call still observes the transient status.
pub const GATEWAY_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// The Lambda function ARN a target configuration names, for the one
/// configuration shape that names one: an MCP target whose tool handler is a
/// Lambda function (`TargetConfiguration.mcp.lambda.lambdaArn`).
///
/// `None` for every other target shape — an OpenAPI schema, a Smithy model, an
/// MCP server endpoint, an API Gateway stage, a connector, an HTTP target, an
/// inference target — none of which references a function.
pub fn mcp_lambda_arn(configuration: &TargetConfiguration) -> Option<&str> {
    match configuration {
        TargetConfiguration::Mcp(McpTargetConfiguration::Lambda(lambda)) => {
            Some(lambda.lambda_arn.as_str())
        }
        _ => None,
    }
}

/// The `fieldList` member name a Lambda MCP target's function ARN is reported
/// under: the path to it through the request, so a caller sees which member of
/// the nested union failed.
pub const LAMBDA_ARN_MEMBER: &str = "targetConfiguration.mcp.lambda.lambdaArn";

/// Why a gateway target naming a function that does not exist is refused.
pub const LAMBDA_FUNCTION_ABSENT: &str =
    "names a Lambda function that does not exist in this account and Region";

/// The ARN vendor (service namespace) whose endpoint a Lambda MCP target is resolved
/// against. `aws.api#service$arnNamespace` of the Lambda model, and the first label of
/// its endpoint host.
const LAMBDA_VENDOR: &str = "lambda";

/// Whether Lambda reports that the function `lambda_arn` names does not exist.
///
/// A gateway target's Lambda is resolved when the target is WRITTEN — the
/// gateway has to invoke that function to serve a tool call, so a target naming
/// a function that is not there is refused with the operation's modelled
/// validation error and nothing is persisted. Lambda is reached the way any
/// emulated service reaches another: through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`]), so this crate names
/// no other service crate.
///
/// `false` means the target may be written. That includes the case where this
/// emulator has no Lambda to ask (a router with only this service mounted, say):
/// nothing is then known about the function, and a refusal has to be grounded in
/// what Lambda answered, never assumed.
pub async fn lambda_function_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    lambda_arn: &str,
) -> bool {
    // The function is asked for in ITS OWN Region — the ARN's — which is not
    // necessarily the one this request was signed for. The label is read through
    // `service_common`'s one ARN parse, which reports it only for an ARN OF LAMBDA
    // whose Region field is a Region label: it lands in the authority of the
    // endpoint URL below, so a caller-supplied value outside that grammar must not
    // reach it. Anything else falls back to this emulator's default Region.
    let region = ::service_common::arn::region_of(lambda_arn, LAMBDA_VENDOR)
        .unwrap_or(DEFAULT_REGION);
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials are handed over directly.
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    // `FunctionName` takes the full ARN, which is what the target carries.
    let answer = ::aws_sdk_lambda::Client::from_conf(config)
        .get_function()
        .function_name(lambda_arn)
        .send()
        .await;
    match answer {
        Ok(_) => false,
        // Lambda says this account holds no such function: the refusal is grounded.
        Err(error) => {
            if function_is_absent(&error) {
                return true;
            }
            // Any other answer is this emulator failing to ask, not Lambda
            // reporting a missing function, so it refuses nothing — and it is
            // worth a warning, because a deployment with no Lambda to ask is one
            // where this check does not run.
            ::tracing::warn!(
                %lambda_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the gateway target's Lambda function could not be verified against Lambda"
            );
            false
        }
    }
}

/// Whether a Lambda `GetFunction` failure is Lambda reporting that the function
/// does not exist (`ResourceNotFoundException`), as opposed to the call not
/// reaching Lambda at all.
fn function_is_absent(
    error: &::aws_sdk_lambda::error::SdkError<
        ::aws_sdk_lambda::operation::get_function::GetFunctionError,
        ::aws_smithy_runtime_api::http::Response,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(
            ::aws_sdk_lambda::operation::get_function::GetFunctionError::is_resource_not_found_exception,
        )
}

/// The credential a gateway target reports when the caller configured none: the
/// gateway's own IAM role.
///
/// `credentialProviderConfigurations` is optional on the request and `@required`
/// with at least one entry on every response that reports a target, so the
/// stored target carries this one rather than an empty list the wire cannot
/// render.
pub fn effective_credential_provider_configurations(
    requested: Option<Vec<CredentialProviderConfiguration>>,
) -> Vec<CredentialProviderConfiguration> {
    match requested {
        Some(configured) if !configured.is_empty() => configured,
        _ => vec![CredentialProviderConfiguration {
            credential_provider_type: CREDENTIAL_PROVIDER_GATEWAY_IAM_ROLE.to_string(),
            credential_provider: None,
        }],
    }
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
