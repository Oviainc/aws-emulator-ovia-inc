// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol service.
//!
//! Three resources are persisted here:
//!
//! * the agent runtime (`/runtimes/`): one [`AgentRuntimeRecord`] per
//!   `agentRuntimeId`, each holding every version that has ever been published
//!   for it and the endpoints created on it ([`AgentRuntimeEndpointRecord`],
//!   each pinned to one version), so `GetAgentRuntime?version=`,
//!   `ListAgentRuntimes`, `ListAgentRuntimeVersions` and every read of an
//!   endpoint all read one store;
//! * the gateway (`/gateways/`): one [`GatewayRecord`] per `gatewayId`, each
//!   holding the targets configured on it ([`GatewayTargetRecord`]), because a
//!   target is addressed only through its gateway;
//! * the memory (`/memories/`): one [`MemoryRecord`] per `memoryId`, holding the
//!   configuration the caller supplied and the strategies minted for it, so
//!   `GetMemory` and any later memory read answer from one store;
//! * the harness (`/harnesses/`): one [`HarnessRecord`] per `harnessId`, holding
//!   the whole agent-loop configuration `CreateHarness` persisted and the
//!   endpoints created on it ([`HarnessEndpointRecord`]), because an endpoint is
//!   addressed only through its harness, so `GetHarness` reports exactly that and
//!   `DeleteHarness` reports what it removed;
//! * the registry (`/registries/`): one [`RegistryRecord`] per `registryId`,
//!   holding the catalog's configuration and the status the clock owes it, so
//!   `GetRegistry` and any later registry read answer from one store;
//! * the browser (`/browsers/`): one [`BrowserRecord`] per `browserId`, holding
//!   the configuration `CreateBrowser` persisted and the statuses the clock owes
//!   it, so `GetBrowser` reports exactly that and `DeleteBrowser` reports what it
//!   is removing;
//! * the browser profile (`/browser-profiles/`): one [`BrowserProfileRecord`]
//!   per `profileId`, holding the name, description and tags
//!   `CreateBrowserProfile` persisted, the instant a browser session last saved
//!   data into it and the removal the clock owes an accepted deletion, so
//!   `GetBrowserProfile` reports exactly that and `DeleteBrowserProfile`
//!   reports what it is removing;
//! * the code interpreter (`/code-interpreters/`): one
//!   [`CodeInterpreterRecord`] per `codeInterpreterId`, holding the
//!   configuration `CreateCodeInterpreter` persisted and the statuses the clock
//!   owes it, so `GetCodeInterpreter` reports exactly that and
//!   `DeleteCodeInterpreter` reports what it is removing;
//! * the token vault ([`TokenVaults`]): the KMS configuration each vault of the
//!   account is encrypted under, so `SetTokenVaultCMK` and `GetTokenVault`
//!   answer from one store;
//! * the configuration bundle (`/configuration-bundles/`): one
//!   [`ConfigurationBundleRecord`] per `bundleId`, holding every version ever
//!   committed to it ([`ConfigurationBundleVersionRecord`]), so a create and
//!   every later read of the bundle or of one of its versions answer from one
//!   store.
//!
//! Helpers the handlers share (the region a request was signed for, the
//! canonical ARNs, id minting, the clock-owed status transition, the response
//! envelopes, the cross-service Lambda check a gateway target needs and the
//! cross-service IAM check a caller-supplied execution role needs) live here
//! rather than in any one `ops/` file, so create/get/list/update/delete cannot
//! drift from each other.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AgentRuntime, AgentRuntimeArtifact, AgentRuntimeVersionSummary, ApprovalConfiguration,
    AuthorizerConfiguration,
    BrowserEnterprisePolicy, BrowserNetworkConfiguration, BrowserSigningConfigOutput,
    CapacityProviderConfiguration, Certificate, CertificateConfiguration, Code,
    ComponentConfiguration,
    CodeInterpreterNetworkConfiguration, CreateCodeInterpreterOutput, DeleteCodeInterpreterOutput,
    GetCodeInterpreterOutput,
    CreateBrowserOutput, CreateGatewayOutput, DeleteBrowserOutput, GetBrowserOutput,
    CreateBrowserProfileOutput, DeleteBrowserProfileOutput, GetBrowserProfileOutput,
    RecordingConfig, ToolsFileSystemConfiguration,
    CreateGatewayTargetOutput, CredentialProviderConfiguration, CustomTransformConfiguration,
    DeleteGatewayTargetOutput,
    FilesystemConfiguration, GatewayInterceptorConfiguration, GatewayPolicyEngineConfiguration,
    GatewayProtocolConfiguration, GatewaySummary, GetGatewayOutput, GetGatewayTargetOutput,
    GetConfigurationBundleOutput, GetRegistryOutput,
    Harness, HarnessAgentCoreRuntimeEnvironment, HarnessAgentCoreRuntimeEnvironmentRequest,
    HarnessEndpoint,
    HarnessEnvironmentArtifact, HarnessEnvironmentProvider, HarnessEnvironmentProviderRequest,
    HarnessHook, HarnessMemoryConfiguration, HarnessModelConfiguration, HarnessSkill,
    HarnessSystemContentBlock, HarnessTool, HarnessTruncationConfiguration, HarnessVersionSummary,
    HttpTargetConfiguration, IndexedKey, InferenceTargetConfiguration, KmsConfiguration,
    LifecycleConfiguration,
    McpTargetConfiguration, Memory, MemoryExtractionHealth, MemoryStrategy, MetadataConfiguration,
    NamespaceKeyEntry,
    NetworkConfiguration, PrivateEndpoint, ProtocolConfiguration, RequestHeaderConfiguration,
    ResourceNotFoundException,
    ConsolidationConfiguration, CustomConfigurationInput, CustomConsolidationConfiguration,
    CustomExtractionConfiguration, CustomReflectionConfiguration, EpisodicConsolidationOverride,
    EpisodicExtractionOverride, EpisodicReflectionConfiguration, EpisodicReflectionOverride,
    ExtractionConfiguration, InvocationConfiguration, MemoryRecordSchema, MemoryStrategyInput,
    MessageBasedTrigger, RecordExpirySpecification, ReflectionConfiguration,
    SelfManagedConfiguration, SemanticConsolidationOverride, SemanticExtractionOverride,
    ServiceQuotaExceededException, StrategyConfiguration, SummaryConsolidationOverride,
    TimeBasedTrigger, TokenBasedTrigger, TriggerCondition, TriggerConditionInput,
    UserPreferenceConsolidationOverride, UserPreferenceExtractionOverride,
    RuntimeMetadataConfiguration, S3Location, StreamDeliveryResources, TargetConfiguration,
    TargetSummary,
    ToolDefinition, ToolSchema, VersionCreatedBySource, VersionLineageMetadata,
    CreateWorkloadIdentityOutput, GetWorkloadIdentityOutput, UpdateWorkloadIdentityOutput,
    UpdateGatewayOutput, UpdateGatewayTargetOutput, ValidationException, ValidationExceptionField,
    WafConfiguration,
    WorkloadIdentityDetails, WorkloadIdentityType,
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

/// The `authorizerType` that admits an inbound request on the caller's IAM
/// credentials: "AWS_IAM - Authorize with your Amazon Web Services IAM
/// credentials" (`smithy_query shape=AuthorizerType`). The enum's full value
/// list is [`crate::ops::types::AUTHORIZER_TYPE_VALUES`].
pub const AUTHORIZER_TYPE_AWS_IAM: &str = "AWS_IAM";

/// The `authorizerType` that admits every inbound request: "NONE - No
/// authorization" (`smithy_query shape=AuthorizerType`).
pub const AUTHORIZER_TYPE_NONE: &str = "NONE";

/// `GatewayProtocolType`'s single value, which `protocolType` also carries as its
/// modelled default (`smithy_query shape=GatewayProtocolType`): the protocol a
/// gateway's own endpoint speaks.
pub const PROTOCOL_TYPE_MCP: &str = "MCP";

/// `ValidationExceptionReason` for a member of the request that failed
/// validation. The enum's full value list is `CannotParse |
/// FieldValidationFailed | IdempotentParameterMismatchException |
/// EventInOtherSession | ResourceConflict`
/// (`smithy_query shape=ValidationExceptionReason`).
pub const VALIDATION_REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// `ValidationExceptionReason` for a request the stored resource's own state
/// refuses — an operation asked for while the resource is in a status that
/// cannot serve it. Same enum as
/// [`VALIDATION_REASON_FIELD_VALIDATION_FAILED`]: nothing is wrong with the
/// request's members, so `FieldValidationFailed` would misreport it.
pub const VALIDATION_REASON_RESOURCE_CONFLICT: &str = "ResourceConflict";

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

/// The modelled validation error for a request that did not DEFINE a member
/// this service needs, in the form this service's recorded refusals take
/// ("Credential provider configurations is not defined").
///
/// `member_prose` is the member's name as the sentence spells it. Nothing from
/// the request is echoed — the member is missing, so there is no value to quote
/// — which makes the sentence the same for every body that leaves it out.
/// `reason` says a field failed validation, and there is no `fieldList`: no
/// value the request carried broke a constraint.
pub fn member_not_defined(member_prose: &str) -> ValidationException {
    ValidationException {
        message: format!("{member_prose} is not defined"),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: None,
    }
}

/// `credentialProviderConfigurations` as [`member_not_defined`] spells it.
pub const CREDENTIAL_PROVIDER_CONFIGURATIONS_PROSE: &str = "Credential provider configurations";

/// A status enum token as this service's PROSE spells it: every
/// `_`-separated word capitalised and the words run together (`CREATING` ->
/// `Creating`, `UPDATE_UNSUCCESSFUL` -> `UpdateUnsuccessful`).
///
/// A response member carries a status as the screaming-snake token its enum
/// declares (`smithy_query shape=GatewayStatus`), while a message that talks
/// ABOUT the status names it as a word ("... when it is in Creating state"), so
/// the token is rendered for prose rather than quoted into it.
pub fn status_in_prose(status: &str) -> String {
    status
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>()
                        + &characters.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect()
}

/// The modelled validation error a gateway operation gets while the gateway is
/// in a status that cannot serve it: the operation that was asked for and the
/// status it found, with the one remedy the caller has ("DeleteGateway operation
/// can't be performed on gateway when it is in Creating state. Retry the request
/// when the gateway is in a valid state.").
///
/// The sentence is fixed; its variable parts are the asking operation's own name
/// and the gateway's current status, the latter spelled as prose
/// ([`status_in_prose`]) rather than as the wire's enum token.
///
/// A gateway reaches `READY` on the clock, so every mutation of it is refused
/// while it is still `CREATING`, `UPDATING` or on its way out in `DELETING`:
/// nothing is wrong with the request's members, so `reason` is
/// [`VALIDATION_REASON_RESOURCE_CONFLICT`] and there is no `fieldList`.
pub fn gateway_status_conflict(operation: &str, status: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "{operation} operation can't be performed on {RESOURCE_GATEWAY} when it is in {} \
             state. Retry the request when the {RESOURCE_GATEWAY} is in a valid state.",
            status_in_prose(status)
        ),
        reason: VALIDATION_REASON_RESOURCE_CONFLICT.to_string(),
        field_list: None,
    }
}

/// How this service words the refusal of a `DeleteGateway` for a gateway that
/// still holds at least one target: the gateway, by the id the request named it
/// with, and what the caller has to delete before the gateway can go.
///
/// A target is addressed only THROUGH the gateway that serves it
/// (`/gateways/{gatewayIdentifier}/targets/{targetId}/`), so the gateway is not
/// destroyed while any target is still attached: the delete is refused and the
/// caller deletes the targets first, rather than the targets being silently
/// cascaded away with the gateway that fronts them. The sentence around the id
/// is fixed; the id itself is the one the request's path carried.
///
/// Shared by the handlers rather than written per operation, so the one refusal
/// cannot be worded two ways.
pub fn gateway_has_targets(gateway_id: &str) -> String {
    format!(
        "Gateway with ID: {gateway_id} has targets associated with it. Delete all targets \
         before deleting the gateway."
    )
}

/// How this service words the refusal of a `DeleteHarness` for a harness that
/// still holds endpoints: the harness, by the name it was created with, and what
/// the caller has to delete before the harness can go.
///
/// An endpoint is addressed only THROUGH the harness that holds it, so the
/// harness is not destroyed while any endpoint is attached: the delete is
/// refused and the caller deletes the endpoints first, rather than the endpoints
/// being silently cascaded away with the harness they front. The sentence around
/// the name is fixed; the name itself is the one the request's harness carries.
///
/// Shared by the handlers rather than written per operation, so the one refusal
/// cannot be worded two ways.
pub fn harness_has_endpoints(harness_name: &str) -> String {
    format!(
        "Harness {harness_name} has endpoints that must be deleted before the harness can be \
         deleted."
    )
}

/// The modelled not-found error for a resource of this service that is not
/// there: what could not be carried out, the resource it was for, and the one
/// remedy the caller has, in the form this service's recorded refusals take
/// ("Failed to retrieve gateway because it doesn't exist. Retry the request
/// with a different resource identifier.").
///
/// The verb is "retrieve" whatever the operation was doing: what failed is the
/// LOOKUP of the resource, which every operation performs the same way, so the
/// sentence does not vary with the caller's intent — an update, a delete and a
/// read of a missing resource are all refused in these same words. `resource` is
/// the missing resource's own noun. The identifier the request named is NOT
/// echoed: it is the caller's own value, and the remedy does not depend on it.
///
/// Shared by the handlers rather than written per operation, so one resource
/// cannot be reported missing in two different sentences by two operations.
pub fn resource_not_found(resource: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Failed to retrieve {resource} because it doesn't exist. Retry the request with a \
             different resource identifier."
        )),
    }
}

/// The modelled not-found error for a MEMORY this service does not hold, which
/// the memory family words in its own sentence rather than through
/// [`resource_not_found`]: what could not be found, the operation that was
/// asking, and the identifier it asked for ("Resource not found during
/// GetMemory: Failed to retrieve memory item for ID: <memoryId>").
///
/// `operation` is the asking operation's own name and `memory_id` the id the
/// request carried, so every memory operation reports a missing memory in the
/// one sentence with its own two values filled in.
pub fn memory_not_found(operation: &str, memory_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Resource not found during {operation}: Failed to retrieve memory item for ID: \
             {memory_id}"
        )),
    }
}

/// How [`agent_runtime_not_found`] spells "the request asked for no particular
/// version": the lookup ran with no version, and the sentence reports the
/// missing value rather than leaving a gap.
pub const AGENT_RUNTIME_VERSION_UNSPECIFIED: &str = "null";

/// The modelled not-found error for an agent runtime — or for a VERSION of one —
/// that this service does not hold: the id the lookup ran for, the account it ran
/// in, and the version it asked for ("Agent with agentId: <id>, accountId:
/// <account>, and version: <version> not found!").
///
/// This control plane words a missing runtime in terms of the agent it runs. The
/// sentence around the three values is fixed: `agent_runtime_id` is the id the
/// request carried, the account is the single one this emulator serves
/// (`engine::emulator_account_id`), and `version` is the version the request
/// asked for — a request that named none is reported with
/// [`AGENT_RUNTIME_VERSION_UNSPECIFIED`].
///
/// Shared by the handlers rather than written per operation, so one runtime
/// cannot be reported missing in two different sentences by two operations.
pub fn agent_runtime_not_found(
    agent_runtime_id: &str,
    version: Option<&str>,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Agent with agentId: {agent_runtime_id}, accountId: {}, and version: {} not found!",
            ::engine::emulator_account_id(),
            version.unwrap_or(AGENT_RUNTIME_VERSION_UNSPECIFIED)
        )),
    }
}

/// The resource nouns [`resource_not_found`] names, as the recorded refusals
/// word them: a gateway, and a target OF a gateway.
pub const RESOURCE_GATEWAY: &str = "gateway";
pub const RESOURCE_GATEWAY_TARGET: &str = "target";

/// The resource noun for one STRATEGY of a memory, which a request addresses by
/// `memoryStrategyId` through the memory that holds it.
pub const RESOURCE_MEMORY_STRATEGY: &str = "memory strategy";

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

/// `MemoryStatus` members this service drives. The enum's full value list is
/// `CREATING | ACTIVE | FAILED | DELETING | UPDATING`
/// (`smithy_query shape=MemoryStatus`).
pub const MEMORY_STATUS_CREATING: &str = "CREATING";
pub const MEMORY_STATUS_ACTIVE: &str = "ACTIVE";
pub const MEMORY_STATUS_UPDATING: &str = "UPDATING";

/// `MemoryStrategyStatus` members a strategy of a memory this emulator drives
/// reports. The enum's full value list is `CREATING | ACTIVE | DELETING |
/// FAILED` (`smithy_query shape=MemoryStrategyStatus`).
pub const MEMORY_STRATEGY_STATUS_CREATING: &str = "CREATING";
pub const MEMORY_STRATEGY_STATUS_ACTIVE: &str = "ACTIVE";
pub const MEMORY_STRATEGY_STATUS_DELETING: &str = "DELETING";

/// How long a freshly created memory stays in its transient status (`CREATING`)
/// before the clock owes it `ACTIVE`.
///
/// The same span an updated memory stays `UPDATING`, and a strategy an update
/// removed stays `DELETING` before it is gone.
///
/// AWS documents no duration for this transition, so the span is chosen: short
/// enough to keep a client script quick, long enough that a caller acting
/// straight after the call still observes the transient status.
pub const MEMORY_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// How many memories this emulator will hold.
///
/// The real service caps memories per account per Region with a service quota,
/// and `CreateMemory` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding one. The number is NOT a fidelity claim about the published quota:
/// it is the emulator's own ceiling, set generously so no realistic workflow
/// meets it, so that a caller looping on create cannot grow this process's
/// memory without bound.
pub const MAX_MEMORIES: usize = 1_000;

/// How many strategies one memory will hold.
///
/// `MemoryStrategyInputList` carries no `length` constraint, so the boundary
/// walk bounds the requested list at nothing and the handler refuses a longer
/// one with `CreateMemory`'s modelled `ServiceQuotaExceededException` — the real
/// service caps strategies per memory with a service quota too. Like the other
/// ceilings here the number is the emulator's own, set generously so no
/// realistic workflow meets it, so that neither the work one request implies nor
/// the strategies a stored memory retains grows without bound.
pub const MAX_STRATEGIES_PER_MEMORY: usize = 100;

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

    /// This version's `AgentRuntimeVersionSummary`, as every listing of versions
    /// reports it. `agent_runtime_arn` is the owning runtime's, which the summary
    /// reports and the version itself does not store twice.
    pub fn summary(&self, agent_runtime_arn: &str, now: SystemTime) -> AgentRuntimeVersionSummary {
        AgentRuntimeVersionSummary {
            agent_runtime_arn: agent_runtime_arn.to_string(),
            agent_runtime_version: self.version_string(),
            status: self.status_at(now).to_string(),
        }
    }

    /// The `capacityProviderId` this version runs on, when it was configured with
    /// a capacity provider at all.
    pub fn capacity_provider_id(&self) -> Option<&str> {
        let configuration = self.capacity_provider_configuration.as_ref()?;
        ::service_common::arn::resource_id_of_type(
            &configuration.capacity_provider_arn,
            CAPACITY_PROVIDER_RESOURCE,
        )
        .filter(|id| !id.is_empty())
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
    /// The endpoints created on this runtime, keyed by `name` — the one
    /// identifier every runtime-endpoint route addresses an endpoint by, and
    /// what its ARN ends in. An endpoint is reached only THROUGH its runtime, so
    /// it is held here rather than in a map of its own: a runtime and its
    /// endpoints cannot drift apart, and the endpoints go when the runtime does.
    pub endpoints: BTreeMap<String, AgentRuntimeEndpointRecord>,
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

/// How many endpoints ONE agent runtime will hold.
///
/// `CreateAgentRuntimeEndpoint` models `ServiceQuotaExceededException` (HTTP
/// 402) for exceeding a quota, and the real service caps endpoints per runtime
/// with one. The number is NOT a fidelity claim about the published quota: it is
/// the emulator's own ceiling, set generously so no realistic workflow meets it,
/// so that a caller looping on create cannot grow this process's memory without
/// bound.
pub const MAX_ENDPOINTS_PER_AGENT_RUNTIME: usize = 1_000;

/// How long a freshly created agent runtime endpoint stays `CREATING` before
/// the clock owes it `READY`, counted from the instant the runtime version it
/// fronts is itself ready.
///
/// AWS documents no duration for this transition, so the span is chosen: long
/// enough that a caller acting straight after the create still observes the
/// transient status, short enough to keep a client script quick.
pub const AGENT_RUNTIME_ENDPOINT_SETTLE_DELAY: Duration = Duration::from_secs(3);

/// The modelled not-found error for an agent runtime — or for a VERSION of one —
/// that an ENDPOINT operation could not resolve: the id the lookup ran for
/// ("Agent with agentId: <id> not found").
///
/// The endpoint family words a runtime it cannot resolve in this shorter
/// sentence than the runtime family's own [`agent_runtime_not_found`], and
/// reports the agent rather than the version even when it is a version that is
/// missing: an endpoint names a version only to pin one, so there is one
/// sentence for "no such agent to put an endpoint on".
pub fn agent_not_found_for_endpoint(agent_runtime_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Agent with agentId: {agent_runtime_id} not found")),
    }
}

/// The modelled not-found error for an endpoint the named agent runtime does
/// not hold.
///
/// The runtime's own absence is reported by [`agent_runtime_not_found`]; this is
/// for a runtime that IS held and an endpoint name it does not carry, so the
/// sentence names the endpoint rather than the agent. Shared by the handlers
/// rather than written per operation, so one endpoint cannot be reported missing
/// in two different sentences.
pub fn agent_runtime_endpoint_not_found(endpoint_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Agent endpoint with name {endpoint_name} not found.")),
    }
}

/// How this service words the conflict a SECOND endpoint under one name on one
/// agent runtime is.
///
/// The name is what an endpoint's route and its ARN address it by, so one
/// runtime holds one endpoint per name: a repeat is a conflict rather than a
/// second endpoint later reads cannot tell apart. The sentence names no
/// endpoint: this service reports the clash by class, and the name the caller
/// asked for is its own request's.
pub const AGENT_RUNTIME_ENDPOINT_NAME_TAKEN: &str =
    "An endpoint with the specified name already exists. Check the request and try again.";

/// How this service words the endpoint ceiling of one agent runtime, in the
/// modelled `ServiceQuotaExceededException`.
pub fn agent_runtime_endpoint_quota_met(agent_runtime_id: &str) -> String {
    format!(
        "agent runtime {agent_runtime_id} already holds the maximum of \
         {MAX_ENDPOINTS_PER_AGENT_RUNTIME} endpoints"
    )
}

/// One endpoint of an agent runtime: the stable name callers invoke, the runtime
/// version it points to, the version it is actually serving, and the status the
/// clock drives.
///
/// The endpoint PINS a version: the runtime publishing another version does not
/// move an endpoint onto it, so the version lives here rather than being read
/// off the runtime at reporting time. The runtime's own id, name and ARN are NOT
/// copied in — they are read from the runtime that holds the record, so one
/// resource is never spelled two ways.
#[derive(Debug, Clone)]
pub struct AgentRuntimeEndpointRecord {
    /// The endpoint's own identifier (`AgentRuntimeEndpointId`), minted at
    /// create and stable for the endpoint's life.
    pub endpoint_id: String,
    pub endpoint_name: String,
    pub endpoint_arn: String,
    pub description: Option<String>,
    /// The runtime version this endpoint points at, as a number; rendered like
    /// [`AgentRuntimeVersionRecord::version_string`] does.
    pub target_version: u32,
    /// The version the endpoint is serving UNTIL
    /// [`settles_at`](Self::settles_at) passes — `None` for an endpoint that has
    /// never finished a transition, which serves nothing yet.
    pub live_version: Option<u32>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// The status until [`settles_at`](Self::settles_at) passes.
    pub transient_status: String,
    /// The status from [`settles_at`](Self::settles_at) onwards.
    pub settled_status: String,
    /// The instant the transient status is owed its transition. Stored, never
    /// slept on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    pub tags: BTreeMap<String, String>,
    pub failure_reason: Option<String>,
}

impl AgentRuntimeEndpointRecord {
    /// The status this endpoint reports at `now`: the transient one until the
    /// transition the clock owes comes due, the settled one afterwards.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// The version this endpoint points at, as the wire renders it
    /// (`AgentRuntimeVersion`, pattern `^([1-9][0-9]{0,4})$`).
    pub fn target_version_string(&self) -> String {
        self.target_version.to_string()
    }

    /// The version this endpoint is SERVING at `now`.
    ///
    /// An endpoint serves the version it points to only once the transition onto
    /// it has come due; until then it serves whatever it served before, which is
    /// nothing at all for an endpoint that was only just created.
    pub fn live_version_at(&self, now: SystemTime) -> Option<String> {
        if now < self.settles_at {
            self.live_version.map(|version| version.to_string())
        } else {
            Some(self.target_version_string())
        }
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
    /// The custom transformation configuration, which only `UpdateGateway`
    /// carries (`CreateGatewayRequest` declares no such member), so a gateway
    /// holds none until an update supplies one.
    pub custom_transform_configuration: Option<CustomTransformConfiguration>,
    /// The WAF configuration, carried by `UpdateGateway` only, for the same
    /// reason as [`custom_transform_configuration`](Self::custom_transform_configuration).
    pub waf_configuration: Option<WafConfiguration>,
    /// The instant an accepted deletion removes this record, once `DeleteGateway`
    /// has accepted one; `None` for a gateway nobody has deleted. Stored, never
    /// slept on: the removal is the clock's, materialised by
    /// [`ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState::expire_deleted_gateways_and_targets`]
    /// on the next call that reads the store.
    pub removal_due_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    pub workload_identity_details: Option<WorkloadIdentityDetails>,
    /// Every target on this gateway, keyed by `targetId`. The key order is the
    /// deterministic list order a target listing pages over.
    pub targets: BTreeMap<String, GatewayTargetRecord>,
}

impl GatewayRecord {
    /// Whether a deletion has been accepted for this gateway: it reports
    /// `DELETING` and the clock owes its removal.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether any target is still ATTACHED to this gateway — a child the
    /// gateway cannot be destroyed out from under.
    ///
    /// A target whose own deletion has already been accepted is on its way out
    /// on the clock and is no longer attached: the caller has already asked for
    /// it to go, so it does not hold the gateway back.
    pub fn has_attached_targets(&self) -> bool {
        self.targets.values().any(|target| !target.is_deleting())
    }

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
            // `UpdateGateway` is the only call of this service that carries a
            // custom transform or a WAF configuration, so a gateway reports what
            // the last update stored it with, and none until one does.
            custom_transform_configuration: self.custom_transform_configuration.clone(),
            interceptor_configurations: self.interceptor_configurations.clone(),
            policy_engine_configuration: self.policy_engine_configuration.clone(),
            workload_identity_details: self.workload_identity_details.clone(),
            exception_level: self.exception_level.clone(),
            // No member of any request this service models names a web ACL, so a
            // gateway is associated with none.
            web_acl_arn: None,
            waf_configuration: self.waf_configuration.clone(),
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

    /// This gateway as `UpdateGateway`'s response envelope — the same members
    /// [`Self::create_output`] renders, so an update reports exactly the record it
    /// just wrote and a later `GetGateway` cannot differ from it.
    pub fn update_output(&self, now: SystemTime) -> UpdateGatewayOutput {
        let created = self.create_output(now);
        UpdateGatewayOutput {
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

    /// This gateway as one entry of a gateway listing (`GatewaySummary`), with the
    /// status the clock owes it at `now` — the same status `GetGateway` reports,
    /// off the same record, so a listing cannot drift from a read.
    pub fn summary(&self, now: SystemTime) -> GatewaySummary {
        GatewaySummary {
            gateway_id: self.gateway_id.clone(),
            name: self.name.clone(),
            status: self.status_at(now).to_string(),
            description: self.description.clone(),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            authorizer_type: self.authorizer_type.clone(),
            protocol_type: self.protocol_type.clone(),
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
    /// The instant an accepted deletion removes this target, once
    /// `DeleteGatewayTarget` has accepted one; `None` for a target nobody has
    /// deleted. Stored, never slept on: the removal is the clock's, materialised
    /// by
    /// [`ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState::expire_deleted_gateways_and_targets`]
    /// on the next call that reads the gateway store.
    pub removal_due_at: Option<SystemTime>,
}

impl GatewayTargetRecord {
    /// Whether a deletion has been accepted for this target: it reports
    /// `DELETING` and the clock owes its removal.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }
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

    /// This target as one entry of a target listing (`TargetSummary`), with the
    /// status the clock owes it at `now` — the same status the single-target read
    /// reports, off the same record, so a listing cannot drift from a read.
    pub fn summary(&self, now: SystemTime) -> TargetSummary {
        // `resourcePriority` and `listingMode` are configurable on exactly one
        // target shape, the MCP server target
        // (`smithy_query shape=McpServerTargetConfiguration`), so they are
        // reported for that shape and omitted for every other one, which carries
        // neither.
        let mcp_server = match &self.target_configuration {
            TargetConfiguration::Mcp(McpTargetConfiguration::mcpServer(server)) => Some(server),
            _ => None,
        };
        TargetSummary {
            target_id: self.target_id.clone(),
            name: self.name.clone(),
            status: self.status_at(now).to_string(),
            description: self.description.clone(),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            resource_priority: mcp_server.map(|server| {
                server
                    .resource_priority
                    .unwrap_or(DEFAULT_TARGET_RESOURCE_PRIORITY)
            }),
            // Nothing has synchronized this target: `SynchronizeGatewayTargets`
            // is the operation that would, and it is not implemented.
            last_synchronized_at: None,
            // Carried only by a credential provider with an authorization-code
            // grant that needs user federation; none of the providers a target
            // can be created with here does.
            authorization_data: None,
            target_type: Some(target_type(&self.target_configuration).to_string()),
            listing_mode: mcp_server.and_then(|server| server.listing_mode.clone()),
        }
    }

    /// This target as `UpdateGatewayTarget`'s response envelope — the same members
    /// [`Self::create_output`] renders, so an update reports exactly the record it
    /// just wrote and a later `GetGatewayTarget` cannot differ from it.
    pub fn update_output(
        &self,
        gateway_arn: &str,
        now: SystemTime,
    ) -> UpdateGatewayTargetOutput {
        let created = self.create_output(gateway_arn, now);
        UpdateGatewayTargetOutput {
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

    /// This target as `DeleteGatewayTarget`'s response envelope: the target the
    /// deletion was accepted for, the gateway it was served by, and the status it
    /// reports from now on.
    pub fn delete_output(&self, gateway_arn: &str, now: SystemTime) -> DeleteGatewayTargetOutput {
        DeleteGatewayTargetOutput {
            gateway_arn: gateway_arn.to_string(),
            target_id: self.target_id.clone(),
            status: self.status_at(now).to_string(),
            // Reported only for a status that HAS a reason; a deletion this
            // emulator accepts has none.
            status_reasons: None,
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

/// How many harnesses this emulator will hold.
///
/// `CreateHarness` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding the real service's per-account quota. This number is NOT a fidelity
/// claim about that quota: it is the emulator's own ceiling, set generously so no
/// realistic workflow meets it, so that a caller looping on create cannot grow
/// this process's memory without bound.
pub const MAX_HARNESSES: usize = 1_000;

/// How long a freshly created harness stays `CREATING` before the clock owes it
/// `READY`.
///
/// AWS documents no duration for this transition, so the span is chosen: short
/// enough to keep a client script quick, long enough that a caller acting
/// straight after the call still observes the transient status.
pub const HARNESS_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// The version a created harness carries. `HarnessVersion` is
/// `^([1-9][0-9]{0,4})$`, so versions start at 1; only `UpdateHarness` publishes
/// another one, and it is not implemented, so a stored harness is at version 1.
pub const FIRST_HARNESS_VERSION: u32 = 1;

/// How many endpoints ONE harness will hold.
///
/// `CreateHarnessEndpoint` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding the real service's quota. The number is NOT a fidelity claim about
/// that quota: it is the emulator's own ceiling, set generously so no realistic
/// workflow meets it, so that a caller looping on create cannot grow this
/// process's memory without bound.
pub const MAX_ENDPOINTS_PER_HARNESS: usize = 1_000;

/// How long a freshly created harness endpoint stays `CREATING` before the clock
/// owes it `READY`.
///
/// AWS documents no duration for this transition, so the span is chosen: long
/// enough that a caller acting straight after the call still observes the
/// transient status, short enough to keep a client script quick. An endpoint is
/// served BY its harness, so the span runs from the instant the HARNESS itself is
/// ready ([`HarnessRecord::settles_at`]), not from the create call.
pub const HARNESS_ENDPOINT_SETTLE_DELAY: Duration = Duration::from_secs(2);

/// `HarnessTruncationStrategy` for "do not truncate". The enum's full value list
/// is `sliding_window | summarization | none`
/// (`smithy_query shape=HarnessTruncationStrategy`), and `Harness.truncation` is
/// `@required` on every response while `CreateHarness`'s is optional, so a
/// harness created without one reports the strategy that truncates nothing.
pub const HARNESS_TRUNCATION_NONE: &str = "none";

/// `NetworkMode` for a runtime reachable over the public internet. The enum's
/// full value list is `PUBLIC | VPC` (`smithy_query shape=NetworkMode`), and
/// `VPC` cannot be defaulted into: `VpcConfig` makes `securityGroups` and
/// `subnets` `@required`, so only the caller can ask for it.
pub const NETWORK_MODE_PUBLIC: &str = "PUBLIC";

/// The member `CreateHarness` refuses a request for when it carries none, and
/// why.
///
/// `Harness.model` is `@required` on every response while
/// `CreateHarnessRequest.model` is optional, and AWS documents no default model
/// for a harness. Rather than invent a model id no AWS rule produces, the
/// operation answers its own modelled `ValidationException` naming the member —
/// the request cannot be served faithfully. If AWS's default ever becomes known,
/// this becomes that default instead.
pub const HARNESS_MODEL_MEMBER: &str = "model";
pub const HARNESS_MODEL_REQUIRED: &str =
    "must be specified: a harness reports the model it runs with, and this service has no \
     default model to run one with";

/// The modelled not-found error for a harness this account does not hold.
///
/// This control plane words a missing harness in terms of the agent it runs:
/// "Agent with name <harnessId> not found." — the sentence around the
/// identifier is fixed, and the identifier itself is the one the request named.
/// Shared by the handlers rather than written per operation, so one harness
/// cannot be reported missing in two different sentences by two operations.
pub fn harness_not_found(harness_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Agent with name {harness_id} not found.")),
    }
}

/// The modelled not-found error for an endpoint the named harness does not hold.
///
/// Worded like the harness's own 404 ([`harness_not_found`]) — this control plane
/// words a missing resource of the harness family in terms of the agent it runs —
/// with the endpoint named by the one identifier its route addresses it by.
/// Shared by the handlers rather than written per operation, so one endpoint
/// cannot be reported missing in two different sentences by two operations.
pub fn harness_endpoint_not_found(endpoint_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Agent endpoint with name {endpoint_name} not found.")),
    }
}

/// How this service words the conflict a SECOND endpoint under one name on one
/// harness is: the name the request asked for and the harness that already holds
/// an endpoint under it.
///
/// The name is what an endpoint's route and its ARN address it by, so one harness
/// holds one endpoint per name. The sentence takes the form every create of this
/// service refuses a duplicate name in (`CreateHarness`, `CreateGateway`,
/// `CreateMemory` and the rest): the resource, the name the request carried, and
/// the resource that already holds it.
pub fn harness_endpoint_name_taken(endpoint_name: &str, harness_id: &str) -> String {
    format!("an endpoint named {endpoint_name} already exists on harness {harness_id}")
}

/// How this service words [`MAX_ENDPOINTS_PER_HARNESS`] being met: the harness
/// the request named and the ceiling it is at, in the form every ceiling of this
/// service is reported in (`CreateGatewayTarget`'s targets per gateway,
/// `UpdateAgentRuntime`'s versions per runtime).
pub fn harness_endpoint_quota_met(harness_id: &str) -> String {
    format!("harness {harness_id} already holds the maximum of {MAX_ENDPOINTS_PER_HARNESS} endpoints")
}

/// One harness version's key in the version-ordered map a listing pages over:
/// the version number zero-padded to the five digits `HarnessVersion` allows
/// (pattern `^([1-9][0-9]{0,4})$`), so ascending KEY order is ascending VERSION
/// order for every version the wire can carry — which is what makes the shared
/// page walk's "resume strictly after the key the last page ended on" cursor
/// sound for a listing of versions.
fn harness_version_key(version: u32) -> String {
    format!("{version:05}")
}

/// One harness: everything the caller configured, plus what the service minted
/// for it and the status the clock drives.
///
/// `CreateHarness`, `GetHarness` and `DeleteHarness` all report the SAME
/// `Harness` structure, so it is rendered from this one record
/// ([`Self::harness`]) rather than in any handler: a member the create reports
/// and the read does not would be a difference no stored state justifies.
#[derive(Debug, Clone)]
pub struct HarnessRecord {
    pub harness_id: String,
    pub harness_arn: String,
    pub harness_name: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The version this harness reports, as a number; rendered by
    /// [`Self::version_string`].
    pub version: u32,
    /// The status until [`settles_at`](Self::settles_at) passes.
    pub transient_status: String,
    /// The status from [`settles_at`](Self::settles_at) onwards.
    pub settled_status: String,
    /// The instant the transient status is owed its transition. Stored, never
    /// slept on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    pub execution_role_arn: String,
    pub model: HarnessModelConfiguration,
    pub system_prompt: Vec<HarnessSystemContentBlock>,
    pub tools: Vec<HarnessTool>,
    pub skills: Vec<HarnessSkill>,
    pub allowed_tools: Vec<String>,
    pub truncation: HarnessTruncationConfiguration,
    /// The AgentCore Runtime environment this harness runs in: the identity the
    /// service minted for it, with the lifecycle, network and filesystem
    /// configuration the request asked for (or the documented defaults).
    pub environment: HarnessAgentCoreRuntimeEnvironment,
    pub environment_artifact: Option<HarnessEnvironmentArtifact>,
    pub environment_variables: BTreeMap<String, String>,
    pub authorizer_configuration: Option<AuthorizerConfiguration>,
    pub memory: Option<HarnessMemoryConfiguration>,
    pub hooks: Vec<HarnessHook>,
    pub max_iterations: Option<i32>,
    pub max_tokens: Option<i32>,
    pub timeout_seconds: Option<i32>,
    pub tags: BTreeMap<String, String>,
    /// The endpoints created on this harness, keyed by `endpointName` — the one
    /// identifier every harness-endpoint route addresses an endpoint by, and what
    /// its ARN ends in. An endpoint is reached only THROUGH its harness, so it is
    /// held here rather than in a map of its own: a harness and its endpoints
    /// cannot drift apart, and deleting the harness takes them with it.
    pub endpoints: BTreeMap<String, HarnessEndpointRecord>,
}

impl HarnessRecord {
    /// The status this harness reports at `now`: the transient one until the
    /// transition the clock owes comes due, the settled one afterwards.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// The version number as the wire renders it (`HarnessVersion`, pattern
    /// `^([1-9][0-9]{0,4})$`).
    pub fn version_string(&self) -> String {
        self.version.to_string()
    }

    /// Whether a `harnessVersion` query value names the version this harness
    /// holds. `None` asks for the latest, which it always holds; an unparseable
    /// or unknown version names no stored version, which the caller answers with
    /// `ResourceNotFoundException`.
    pub fn holds_version(&self, requested: Option<&str>) -> bool {
        match requested {
            None => true,
            Some(raw) => raw.parse::<u32>().is_ok_and(|version| version == self.version),
        }
    }

    /// This harness's `HarnessVersionSummary` for the version it holds, as a
    /// listing of versions reports it at `now`. Rendered from the same record
    /// [`Self::harness`] reads, so a summary and a full read cannot disagree on
    /// the identity, the version or the status of one harness.
    pub fn version_summary(&self, now: SystemTime) -> HarnessVersionSummary {
        HarnessVersionSummary {
            harness_id: self.harness_id.clone(),
            harness_name: self.harness_name.clone(),
            arn: self.harness_arn.clone(),
            harness_version: self.version_string(),
            status: self.status_at(now).to_string(),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            // Reported only for a version in a failed status; one this emulator
            // drives reaches READY, which has no failure.
            failure_reason: None,
        }
    }

    /// Every version this harness holds, keyed by [`harness_version_key`] so the
    /// map's own order IS ascending version order.
    ///
    /// A harness gains one version per publish: a create publishes
    /// [`FIRST_HARNESS_VERSION`] and only an update publishes another, so a
    /// stored harness holds exactly the version in [`Self::version`] — the
    /// listing reports the versions this record persists rather than a range of
    /// numbers nothing was ever stored for.
    pub fn version_summaries(&self, now: SystemTime) -> BTreeMap<String, HarnessVersionSummary> {
        [(harness_version_key(self.version), self.version_summary(now))]
            .into_iter()
            .collect()
    }

    /// This harness as every harness operation reports it, at `now`.
    pub fn harness(&self, now: SystemTime) -> Harness {
        self.harness_with_status(self.status_at(now).to_string())
    }

    /// This harness reported with `status` instead of the one the clock resolves
    /// — the delete path, which reports the harness it just removed as
    /// `DELETING`.
    pub fn harness_with_status(&self, status: String) -> Harness {
        Harness {
            harness_id: self.harness_id.clone(),
            harness_name: self.harness_name.clone(),
            arn: self.harness_arn.clone(),
            status,
            harness_version: Some(self.version_string()),
            execution_role_arn: self.execution_role_arn.clone(),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            model: self.model.clone(),
            system_prompt: self.system_prompt.clone(),
            tools: self.tools.clone(),
            skills: self.skills.clone(),
            allowed_tools: self.allowed_tools.clone(),
            truncation: self.truncation.clone(),
            environment: HarnessEnvironmentProvider::agentCoreRuntimeEnvironment(
                self.environment.clone(),
            ),
            environment_artifact: self.environment_artifact.clone(),
            environment_variables: (!self.environment_variables.is_empty()).then(|| {
                self.environment_variables
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
            authorizer_configuration: self.authorizer_configuration.clone(),
            memory: self.memory.clone(),
            hooks: (!self.hooks.is_empty()).then(|| self.hooks.clone()),
            max_iterations: self.max_iterations,
            max_tokens: self.max_tokens,
            timeout_seconds: self.timeout_seconds,
            // Reported only for a harness in a failed status; one this emulator
            // drives reaches READY, which has no failure.
            failure_reason: None,
        }
    }
}

/// One endpoint of a harness: the stable name callers invoke, the harness
/// version it serves, and the status the clock drives.
///
/// `CreateHarnessEndpoint` and every later read of the endpoint report the SAME
/// `HarnessEndpoint` structure, so it is rendered from this one record
/// ([`Self::endpoint`]) rather than in any handler: a member the create reports
/// and a read does not would be a difference no stored state justifies. The
/// harness's own id and name are NOT copied in — they are read from the harness
/// that holds the record, so a rename cannot leave two spellings behind.
#[derive(Debug, Clone)]
pub struct HarnessEndpointRecord {
    pub endpoint_name: String,
    pub endpoint_arn: String,
    pub description: Option<String>,
    /// The harness version this endpoint points to, as a number; rendered like
    /// [`HarnessRecord::version_string`] does.
    pub target_version: u32,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status until [`settles_at`](Self::settles_at) passes.
    pub transient_status: String,
    /// The status from [`settles_at`](Self::settles_at) onwards.
    pub settled_status: String,
    /// The instant the transient status is owed its transition. Stored, never
    /// slept on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    pub tags: BTreeMap<String, String>,
}

impl HarnessEndpointRecord {
    /// The status this endpoint reports at `now`: the transient one until the
    /// transition the clock owes comes due, the settled one afterwards.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// The version this endpoint is SERVING at `now`.
    ///
    /// An endpoint serves the version it points to only once it has finished
    /// transitioning onto it: until then the target version is the one it is
    /// moving to and nothing is live behind the name yet, which is exactly the
    /// difference the model documents between `liveVersion` and `targetVersion`.
    fn live_version(&self, now: SystemTime) -> Option<String> {
        (self.status_at(now) == STATUS_READY).then(|| self.target_version.to_string())
    }

    /// This endpoint as every harness-endpoint operation reports it at `now`, for
    /// the harness that holds it.
    pub fn endpoint(&self, harness_id: &str, harness_name: &str, now: SystemTime) -> HarnessEndpoint {
        self.endpoint_with_status(harness_id, harness_name, self.status_at(now).to_string(), now)
    }

    /// This endpoint reported with `status` instead of the one the clock resolves
    /// — the delete path, which reports the endpoint it just removed as
    /// `DELETING`.
    pub fn endpoint_with_status(
        &self,
        harness_id: &str,
        harness_name: &str,
        status: String,
        now: SystemTime,
    ) -> HarnessEndpoint {
        HarnessEndpoint {
            harness_id: harness_id.to_string(),
            harness_name: harness_name.to_string(),
            endpoint_name: self.endpoint_name.clone(),
            arn: self.endpoint_arn.clone(),
            status,
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            live_version: self.live_version(now),
            target_version: Some(self.target_version.to_string()),
            description: self.description.clone(),
            // Reported only for an endpoint whose create or update failed; one
            // this emulator drives reaches READY, which has no failure.
            failure_reason: None,
        }
    }
}

/// One AgentCore Memory resource: everything the caller configured, the
/// strategies minted for it, and the status the clock drives.
///
/// The strategies are stored as the `MemoryStrategy` the wire reports, minus the
/// status — that one member is the clock's, so it is computed on every read
/// ([`Self::memory`]) rather than frozen into the record, from the ids the
/// mutation in flight minted and removed.
#[derive(Debug, Clone)]
pub struct MemoryRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub encryption_key_arn: Option<String>,
    pub memory_execution_role_arn: Option<String>,
    /// `eventExpiryDuration`, in days, as the caller set it.
    pub event_expiry_duration: i32,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status until [`settles_at`](Self::settles_at) passes.
    pub transient_status: String,
    /// The status from [`settles_at`](Self::settles_at) onwards.
    pub settled_status: String,
    /// The instant the transient status is owed its transition. Stored, never
    /// slept on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    pub strategies: Vec<MemoryStrategy>,
    /// The `strategyId`s the CREATE of this memory minted: each is `CREATING`
    /// until [`settles_at`](Self::settles_at) passes, `ACTIVE` afterwards — a
    /// strategy created WITH the memory is provisioned for as long as the memory
    /// is.
    ///
    /// An UPDATE adds no id here and clears what it finds: the strategy an
    /// update adds is reported `ACTIVE` by the update itself, so nothing the
    /// memory holds is `CREATING` once an update of it has been accepted.
    pub creating_strategy_ids: BTreeSet<String>,
    /// The `strategyId`s a mutation REMOVED, each with the instant its removal
    /// comes due: the strategy is `DELETING` until then and is reported by no
    /// read afterwards, the way the clock owes a removal.
    ///
    /// Each removal carries its OWN deadline rather than hanging off
    /// [`settles_at`](Self::settles_at), so a later update cannot defer an
    /// earlier removal by re-arming the record's transition.
    pub deleting_strategy_ids: BTreeMap<String, SystemTime>,
    pub indexed_keys: Option<Vec<IndexedKey>>,
    pub namespace_keys: Option<Vec<NamespaceKeyEntry>>,
    pub stream_delivery_resources: Option<StreamDeliveryResources>,
    pub tags: BTreeMap<String, String>,
}

impl MemoryRecord {
    /// The status this memory reports at `now`: the transient one until the
    /// transition the clock owes comes due, the settled one afterwards.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// The status one of this memory's strategies reports at `now`: `DELETING`
    /// while the removal a mutation started is still owed, `CREATING` while the
    /// strategy the memory's own create minted is still owed its transition,
    /// `ACTIVE` afterwards.
    fn strategy_status_at(&self, strategy_id: &str, now: SystemTime) -> &'static str {
        if self.deleting_strategy_ids.contains_key(strategy_id) {
            MEMORY_STRATEGY_STATUS_DELETING
        } else if now < self.settles_at && self.creating_strategy_ids.contains(strategy_id) {
            MEMORY_STRATEGY_STATUS_CREATING
        } else {
            MEMORY_STRATEGY_STATUS_ACTIVE
        }
    }

    /// The strategies this memory still HAS: the ones no mutation has removed.
    /// A strategy a mutation removed is addressed by no later request, however
    /// long its `DELETING` span still has to run.
    pub fn live_strategies(&self) -> impl Iterator<Item = &MemoryStrategy> {
        self.strategies
            .iter()
            .filter(|strategy| !self.deleting_strategy_ids.contains_key(&strategy.strategy_id))
    }

    /// Drop the strategies whose own removal deadline the clock has already
    /// passed, so the record stops carrying what no read reports.
    ///
    /// Called from the MUTATING paths only: a read computes what it reports
    /// ([`Self::memory`]) without writing.
    pub fn prune_settled_strategy_deletions(&mut self, now: SystemTime) {
        let removed: BTreeSet<String> = self
            .deleting_strategy_ids
            .iter()
            .filter(|(_, due)| now >= **due)
            .map(|(strategy_id, _)| strategy_id.clone())
            .collect();
        if removed.is_empty() {
            return;
        }
        self.deleting_strategy_ids
            .retain(|strategy_id, _| !removed.contains(strategy_id));
        self.strategies
            .retain(|strategy| !removed.contains(&strategy.strategy_id));
        self.creating_strategy_ids
            .retain(|strategy_id| !removed.contains(strategy_id));
    }

    /// The namespace variable keys this memory's live strategies REFERENCE
    /// through their namespace templates, minus the three the service defines
    /// itself ([`SERVICE_NAMESPACE_VARIABLES`]).
    ///
    /// What the caller must still define under `namespaceKeys`: a template's
    /// `{variable}` resolves either to a service-defined variable or to one of
    /// those definitions. Which strategy references a variable is deliberately
    /// NOT carried: no refusal of this service names a strategy.
    pub fn referenced_namespace_variables(&self) -> BTreeSet<String> {
        let mut referenced = BTreeSet::new();
        for strategy in self.live_strategies() {
            for template in strategy
                .namespace_templates
                .iter()
                .chain(strategy.namespaces.iter())
            {
                for variable in namespace_variables(template) {
                    if SERVICE_NAMESPACE_VARIABLES.contains(&variable) {
                        continue;
                    }
                    referenced.insert(variable.to_string());
                }
            }
        }
        referenced
    }

    /// The namespace variable keys this memory DEFINES and its live strategies
    /// still reference: the stored `namespaceKeys`, in the order the memory
    /// holds them, kept where a live strategy's namespace template resolves
    /// through them ([`Self::referenced_namespace_variables`]).
    ///
    /// These are the keys a `namespaceKeys` replacement may not drop — a key
    /// the memory does not define resolves through no definition whether the
    /// replacement carries it or not — and the ones the refusal reports.
    pub fn still_referenced_namespace_keys(&self) -> Vec<String> {
        let referenced = self.referenced_namespace_variables();
        let mut keys: Vec<String> = Vec::new();
        for entry in self.namespace_keys.iter().flatten() {
            if referenced.contains(&entry.key) && !keys.contains(&entry.key) {
                keys.push(entry.key.clone());
            }
        }
        keys
    }

    /// This memory as the `Memory` shape every operation that reports one
    /// carries, rendered from the stored record so a create and a later read
    /// cannot differ on a member no state justifies.
    pub fn memory(&self, now: SystemTime) -> Memory {
        let strategies = self
            .strategies
            .iter()
            // A strategy whose own `DELETING` span has run is gone: the record is
            // trimmed by the next mutation, and no read reports it meanwhile.
            .filter(|strategy| {
                self.deleting_strategy_ids
                    .get(&strategy.strategy_id)
                    .is_none_or(|due| now < *due)
            })
            .map(|strategy| MemoryStrategy {
                status: Some(self.strategy_status_at(&strategy.strategy_id, now).to_string()),
                ..strategy.clone()
            })
            .collect();
        Memory {
            arn: self.arn.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            encryption_key_arn: self.encryption_key_arn.clone(),
            memory_execution_role_arn: self.memory_execution_role_arn.clone(),
            event_expiry_duration: self.event_expiry_duration,
            status: self.status_at(now).to_string(),
            // Reported only for a status that HAS a reason; a memory this
            // emulator drives reaches ACTIVE, which has none.
            failure_reason: None,
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            strategies: Some(strategies),
            indexed_keys: self.indexed_keys.clone(),
            namespace_keys: self.namespace_keys.clone(),
            stream_delivery_resources: self.stream_delivery_resources.clone(),
            // Set only on a memory another resource (a harness) manages, and no
            // operation of this service creates one, so a memory carries none.
            managed_by_resource_arn: None,
            // Carried only by a READ of the memory ([`Self::memory_read`]).
            extraction_health: None,
        }
    }

    /// This memory as a READ of it reports it ([`Self::memory`]), plus the
    /// extraction health such a read carries at BOTH levels: on the memory and
    /// inside every strategy it reports.
    ///
    /// The model declares no member for it; the recorded service answers a
    /// memory read with one at each level, so a client can ask whether
    /// long-term extraction is succeeding for the memory as a whole and for one
    /// strategy of it.
    pub fn memory_read(&self, now: SystemTime) -> Memory {
        let mut memory = self.memory(now);
        for strategy in memory.strategies.iter_mut().flatten() {
            strategy.extraction_health = Some(extraction_health());
        }
        memory.extraction_health = Some(extraction_health());
        memory
    }
}

/// The extraction health a memory read reports, for the memory and for each of
/// its strategies: how many of its long-term memory extraction jobs FAILED.
///
/// A memory extraction job is started and listed on the memory DATA plane
/// (`StartMemoryExtractionJob`, `ListMemoryExtractionJobs`), not here, so this
/// control plane's store holds no job of any memory and no operation of it can
/// fail one: the count every read reports is therefore zero, for the same reason
/// a memory reports no `failureReason`. It is built in one place rather than per
/// handler, so the memory level and the strategy level cannot disagree.
pub fn extraction_health() -> MemoryExtractionHealth {
    MemoryExtractionHealth {
        failed_memory_extraction_job_count: 0,
    }
}

/// The three namespace variables the service defines itself, which a caller may
/// therefore not define again: `NamespaceVariableKey`'s pattern
/// `^(?!memoryStrategyId$|actorId$|sessionId$)[a-z][a-z0-9]*$` forbids exactly
/// these names (`smithy_query shape=NamespaceVariableKey`), and a namespace
/// template resolves them without any `namespaceKeys` entry.
pub const SERVICE_NAMESPACE_VARIABLES: [&str; 3] = ["memoryStrategyId", "actorId", "sessionId"];

/// The `{variable}` names one namespace template references, in the order they
/// appear. An unclosed `{` references nothing.
pub fn namespace_variables(template: &str) -> impl Iterator<Item = &str> {
    template.split('{').skip(1).filter_map(|rest| {
        let (variable, _) = rest.split_once('}')?;
        Some(variable)
    })
}

/// `MemoryStrategyType`, one value per member of the `MemoryStrategyInput` union
/// a request carries. The enum's full value list is `SEMANTIC | SUMMARIZATION |
/// USER_PREFERENCE | CUSTOM | EPISODIC` (`smithy_query shape=MemoryStrategyType`).
pub const STRATEGY_TYPE_SEMANTIC: &str = "SEMANTIC";
pub const STRATEGY_TYPE_SUMMARIZATION: &str = "SUMMARIZATION";
pub const STRATEGY_TYPE_USER_PREFERENCE: &str = "USER_PREFERENCE";
pub const STRATEGY_TYPE_CUSTOM: &str = "CUSTOM";
pub const STRATEGY_TYPE_EPISODIC: &str = "EPISODIC";

/// `StrategyConfiguration.type` for a custom strategy, one value per member of
/// the `CustomConfigurationInput` union the request carries. The enum's full
/// value list is `SEMANTIC_OVERRIDE | SUMMARY_OVERRIDE | USER_PREFERENCE_OVERRIDE
/// | SELF_MANAGED | EPISODIC_OVERRIDE` (`smithy_query shape=OverrideType`).
pub const OVERRIDE_TYPE_SEMANTIC: &str = "SEMANTIC_OVERRIDE";
pub const OVERRIDE_TYPE_SUMMARY: &str = "SUMMARY_OVERRIDE";
pub const OVERRIDE_TYPE_USER_PREFERENCE: &str = "USER_PREFERENCE_OVERRIDE";
pub const OVERRIDE_TYPE_EPISODIC: &str = "EPISODIC_OVERRIDE";
pub const OVERRIDE_TYPE_SELF_MANAGED: &str = "SELF_MANAGED";

/// The `fieldList` member name a strategy's record-expiry specification is
/// reported under: the path to it through the request, so a caller sees which
/// member of the nested union failed.
pub const RECORD_EXPIRY_MEMBER: &str = "memoryStrategies.recordExpirySpecification.action";
pub const RECORD_EXPIRY_DAYS_MEMBER: &str =
    "memoryStrategies.recordExpirySpecification.recordExpiryDurationDays";

/// The shortest record expiry a strategy may be given: `RecordExpiryDurationDays`
/// is `range 31..`, which the model also states in prose ("The minimum is 31
/// days", `smithy_query shape=RecordExpirySpecification`). The constraint sits on
/// a member of a union nested in a list, which the framework's boundary walk does
/// not descend into, so the handler refuses it — and `MemoryStrategy` reports the
/// same shape, so a shorter duration could not be rendered back either.
pub const MIN_RECORD_EXPIRY_DURATION_DAYS: i32 = 31;

/// Why a requested strategy list is refused, in the two modelled shapes every
/// operation that takes one declares: each handler lifts this into its OWN
/// error enum, so the refusal is worded once however many operations share it.
#[derive(Debug)]
pub enum StrategyRefusal {
    Validation(ValidationException),
    QuotaExceeded(ServiceQuotaExceededException),
}

/// What one requested strategy asked for, read off whichever member of the
/// `MemoryStrategyInput` union the request carried: the type that member names,
/// and the members every one of them declares.
struct RequestedStrategy {
    strategy_type: &'static str,
    name: String,
    description: Option<String>,
    /// The legacy `namespaces` member ("Use namespaceTemplates instead").
    namespaces: Option<Vec<String>>,
    namespace_templates: Option<Vec<String>>,
    configuration: Option<StrategyConfiguration>,
    memory_record_schema: Option<MemoryRecordSchema>,
    record_expiry: Option<RecordExpirySpecification>,
}

/// The namespace template a strategy gets when the caller supplies neither
/// `namespaceTemplates` nor the legacy `namespaces`, which `MemoryStrategy`
/// reports as `@required` (exactly one entry) either way.
///
/// The three namespace variables the service defines itself are
/// [`SERVICE_NAMESPACE_VARIABLES`] — exactly the names `NamespaceVariableKey`'s
/// pattern forbids a caller to redefine. A strategy whose records summarise or
/// recount ONE conversation (summarization, episodic) is therefore scoped down
/// to the session; one whose records are facts about the actor (semantic, user
/// preference, and a custom strategy, which overrides one of those) is scoped to
/// the actor.
fn default_namespace_template(strategy_type: &str) -> String {
    if strategy_type == STRATEGY_TYPE_SUMMARIZATION || strategy_type == STRATEGY_TYPE_EPISODIC {
        "/strategies/{memoryStrategyId}/actors/{actorId}/sessions/{sessionId}".to_string()
    } else {
        "/strategies/{memoryStrategyId}/actors/{actorId}".to_string()
    }
}

/// The namespace list a strategy is stored with: the caller's
/// `namespaceTemplates`, else the legacy `namespaces` they supplied instead,
/// else [`default_namespace_template`]. `MemoryStrategy` declares both members
/// `@required` with exactly one entry, so the stored strategy carries one list
/// under both names rather than an empty one the wire cannot render.
fn effective_namespaces(requested: &RequestedStrategy) -> Vec<String> {
    for namespaces in [&requested.namespace_templates, &requested.namespaces]
        .into_iter()
        .flatten()
    {
        if !namespaces.is_empty() {
            return namespaces.clone();
        }
    }
    vec![default_namespace_template(requested.strategy_type)]
}

/// `MemoryStrategy.recordExpiryDurationDays` for a strategy's requested
/// `recordExpirySpecification`, on the path that CREATES the strategy.
///
/// The union carries either a duration or an `action`, and the model says of
/// `action`: "Set to REMOVE to remove a previously configured time-to-live …
/// This value is not applicable on create paths" — there is no previously
/// configured expiry on a strategy being created, so it is refused.
fn record_expiry_duration_days(
    requested: Option<RecordExpirySpecification>,
) -> Result<Option<i32>, ValidationException> {
    match requested {
        None => Ok(None),
        Some(RecordExpirySpecification::recordExpiryDurationDays(days)) => {
            if days < MIN_RECORD_EXPIRY_DURATION_DAYS {
                return Err(field_validation_failed(
                    RECORD_EXPIRY_DAYS_MEMBER,
                    &format!("must be at least {MIN_RECORD_EXPIRY_DURATION_DAYS} days, got {days}"),
                ));
            }
            Ok(Some(days))
        }
        Some(RecordExpirySpecification::Action(action)) => Err(field_validation_failed(
            RECORD_EXPIRY_MEMBER,
            &format!("{action} is not applicable on create paths"),
        )),
    }
}

/// One requested strategy, read off the union member the request carried.
fn requested_strategy(requested: MemoryStrategyInput) -> RequestedStrategy {
    match requested {
        MemoryStrategyInput::semanticMemoryStrategy(strategy) => RequestedStrategy {
            strategy_type: STRATEGY_TYPE_SEMANTIC,
            name: strategy.name,
            description: strategy.description,
            namespaces: strategy.namespaces,
            namespace_templates: strategy.namespace_templates,
            // A built-in strategy carries no configuration of its own: the
            // overrides `StrategyConfiguration` reports are what a CUSTOM
            // strategy supplies.
            configuration: None,
            memory_record_schema: strategy.memory_record_schema,
            record_expiry: strategy.record_expiry_specification,
        },
        MemoryStrategyInput::summaryMemoryStrategy(strategy) => RequestedStrategy {
            strategy_type: STRATEGY_TYPE_SUMMARIZATION,
            name: strategy.name,
            description: strategy.description,
            namespaces: strategy.namespaces,
            namespace_templates: strategy.namespace_templates,
            configuration: None,
            memory_record_schema: strategy.memory_record_schema,
            record_expiry: strategy.record_expiry_specification,
        },
        MemoryStrategyInput::userPreferenceMemoryStrategy(strategy) => RequestedStrategy {
            strategy_type: STRATEGY_TYPE_USER_PREFERENCE,
            name: strategy.name,
            description: strategy.description,
            namespaces: strategy.namespaces,
            namespace_templates: strategy.namespace_templates,
            configuration: None,
            memory_record_schema: strategy.memory_record_schema,
            record_expiry: strategy.record_expiry_specification,
        },
        MemoryStrategyInput::episodicMemoryStrategy(strategy) => RequestedStrategy {
            strategy_type: STRATEGY_TYPE_EPISODIC,
            name: strategy.name,
            description: strategy.description,
            namespaces: strategy.namespaces,
            namespace_templates: strategy.namespace_templates,
            // The one built-in strategy with a configuration member of its own:
            // the reflections it creates.
            configuration: strategy.reflection_configuration.map(|reflection| {
                StrategyConfiguration {
                    r#type: None,
                    extraction: None,
                    consolidation: None,
                    reflection: Some(ReflectionConfiguration::episodicReflectionConfiguration(
                        EpisodicReflectionConfiguration {
                            namespaces: reflection.namespaces,
                            namespace_templates: reflection.namespace_templates,
                            memory_record_schema: reflection.memory_record_schema,
                        },
                    )),
                    self_managed_configuration: None,
                }
            }),
            memory_record_schema: strategy.memory_record_schema,
            record_expiry: strategy.record_expiry_specification,
        },
        MemoryStrategyInput::customMemoryStrategy(strategy) => RequestedStrategy {
            strategy_type: STRATEGY_TYPE_CUSTOM,
            name: strategy.name,
            description: strategy.description,
            namespaces: strategy.namespaces,
            namespace_templates: strategy.namespace_templates,
            configuration: strategy.configuration.map(custom_configuration),
            memory_record_schema: strategy.memory_record_schema,
            record_expiry: strategy.record_expiry_specification,
        },
    }
}

/// A custom strategy's requested `CustomConfigurationInput` as the
/// `StrategyConfiguration` every operation that reports a strategy carries:
/// `type` names the override the request chose, and each prompt/model override
/// lands under the member of that shape which reports it.
fn custom_configuration(requested: CustomConfigurationInput) -> StrategyConfiguration {
    let mut configuration = StrategyConfiguration::default();
    match requested {
        CustomConfigurationInput::semanticOverride(override_input) => {
            configuration.r#type = Some(OVERRIDE_TYPE_SEMANTIC.to_string());
            configuration.extraction = override_input.extraction.map(|extraction| {
                ExtractionConfiguration::customExtractionConfiguration(
                    CustomExtractionConfiguration::semanticExtractionOverride(
                        SemanticExtractionOverride {
                            append_to_prompt: extraction.append_to_prompt,
                            model_id: extraction.model_id,
                        },
                    ),
                )
            });
            configuration.consolidation = override_input.consolidation.map(|consolidation| {
                ConsolidationConfiguration::customConsolidationConfiguration(
                    CustomConsolidationConfiguration::semanticConsolidationOverride(
                        SemanticConsolidationOverride {
                            append_to_prompt: consolidation.append_to_prompt,
                            model_id: consolidation.model_id,
                        },
                    ),
                )
            });
        }
        CustomConfigurationInput::summaryOverride(override_input) => {
            configuration.r#type = Some(OVERRIDE_TYPE_SUMMARY.to_string());
            configuration.consolidation = override_input.consolidation.map(|consolidation| {
                ConsolidationConfiguration::customConsolidationConfiguration(
                    CustomConsolidationConfiguration::summaryConsolidationOverride(
                        SummaryConsolidationOverride {
                            append_to_prompt: consolidation.append_to_prompt,
                            model_id: consolidation.model_id,
                        },
                    ),
                )
            });
        }
        CustomConfigurationInput::userPreferenceOverride(override_input) => {
            configuration.r#type = Some(OVERRIDE_TYPE_USER_PREFERENCE.to_string());
            configuration.extraction = override_input.extraction.map(|extraction| {
                ExtractionConfiguration::customExtractionConfiguration(
                    CustomExtractionConfiguration::userPreferenceExtractionOverride(
                        UserPreferenceExtractionOverride {
                            append_to_prompt: extraction.append_to_prompt,
                            model_id: extraction.model_id,
                        },
                    ),
                )
            });
            configuration.consolidation = override_input.consolidation.map(|consolidation| {
                ConsolidationConfiguration::customConsolidationConfiguration(
                    CustomConsolidationConfiguration::userPreferenceConsolidationOverride(
                        UserPreferenceConsolidationOverride {
                            append_to_prompt: consolidation.append_to_prompt,
                            model_id: consolidation.model_id,
                        },
                    ),
                )
            });
        }
        CustomConfigurationInput::episodicOverride(override_input) => {
            configuration.r#type = Some(OVERRIDE_TYPE_EPISODIC.to_string());
            configuration.extraction = override_input.extraction.map(|extraction| {
                ExtractionConfiguration::customExtractionConfiguration(
                    CustomExtractionConfiguration::episodicExtractionOverride(
                        EpisodicExtractionOverride {
                            append_to_prompt: extraction.append_to_prompt,
                            model_id: extraction.model_id,
                        },
                    ),
                )
            });
            configuration.consolidation = override_input.consolidation.map(|consolidation| {
                ConsolidationConfiguration::customConsolidationConfiguration(
                    CustomConsolidationConfiguration::episodicConsolidationOverride(
                        EpisodicConsolidationOverride {
                            append_to_prompt: consolidation.append_to_prompt,
                            model_id: consolidation.model_id,
                        },
                    ),
                )
            });
            configuration.reflection = override_input.reflection.map(|reflection| {
                ReflectionConfiguration::customReflectionConfiguration(
                    CustomReflectionConfiguration::episodicReflectionOverride(
                        EpisodicReflectionOverride {
                            append_to_prompt: reflection.append_to_prompt,
                            model_id: reflection.model_id,
                            namespaces: reflection.namespaces,
                            namespace_templates: reflection.namespace_templates,
                            memory_record_schema: reflection.memory_record_schema,
                        },
                    ),
                )
            });
        }
        CustomConfigurationInput::selfManagedConfiguration(self_managed) => {
            configuration.r#type = Some(OVERRIDE_TYPE_SELF_MANAGED.to_string());
            configuration.self_managed_configuration = Some(SelfManagedConfiguration {
                trigger_conditions: self_managed
                    .trigger_conditions
                    .unwrap_or_default()
                    .into_iter()
                    .map(trigger_condition)
                    .collect(),
                invocation_configuration: InvocationConfiguration {
                    topic_arn: self_managed.invocation_configuration.topic_arn,
                    payload_delivery_bucket_name: self_managed
                        .invocation_configuration
                        .payload_delivery_bucket_name,
                },
                historical_context_window_size: self_managed.historical_context_window_size,
            });
        }
    }
    configuration
}

/// One requested `TriggerConditionInput` as the `TriggerCondition` a stored
/// self-managed configuration reports.
pub fn trigger_condition(requested: TriggerConditionInput) -> TriggerCondition {
    match requested {
        TriggerConditionInput::messageBasedTrigger(trigger) => {
            TriggerCondition::messageBasedTrigger(MessageBasedTrigger {
                message_count: Some(trigger.message_count),
            })
        }
        TriggerConditionInput::tokenBasedTrigger(trigger) => {
            TriggerCondition::tokenBasedTrigger(TokenBasedTrigger {
                token_count: Some(trigger.token_count),
            })
        }
        TriggerConditionInput::timeBasedTrigger(trigger) => {
            TriggerCondition::timeBasedTrigger(TimeBasedTrigger {
                idle_session_timeout: Some(trigger.idle_session_timeout),
            })
        }
    }
}

/// One requested strategy after everything that does NOT need the store has
/// been resolved: the type and members to report, the namespace list the rule
/// gives it, and its validated record expiry.
///
/// Prepared BEFORE the state guard is taken, so none of the caller-sized work a
/// strategy list implies runs while this service's one store is locked.
pub struct PreparedStrategy {
    pub strategy_type: &'static str,
    pub name: String,
    pub description: Option<String>,
    pub configuration: Option<StrategyConfiguration>,
    pub memory_record_schema: Option<MemoryRecordSchema>,
    pub namespaces: Vec<String>,
    pub record_expiry_duration_days: Option<i32>,
}

/// Every requested strategy, read off its union member and validated — the same
/// walk whether the list creates a memory or is added to one.
///
/// A list longer than [`MAX_STRATEGIES_PER_MEMORY`] is refused BEFORE any entry
/// is processed, so the per-request work (and the strategies a stored memory
/// retains) is bounded by that ceiling rather than by the request body's size.
pub fn prepared_strategies(
    requested: Option<Vec<MemoryStrategyInput>>,
) -> Result<Vec<PreparedStrategy>, StrategyRefusal> {
    let requested = requested.unwrap_or_default();
    if requested.len() > MAX_STRATEGIES_PER_MEMORY {
        return Err(StrategyRefusal::QuotaExceeded(ServiceQuotaExceededException {
            message: Some(format!(
                "a memory holds at most {} strategies, and this request asked for {}",
                MAX_STRATEGIES_PER_MEMORY,
                requested.len()
            )),
        }));
    }
    let mut prepared = Vec::with_capacity(requested.len());
    for one in requested {
        let one = requested_strategy(one);
        let namespaces = effective_namespaces(&one);
        prepared.push(PreparedStrategy {
            strategy_type: one.strategy_type,
            name: one.name,
            description: one.description,
            configuration: one.configuration,
            memory_record_schema: one.memory_record_schema,
            namespaces,
            record_expiry_duration_days: record_expiry_duration_days(one.record_expiry)
                .map_err(StrategyRefusal::Validation)?,
        });
    }
    Ok(prepared)
}

/// A prepared strategy as the `MemoryStrategy` the record stores and the wire
/// reports, under the `strategy_id` minted for it.
///
/// `status` is left unset: it is the clock's, computed on every read of the
/// memory from the ids its last mutation minted and removed.
pub fn memory_strategy(
    prepared: PreparedStrategy,
    strategy_id: String,
    now: SystemTime,
) -> MemoryStrategy {
    MemoryStrategy {
        strategy_id,
        name: prepared.name,
        description: prepared.description,
        configuration: prepared.configuration,
        r#type: prepared.strategy_type.to_string(),
        namespaces: prepared.namespaces.clone(),
        namespace_templates: prepared.namespaces,
        created_at: Some(timestamp(now)),
        updated_at: Some(timestamp(now)),
        status: None,
        memory_record_schema: prepared.memory_record_schema,
        record_expiry_duration_days: prepared.record_expiry_duration_days,
        // Carried only by a READ of the memory ([`MemoryRecord::memory_read`]).
        extraction_health: None,
    }
}

/// The AgentCore Runtime environment a harness runs in, for a harness whose id
/// is `harness_id` in `region`.
///
/// The runtime a harness runs on is managed BY the harness: the caller never
/// named it, so its identity is minted 1:1 with the harness (same id, same name,
/// the runtime ARN form `AgentRuntimeArn` publishes) and is reported so a caller
/// can tell two harnesses' environments apart. The lifecycle configuration falls
/// back to the defaults the model documents
/// ([`effective_lifecycle_configuration`]) and the network configuration to
/// [`NETWORK_MODE_PUBLIC`], the only mode that needs no caller-supplied VPC.
pub fn harness_runtime_environment(
    region: &str,
    harness_id: &str,
    harness_name: &str,
    requested: Option<HarnessEnvironmentProviderRequest>,
) -> HarnessAgentCoreRuntimeEnvironment {
    let requested = requested.map(|provider| match provider {
        HarnessEnvironmentProviderRequest::agentCoreRuntimeEnvironment(environment) => environment,
    });
    let requested = requested.unwrap_or(HarnessAgentCoreRuntimeEnvironmentRequest {
        lifecycle_configuration: None,
        network_configuration: None,
        filesystem_configurations: None,
    });
    HarnessAgentCoreRuntimeEnvironment {
        agent_runtime_arn: runtime_arn(region, harness_id),
        agent_runtime_name: harness_name.to_string(),
        agent_runtime_id: harness_id.to_string(),
        lifecycle_configuration: effective_lifecycle_configuration(
            requested.lifecycle_configuration,
        ),
        network_configuration: requested.network_configuration.unwrap_or(NetworkConfiguration {
            network_mode: NETWORK_MODE_PUBLIC.to_string(),
            network_mode_config: None,
        }),
        filesystem_configurations: requested.filesystem_configurations,
    }
}

/// `arn:aws:bedrock-agentcore:{region}:{account}:harness/{harnessId}` —
/// `HarnessArn`'s pattern.
pub fn harness_arn(region: &str, harness_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("harness/{harness_id}"))
}

/// `arn:aws:bedrock-agentcore:{region}:{account}:harness/{harnessId}/harness-endpoint/{endpointName}`
/// — `HarnessEndpointArn`'s pattern, which nests the endpoint under the harness
/// that holds it.
pub fn harness_endpoint_arn(region: &str, harness_id: &str, endpoint_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("harness/{harness_id}/harness-endpoint/{endpoint_name}"),
    )
}

/// How many workload identities this emulator will hold.
///
/// `CreateWorkloadIdentity` models no quota error, so the ceiling is reported
/// with its modelled `ValidationException` ([`resource_conflict`]). The
/// number is NOT a fidelity claim about any published quota: it is the
/// emulator's own ceiling, set generously so no realistic workflow meets it, so
/// that a caller looping on create cannot grow this process's memory without
/// bound.
pub const MAX_WORKLOAD_IDENTITIES: usize = 1_000;

/// How this service words the 404 for a workload identity the account does not
/// hold: the resource type, how the route addresses the identity, and the
/// identifier the request named.
///
/// The workload-identity routes do NOT take the shared [`resource_not_found`]
/// sentence: they name the resource type and echo the `name` the caller asked
/// for. The identifier word differs per route — the read and update routes say
/// `id`, the delete route says `name` — so each route picks its own constructor
/// below rather than inventing a sentence of its own.
fn workload_identity_not_found(identifier_word: &str, name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("WorkloadIdentity with {identifier_word} {name} not found.")),
    }
}

/// The 404 for an identity a READ or an UPDATE could not resolve.
pub fn workload_identity_not_found_by_id(name: &str) -> ResourceNotFoundException {
    workload_identity_not_found("id", name)
}

/// The 404 for an identity a DELETE could not resolve — including a second
/// delete of a name an earlier delete already removed, which is refused rather
/// than answered as a silent success.
pub fn workload_identity_not_found_by_name(name: &str) -> ResourceNotFoundException {
    workload_identity_not_found("name", name)
}

/// One workload identity of this account, as `CreateWorkloadIdentity`
/// persisted it.
///
/// The name is the identifier: `GetWorkloadIdentity`,
/// `UpdateWorkloadIdentity` and `DeleteWorkloadIdentity` all address an
/// identity by `name`, and it is "unique within your account"
/// (`smithy_query doc shape=CreateWorkloadIdentityRequest member=name`), so the
/// store is keyed by it.
#[derive(Debug, Clone)]
pub struct WorkloadIdentityRecord {
    pub name: String,
    pub workload_identity_arn: String,
    /// The caller's `allowedResourceOauth2ReturnUrls`, optional on every
    /// request and every response that reports an identity, so an identity
    /// created without one reports none rather than an empty list.
    pub allowed_resource_oauth2_return_urls: Option<Vec<String>>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl WorkloadIdentityRecord {
    /// This identity as one entry of a workload-identity listing
    /// (`WorkloadIdentityType`), read off the same record the single-identity
    /// read reports, so a listing cannot disagree with `GetWorkloadIdentity`.
    pub fn summary(&self) -> WorkloadIdentityType {
        WorkloadIdentityType {
            name: self.name.clone(),
            workload_identity_arn: self.workload_identity_arn.clone(),
        }
    }

    /// This identity as `CreateWorkloadIdentity`'s response envelope.
    pub fn create_output(&self) -> CreateWorkloadIdentityOutput {
        CreateWorkloadIdentityOutput {
            name: self.name.clone(),
            workload_identity_arn: self.workload_identity_arn.clone(),
            allowed_resource_oauth2_return_urls: self
                .allowed_resource_oauth2_return_urls
                .clone(),
        }
    }

    /// This identity as `GetWorkloadIdentity`'s response envelope — the members
    /// [`Self::create_output`] renders plus the two instants the read reports,
    /// off the one record.
    pub fn get_output(&self) -> GetWorkloadIdentityOutput {
        let created = self.create_output();
        GetWorkloadIdentityOutput {
            name: created.name,
            workload_identity_arn: created.workload_identity_arn,
            allowed_resource_oauth2_return_urls: created.allowed_resource_oauth2_return_urls,
            created_time: timestamp(self.created_at),
            last_updated_time: timestamp(self.updated_at),
        }
    }

    /// This identity as `UpdateWorkloadIdentity`'s response envelope. The two
    /// operations report the same members off the same record, so an update
    /// answers exactly what the following read reports.
    pub fn update_output(&self) -> UpdateWorkloadIdentityOutput {
        let read = self.get_output();
        UpdateWorkloadIdentityOutput {
            name: read.name,
            workload_identity_arn: read.workload_identity_arn,
            allowed_resource_oauth2_return_urls: read.allowed_resource_oauth2_return_urls,
            created_time: read.created_time,
            last_updated_time: read.last_updated_time,
        }
    }

    /// Apply an `UpdateWorkloadIdentity` to this record: the request's list
    /// REPLACES the stored one ("This list replaces the existing list",
    /// `smithy_query doc shape=UpdateWorkloadIdentityRequest
    /// member=allowedResourceOauth2ReturnUrls`), so a request carrying no list
    /// leaves the identity with none, and the instant the change was applied
    /// becomes the identity's `lastUpdatedTime`.
    pub fn apply_update(&mut self, urls: Option<Vec<String>>, now: SystemTime) {
        self.allowed_resource_oauth2_return_urls = urls;
        self.updated_at = now;
    }
}

/// The modelled validation error for a request the stored state of this account
/// REFUSES rather than one whose members are malformed: a create whose `name` is
/// already held by a resource of this account, one this emulator's own ceiling
/// refuses, and a delete the resource's own dependents hold back.
///
/// Nothing is wrong with the request's own form, so `reason` is
/// [`VALIDATION_REASON_RESOURCE_CONFLICT`] and there is no `fieldList` — the
/// same reason a request the stored state refuses carries everywhere else here.
pub fn resource_conflict(why: String) -> ValidationException {
    ValidationException {
        message: why,
        reason: VALIDATION_REASON_RESOURCE_CONFLICT.to_string(),
        field_list: None,
    }
}

/// The truncation configuration to store for a request: the caller's, or the
/// strategy that truncates nothing when the caller named none.
pub fn effective_truncation_configuration(
    requested: Option<HarnessTruncationConfiguration>,
) -> HarnessTruncationConfiguration {
    requested.unwrap_or_else(|| HarnessTruncationConfiguration {
        strategy: HARNESS_TRUNCATION_NONE.to_string(),
        config: None,
    })
}

/// The `tokenVaultId` of the token vault an account holds without anything
/// having created one: the vault a request naming no `tokenVaultId` addresses,
/// and the workload-identity directory this service already mints into
/// ([`workload_identity_arn`]).
pub const DEFAULT_TOKEN_VAULT_ID: &str = "default";

/// `KeyType` for a key the service itself owns — what a token vault no caller
/// has re-keyed is encrypted under. The enum's full value list is
/// `CustomerManagedKey | ServiceManagedKey` (`smithy_query shape=KeyType`).
pub const KEY_TYPE_SERVICE_MANAGED: &str = "ServiceManagedKey";

/// The resource noun [`resource_not_found`] names for a token vault.
pub const RESOURCE_TOKEN_VAULT: &str = "token vault";

/// One token vault: the KMS configuration its contents are encrypted under, and
/// when that configuration was last set.
///
/// `kmsConfiguration` and `lastModifiedDate` are the only members any token
/// vault response carries, so the record holds exactly those two.
#[derive(Debug, Clone)]
pub struct TokenVaultRecord {
    pub kms_configuration: KmsConfiguration,
    pub last_modified_at: SystemTime,
}

impl TokenVaultRecord {
    /// A vault encrypted under the key the service manages — the configuration a
    /// vault carries until a caller points it at a key of their own. The service
    /// managed key is not the caller's, so no `kmsKeyArn` is reported for it.
    pub fn service_managed(at: SystemTime) -> Self {
        Self {
            kms_configuration: KmsConfiguration {
                key_type: KEY_TYPE_SERVICE_MANAGED.to_string(),
                kms_key_arn: None,
            },
            last_modified_at: at,
        }
    }
}

/// The account's token vaults, keyed by `tokenVaultId`.
///
/// A newtype rather than a bare map because of one invariant no caller
/// establishes: this service models no operation that creates a vault, so the
/// default vault is present from the moment the account's state is, and a read
/// neither finds it absent nor re-dates it.
#[derive(Debug)]
pub struct TokenVaults(BTreeMap<String, TokenVaultRecord>);

impl Default for TokenVaults {
    fn default() -> Self {
        Self(BTreeMap::from([(
            DEFAULT_TOKEN_VAULT_ID.to_string(),
            TokenVaultRecord::service_managed(SystemTime::now()),
        )]))
    }
}

impl TokenVaults {
    /// The vault `token_vault_id` addresses, or `None` when no vault of this
    /// account carries that id.
    pub fn get(&self, token_vault_id: &str) -> Option<&TokenVaultRecord> {
        self.0.get(token_vault_id)
    }

    /// Point the vault `token_vault_id` addresses at `kms_configuration`,
    /// stamping `at` as the instant it was last modified, and hand back the
    /// stored record. `None`, writing nothing, when no vault carries that id:
    /// this call re-keys a vault, it never creates one.
    pub fn set_kms_configuration(
        &mut self,
        token_vault_id: &str,
        kms_configuration: KmsConfiguration,
        at: SystemTime,
    ) -> Option<&TokenVaultRecord> {
        let record = self.0.get_mut(token_vault_id)?;
        record.kms_configuration = kms_configuration;
        record.last_modified_at = at;
        Some(record)
    }
}

/// The resource noun [`resource_not_found`] names for a registry, and the ARN
/// resource type `RegistryArn` carries (`…:registry/{registryId}`).
pub const RESOURCE_REGISTRY: &str = "registry";

/// `RegistryStatus` members this service drives. The enum's full value list is
/// `CREATING | READY | UPDATING | CREATE_FAILED | UPDATE_FAILED | DELETING |
/// DELETE_FAILED` (`shared_types::REGISTRY_STATUS_VALUES`, from `smithy_query
/// shape=RegistryStatus`).
pub const REGISTRY_STATUS_CREATING: &str = "CREATING";
pub const REGISTRY_STATUS_READY: &str = "READY";

/// How long a freshly created registry stays `CREATING` before the clock owes it
/// `READY`.
///
/// AWS documents no duration for this transition, so the span is chosen: long
/// enough that a caller acting straight after the call still observes the
/// transient status, short enough to keep a client script quick.
pub const REGISTRY_SETTLE_DELAY: Duration = Duration::from_secs(3);

/// How many registries this emulator will hold.
///
/// `CreateRegistry` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding the real service's quota; this number is not a fidelity claim about
/// that quota but the emulator's own ceiling, set generously so no realistic
/// workflow meets it, so a caller looping on create cannot grow this process's
/// memory without bound.
pub const MAX_REGISTRIES: usize = 1_000;

/// How wide a minted `registryId` is: `RegistryId` is `^[a-zA-Z0-9]{12,16}$`
/// (`smithy_query shape=RegistryId`), so 12 is the shortest id the pattern
/// admits. Unlike a runtime, gateway or memory id, it carries no name.
const REGISTRY_ID_LEN: usize = 12;

/// The `registryId` a `RegistryIdentifier` names.
///
/// That member accepts either the bare id or the registry's whole ARN
/// (`^(arn:aws(-[^:]+)?:bedrock-agentcore:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}$`),
/// and the id itself holds no `/`, so the last path segment IS the id for both
/// spellings. Shared by every operation that addresses a registry, so two of them
/// cannot resolve one identifier to two different records.
pub fn registry_id_from_identifier(identifier: &str) -> &str {
    identifier.rsplit_once('/').map_or(identifier, |(_, id)| id)
}

/// One registry: the catalog `CreateRegistry` minted, the configuration the
/// caller supplied for it, and the `CREATING` -> `READY` transition the clock
/// owes it.
///
/// Every read of the registry reports this record, so a create and a later read
/// cannot disagree about the registry's name, configuration or status.
#[derive(Debug, Clone)]
pub struct RegistryRecord {
    pub registry_id: String,
    pub registry_arn: String,
    pub name: String,
    pub description: Option<String>,
    /// As the caller supplied it: `authorizerType` carries no modelled default,
    /// so a registry created without one reports none.
    pub authorizer_type: Option<String>,
    pub authorizer_configuration: Option<AuthorizerConfiguration>,
    /// The EFFECTIVE approval configuration: the caller's, or the default the
    /// model documents (`autoApproval` false), so a read reports the rule the
    /// records of this registry are actually approved under.
    pub approval_configuration: ApprovalConfiguration,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status the registry reports until [`Self::settles_at`].
    pub transient_status: String,
    /// The status it reports from then on.
    pub settled_status: String,
    /// When the clock owes the move from one to the other.
    pub settles_at: SystemTime,
    /// Why the registry is in its current status, which the model documents as
    /// "typically set when the status is a failure state" — so nothing a
    /// successful create or read sets.
    pub status_reason: Option<String>,
}

impl RegistryRecord {
    /// The status at `now`: the transient one until the clock owes the
    /// transition, the settled one from then on. No read performs it — it is due
    /// at that instant whether or not anybody looks.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// What a read of this registry reports.
    pub fn get_output(&self, now: SystemTime) -> GetRegistryOutput {
        GetRegistryOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            registry_id: self.registry_id.clone(),
            registry_arn: self.registry_arn.clone(),
            authorizer_type: self.authorizer_type.clone(),
            authorizer_configuration: self.authorizer_configuration.clone(),
            approval_configuration: Some(self.approval_configuration.clone()),
            status: self.status_at(now).to_string(),
            status_reason: self.status_reason.clone(),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
        }
    }
}

/// The modelled not-found error for a browser this account does not hold.
///
/// This control plane words a missing browser in terms of the id the request
/// named: "Browser with ID <browserId> does not exist" — the sentence around the
/// identifier is fixed, and the identifier itself is the caller's own value.
/// Shared by the handlers rather than written per operation, so one browser
/// cannot be reported missing in two different sentences by two operations.
pub fn browser_not_found(browser_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Browser with ID {browser_id} does not exist")),
    }
}

/// The ARN resource type a browser THIS SERVICE CREATED carries. `BrowserArn` is
/// `…:browser(-custom)?/(aws\.browser\.v1|…)` (`smithy_query shape=BrowserArn`),
/// whose `arnReference` names `AWS::BedrockAgentCore::BrowserCustom`: the
/// `-custom` spelling is the customer-created browser, and the bare one the
/// service-provided tool (`aws.browser.v1`), which no operation mints.
pub const BROWSER_ARN_RESOURCE: &str = "browser-custom";

/// `BrowserStatus` members this service drives. The enum's full value list is
/// `CREATING | CREATE_FAILED | READY | DELETING | DELETE_FAILED | DELETED`
/// (`shared_types::BROWSER_STATUS_VALUES`, from `smithy_query
/// shape=BrowserStatus`).
pub const BROWSER_STATUS_CREATING: &str = "CREATING";
pub const BROWSER_STATUS_READY: &str = "READY";
pub const BROWSER_STATUS_DELETING: &str = "DELETING";

/// How long a freshly created browser stays `CREATING` before the clock owes it
/// `READY`, and how long an accepted deletion leaves it `DELETING` before the
/// record is gone.
///
/// AWS documents no duration for either transition, so the span is chosen: long
/// enough that a caller acting straight after the call still observes the
/// transient status, short enough to keep a client script quick.
pub const BROWSER_SETTLE_DELAY: Duration = Duration::from_secs(3);

/// How many browsers this emulator will hold.
///
/// `CreateBrowser` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding the real service's quota; this number is not a fidelity claim about
/// that quota but the emulator's own ceiling, set generously so no realistic
/// workflow meets it, so a caller looping on create cannot grow this process's
/// memory without bound.
pub const MAX_BROWSERS: usize = 1_000;

/// The member path `CreateBrowser` refuses a VPC-mode network configuration for
/// when it carries no VPC configuration, and why.
///
/// The model states the rule in prose on the member itself — "This configuration
/// is required when the network mode is set to `VPC`" (`smithy_query
/// shape=BrowserNetworkConfiguration`) — so the framework's constraint walk
/// cannot refuse it.
pub const BROWSER_VPC_CONFIG_MEMBER: &str = "networkConfiguration.vpcConfig";

/// `BrowserNetworkMode` for a browser that connects to a caller-supplied VPC.
/// The enum's full value list is `PUBLIC | VPC`
/// (`shared_types::BROWSER_NETWORK_MODE_VALUES`, from `smithy_query
/// shape=BrowserNetworkMode`).
pub const BROWSER_NETWORK_MODE_VPC: &str = "VPC";

/// One custom browser: everything `CreateBrowser` persisted for it, and the
/// statuses the clock owes it.
///
/// Every read of the browser reports this record, so a create and a later read
/// cannot disagree about its configuration or status.
#[derive(Debug, Clone)]
pub struct BrowserRecord {
    pub browser_id: String,
    pub browser_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub execution_role_arn: Option<String>,
    pub network_configuration: BrowserNetworkConfiguration,
    pub recording: Option<RecordingConfig>,
    /// The signing configuration as a READ reports it: the request carries
    /// `BrowserSigningConfigInput` and every response the `…Output` shape, and
    /// the two declare the same one member, so the stored form is the reported
    /// one.
    pub browser_signing: Option<BrowserSigningConfigOutput>,
    pub enterprise_policies: Option<Vec<BrowserEnterprisePolicy>>,
    pub certificates: Option<Vec<Certificate>>,
    pub filesystem_configurations: Option<Vec<ToolsFileSystemConfiguration>>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status the browser reports until [`Self::settles_at`].
    pub transient_status: String,
    /// The status it reports from then on.
    pub settled_status: String,
    /// When the clock owes the move from one to the other. Stored, never slept
    /// on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    /// The instant an accepted deletion removes this record, once
    /// `DeleteBrowser` has accepted one; `None` for a browser nobody has
    /// deleted. Stored, never slept on, like
    /// [`GatewayRecord::removal_due_at`](GatewayRecord#structfield.removal_due_at).
    pub removal_due_at: Option<SystemTime>,
}

impl BrowserRecord {
    /// The status at `now`: the transient one until the clock owes the
    /// transition, the settled one from then on. No read performs it — it is due
    /// at that instant whether or not anybody looks.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// Whether a deletion has been accepted for this browser: it reports
    /// `DELETING` and the clock owes its removal.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// What the create of this browser reports: the four members
    /// `CreateBrowserResponse` declares, every one of them off the record just
    /// written.
    pub fn create_output(&self, now: SystemTime) -> CreateBrowserOutput {
        CreateBrowserOutput {
            browser_id: self.browser_id.clone(),
            browser_arn: self.browser_arn.clone(),
            created_at: timestamp(self.created_at),
            status: self.status_at(now).to_string(),
        }
    }

    /// What a read of this browser reports.
    pub fn get_output(&self, now: SystemTime) -> GetBrowserOutput {
        GetBrowserOutput {
            browser_id: self.browser_id.clone(),
            browser_arn: self.browser_arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            network_configuration: self.network_configuration.clone(),
            recording: self.recording.clone(),
            browser_signing: self.browser_signing.clone(),
            enterprise_policies: self.enterprise_policies.clone(),
            certificates: self.certificates.clone(),
            filesystem_configurations: self.filesystem_configurations.clone(),
            status: self.status_at(now).to_string(),
            // Reported only for a status that HAS a reason; a browser this
            // emulator drives reaches READY, which has none.
            failure_reason: None,
            created_at: timestamp(self.created_at),
            last_updated_at: timestamp(self.updated_at),
        }
    }

    /// What the deletion of this browser reports.
    pub fn delete_output(&self, now: SystemTime) -> DeleteBrowserOutput {
        DeleteBrowserOutput {
            browser_id: self.browser_id.clone(),
            status: self.status_at(now).to_string(),
            last_updated_at: timestamp(self.updated_at),
        }
    }
}

/// The modelled not-found error for a browser profile this account does not
/// hold.
///
/// This control plane words a missing resource in terms of the id the request
/// named — the sentence around the identifier is fixed, and the identifier
/// itself is the caller's own value — exactly as [`browser_not_found`] words the
/// sibling resource. Shared by the handlers rather than written per operation,
/// so one profile cannot be reported missing in two different sentences by two
/// operations.
pub fn browser_profile_not_found(profile_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Browser profile with ID {profile_id} does not exist"
        )),
    }
}

/// The ARN resource type a browser profile carries. `BrowserProfileArn` is
/// `…:browser-profile/{profileId}` (`smithy_query shape=BrowserProfileArn`),
/// whose `arnReference` names `AWS::BedrockAgentCore::BrowserProfile`.
pub const BROWSER_PROFILE_ARN_RESOURCE: &str = "browser-profile";

/// The `BrowserProfileStatus` a profile holds from its creation on. The enum's
/// full value list is `READY | SAVING | DELETING | DELETED`
/// (`shared_types::BROWSER_PROFILE_STATUS_VALUES`, from `smithy_query
/// shape=BrowserProfileStatus`): it models no creating status, so a created
/// profile is one that is ready to be used by a browser session, and the other
/// three are driven by a session saving into it and by its deletion.
pub const BROWSER_PROFILE_STATUS_READY: &str = "READY";

/// The `BrowserProfileStatus` a profile reports once a deletion has been
/// accepted for it, until the clock removes the record.
pub const BROWSER_PROFILE_STATUS_DELETING: &str = "DELETING";

/// How long an accepted browser-profile deletion leaves the profile `DELETING`
/// before the record is gone.
///
/// AWS documents no duration for the transition, so the span is chosen: long
/// enough that a caller acting straight after the delete still observes the
/// transient status, short enough to keep a client script quick — the same span
/// the sibling browser's transitions take ([`BROWSER_SETTLE_DELAY`]).
pub const BROWSER_PROFILE_REMOVAL_DELAY: Duration = BROWSER_SETTLE_DELAY;

/// How many browser profiles this emulator will hold.
///
/// `CreateBrowserProfile` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding the real service's quota; this number is not a fidelity claim about
/// that quota but the emulator's own ceiling, set generously so no realistic
/// workflow meets it, so a caller looping on create cannot grow this process's
/// memory without bound.
pub const MAX_BROWSER_PROFILES: usize = 1_000;

/// One browser profile: everything `CreateBrowserProfile` persisted for it, and
/// what a browser session last saved into it.
///
/// Every read of the profile reports this record, so a create and a later read
/// cannot disagree about its name, description or status.
#[derive(Debug, Clone)]
pub struct BrowserProfileRecord {
    pub profile_id: String,
    pub profile_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status the profile reports. A created profile is `READY` and stays
    /// there: `BrowserProfileStatus` models no creating status, so the clock
    /// owes a fresh profile no transition.
    pub status: String,
    /// When browser session data was last saved into this profile, and the
    /// session and browser it came from — `None` for a profile nothing has
    /// saved into yet. The data plane's session save is what sets them; no
    /// operation of this control plane does.
    pub last_saved_at: Option<SystemTime>,
    pub last_saved_browser_session_id: Option<String>,
    pub last_saved_browser_id: Option<String>,
    /// The instant an accepted deletion removes this record, once
    /// `DeleteBrowserProfile` has accepted one; `None` for a profile nobody has
    /// deleted. Stored, never slept on, like
    /// [`BrowserRecord::removal_due_at`](BrowserRecord#structfield.removal_due_at).
    pub removal_due_at: Option<SystemTime>,
}

impl BrowserProfileRecord {
    /// Whether a deletion has been accepted for this profile: it reports
    /// `DELETING` and the clock owes its removal.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// What the create of this profile reports: the four members
    /// `CreateBrowserProfileResponse` declares, every one of them off the record
    /// just written.
    pub fn create_output(&self) -> CreateBrowserProfileOutput {
        CreateBrowserProfileOutput {
            profile_id: self.profile_id.clone(),
            profile_arn: self.profile_arn.clone(),
            created_at: timestamp(self.created_at),
            status: self.status.clone(),
        }
    }

    /// What a read of this profile reports.
    pub fn get_output(&self) -> GetBrowserProfileOutput {
        GetBrowserProfileOutput {
            profile_id: self.profile_id.clone(),
            profile_arn: self.profile_arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            status: self.status.clone(),
            created_at: timestamp(self.created_at),
            last_updated_at: timestamp(self.updated_at),
            last_saved_at: self.last_saved_at.map(timestamp),
            last_saved_browser_session_id: self.last_saved_browser_session_id.clone(),
            last_saved_browser_id: self.last_saved_browser_id.clone(),
        }
    }

    /// What the deletion of this profile reports: the identifiers it addressed,
    /// the status it now holds, when it was last updated — the deletion itself
    /// is the last update — and, for a profile a browser session had saved
    /// into, the instant of that save.
    pub fn delete_output(&self) -> DeleteBrowserProfileOutput {
        DeleteBrowserProfileOutput {
            profile_id: self.profile_id.clone(),
            profile_arn: self.profile_arn.clone(),
            status: self.status.clone(),
            last_updated_at: timestamp(self.updated_at),
            last_saved_at: self.last_saved_at.map(timestamp),
        }
    }
}

/// The modelled not-found error for a code interpreter this account does not
/// hold.
///
/// This control plane words a missing code interpreter in terms of the id the
/// request named: "Code interpreter with ID <codeInterpreterId> does not exist"
/// — the sentence around the identifier is fixed, and the identifier itself is
/// the caller's own value, exactly as [`browser_not_found`] words the sibling
/// resource. Shared by the handlers rather than written per operation, so one
/// code interpreter cannot be reported missing in two different sentences by two
/// operations.
pub fn code_interpreter_not_found(code_interpreter_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Code interpreter with ID {code_interpreter_id} does not exist"
        )),
    }
}

/// The ARN resource type a code interpreter THIS SERVICE CREATED carries.
/// `CodeInterpreterArn` is
/// `…:code-interpreter(-custom)?/(aws\.codeinterpreter\.v1|…)` (`smithy_query
/// shape=CodeInterpreterArn`), whose `arnReference` names
/// `AWS::BedrockAgentCore::CodeInterpreterCustom`: the `-custom` spelling is the
/// customer-created interpreter, and the bare one the service-provided tool
/// (`aws.codeinterpreter.v1`), which no operation mints.
pub const CODE_INTERPRETER_ARN_RESOURCE: &str = "code-interpreter-custom";

/// `CodeInterpreterStatus` members this service drives. The enum's full value
/// list is `CREATING | CREATE_FAILED | READY | DELETING | DELETE_FAILED |
/// DELETED` (`smithy_query shape=CodeInterpreterStatus`).
pub const CODE_INTERPRETER_STATUS_CREATING: &str = "CREATING";
pub const CODE_INTERPRETER_STATUS_READY: &str = "READY";
pub const CODE_INTERPRETER_STATUS_DELETING: &str = "DELETING";

/// How long a freshly created code interpreter stays `CREATING` before the clock
/// owes it `READY`, and how long an accepted deletion leaves it `DELETING`
/// before the record is gone.
///
/// AWS documents no duration for either transition, so the span is chosen: long
/// enough that a caller acting straight after the call still observes the
/// transient status, short enough to keep a client script quick.
pub const CODE_INTERPRETER_SETTLE_DELAY: Duration = Duration::from_secs(3);

/// How many code interpreters this emulator will hold.
///
/// `CreateCodeInterpreter` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding the real service's quota; this number is not a fidelity claim about
/// that quota but the emulator's own ceiling, set generously so no realistic
/// workflow meets it, so a caller looping on create cannot grow this process's
/// memory without bound.
pub const MAX_CODE_INTERPRETERS: usize = 1_000;

/// The member path `CreateCodeInterpreter` refuses a VPC-mode network
/// configuration for when it carries no VPC configuration, and why.
///
/// The model states the rule in prose on the member itself — "This configuration
/// is required when the network mode is set to `VPC`" (`smithy_query
/// shape=CodeInterpreterNetworkConfiguration`) — so the framework's constraint
/// walk cannot refuse it.
pub const CODE_INTERPRETER_VPC_CONFIG_MEMBER: &str = "networkConfiguration.vpcConfig";

/// `CodeInterpreterNetworkMode` for an interpreter that connects to a
/// caller-supplied VPC. The enum's full value list is `PUBLIC | SANDBOX | VPC`
/// (`smithy_query shape=CodeInterpreterNetworkMode`).
pub const CODE_INTERPRETER_NETWORK_MODE_VPC: &str = "VPC";

/// One custom code interpreter: everything `CreateCodeInterpreter` persisted for
/// it, and the statuses the clock owes it.
///
/// Every read of the interpreter reports this record, so a create and a later
/// read cannot disagree about its configuration or status.
#[derive(Debug, Clone)]
pub struct CodeInterpreterRecord {
    pub code_interpreter_id: String,
    pub code_interpreter_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub execution_role_arn: Option<String>,
    pub network_configuration: CodeInterpreterNetworkConfiguration,
    pub certificates: Option<Vec<Certificate>>,
    pub filesystem_configurations: Option<Vec<ToolsFileSystemConfiguration>>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status the interpreter reports until [`Self::settles_at`].
    pub transient_status: String,
    /// The status it reports from then on.
    pub settled_status: String,
    /// When the clock owes the move from one to the other. Stored, never slept
    /// on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    /// The instant an accepted deletion removes this record, once
    /// `DeleteCodeInterpreter` has accepted one; `None` for an interpreter
    /// nobody has deleted. Stored, never slept on, like
    /// [`BrowserRecord::removal_due_at`](BrowserRecord#structfield.removal_due_at).
    pub removal_due_at: Option<SystemTime>,
}

impl CodeInterpreterRecord {
    /// The status at `now`: the transient one until the clock owes the
    /// transition, the settled one from then on. No read performs it — it is due
    /// at that instant whether or not anybody looks.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// Whether a deletion has been accepted for this code interpreter: it
    /// reports `DELETING` and the clock owes its removal.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// What the create of this code interpreter reports: the four members
    /// `CreateCodeInterpreterResponse` declares, every one of them off the
    /// record just written.
    pub fn create_output(&self, now: SystemTime) -> CreateCodeInterpreterOutput {
        CreateCodeInterpreterOutput {
            code_interpreter_id: self.code_interpreter_id.clone(),
            code_interpreter_arn: self.code_interpreter_arn.clone(),
            created_at: timestamp(self.created_at),
            status: self.status_at(now).to_string(),
        }
    }

    /// What a read of this code interpreter reports.
    pub fn get_output(&self, now: SystemTime) -> GetCodeInterpreterOutput {
        GetCodeInterpreterOutput {
            code_interpreter_id: self.code_interpreter_id.clone(),
            code_interpreter_arn: self.code_interpreter_arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            network_configuration: self.network_configuration.clone(),
            status: self.status_at(now).to_string(),
            certificates: self.certificates.clone(),
            filesystem_configurations: self.filesystem_configurations.clone(),
            // Reported only for a status that HAS a reason; a code interpreter
            // this emulator drives reaches READY, which has none.
            failure_reason: None,
            created_at: timestamp(self.created_at),
            last_updated_at: timestamp(self.updated_at),
        }
    }

    /// What the deletion of this code interpreter reports.
    pub fn delete_output(&self, now: SystemTime) -> DeleteCodeInterpreterOutput {
        DeleteCodeInterpreterOutput {
            code_interpreter_id: self.code_interpreter_id.clone(),
            status: self.status_at(now).to_string(),
            last_updated_at: timestamp(self.updated_at),
        }
    }
}

/// The resource noun [`resource_not_found`] names for a configuration bundle.
pub const RESOURCE_CONFIGURATION_BUNDLE: &str = "configuration bundle";

/// The resource noun for ONE version of a configuration bundle — a bundle that
/// holds no version on the branch a read names is not a missing bundle.
pub const RESOURCE_CONFIGURATION_BUNDLE_VERSION: &str = "configuration bundle version";

/// The resource type a `ConfigurationBundleArn` carries
/// (`…:configuration-bundle/{bundleId}`).
const ARN_TYPE_CONFIGURATION_BUNDLE: &str = "configuration-bundle";

/// The branch a configuration bundle version belongs to when the request names
/// none: "Defaults to mainline if not specified"
/// (`smithy_query(query="doc",shape="CreateConfigurationBundleRequest",member="branchName")`),
/// and "If not specified, returns the latest version on the mainline branch" for
/// a read.
pub const DEFAULT_BRANCH_NAME: &str = "mainline";

/// How many configuration bundles this emulator will hold.
///
/// `CreateConfigurationBundle` models `ServiceQuotaExceededException` (HTTP 402)
/// for exceeding the real service's quota; this number is not a fidelity claim
/// about that quota but the emulator's own ceiling, set generously so no
/// realistic workflow meets it, so a caller looping on create cannot grow this
/// process's memory without bound.
pub const MAX_CONFIGURATION_BUNDLES: usize = 1_000;

/// How many versions ONE configuration bundle holds. Each commit stores the
/// components, description and commit message the caller supplied, so the
/// version history is caller-driven growth and is bounded for the same reason
/// [`MAX_CONFIGURATION_BUNDLES`] is: not a fidelity claim about the real
/// service's quota, but the emulator's own ceiling, set generously so no
/// realistic workflow meets it.
pub const MAX_VERSIONS_PER_CONFIGURATION_BUNDLE: usize = 1_000;

/// One version (commit) of a configuration bundle: the component
/// configurations, the description and the encryption key this version holds,
/// and the lineage it was committed with.
///
/// A bundle's content is versioned, so the description, the components and the
/// KMS key live here rather than on the bundle: a read of one version reports
/// exactly what was committed in it.
#[derive(Debug, Clone)]
pub struct ConfigurationBundleVersionRecord {
    pub version_id: String,
    /// The components the commit carries, as the ordered map the store keeps so
    /// a read-back is deterministic.
    pub components: BTreeMap<String, ComponentConfiguration>,
    pub description: Option<String>,
    pub kms_key_arn: Option<String>,
    /// The versions this one was committed on top of: empty for the version a
    /// create mints, which has nothing before it ("Regular commits have 0-1
    /// parents. Merge commits have 2 parents").
    pub parent_version_ids: Vec<String>,
    /// The EFFECTIVE branch: the caller's, or [`DEFAULT_BRANCH_NAME`].
    pub branch_name: String,
    pub created_by: Option<VersionCreatedBySource>,
    pub commit_message: Option<String>,
    pub created_at: SystemTime,
}

impl ConfigurationBundleVersionRecord {
    /// The lineage this version was committed with, as every read of a version
    /// reports it.
    pub fn lineage_metadata(&self) -> VersionLineageMetadata {
        VersionLineageMetadata {
            parent_version_ids: Some(self.parent_version_ids.clone()),
            branch_name: Some(self.branch_name.clone()),
            created_by: self.created_by.clone(),
            commit_message: self.commit_message.clone(),
        }
    }

    /// The components this version holds, as the wire map.
    pub fn component_map(&self) -> std::collections::HashMap<String, ComponentConfiguration> {
        self.components
            .iter()
            .map(|(identifier, configuration)| (identifier.clone(), configuration.clone()))
            .collect()
    }
}

/// One configuration bundle: the resource `CreateConfigurationBundle` minted and
/// every version ever committed to it, so a create, a read of the latest version
/// on a branch and a read of one historical version all answer from one record.
#[derive(Debug, Clone)]
pub struct ConfigurationBundleRecord {
    pub bundle_id: String,
    pub bundle_arn: String,
    /// The name the bundle is addressed by in this account, which an update can
    /// change — so it lives on the bundle rather than on a version.
    pub bundle_name: String,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    /// When the bundle last gained a version.
    pub updated_at: SystemTime,
    /// Every version, in the order they were committed, so "the latest version"
    /// is the last entry rather than whatever a version id happens to sort as.
    /// Grown only through [`ConfigurationBundleRecord::commit`], which caps it
    /// at [`MAX_VERSIONS_PER_CONFIGURATION_BUNDLE`].
    versions: Vec<ConfigurationBundleVersionRecord>,
}

impl ConfigurationBundleRecord {
    /// A bundle with no commit yet: what a create builds before committing the
    /// initial version through [`Self::commit`].
    pub fn new(
        bundle_id: String,
        bundle_arn: String,
        bundle_name: String,
        tags: BTreeMap<String, String>,
        at: SystemTime,
    ) -> Self {
        Self {
            bundle_id,
            bundle_arn,
            bundle_name,
            tags,
            created_at: at,
            updated_at: at,
            versions: Vec::new(),
        }
    }

    /// Append `version` as this bundle's next commit and date the bundle at it,
    /// or answer `false` when the bundle already holds
    /// [`MAX_VERSIONS_PER_CONFIGURATION_BUNDLE`] versions — whereupon the caller
    /// answers the operation's modelled quota error rather than growing this
    /// process's memory. The ceiling lives here, at the ONE place a version is
    /// appended (`versions` is private for exactly that reason), so no commit
    /// path can skip it.
    #[must_use]
    pub fn commit(&mut self, version: ConfigurationBundleVersionRecord) -> bool {
        if self.versions.len() >= MAX_VERSIONS_PER_CONFIGURATION_BUNDLE {
            return false;
        }
        self.updated_at = version.created_at;
        self.versions.push(version);
        true
    }

    /// Every version of this bundle, in the order they were committed.
    pub fn versions(&self) -> &[ConfigurationBundleVersionRecord] {
        &self.versions
    }

    /// The version the create minted, which is the first commit of the bundle.
    pub fn initial_version(&self) -> Option<&ConfigurationBundleVersionRecord> {
        self.versions.first()
    }

    /// The version a `versionId` names, or `None` when this bundle holds no such
    /// version.
    pub fn version(&self, version_id: &str) -> Option<&ConfigurationBundleVersionRecord> {
        self.versions
            .iter()
            .find(|version| version.version_id == version_id)
    }

    /// The latest version committed on `branch_name` — the caller's branch, or
    /// [`DEFAULT_BRANCH_NAME`] when the request names none.
    pub fn latest_on_branch(
        &self,
        branch_name: Option<&str>,
    ) -> Option<&ConfigurationBundleVersionRecord> {
        let branch = branch_name.unwrap_or(DEFAULT_BRANCH_NAME);
        self.versions
            .iter()
            .rev()
            .find(|version| version.branch_name == branch)
    }

    /// What a read of one version of this bundle reports.
    pub fn get_output(
        &self,
        version: &ConfigurationBundleVersionRecord,
    ) -> GetConfigurationBundleOutput {
        GetConfigurationBundleOutput {
            bundle_arn: self.bundle_arn.clone(),
            bundle_id: self.bundle_id.clone(),
            bundle_name: self.bundle_name.clone(),
            description: version.description.clone(),
            version_id: version.version_id.clone(),
            components: version.component_map(),
            lineage_metadata: Some(version.lineage_metadata()),
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            kms_key_arn: version.kms_key_arn.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol. A handler reaches it through [`service_state`].
///
/// Every map here is caller-driven growth, so every one is bounded:
/// [`MAX_AGENT_RUNTIMES`] caps the runtimes (and with them the 1:1 name index),
/// [`MAX_VERSIONS_PER_RUNTIME`] caps the versions one runtime accumulates,
/// [`MAX_ENDPOINTS_PER_AGENT_RUNTIME`] the endpoints one runtime holds,
/// [`MAX_GATEWAYS`] caps the gateways (and with them their 1:1 name index),
/// [`MAX_TARGETS_PER_GATEWAY`] the targets one gateway holds, [`MAX_MEMORIES`]
/// the memories (and with them their 1:1 name index),
/// [`MAX_STRATEGIES_PER_MEMORY`] the strategies one memory holds,
/// [`MAX_HARNESSES`] the harnesses (and with them their 1:1 name index),
/// [`MAX_ENDPOINTS_PER_HARNESS`] the endpoints one harness holds,
/// [`MAX_REGISTRIES`] the registries (and with them their 1:1 name index),
/// [`MAX_BROWSERS`] the browsers (and with them their 1:1 name index),
/// [`MAX_BROWSER_PROFILES`] the browser profiles (and with them their 1:1 name
/// index),
/// [`MAX_CODE_INTERPRETERS`] the code interpreters (and with them their 1:1
/// name index),
/// [`MAX_CONFIGURATION_BUNDLES`] the configuration bundles (and with them their
/// 1:1 name index), [`MAX_VERSIONS_PER_CONFIGURATION_BUNDLE`] the versions one
/// bundle accumulates and
/// [`MAX_WORKLOAD_IDENTITIES`] the workload identities, and the
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
    /// Every memory, keyed by `memoryId` — the identifier every memory route
    /// addresses it by. The key order is the deterministic list order a memory
    /// listing pages over.
    pub memories: BTreeMap<String, MemoryRecord>,
    /// `name` -> `memoryId`, so a duplicate name at create time is a
    /// `ConflictException` ("The name must be unique within your account").
    /// One entry per stored memory.
    pub memory_names: BTreeMap<String, String>,
    /// Every harness, keyed by `harnessId` — the identifier every harness route
    /// addresses it by. The key order is the deterministic list order a harness
    /// listing pages over.
    pub harnesses: BTreeMap<String, HarnessRecord>,
    /// `harnessName` -> `harnessId`, so a duplicate name at create time is a
    /// `ConflictException` rather than a second harness later reads cannot tell
    /// apart. One entry per stored harness.
    pub harness_names: BTreeMap<String, String>,
    /// Every workload identity, keyed by `name` — the identifier every
    /// workload-identity route addresses it by, and what the identity's own ARN
    /// ends in. The key order is the deterministic list order
    /// `ListWorkloadIdentities` pages over.
    ///
    /// The identity a runtime or a gateway mints for itself is reported on that
    /// resource (`workloadIdentityDetails`) and is not an entry here: the
    /// operations that create one carry no identity name of their own, and
    /// nothing this service models reads an identity through a resource.
    pub workload_identities: BTreeMap<String, WorkloadIdentityRecord>,
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
    /// `<operation>/<clientToken>` -> the (runtime, endpoint name) that token
    /// first produced, so a replayed `CreateAgentRuntimeEndpoint` answers with
    /// its first result instead of adding a second endpoint. Separate from
    /// [`Self::idempotency`] because the record is validated against the
    /// endpoints of a runtime rather than against its versions; the key carries
    /// the operation for the same reason as there.
    runtime_endpoint_idempotency: BTreeMap<String, (String, String)>,
    /// The keys of [`Self::runtime_endpoint_idempotency`] in the order they were
    /// first recorded, so the oldest record is the one evicted at the ceiling.
    runtime_endpoint_idempotency_order: VecDeque<String>,
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
    /// `<operation>/<clientToken>` -> the memory that token first produced, so a
    /// replayed `CreateMemory` answers with its first result instead of minting
    /// a second memory. Separate from the maps above because the record is
    /// validated against a different store; the key carries the operation for
    /// the same reason as there.
    memory_idempotency: BTreeMap<String, String>,
    /// The keys of [`Self::memory_idempotency`] in the order they were first
    /// recorded, so the oldest record is the one evicted at the ceiling.
    memory_idempotency_order: VecDeque<String>,
    /// `<operation>/<clientToken>` -> the harness that token first produced, so a
    /// replayed `CreateHarness` answers with its first result instead of minting
    /// a second harness. Separate from the maps above because the record is
    /// validated against a different store; the key carries the operation for
    /// the same reason as there.
    harness_idempotency: BTreeMap<String, String>,
    /// The keys of [`Self::harness_idempotency`] in the order they were first
    /// recorded, so the oldest record is the one evicted at the ceiling.
    harness_idempotency_order: VecDeque<String>,
    /// `<operation>/<clientToken>` -> the (harness, endpointName) that token
    /// first produced, so a replayed `CreateHarnessEndpoint` answers with its
    /// first result instead of adding a second endpoint. Separate from
    /// [`Self::harness_idempotency`] because the record is validated against the
    /// endpoints of a harness rather than against the harness itself; the key
    /// carries the operation for the same reason as there.
    harness_endpoint_idempotency: BTreeMap<String, (String, String)>,
    /// The keys of [`Self::harness_endpoint_idempotency`] in the order they were
    /// first recorded, so the oldest record is the one evicted at the ceiling.
    harness_endpoint_idempotency_order: VecDeque<String>,
    /// Every registry, keyed by `registryId` — the identifier every registry
    /// route addresses it by. The key order is the deterministic list order a
    /// registry listing pages over.
    pub registries: BTreeMap<String, RegistryRecord>,
    /// `name` -> `registryId`, so a duplicate name at create time is a
    /// `ConflictException` ("The name must be unique within your account").
    /// One entry per stored registry.
    pub registry_names: BTreeMap<String, String>,
    /// `<operation>/<clientToken>` -> the registry that token first produced, so
    /// a replayed `CreateRegistry` answers with its first result instead of
    /// minting a second registry. Separate from the maps above because the record
    /// is validated against a different store; the key carries the operation for
    /// the same reason as there.
    registry_idempotency: BTreeMap<String, String>,
    /// The keys of [`Self::registry_idempotency`] in the order they were first
    /// recorded, so the oldest record is the one evicted at the ceiling.
    registry_idempotency_order: VecDeque<String>,
    /// Every custom browser, keyed by `browserId` — the identifier every browser
    /// route addresses it by. The key order is the deterministic list order a
    /// browser listing pages over.
    pub browsers: BTreeMap<String, BrowserRecord>,
    /// `name` -> `browserId`, so a duplicate name at create time is a
    /// `ConflictException` ("The name must be unique within your account").
    /// One entry per stored browser.
    pub browser_names: BTreeMap<String, String>,
    /// `<operation>/<clientToken>` -> the browser that token first produced, so
    /// a replayed `CreateBrowser` answers with its first result instead of
    /// minting a second browser. Separate from the maps above because the record
    /// is validated against a different store; the key carries the operation for
    /// the same reason as there.
    browser_idempotency: BTreeMap<String, String>,
    /// The keys of [`Self::browser_idempotency`] in the order they were first
    /// recorded, so the oldest record is the one evicted at the ceiling.
    browser_idempotency_order: VecDeque<String>,
    /// Every browser profile, keyed by `profileId` — the identifier every
    /// browser-profile route addresses it by. The key order is the
    /// deterministic list order a browser-profile listing pages over.
    pub browser_profiles: BTreeMap<String, BrowserProfileRecord>,
    /// `name` -> `profileId`, so a duplicate name at create time is a
    /// `ConflictException` ("The name must be unique within your account").
    /// One entry per stored profile.
    pub browser_profile_names: BTreeMap<String, String>,
    /// `<operation>/<clientToken>` -> the browser profile that token first
    /// produced, so a replayed `CreateBrowserProfile` answers with its first
    /// result instead of minting a second profile. Separate from the maps above
    /// because the record is validated against a different store; the key
    /// carries the operation for the same reason as there.
    browser_profile_idempotency: BTreeMap<String, String>,
    /// The keys of [`Self::browser_profile_idempotency`] in the order they were
    /// first recorded, so the oldest record is the one evicted at the ceiling.
    browser_profile_idempotency_order: VecDeque<String>,
    /// Every custom code interpreter, keyed by `codeInterpreterId` — the
    /// identifier every code-interpreter route addresses it by. The key order is
    /// the deterministic list order a code-interpreter listing pages over.
    pub code_interpreters: BTreeMap<String, CodeInterpreterRecord>,
    /// `name` -> `codeInterpreterId`, so a duplicate name at create time is a
    /// `ConflictException` ("The name must be unique within your account").
    /// One entry per stored code interpreter.
    pub code_interpreter_names: BTreeMap<String, String>,
    /// `<operation>/<clientToken>` -> the code interpreter that token first
    /// produced, so a replayed `CreateCodeInterpreter` answers with its first
    /// result instead of minting a second interpreter. Separate from the maps
    /// above because the record is validated against a different store; the key
    /// carries the operation for the same reason as there.
    code_interpreter_idempotency: BTreeMap<String, String>,
    /// The keys of [`Self::code_interpreter_idempotency`] in the order they were
    /// first recorded, so the oldest record is the one evicted at the ceiling.
    code_interpreter_idempotency_order: VecDeque<String>,
    /// Every configuration bundle, keyed by `bundleId` — the identifier every
    /// configuration-bundle route addresses it by. The key order is the
    /// deterministic list order a bundle listing pages over.
    pub configuration_bundles: BTreeMap<String, ConfigurationBundleRecord>,
    /// `bundleName` -> `bundleId`, so a duplicate name at create time is a
    /// `ConflictException` ("Names must be unique within your account"). One
    /// entry per stored bundle.
    pub configuration_bundle_names: BTreeMap<String, String>,
    /// `<operation>/<clientToken>` -> the configuration bundle that token first
    /// produced, so a replayed `CreateConfigurationBundle` answers with its
    /// first result instead of minting a second bundle. Separate from the maps
    /// above because the record is validated against a different store; the key
    /// carries the operation for the same reason as there.
    configuration_bundle_idempotency: BTreeMap<String, String>,
    /// The keys of [`Self::configuration_bundle_idempotency`] in the order they
    /// were first recorded, so the oldest record is the one evicted at the
    /// ceiling.
    configuration_bundle_idempotency_order: VecDeque<String>,
    /// The account's token vaults, keyed by `tokenVaultId`. Bounded by its own
    /// type: no operation of this service creates a vault, so the map holds the
    /// one default vault [`TokenVaults`] seeds and never grows.
    pub token_vaults: TokenVaults,
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
        remember_token(
            &mut self.idempotency,
            &mut self.idempotency_order,
            token_key(operation, &token),
            (agent_runtime_id.to_string(), version),
        );
    }

    /// Drop the idempotency records naming a runtime that no longer exists.
    ///
    /// Called from the delete path: such a record can never replay anything
    /// again, and nothing else would ever reclaim it. The endpoint records
    /// naming it go with them: the endpoints were held on the runtime, so they
    /// are gone too.
    pub fn forget_client_tokens_for(&mut self, agent_runtime_id: &str) {
        self.idempotency.retain(|_, (id, _)| id != agent_runtime_id);
        let live = &self.idempotency;
        self.idempotency_order.retain(|token| live.contains_key(token));
        self.runtime_endpoint_idempotency
            .retain(|_, (id, _)| id != agent_runtime_id);
        let live = &self.runtime_endpoint_idempotency;
        self.runtime_endpoint_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// Mint an `agentRuntimeEndpointId` for `endpoint_name` that no endpoint of
    /// `agent_runtime_id` holds: the endpoint's name + `-` + 10 alphanumerics,
    /// per `AgentRuntimeEndpointId`'s pattern
    /// `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$`.
    pub fn mint_runtime_endpoint_id(
        &mut self,
        agent_runtime_id: &str,
        endpoint_name: &str,
        at: SystemTime,
    ) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!(
                "{endpoint_name}-{}",
                suffix_for(
                    &format!("{agent_runtime_id}/{endpoint_name}"),
                    self.mint_counter,
                    at
                )
            );
            let taken = self.agent_runtimes.get(agent_runtime_id).is_some_and(|runtime| {
                runtime
                    .endpoints
                    .values()
                    .any(|endpoint| endpoint.endpoint_id == candidate)
            });
            if !taken {
                return candidate;
            }
        }
    }

    /// The (runtime, endpoint name) a previous request to `operation` carrying
    /// this `clientToken` produced, if the record is still held and the endpoint
    /// still exists.
    pub fn replayed_runtime_endpoint_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<(String, String)> {
        let (agent_runtime_id, endpoint_name) = self
            .runtime_endpoint_idempotency
            .get(&token_key(operation, client_token?))?;
        self.agent_runtimes
            .get(agent_runtime_id)?
            .endpoints
            .contains_key(endpoint_name)
            .then(|| (agent_runtime_id.clone(), endpoint_name.clone()))
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_runtime_endpoint_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        agent_runtime_id: &str,
        endpoint_name: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.runtime_endpoint_idempotency,
            &mut self.runtime_endpoint_idempotency_order,
            token_key(operation, &token),
            (agent_runtime_id.to_string(), endpoint_name.to_string()),
        );
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
        remember_token(
            &mut self.gateway_idempotency,
            &mut self.gateway_idempotency_order,
            token_key(operation, &token),
            (gateway_id.to_string(), target_id.map(str::to_string)),
        );
    }

    /// Drop the idempotency records naming a gateway that no longer exists, for
    /// the same reason [`Self::forget_client_tokens_for`] does.
    pub fn forget_gateway_client_tokens_for(&mut self, gateway_id: &str) {
        self.gateway_idempotency.retain(|_, (id, _)| id != gateway_id);
        let live = &self.gateway_idempotency;
        self.gateway_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// Every stored agent-runtime VERSION associated with the capacity provider
    /// `capacity_provider_id`, each with the opaque page cursor that identifies
    /// it, in one deterministic order.
    ///
    /// The association is the one this service persists: a version carries a
    /// `capacityProviderConfiguration` naming the provider's ARN
    /// ([`AgentRuntimeVersionRecord::capacity_provider_id`]), which is what makes
    /// that version one "you must disassociate before you can delete the
    /// capacity provider" (`smithy_query doc
    /// shape=ListAgentRuntimeVersionsByCapacityProvider`).
    ///
    /// The cursor is `<agentRuntimeId>/<version zero-padded to five digits>` —
    /// five because `AgentRuntimeVersion` is `^([1-9][0-9]{0,4})$` — so the
    /// cursors sort exactly as the rows do and a page can resume strictly after
    /// the one it ended on.
    pub fn runtime_versions_for_capacity_provider(
        &self,
        capacity_provider_id: &str,
        now: SystemTime,
    ) -> Vec<(String, AgentRuntimeVersionSummary)> {
        let mut rows = Vec::new();
        for record in self.agent_runtimes.values() {
            for version in record.versions.values() {
                if version.capacity_provider_id() != Some(capacity_provider_id) {
                    continue;
                }
                rows.push((
                    format!("{}/{:05}", record.agent_runtime_id, version.version),
                    version.summary(&record.agent_runtime_arn, now),
                ));
            }
        }
        // The maps are ordered, but a runtime id and a version number are two
        // keys: sorting on the composed cursor is what makes the one order the
        // pager walks and the cursors it hands out agree.
        rows.sort_by(|(left, _), (right, _)| left.cmp(right));
        rows
    }

    /// Whether this account holds a capacity provider that
    /// `capacity_provider_id` names.
    ///
    /// The only record of a capacity provider this service persists today is the
    /// association an agent-runtime version carries: `CreateCapacityProvider` is
    /// still scaffolded (see its own `src/ops/create_capacity_provider.rs`), so
    /// an id no stored version is configured with names nothing in this account.
    ///
    /// KNOWN DEVIATION, which that stub owns and this read resolves the day it
    /// lands: a capacity provider that exists with NO associated runtime version
    /// is answered `AccessDeniedException` (403) here, where the real service
    /// answers 200 with an empty `agentRuntimes` list — the listing is documented
    /// as naming "the runtimes you must disassociate before you can delete the
    /// capacity provider", so an empty page is a legitimate answer for a provider
    /// nothing uses. Nothing in this emulator can mint or hold such a provider
    /// yet, so there is no state from which to tell it apart from an id that
    /// names nothing; when `CreateCapacityProvider` is implemented, its store
    /// becomes the authority this reads and the empty-provider case answers 200.
    pub fn holds_capacity_provider(&self, capacity_provider_id: &str) -> bool {
        self.agent_runtimes.values().any(|record| {
            record
                .versions
                .values()
                .any(|version| version.capacity_provider_id() == Some(capacity_provider_id))
        })
    }

    /// The gateway whose PUBLISHED endpoint host is the one a data-plane request
    /// addressed: the gateway `gateway_id` names, and only while the URL it
    /// published is the one on `region`'s gateway host.
    ///
    /// The Region is part of the address, so a gateway created in one Region is
    /// not reachable through another Region's host — the gateway store is keyed
    /// by id alone, and the published URL is what ties a record to a host
    /// ([`gateway_url`]).
    pub fn gateway_on_endpoint_host(
        &self,
        region: &str,
        gateway_id: &str,
    ) -> Option<&GatewayRecord> {
        let record = self.gateways.get(gateway_id)?;
        let published = gateway_url(region, gateway_id, record.protocol_type.as_deref());
        (record.gateway_url == published).then_some(record)
    }

    /// Remove every gateway whose deletion the clock has already made due, with
    /// the name it reserved, the targets on it and the idempotency records naming
    /// it — and, on the gateways that remain, every TARGET whose own deletion the
    /// clock has made due.
    ///
    /// `DeleteGateway` and `DeleteGatewayTarget` ACCEPT the deletion (202) and
    /// report the resource `DELETING`; the record itself goes when the span it
    /// owes passes, so a caller polling it sees `DELETING` and then the modelled
    /// 404. The removal is owed by the CLOCK — it was due at this instant whether
    /// or not anybody called — so every operation that reads the gateway store
    /// materialises it first, and nothing ever sleeps for it.
    pub fn expire_deleted_gateways_and_targets(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .gateways
            .iter()
            .filter(|(_, record)| record.removal_due_at.is_some_and(|at| now >= at))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            if let Some(record) = self.gateways.remove(&id) {
                self.gateway_names.remove(&record.name);
                self.forget_gateway_client_tokens_for(&record.gateway_id);
            }
        }
        for gateway in self.gateways.values_mut() {
            gateway
                .targets
                .retain(|_, target| target.removal_due_at.is_none_or(|at| now < at));
        }
    }

    /// Mint a `harnessId` for `name` that no stored harness holds: the name + `-`
    /// + 10 alphanumerics, per `HarnessId`'s pattern
    /// `^[a-zA-Z][a-zA-Z0-9_]{0,39}-[a-zA-Z0-9]{10}$`.
    ///
    /// `HarnessName` is `^[a-zA-Z][a-zA-Z0-9_]{0,39}$`, which is exactly the
    /// stem that pattern admits, so the name needs no reshaping — unlike a
    /// gateway id, whose pattern is narrower than its name's.
    pub fn mint_harness_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.harnesses.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint a `memoryId` for `name` that no stored memory holds: `name` + `-` +
    /// 10 alphanumerics, per `MemoryId`'s pattern
    /// `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$` (min length 12, which
    /// the 11 characters the suffix contributes already satisfy for any `Name`).
    pub fn mint_memory_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.memories.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint a `strategyId` none of `minted` holds, for a strategy named
    /// `strategy_name`. `MemoryStrategyId` carries the same pattern as
    /// `MemoryId`, so the id has the same shape; `minted` is the ids already
    /// minted for the memory being created, which the store does not hold yet.
    pub fn mint_memory_strategy_id(
        &mut self,
        strategy_name: &str,
        at: SystemTime,
        minted: &BTreeSet<String>,
    ) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!(
                "{strategy_name}-{}",
                suffix_for(strategy_name, self.mint_counter, at)
            );
            if !minted.contains(&candidate) {
                return candidate;
            }
        }
    }

    /// The harness a previous request to `operation` carrying this `clientToken`
    /// produced, if the record is still held and that harness still exists.
    pub fn replayed_harness_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let harness_id = self
            .harness_idempotency
            .get(&token_key(operation, client_token?))?;
        self.harnesses
            .contains_key(harness_id)
            .then(|| harness_id.clone())
    }

    /// The memory a previous request to `operation` carrying this `clientToken`
    /// produced, if the record is still held and the memory still exists.
    pub fn replayed_memory_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let memory_id = self.memory_idempotency.get(&token_key(operation, client_token?))?;
        self.memories
            .contains_key(memory_id)
            .then(|| memory_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_harness_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        harness_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.harness_idempotency,
            &mut self.harness_idempotency_order,
            token_key(operation, &token),
            harness_id.to_string(),
        );
    }

    /// Drop the idempotency records naming a harness that no longer exists, for
    /// the same reason [`Self::forget_client_tokens_for`] does. The endpoint
    /// records naming it go with them: the endpoints were removed with the
    /// harness, so neither record can ever replay anything again.
    pub fn forget_harness_client_tokens_for(&mut self, harness_id: &str) {
        self.harness_idempotency.retain(|_, id| id != harness_id);
        let live = &self.harness_idempotency;
        self.harness_idempotency_order
            .retain(|token| live.contains_key(token));
        self.harness_endpoint_idempotency
            .retain(|_, (id, _)| id != harness_id);
        let live = &self.harness_endpoint_idempotency;
        self.harness_endpoint_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// The (harness, endpointName) a previous request to `operation` carrying
    /// this `clientToken` produced, if the record is still held and the endpoint
    /// it names still exists.
    pub fn replayed_harness_endpoint_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<(String, String)> {
        let (harness_id, endpoint_name) = self
            .harness_endpoint_idempotency
            .get(&token_key(operation, client_token?))?;
        self.harnesses
            .get(harness_id)?
            .endpoints
            .contains_key(endpoint_name)
            .then(|| (harness_id.clone(), endpoint_name.clone()))
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_harness_endpoint_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        harness_id: &str,
        endpoint_name: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.harness_endpoint_idempotency,
            &mut self.harness_endpoint_idempotency_order,
            token_key(operation, &token),
            (harness_id.to_string(), endpoint_name.to_string()),
        );
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_memory_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        memory_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.memory_idempotency,
            &mut self.memory_idempotency_order,
            token_key(operation, &token),
            memory_id.to_string(),
        );
    }

    /// Mint a `registryId` no stored registry holds: [`REGISTRY_ID_LEN`]
    /// alphanumerics, per `RegistryId`'s pattern `^[a-zA-Z0-9]{12,16}$`, which
    /// — unlike a runtime, gateway or memory id — carries no name. The name is
    /// still part of what the id is derived from, so two registries created in
    /// the same instant under different names cannot collide.
    pub fn mint_registry_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = suffix_of_length(name, self.mint_counter, at, REGISTRY_ID_LEN);
            if !self.registries.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The registry a previous request to `operation` carrying this `clientToken`
    /// produced, if the record is still held and the registry still exists.
    pub fn replayed_registry_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let registry_id = self
            .registry_idempotency
            .get(&token_key(operation, client_token?))?;
        self.registries
            .contains_key(registry_id)
            .then(|| registry_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_registry_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        registry_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.registry_idempotency,
            &mut self.registry_idempotency_order,
            token_key(operation, &token),
            registry_id.to_string(),
        );
    }

    /// Mint a `browserId` for `name` that no stored browser holds: the name +
    /// `-` + 10 alphanumerics, per `BrowserId`'s pattern
    /// `^(aws\.browser\.v1|[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10})$`.
    ///
    /// `SandboxName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`, which is exactly the
    /// stem that pattern admits, so the name needs no reshaping. The other
    /// alternative, `aws.browser.v1`, is the service-provided browser tool,
    /// which no create mints.
    pub fn mint_browser_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.browsers.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The browser a previous request to `operation` carrying this `clientToken`
    /// produced, if the record is still held and the browser still exists.
    pub fn replayed_browser_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let browser_id = self
            .browser_idempotency
            .get(&token_key(operation, client_token?))?;
        self.browsers
            .contains_key(browser_id)
            .then(|| browser_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_browser_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        browser_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.browser_idempotency,
            &mut self.browser_idempotency_order,
            token_key(operation, &token),
            browser_id.to_string(),
        );
    }

    /// Drop the idempotency records naming a browser that no longer exists, for
    /// the same reason [`Self::forget_client_tokens_for`] does.
    fn forget_browser_client_tokens_for(&mut self, browser_id: &str) {
        self.browser_idempotency.retain(|_, id| id != browser_id);
        let live = &self.browser_idempotency;
        self.browser_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// Mint a `profileId` for `name` that no stored browser profile holds: the
    /// name + `-` + 10 alphanumerics, per `BrowserProfileId`'s pattern
    /// `^[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}$`.
    ///
    /// `BrowserProfileName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`, which is exactly
    /// the stem that pattern admits, so the name needs no reshaping.
    pub fn mint_browser_profile_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.browser_profiles.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The browser profile a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and the profile still
    /// exists.
    pub fn replayed_browser_profile_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let profile_id = self
            .browser_profile_idempotency
            .get(&token_key(operation, client_token?))?;
        self.browser_profiles
            .contains_key(profile_id)
            .then(|| profile_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held, for the same
    /// reason [`Self::remember_browser_client_token`] does.
    pub fn remember_browser_profile_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        profile_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.browser_profile_idempotency,
            &mut self.browser_profile_idempotency_order,
            token_key(operation, &token),
            profile_id.to_string(),
        );
    }

    /// Drop the idempotency records naming a browser profile that no longer
    /// exists, for the same reason [`Self::forget_client_tokens_for`] does.
    fn forget_browser_profile_client_tokens_for(&mut self, profile_id: &str) {
        self.browser_profile_idempotency
            .retain(|_, id| id != profile_id);
        let live = &self.browser_profile_idempotency;
        self.browser_profile_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// Remove every browser profile whose accepted deletion the clock has made
    /// due, and the name index entry and idempotency records naming it, on the
    /// same clock-owed rule as [`Self::expire_deleted_browsers`]:
    /// `DeleteBrowserProfile` reports the profile `DELETING`, and the record
    /// itself goes when the span it owes passes, so a caller polling the profile
    /// sees `DELETING` and then the modelled 404.
    pub fn expire_deleted_browser_profiles(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .browser_profiles
            .iter()
            .filter(|(_, record)| record.removal_due_at.is_some_and(|at| now >= at))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            if let Some(record) = self.browser_profiles.remove(&id) {
                self.browser_profile_names.remove(&record.name);
                self.forget_browser_profile_client_tokens_for(&record.profile_id);
            }
        }
    }

    /// Mint a `codeInterpreterId` for `name` that no stored code interpreter
    /// holds: the name + `-` + 10 alphanumerics, per `CodeInterpreterId`'s
    /// pattern
    /// `^(aws\.codeinterpreter\.v1|[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10})$`.
    ///
    /// `SandboxName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`, which is exactly the
    /// stem that pattern admits, so the name needs no reshaping. The other
    /// alternative, `aws.codeinterpreter.v1`, is the service-provided code
    /// interpreter tool, which no create mints.
    pub fn mint_code_interpreter_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.code_interpreters.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The code interpreter a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and the interpreter
    /// still exists.
    pub fn replayed_code_interpreter_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let code_interpreter_id = self
            .code_interpreter_idempotency
            .get(&token_key(operation, client_token?))?;
        self.code_interpreters
            .contains_key(code_interpreter_id)
            .then(|| code_interpreter_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held, for the same
    /// reason [`Self::remember_browser_client_token`] does.
    pub fn remember_code_interpreter_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        code_interpreter_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.code_interpreter_idempotency,
            &mut self.code_interpreter_idempotency_order,
            token_key(operation, &token),
            code_interpreter_id.to_string(),
        );
    }

    /// Drop the idempotency records naming a code interpreter that no longer
    /// exists, for the same reason [`Self::forget_client_tokens_for`] does.
    fn forget_code_interpreter_client_tokens_for(&mut self, code_interpreter_id: &str) {
        self.code_interpreter_idempotency
            .retain(|_, id| id != code_interpreter_id);
        let live = &self.code_interpreter_idempotency;
        self.code_interpreter_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// Remove every code interpreter whose accepted deletion the clock has made
    /// due, and the name index entry and idempotency records naming it, on the
    /// same clock-owed rule as [`Self::expire_deleted_browsers`]:
    /// `DeleteCodeInterpreter` ACCEPTS the deletion (202) and reports the
    /// interpreter `DELETING`, and the record itself goes when the span it owes
    /// passes, so a caller polling it sees `DELETING` and then the modelled 404.
    pub fn expire_deleted_code_interpreters(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .code_interpreters
            .iter()
            .filter(|(_, record)| record.removal_due_at.is_some_and(|at| now >= at))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            if let Some(record) = self.code_interpreters.remove(&id) {
                self.code_interpreter_names.remove(&record.name);
                self.forget_code_interpreter_client_tokens_for(&record.code_interpreter_id);
            }
        }
    }

    /// Remove every browser whose accepted deletion the clock has made due, and
    /// the name index entry and idempotency records naming it.
    ///
    /// `DeleteBrowser` ACCEPTS the deletion (202) and reports the browser
    /// `DELETING`; the record itself goes when the span it owes passes, so a
    /// caller polling the browser sees `DELETING` and then the modelled 404. The
    /// removal is owed by the CLOCK — it was due at this instant whether or not
    /// anybody called — so every operation that reads the browser store
    /// materialises it first, and nothing ever sleeps for it.
    pub fn expire_deleted_browsers(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .browsers
            .iter()
            .filter(|(_, record)| record.removal_due_at.is_some_and(|at| now >= at))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            if let Some(record) = self.browsers.remove(&id) {
                self.browser_names.remove(&record.name);
                self.forget_browser_client_tokens_for(&record.browser_id);
            }
        }
    }

    /// Mint a `bundleId` for `bundle_name` that no stored bundle holds:
    /// `bundle_name` + `-` + 10 alphanumerics, per `ConfigurationBundleId`'s
    /// pattern `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`.
    pub fn mint_configuration_bundle_id(&mut self, bundle_name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{bundle_name}-{}", suffix_for(bundle_name, self.mint_counter, at));
            if !self.configuration_bundles.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint a `versionId` for a commit of `bundle_name`: the UUID form
    /// `ConfigurationBundleVersion` takes
    /// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
    ///
    /// The counter is bumped first, so two commits of one bundle in the same
    /// instant are digested from different inputs and cannot share an id.
    pub fn mint_configuration_bundle_version_id(
        &mut self,
        bundle_name: &str,
        at: SystemTime,
    ) -> String {
        self.mint_counter = self.mint_counter.wrapping_add(1);
        uuid_shaped(bundle_name, self.mint_counter, at)
    }

    /// The configuration bundle a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and the bundle still
    /// exists.
    pub fn replayed_configuration_bundle_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let bundle_id = self
            .configuration_bundle_idempotency
            .get(&token_key(operation, client_token?))?;
        self.configuration_bundles
            .contains_key(bundle_id)
            .then(|| bundle_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_configuration_bundle_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        bundle_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.configuration_bundle_idempotency,
            &mut self.configuration_bundle_idempotency_order,
            token_key(operation, &token),
            bundle_id.to_string(),
        );
    }
}

/// One idempotency key: the operation the token was presented to, then the
/// token itself, so the two operations' tokens cannot collide.
fn token_key(operation: &str, client_token: &str) -> String {
    format!("{operation}/{client_token}")
}

/// Record what one `clientToken` produced, in the map/order pair of whichever
/// store holds it, evicting the oldest record once
/// [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
/// indistinguishable from one whose idempotency window has passed: the next
/// request carrying it is served as a fresh one.
///
/// Shared by every `remember_*_client_token` above — the stores differ only in
/// what a token produced — so the ceiling and the eviction rule cannot drift
/// between them.
fn remember_token<V>(
    records: &mut BTreeMap<String, V>,
    order: &mut VecDeque<String>,
    key: String,
    produced: V,
) {
    if records.insert(key.clone(), produced).is_none() {
        order.push_back(key);
    }
    while order.len() > MAX_REMEMBERED_CLIENT_TOKENS {
        if let Some(oldest) = order.pop_front() {
            records.remove(&oldest);
        }
    }
}

/// How many characters the suffix of a NAME-carrying id contributes, in every
/// `*Id` pattern of this service that ends in one (`-[a-zA-Z0-9]{10}`).
const ID_SUFFIX_LEN: usize = 10;

/// The 10-character id suffix: SHA-256 over the name, the mint counter and the
/// creation instant, mapped into `[a-z0-9]` (a subset of the `[a-zA-Z0-9]{10}`
/// the model's pattern allows).
fn suffix_for(name: &str, counter: u64, at: SystemTime) -> String {
    suffix_of_length(name, counter, at, ID_SUFFIX_LEN)
}

/// `len` characters of the same derivation [`suffix_for`] uses, for an id whose
/// pattern asks for a different width — a registry id is the whole id rather
/// than a suffix on a name, so it is longer.
fn suffix_of_length(name: &str, counter: u64, at: SystemTime, len: usize) -> String {
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
        .take(len)
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect()
}

/// A UUID-shaped identifier for an id whose pattern is the hyphenated UUID form
/// rather than this service's `name-suffix` ids.
///
/// The 128 bits come from the same derivation [`suffix_of_length`] uses — a
/// SHA-256 (`sha2`) over the name, the mint counter and the instant, hex-encoded
/// (`hex`) — with the version nibble and the variant bits RFC 4122 §4.4 fixes
/// set, so the value is a well-formed version-4 UUID in lower case. It names a
/// resource version; a credential would come from `ctx.credential_store()`,
/// never here.
fn uuid_shaped(name: &str, counter: u64, at: SystemTime) -> String {
    use sha2::{Digest, Sha256};

    let nanos = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let rendered = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..]
    )
}

/// Encode the store key a page ended on as the opaque `nextToken` every
/// `@paginated` listing in this service hands back.
///
/// Hex keeps the token inside the modelled `^\S*$` pattern for every key the
/// store can hold, and the codec lives here rather than in one `ops/` file so
/// two listings cannot issue tokens the other cannot read.
pub fn encode_page_token(key: &str) -> String {
    ::hex::encode(key.as_bytes())
}

/// Recover the store key a `nextToken` was issued for, or `None` when the token
/// was not issued by [`encode_page_token`] — which the listing refuses with its
/// own modelled `ValidationException`.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// How many entries one page of a listing holds: the caller's `maxResults`, or
/// `modelled_max` (that member's own modelled ceiling) when the caller names
/// none, clamped into the modelled range.
///
/// The boundary walk already refuses a `maxResults` outside the modelled range
/// before a handler runs, so the clamp is defensive: it keeps every listing
/// correct on its own terms rather than on that walk, and a page size of zero
/// would be a silent dead end.
pub fn page_size(requested: Option<i32>, modelled_max: i32) -> usize {
    let ceiling = modelled_max.max(1);
    let size = requested.unwrap_or(ceiling).clamp(1, ceiling);
    usize::try_from(size).unwrap_or(1)
}

/// One page of a listing over a key-ordered store: the summaries of at most
/// `page_size` records that sort after `resume_after`, and the `nextToken` that
/// resumes where the page ended — `None` when nothing is left over.
///
/// The store is a [`BTreeMap`], so its iteration order IS ascending key order;
/// that is what makes "resume strictly after the key the last page ended on" a
/// sound cursor, and why the skip below compares keys rather than counting.
/// Shared by every `@paginated` listing of this service so two of them cannot
/// page by different rules, with the per-operation parts — which token it
/// refuses and with which modelled error, and which record it summarises — left
/// to the handler.
///
/// `summarise` is fallible: a record that has no summary to report (every
/// version of it deleted, say) is passed over without consuming a page slot.
pub fn page_over<R, T>(
    records: &BTreeMap<String, R>,
    resume_after: Option<&str>,
    page_size: usize,
    summarise: impl Fn(&R) -> Option<T>,
) -> (Vec<T>, Option<String>) {
    let mut items = Vec::new();
    let mut next_token = None;
    let mut last_included: Option<&str> = None;
    for (key, record) in records {
        if resume_after.is_some_and(|last| key.as_str() <= last) {
            continue;
        }
        let Some(summary) = summarise(record) else {
            continue;
        };
        if items.len() >= page_size {
            // At least one record is left over, so hand back a token that
            // resumes exactly where this page ended.
            next_token = last_included.map(encode_page_token);
            break;
        }
        items.push(summary);
        last_included = Some(key.as_str());
    }
    (items, next_token)
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

/// `arn:aws:bedrock-agentcore:{region}:{account}:runtime/{agentRuntimeId}/runtime-endpoint/{name}`
/// — `AgentRuntimeEndpointArn`'s pattern, which nests the endpoint under the
/// runtime that holds it and ends in the endpoint's name.
pub fn runtime_endpoint_arn(region: &str, agent_runtime_id: &str, endpoint_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("runtime/{agent_runtime_id}/runtime-endpoint/{endpoint_name}"),
    )
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

/// `arn:aws:bedrock-agentcore:{region}:{account}:memory/{memoryId}` —
/// `MemoryArn`'s pattern.
pub fn memory_arn(region: &str, memory_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("memory/{memory_id}"))
}

/// `arn:aws:bedrock-agentcore:{region}:{account}:registry/{registryId}` —
/// `RegistryArn`'s pattern.
pub fn registry_arn(region: &str, registry_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RESOURCE_REGISTRY}/{registry_id}"),
    )
}

/// `arn:aws:bedrock-agentcore:{region}:{account}:browser-custom/{browserId}` —
/// the customer-created alternative of `BrowserArn`'s pattern.
pub fn browser_arn(region: &str, browser_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{BROWSER_ARN_RESOURCE}/{browser_id}"),
    )
}

/// `arn:aws:bedrock-agentcore:{region}:{account}:browser-profile/{profileId}` —
/// `BrowserProfileArn`'s pattern.
pub fn browser_profile_arn(region: &str, profile_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{BROWSER_PROFILE_ARN_RESOURCE}/{profile_id}"),
    )
}

/// `arn:aws:bedrock-agentcore:{region}:{account}:code-interpreter-custom/{codeInterpreterId}`
/// — the customer-created alternative of `CodeInterpreterArn`'s pattern.
pub fn code_interpreter_arn(region: &str, code_interpreter_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CODE_INTERPRETER_ARN_RESOURCE}/{code_interpreter_id}"),
    )
}

/// `arn:aws:bedrock-agentcore:{region}:{account}:configuration-bundle/{bundleId}`
/// — `ConfigurationBundleArn`'s pattern.
pub fn configuration_bundle_arn(region: &str, bundle_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ARN_TYPE_CONFIGURATION_BUNDLE}/{bundle_id}"),
    )
}

/// The gateway's endpoint, in the form AWS returns for a Bedrock AgentCore
/// gateway: its host, plus the transport path the gateway's own protocol is
/// served on —
/// `https://{gatewayId}.gateway.bedrock-agentcore.{region}.amazonaws.com/mcp`
/// for an MCP gateway.
///
/// The PATH IS PART OF THE VALUE: what this publishes is the address a client
/// opens a session against, not an origin a suffix is still owed on, so the URL
/// a gateway publishes is exactly the URL this emulator serves that gateway's
/// protocol on ([`gateway_transport_path`]).
///
/// Derived from the gateway id, the request's Region and the gateway's
/// `protocolType`, so a caller that reads one gateway twice sees one URL.
/// `GatewayUrl` is only constrained in length (1..1024), which this form
/// satisfies for every id the pattern admits.
pub fn gateway_url(region: &str, gateway_id: &str, protocol_type: Option<&str>) -> String {
    format!(
        "https://{}{}",
        gateway_endpoint_host(region, gateway_id),
        gateway_transport_path(protocol_type).unwrap_or_default()
    )
}

/// The transport path a gateway speaking `protocol_type` serves that protocol
/// on, under the host [`gateway_endpoint_host`] publishes — or `None` for a
/// protocol this service serves no transport for.
///
/// `GatewayProtocolType` carries the single value `MCP`, which `protocolType`
/// also carries as its modelled default (`smithy_query shape=GatewayProtocolType`),
/// so a gateway that named no protocol is served the MCP transport path too.
pub fn gateway_transport_path(protocol_type: Option<&str>) -> Option<&'static str> {
    match protocol_type.unwrap_or(PROTOCOL_TYPE_MCP) {
        PROTOCOL_TYPE_MCP => Some(GATEWAY_MCP_PATH),
        _ => None,
    }
}

/// The label a gateway endpoint host carries between the gateway's own id and
/// this service's name, in the host [`gateway_url`] publishes.
const GATEWAY_HOST_LABEL: &str = "gateway";

/// The DNS suffix of the endpoint hosts this emulator publishes.
const ENDPOINT_DNS_SUFFIX: &str = "amazonaws.com";

/// The host half of [`gateway_url`]:
/// `{gatewayId}.gateway.bedrock-agentcore.{region}.amazonaws.com`.
pub fn gateway_endpoint_host(region: &str, gateway_id: &str) -> String {
    format!("{gateway_id}.{GATEWAY_HOST_LABEL}.{ARN_SERVICE}.{region}.{ENDPOINT_DNS_SUFFIX}")
}

/// The `(gatewayId, region)` a gateway endpoint host names, or `None` for any
/// other host.
///
/// The inverse of [`gateway_endpoint_host`], so the host this service PUBLISHES
/// is exactly the host its data plane answers on: a published URL resolves back
/// to the record behind it. Any port the client dialled is not part of the
/// address.
pub fn gateway_endpoint_identity(host: &str) -> Option<(&str, &str)> {
    let host = host.split(':').next().unwrap_or(host);
    let (gateway_id, rest) = host.split_once('.')?;
    let region = rest
        .strip_prefix(&format!("{GATEWAY_HOST_LABEL}.{ARN_SERVICE}."))?
        .strip_suffix(&format!(".{ENDPOINT_DNS_SUFFIX}"))?;
    // Both labels are minted by this service — a `GatewayId`
    // (`^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$`) and the Region of the request
    // that created the gateway — so a label outside that grammar addresses no
    // gateway of this emulator.
    let label = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    (label(gateway_id) && label(region)).then_some((gateway_id, region))
}

/// The fixed sub-path a gateway's MCP JSON-RPC surface is reached at, under the
/// endpoint host [`gateway_endpoint_host`] publishes — the transport path the
/// gateway's own [`gateway_url`] carries for an MCP gateway.
///
/// Recorded behaviour only: the MCP wire surface is not in this service's model,
/// and the blessed recording's `gatewayUrl` ends in `/mcp`.
pub const GATEWAY_MCP_PATH: &str = "/mcp";

/// The separator a gateway qualifies an aggregated tool name with:
/// `<targetName>___<toolName>`, as the recorded `tools/list` reports them.
///
/// A `TargetName` is `^([0-9a-zA-Z][-]?){1,100}$` (`smithy_query
/// shape=TargetName`), so it carries no underscore and the FIRST occurrence of
/// the separator is always the one between the two names.
pub const TOOL_NAME_SEPARATOR: &str = "___";

/// `<target_name>___<tool_name>`: the name one of `target_name`'s tools is
/// aggregated under on the gateway's MCP surface.
pub fn qualified_tool_name(target_name: &str, tool_name: &str) -> String {
    format!("{target_name}{TOOL_NAME_SEPARATOR}{tool_name}")
}

/// The `(targetName, toolName)` a qualified tool name carries, or `None` when it
/// is not qualified at all. The inverse of [`qualified_tool_name`].
pub fn split_qualified_tool_name(qualified: &str) -> Option<(&str, &str)> {
    let (target_name, tool_name) = qualified.split_once(TOOL_NAME_SEPARATOR)?;
    (!target_name.is_empty() && !tool_name.is_empty()).then_some((target_name, tool_name))
}

/// The tool definitions a target declares INLINE, for the one target shape that
/// declares any: an MCP target whose tool handler is a Lambda function carrying
/// `toolSchema.inlinePayload`.
///
/// `None` for a target whose schema lives in S3 and for every other target
/// shape: nothing is known about the tools behind it without reading that
/// schema, and a tool list invented for it would name tools no target declares.
pub fn inline_tool_definitions(configuration: &TargetConfiguration) -> Option<&[ToolDefinition]> {
    match configuration {
        TargetConfiguration::Mcp(McpTargetConfiguration::Lambda(lambda)) => {
            match &lambda.tool_schema {
                ToolSchema::inlinePayload(tools) => Some(tools),
                ToolSchema::S3(_) => None,
            }
        }
        _ => None,
    }
}

/// How long a freshly created gateway or gateway target stays in its transient
/// status (`CREATING`) before the clock owes it `READY`.
///
/// AWS documents no duration for either transition, so the span is chosen: short
/// enough to keep a client script quick, long enough that a caller acting
/// straight after the call still observes the transient status.
/// How long a freshly created or updated gateway stays in its transient status
/// (`CREATING` / `UPDATING`) before the clock owes it `READY`, and how long a
/// gateway whose deletion was accepted stays readable in `DELETING` before the
/// clock owes its removal.
///
/// AWS documents no duration for any of these transitions, so the span is chosen:
/// short enough to keep a client script quick, long enough that a caller acting
/// straight after the call still observes the transient status.
pub const GATEWAY_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// The ARN resource type of a capacity provider: `CapacityProviderArn` is
/// `arn:aws…:bedrock-agentcore:…:capacity-provider/{capacityProviderId}`
/// (`smithy_query shape=CapacityProviderConfiguration`).
pub const CAPACITY_PROVIDER_RESOURCE: &str = "capacity-provider";

/// The error SHAPE this service names when it denies a request on permissions:
/// `AccessDeniedException` (HTTP 403), which
/// `ListAgentRuntimeVersionsByCapacityProvider` models and which the service
/// declares as a common error
/// (`smithy_query query=service_info`, `common_errors`).
pub const ACCESS_DENIED_SHAPE: &str = "AccessDeniedException";

/// Why a request naming a capacity provider this account does not hold is
/// denied, in the words the recorded service uses for this operation's denial:
/// the member it was denied over, and the clause naming where the deny came
/// from, which the authorizer renders.
///
/// ONE sentence, and no echo of the caller's value: authorization here is
/// resolved against the resource the path label names, and the emulator can see
/// only that no capacity provider of this account answers to it — not which of
/// the real authorizer's reasons applied. The status and the error code are what
/// a client branches on; stating a second, unrecorded reason would be this
/// file's words rather than the service's.
pub const CAPACITY_PROVIDER_DENIAL: &str = "Request denied: capacityProviderId does not match \
     the required format with an explicit deny in a resource-based policy";

/// The `capacityProviderId` a `ListAgentRuntimeVersionsByCapacityProvider`
/// request names, read off the request line itself rather than off a
/// deserialized input.
///
/// `None` for every other request. The route is the modelled one —
/// `POST /capacity-providers/{capacityProviderId}/runtime-versions`
/// (`smithy_query operation=ListAgentRuntimeVersionsByCapacityProvider`) — and
/// the label is non-greedy, so the id is exactly one path segment, percent-decoded
/// as the codec would decode it. `uri` is the request line's, which may be
/// absolute and may carry a query string, so the PATH is taken out of it first.
///
/// This exists because that operation authorizes BEFORE it validates (see
/// `crate::service::EmulatedService::override_operation`), which is earlier than
/// any handler runs and so earlier than a deserialized input exists.
pub fn capacity_provider_runtime_versions_id(method: &str, uri: &str) -> Option<String> {
    if !method.eq_ignore_ascii_case("POST") {
        return None;
    }
    // The path is read through the ONE shared reader for a request-line URI, which the
    // routing seams see as a `&str` that may be absolute: `service_common::uri_path`.
    let (id, tail) = ::service_common::uri_path(uri)
        .strip_prefix("/capacity-providers/")?
        .split_once('/')?;
    // A trailing slash is trimmed so the seam fires for every spelling the router
    // claims: this service's other routes are written with one (`/runtimes/`,
    // `/gateways/{id}/targets/`), and a form the modelled route does not match
    // never reaches here at all, because the service does not claim it.
    //
    // The label is decoded with PATH semantics (`+` is a literal there, not a space)
    // through the shared reader that states that rule once for the whole tree,
    // `service_common::percent_decode_path_label`, so this seam and the deserializer
    // see the one id for every spelling of the label.
    (tail.trim_end_matches('/') == "runtime-versions" && !id.is_empty())
        .then(|| ::service_common::percent_decode_path_label(id))
}

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

/// `TargetSummary.resourcePriority` / `McpServerTargetConfiguration.resourcePriority`
/// when the configuration names none. The model documents the default: "Priority
/// for resolving MCP server targets with shared resource URIs. Lower values take
/// precedence. Defaults to 1000 when not set."
pub const DEFAULT_TARGET_RESOURCE_PRIORITY: i32 = 1_000;

/// The `TargetType` a target configuration IS: the union member the caller
/// configured, named in `TargetType`'s own vocabulary (`OPEN_API_SCHEMA |
/// SMITHY_MODEL | MCP_SERVER | LAMBDA | API_GATEWAY | CONNECTOR |
/// AGENTCORE_RUNTIME | PASSTHROUGH | PROVIDER | HTTP_CONNECTOR`, from
/// `smithy_query shape=TargetType`).
///
/// A target's type is not stored twice: it is derived from the configuration the
/// target was created with, so the two cannot disagree. Both connector-shaped
/// configurations that are not HTTP (an MCP connector and an inference
/// connector) report `CONNECTOR` — the only connector value the enum offers
/// besides the HTTP one.
pub fn target_type(configuration: &TargetConfiguration) -> &'static str {
    match configuration {
        TargetConfiguration::Mcp(McpTargetConfiguration::openApiSchema(_)) => "OPEN_API_SCHEMA",
        TargetConfiguration::Mcp(McpTargetConfiguration::smithyModel(_)) => "SMITHY_MODEL",
        TargetConfiguration::Mcp(McpTargetConfiguration::Lambda(_)) => "LAMBDA",
        TargetConfiguration::Mcp(McpTargetConfiguration::mcpServer(_)) => "MCP_SERVER",
        TargetConfiguration::Mcp(McpTargetConfiguration::apiGateway(_)) => "API_GATEWAY",
        TargetConfiguration::Mcp(McpTargetConfiguration::Connector(_)) => "CONNECTOR",
        TargetConfiguration::Http(HttpTargetConfiguration::agentcoreRuntime(_)) => {
            "AGENTCORE_RUNTIME"
        }
        TargetConfiguration::Http(HttpTargetConfiguration::Passthrough(_)) => "PASSTHROUGH",
        TargetConfiguration::Http(HttpTargetConfiguration::Connector(_)) => "HTTP_CONNECTOR",
        TargetConfiguration::Inference(InferenceTargetConfiguration::Connector(_)) => "CONNECTOR",
        TargetConfiguration::Inference(InferenceTargetConfiguration::Provider(_)) => "PROVIDER",
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
    // `FunctionName` takes the full ARN, which is what the target carries.
    let answer = lambda_client(ctx, lambda_arn)
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

/// The Lambda client this service reaches a function through: the in-process
/// callback into this same emulator, at the endpoint of the function's OWN
/// Region.
///
/// The Region comes off the ARN — not off the request — because that is the
/// endpoint the function lives behind, and it is read through `service_common`'s
/// one ARN parse, which reports it only for an ARN OF LAMBDA whose Region field
/// is a Region label: the value lands in the authority of the endpoint URL, so a
/// caller-supplied value outside that grammar must not reach it. Anything else
/// falls back to this emulator's default Region.
fn lambda_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    lambda_arn: &str,
) -> ::aws_sdk_lambda::Client {
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
    ::aws_sdk_lambda::Client::from_conf(config)
}

/// What the function behind a gateway tool answered.
#[derive(Debug)]
pub struct LambdaInvocation {
    /// The bytes the function returned, which the gateway hands back to the MCP
    /// caller as the tool's result.
    pub payload: Vec<u8>,
    /// `FunctionError` — present only when the function itself failed, which is
    /// what makes the tool call an error rather than a result.
    pub function_error: Option<String>,
}

/// Invoke the function behind a gateway tool and report what Lambda answered.
///
/// `Err` carries the code and rendering of Lambda's own refusal: the tool call
/// did not run, and what is reported to the MCP caller is that answer rather
/// than a result nothing produced.
pub async fn invoke_lambda_tool(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    lambda_arn: &str,
    payload: Vec<u8>,
    client_context: Option<String>,
) -> Result<LambdaInvocation, String> {
    let mut request = lambda_client(ctx, lambda_arn)
        .invoke()
        .function_name(lambda_arn)
        // The tool call is synchronous: its result is the function's response
        // payload, so the invocation has to be the one that returns it.
        .invocation_type(::aws_sdk_lambda::types::InvocationType::RequestResponse)
        .payload(::aws_sdk_lambda::primitives::Blob::new(payload));
    if let Some(context) = client_context {
        request = request.client_context(context);
    }
    match request.send().await {
        Ok(answer) => Ok(LambdaInvocation {
            payload: answer
                .payload
                .map(::aws_sdk_lambda::primitives::Blob::into_inner)
                .unwrap_or_default(),
            function_error: answer.function_error,
        }),
        Err(error) => {
            use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;
            let rendered = ::aws_smithy_types::error::display::DisplayErrorContext(&error);
            Err(match error.code() {
                Some(code) => format!("{code}: {rendered}"),
                // No code at all: the call did not reach Lambda, which is this
                // emulator failing to ask rather than the function failing.
                None => rendered.to_string(),
            })
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

/// The IAM service principal this service assumes a caller-supplied execution
/// role as.
///
/// Source: the model's own `awsService.cloudTrailEventSource`,
/// `bedrock-agentcore.amazonaws.com` (`smithy_query query=service_info`) — the
/// principal an AgentCore execution role's trust policy has to admit.
pub const SERVICE_PRINCIPAL: &str = "bedrock-agentcore.amazonaws.com";

/// The request member a caller names an execution role in, on every operation of
/// this service that takes one (`CreateAgentRuntime`, `UpdateAgentRuntime`,
/// `CreateGateway`).
pub const ROLE_ARN_MEMBER: &str = "roleArn";

/// Why a role this service could not assume is refused, in the `fieldList` entry
/// the refusal carries for `roleArn`.
pub const ROLE_UNASSUMABLE: &str =
    "names a role that does not exist in this account, or whose trust policy does not \
     allow this service to assume it";

/// The modelled validation error for an execution role this service could not
/// assume.
///
/// The sentence names the ARN that was refused and the two things to check, the
/// way the real service words it; the member that failed is reported in the
/// `fieldList` the shape declares for exactly that.
pub fn role_validation_failed(role_arn: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "Role validation failed for '{role_arn}'. Please verify that the role exists and \
             its trust policy allows assumption by this service"
        ),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: ROLE_ARN_MEMBER.to_string(),
            message: ROLE_UNASSUMABLE.to_string(),
        }]),
    }
}

/// Whether IAM reports that this service could not assume the execution role
/// `role_arn` — the role is not there, or its trust policy withholds the grant.
///
/// An agent runtime RUNS AS the role the caller names, so the role is resolved
/// when the runtime is WRITTEN, and a request naming one this service cannot
/// assume is refused with the operation's modelled validation error with nothing
/// persisted. IAM is reached the way any emulated service reaches another:
/// through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`]), so this crate names
/// no other service crate.
///
/// Not every `roleArn` of this service is gated on it: the recorded service
/// ACCEPTS a `CreateGateway` whose role cannot be assumed, so only the
/// operations that store an agent runtime's execution role
/// (`CreateAgentRuntime`, `UpdateAgentRuntime`) call this.
///
/// `false` means the request may be written. That includes every case where
/// nothing is KNOWN about the role — a value that names no IAM role of this
/// account (`service_common::iam_role::role_name`, the tree's one reading of a
/// caller-supplied role ARN), or an emulator with no IAM to ask (a router with
/// only this service mounted, say) — because a refusal has to be grounded in
/// what IAM answered, never assumed.
pub async fn execution_role_is_unassumable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    // The whole question — which role of this account the value names, what IAM
    // answers about it, and whether the trust policy it carries admits this
    // service's principal — is the ONE reading every service that resolves a
    // caller-supplied execution role shares
    // (`service_common::iam_role::role_is_unassumable`), so this crate states
    // only WHICH principal has to be admitted and WHICH operations ask.
    ::service_common::iam_role::role_is_unassumable(ctx, region, role_arn, SERVICE_PRINCIPAL).await
}

/// The `fieldList` member path a caller names an agent runtime's code object in:
/// the path through the `AgentRuntimeArtifact` union's `codeConfiguration`
/// variant down to the object-storage location, so a caller sees which member of
/// the nested union failed.
pub const CODE_S3_MEMBER: &str = "agentRuntimeArtifact.codeConfiguration.code.s3";

/// Why a code artifact whose object this account's object storage does not hold
/// is refused — the sentence the recorded refusal carries.
pub const CODE_OBJECT_ABSENT_MESSAGE: &str = "S3 object not found in bucket";

/// The same, in the `fieldList` entry the refusal carries for
/// [`CODE_S3_MEMBER`]: what is wrong with that member's value.
pub const CODE_OBJECT_ABSENT: &str =
    "names an Amazon S3 object that this account's object storage does not hold";

/// The modelled validation error for a code artifact whose object is not in
/// object storage.
///
/// The member that failed is reported in the `fieldList` the shape declares for
/// exactly that; `reason` says a field failed validation. The bucket and the key
/// are the caller's own values and the remedy does not depend on them, so the
/// sentence does not echo them.
pub fn code_object_not_found() -> ValidationException {
    ValidationException {
        message: CODE_OBJECT_ABSENT_MESSAGE.to_string(),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: CODE_S3_MEMBER.to_string(),
            message: CODE_OBJECT_ABSENT.to_string(),
        }]),
    }
}

/// The HTTP status object storage answers a read of an object that IS NOT THERE
/// with: an absent bucket, an absent key and an absent version are all a 404.
const OBJECT_ABSENT_STATUS: u16 = 404;

/// The object-storage error codes that say the object a read named is not there:
/// the bucket does not exist, the key does not, or the named version does not.
/// `NoSuchBucket`, `NoSuchKey` and `NoSuchVersion` are the codes object
/// storage's own model declares for a read, and `NotFound` is what a HEAD —
/// which carries no error document to name them in — reports instead.
const ABSENT_OBJECT_CODES: &[&str] = &["NoSuchBucket", "NoSuchKey", "NoSuchVersion", "NotFound"];

/// The character an object key's path segments are separated by, which is what
/// makes a prefix name a set of keys rather than one key.
const KEY_SEPARATOR: &str = "/";

/// Whether object storage reports that the agent runtime code `artifact` names
/// no object it holds.
///
/// An agent runtime whose artifact is a `codeConfiguration` RUNS the code at the
/// object-storage location that configuration names, so the location is resolved
/// when the runtime version is WRITTEN: a request naming an object that is not
/// there is refused with the operation's modelled validation error and nothing is
/// persisted. A `containerConfiguration` artifact names no object-storage
/// location at all, so there is nothing to resolve for it.
///
/// `false` means the request may be written. That includes every case where
/// nothing is KNOWN about the location — an emulator with no object storage to
/// ask (a router with only this service mounted, say), or any answer that is not
/// about the caller's object — because a refusal has to be grounded in what
/// object storage answered, never assumed.
pub async fn code_object_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    artifact: &AgentRuntimeArtifact,
) -> bool {
    match artifact {
        AgentRuntimeArtifact::codeConfiguration(configuration) => match &configuration.code {
            Code::S3(location) => object_storage_holds_nothing_at(ctx, region, location).await,
        },
        AgentRuntimeArtifact::containerConfiguration(_) => false,
    }
}

/// Whether object storage holds no object at `location`.
///
/// Two reads, both of them reads: they change nothing a later caller sees, in
/// this service's store or in object storage.
///
/// The first is a `HeadObject` of the key the location names — "a HEAD request
/// has the same options as a GET operation on an object", so it answers the same
/// existence verdict with NO body, and nothing here interprets the artifact's
/// contents. The second runs only when that key is not there AND the location
/// names no version, because the member is modelled as a PREFIX ("The prefix for
/// objects in the Amazon S3 bucket. This prefix is added to the object keys",
/// `smithy_query shape=S3Location`): an object stored UNDER the prefix is the
/// artifact too, and one key is enough to settle it. A location that names a
/// version names one stored object instead, so there the first read is the whole
/// answer.
async fn object_storage_holds_nothing_at(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &S3Location,
) -> bool {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let client = s3_client(ctx, region);
    let failure = match client
        .head_object()
        .bucket(&location.bucket)
        .key(&location.prefix)
        // "If not specified, the latest version of the object is used"
        // (`S3Location$versionId`), which is what a read naming no version is.
        .set_version_id(location.version_id.clone())
        .send()
        .await
    {
        // Object storage holds exactly this object, so the request goes ahead.
        Ok(_) => return false,
        Err(failure) => failure,
    };
    let status = failure
        .raw_response()
        .map(|response| response.status().as_u16());
    let code = failure.code().unwrap_or_default().to_string();
    if status != Some(OBJECT_ABSENT_STATUS) || !ABSENT_OBJECT_CODES.contains(&code.as_str()) {
        // Any other answer says nothing about the caller's object — an emulator
        // assembled without object storage answers the router's own
        // `UnknownService`, an authentication refusal is about the signer, and a
        // transport failure answers nothing at all — so the request is not
        // refused for it, and it is worth a warning: a deployment with no object
        // storage to ask is one where this resolution does not run.
        ::tracing::warn!(
            bucket = %location.bucket,
            prefix = %location.prefix,
            code = %code,
            status = ?status,
            detail = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
            "the read of a caller-named code object answered nothing about the object, so the \
             request was not refused for it",
        );
        return false;
    }
    // A location that names a VERSION names ONE stored object rather than a set:
    // the read above resolved exactly that version, so its answer is final and
    // no other key under the prefix stands in for it.
    if location.version_id.is_some() {
        return true;
    }
    // The key itself is not there, so what is left is the prefix standing for a
    // set of keys. It is listed at its own path separator — the prefix plus `/`
    // unless it already ends in one — so a key that merely shares the value as a
    // STRING (`<prefix>.bak`) is not taken for the artifact, while any key under
    // the prefix is. One key settles it, so the listing asks for one.
    let under_prefix = if location.prefix.ends_with(KEY_SEPARATOR) {
        location.prefix.clone()
    } else {
        format!("{}{KEY_SEPARATOR}", location.prefix)
    };
    match client
        .list_objects_v2()
        .bucket(&location.bucket)
        .prefix(&under_prefix)
        .max_keys(1)
        .send()
        .await
    {
        Ok(listing) => listing.contents().is_empty(),
        Err(failure) => {
            // The read above already answered, with a code about the object,
            // that the key itself is not there; the listing adds nothing to
            // that, so the answer stands — and what it could not add is worth
            // a warning.
            ::tracing::warn!(
                bucket = %location.bucket,
                prefix = %under_prefix,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                "the keys under a caller-named code prefix could not be listed, so the \
                 absent key stands as the answer",
            );
            true
        }
    }
}

/// The object-storage client this service resolves a caller-named code location
/// through: the in-process callback into this same emulator, at the endpoint of
/// the request's own Region.
///
/// The callback is the framework's documented seam for calling another emulated
/// service — it routes in process — so no network client is configured here and
/// this crate names no other service's crate.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a
        // virtual-host bucket name is not a host this emulator resolves, so the
        // request addresses its bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials are handed over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// The credential a gateway target reports when the CREATE configured none: the
/// gateway's own IAM role.
///
/// `credentialProviderConfigurations` is optional on `CreateGatewayTarget` and
/// `@required` with at least one entry on every response that reports a target,
/// so the stored target carries this one rather than an empty list the wire
/// cannot render. `UpdateGatewayTarget` has no such fallback: it replaces the
/// target's whole configuration, so a body that defines none is refused
/// ([`member_not_defined`]) instead of being given a default the caller did not
/// ask for.
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
