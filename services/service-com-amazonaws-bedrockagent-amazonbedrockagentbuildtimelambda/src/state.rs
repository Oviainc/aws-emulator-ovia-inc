// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda service.
//!
//! The agent lifecycle (`CreateAgent`, `GetAgent`, `ListAgents`, `PrepareAgent`,
//! `DeleteAgent`) is backed by the [`agents`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::agents)
//! map below: every one of those handlers computes its whole response from a
//! record stored here, and the record -> wire projections
//! ([`AgentRecord::to_agent`], [`AgentRecord::to_summary`]) live here so no two
//! handlers can project the same record differently.
//!
//! The flow lifecycle (`CreateFlow`, `GetFlow`) is backed by the
//! [`flows`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::flows)
//! map the same way, with [`FlowRecord::to_create_output`] /
//! [`FlowRecord::to_get_output`] as its single projections.
//!
//! The knowledge-base lifecycle (`CreateKnowledgeBase`, `GetKnowledgeBase`) is
//! backed by the
//! [`knowledge_bases`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::knowledge_bases)
//! map, with [`KnowledgeBaseRecord::to_knowledge_base`] as its single projection.
//!
//! A knowledge base's data sources (`CreateDataSource`, `GetDataSource`) are
//! backed by the
//! [`data_sources`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::data_sources)
//! map, keyed by `dataSourceId` and carrying the `knowledgeBaseId` it belongs
//! to, with [`DataSourceRecord::to_data_source`] as its single projection.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Agent, AgentSummary, CreateFlowOutput, CustomOrchestration, DataSource,
    DataSourceConfiguration, FlowDefinition, FlowValidation, GetFlowOutput,
    GuardrailConfiguration, KnowledgeBase, KnowledgeBaseConfiguration, MemoryConfiguration,
    PromptOverrideConfiguration, ServerSideEncryptionConfiguration, StorageConfiguration,
    VectorIngestionConfiguration,
};

/// The region an ARN carries when the request is unsigned, so it names no
/// credential-scope region. Matches the `agentArn` pattern's `[a-z0-9-]{1,20}`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The only agent version `CreateAgent` mints, and the only flow version
/// `CreateFlow` mints: both the `Agent$agentVersion` and the `DraftVersion`
/// shapes are modelled `pattern ^DRAFT$`, so a stored agent or flow has no other
/// version.
pub const DRAFT_VERSION: &str = "DRAFT";

/// `CreateAgent`'s default when the request omits `idleSessionTTLInSeconds`
/// (within the member's modelled `range 60..5400`).
///
/// CHOSEN, not documented: the AWS API reference publishes only the member's
/// valid range, so this is the emulator's own default session lifetime.
pub const DEFAULT_IDLE_SESSION_TTL_SECONDS: i32 = 3600;

/// How long after `PrepareAgent` the agent leaves `PREPARING` for `PREPARED`.
///
/// CHOSEN, not documented: AWS publishes no duration for the preparation of an
/// agent. It is deliberately non-zero so a caller that reads the agent back
/// immediately still observes the transient `PREPARING` status the model
/// declares, and short enough that a client script converges promptly.
pub const PREPARE_SPAN: Duration = Duration::from_millis(500);

/// The most agents one `ListAgents` page can carry: the response member
/// `agentSummaries` is modelled `length ..10`, so a page never exceeds it however
/// large `maxResults` is.
pub const MAX_AGENTS_PER_PAGE: usize = 10;

/// The most agents this store holds; past it `CreateAgent` answers the modelled
/// `ServiceQuotaExceededException` instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on agents per account per Region,
/// and this emulator models neither Service Quotas nor an account's adjustments —
/// so this is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It is set far above any workflow an emulator serves, and it is
/// what stops the state growing without end: every record it holds is released by
/// `DeleteAgent`, which also forgets that agent's name and client tokens, so
/// neither index can outgrow this bound either.
pub const AGENT_QUOTA: usize = 1_000;

/// How many ids [`ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::mint_agent_id`]
/// may draw before giving up, so its termination is enforced rather than argued.
/// With a 62^10 id space and a store bounded by [`AGENT_QUOTA`] a collision is
/// already vanishingly unlikely; this caps the work regardless.
const MINT_ATTEMPTS: usize = 64;

/// The `AgentStatus` values this slice of the service can hold, named as the
/// model's `AgentStatus` enum names them on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// Created but never prepared — `CreateAgent`'s result.
    NotPrepared,
    /// `PrepareAgent` accepted; the agent reaches [`AgentStatus::Prepared`] once
    /// [`PREPARE_SPAN`] has elapsed.
    Preparing,
    /// Preparation finished; the DRAFT version is servable.
    Prepared,
    /// `DeleteAgent` accepted.
    Deleting,
}

impl AgentStatus {
    /// The member value for the modelled `AgentStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::NotPrepared => "NOT_PREPARED",
            Self::Preparing => "PREPARING",
            Self::Prepared => "PREPARED",
            Self::Deleting => "DELETING",
        }
    }
}

/// One stored agent: the single source of truth for every agent operation.
#[derive(Debug, Clone)]
pub struct AgentRecord {
    pub agent_id: String,
    pub agent_name: String,
    pub agent_arn: String,
    pub agent_version: String,
    pub agent_status: AgentStatus,
    /// A status the CLOCK owes this agent, and the instant it comes due. A
    /// reporting operation may materialise it once that instant has passed; it
    /// never invents a transition of its own.
    pub pending_status: Option<(SystemTime, AgentStatus)>,
    pub instruction: Option<String>,
    pub foundation_model: Option<String>,
    pub description: Option<String>,
    pub idle_session_ttl_in_seconds: i32,
    pub agent_resource_role_arn: String,
    pub customer_encryption_key_arn: Option<String>,
    pub orchestration_type: Option<String>,
    pub custom_orchestration: Option<CustomOrchestration>,
    pub agent_collaboration: Option<String>,
    pub prompt_override_configuration: Option<PromptOverrideConfiguration>,
    pub guardrail_configuration: Option<GuardrailConfiguration>,
    pub memory_configuration: Option<MemoryConfiguration>,
    pub tags: HashMap<String, String>,
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub prepared_at: Option<SystemTime>,
}

impl AgentRecord {
    /// This record as the modelled `Agent` shape: every required member is
    /// populated from the stored record, never from the request being served.
    pub fn to_agent(&self) -> Agent {
        Agent {
            agent_id: self.agent_id.clone(),
            agent_name: self.agent_name.clone(),
            agent_arn: self.agent_arn.clone(),
            agent_version: self.agent_version.clone(),
            client_token: self.client_token.clone(),
            instruction: self.instruction.clone(),
            agent_status: self.agent_status.as_wire().to_string(),
            foundation_model: self.foundation_model.clone(),
            description: self.description.clone(),
            orchestration_type: self.orchestration_type.clone(),
            custom_orchestration: self.custom_orchestration.clone(),
            idle_session_ttl_in_seconds: self.idle_session_ttl_in_seconds,
            agent_resource_role_arn: self.agent_resource_role_arn.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            prepared_at: self.prepared_at.map(to_date_time),
            failure_reasons: None,
            recommended_actions: None,
            prompt_override_configuration: self.prompt_override_configuration.clone(),
            guardrail_configuration: self.guardrail_configuration.clone(),
            memory_configuration: self.memory_configuration.clone(),
            agent_collaboration: self.agent_collaboration.clone(),
        }
    }

    /// This record projected onto the modelled `AgentSummary` shape, the
    /// `ListAgents` item.
    pub fn to_summary(&self) -> AgentSummary {
        AgentSummary {
            agent_id: self.agent_id.clone(),
            agent_name: self.agent_name.clone(),
            agent_status: self.agent_status.as_wire().to_string(),
            description: self.description.clone(),
            updated_at: to_date_time(self.updated_at),
            latest_agent_version: Some(self.agent_version.clone()),
            guardrail_configuration: self.guardrail_configuration.clone(),
        }
    }

    /// This record's position in the `ListAgents` ordering, as an opaque resume
    /// cursor: creation instant then id, so a page resumes exactly where the
    /// previous one stopped even if the agent it named has since been deleted.
    pub fn page_cursor(&self) -> String {
        format!("{}-{}", epoch_nanos(self.created_at), self.agent_id)
    }

    /// The sort key `ListAgents` orders by: creation instant, ties broken by id.
    fn sort_key(&self) -> (u128, &str) {
        (epoch_nanos(self.created_at), self.agent_id.as_str())
    }
}

/// The most flows this store holds; past it `CreateFlow` answers the modelled
/// `ServiceQuotaExceededException` instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on flows per account per Region,
/// and this emulator models neither Service Quotas nor an account's adjustments —
/// so this is the emulator's OWN bound on the store, set far above any workflow
/// an emulator serves, and it is what stops the flow state growing without end.
pub const FLOW_QUOTA: usize = 1_000;

/// The `GetFlow$includedData` value that asks for resource metadata only: the
/// member is documented "Set to METADATA_ONLY to return only resource metadata.
/// Set to ALL_DATA or omit this field to return the full response", and the flow
/// definition is the one non-metadata part of that response.
pub const INCLUDED_DATA_METADATA_ONLY: &str = "METADATA_ONLY";

/// Whether a `GetFlow` carrying this `includedData` is asking for the flow
/// definition: every value but [`INCLUDED_DATA_METADATA_ONLY`] is, an omitted
/// member included.
pub fn included_data_carries_definition(included_data: Option<&str>) -> bool {
    included_data != Some(INCLUDED_DATA_METADATA_ONLY)
}

/// The `FlowStatus` values this slice of the service can hold, named as the
/// model's `FlowStatus` enum names them on the wire.
///
/// `CreateFlow` mints only [`FlowStatus::NotPrepared`] — the operation's own
/// contract says "When you submit this request, the status will be NotPrepared" —
/// and a flow leaves it only when a later call (`PrepareFlow`, `UpdateFlow`) moves
/// it, never because the clock passed or because somebody read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowStatus {
    /// Created or updated, but not prepared: `CreateFlow`'s result.
    NotPrepared,
    /// `PrepareFlow` accepted; the DRAFT version is being packaged.
    Preparing,
    /// The DRAFT version carries the latest changes.
    Prepared,
    /// The last operation invoked on the flow failed; `validations` says why.
    Failed,
}

impl FlowStatus {
    /// The member value for the modelled `FlowStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::NotPrepared => "NotPrepared",
            Self::Preparing => "Preparing",
            Self::Prepared => "Prepared",
            Self::Failed => "Failed",
        }
    }
}

/// One stored flow: the single source of truth for every flow operation.
#[derive(Debug, Clone)]
pub struct FlowRecord {
    pub flow_id: String,
    pub name: String,
    pub arn: String,
    pub version: String,
    pub status: FlowStatus,
    pub description: Option<String>,
    pub execution_role_arn: String,
    pub customer_encryption_key_arn: Option<String>,
    pub definition: Option<FlowDefinition>,
    /// Validation findings from the last operation on the flow; empty while
    /// nothing has failed.
    pub validations: Vec<FlowValidation>,
    pub tags: HashMap<String, String>,
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl FlowRecord {
    /// This record as the `CreateFlow` response: every member comes from the
    /// stored record, never from the request being served.
    pub fn to_create_output(&self) -> CreateFlowOutput {
        CreateFlowOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            id: self.flow_id.clone(),
            arn: self.arn.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            version: self.version.clone(),
            definition: self.definition.clone(),
        }
    }

    /// This record as the `GetFlow` response. `with_definition` is false for a
    /// metadata-only read, which drops the flow definition and nothing else;
    /// `validations` is reported only while the flow actually carries findings.
    pub fn to_get_output(&self, with_definition: bool) -> GetFlowOutput {
        GetFlowOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            id: self.flow_id.clone(),
            arn: self.arn.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            version: self.version.clone(),
            definition: with_definition.then(|| self.definition.clone()).flatten(),
            validations: (!self.validations.is_empty()).then(|| self.validations.clone()),
        }
    }
}

/// The modelled `FlowIdentifier` pattern, quoted verbatim so a refusal can name
/// the constraint the value failed.
pub const FLOW_IDENTIFIER_PATTERN: &str =
    "^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$";

/// Whether `value` is a modelled `FlowId` (`^[0-9a-zA-Z]{10}$`).
fn is_flow_id(value: &str) -> bool {
    value.len() == ID_LEN && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// The `flowId` a modelled `FlowIdentifier` names, or `None` when the value is
/// neither form the shape admits.
///
/// `FlowIdentifier` is modelled [`FLOW_IDENTIFIER_PATTERN`]: either a flow ARN,
/// whose `flow/<id>` segment carries the id, or the bare id. Both forms name the
/// same flow, so both select the same stored record; anything else names no flow
/// at all and is a bad request rather than a missing one.
pub fn flow_id_from_identifier(identifier: &str) -> Option<&str> {
    let id = match identifier.strip_prefix("arn:") {
        None => identifier,
        Some(rest) => {
            // partition: aws | service | region | account | flow/<id>
            let segments: Vec<&str> = rest.split(':').collect();
            let [partition, service, region, account, resource] = segments[..] else {
                return None;
            };
            let region_shaped = (1..=20).contains(&region.len())
                && region.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                });
            if partition != "aws"
                || service != "bedrock"
                || !region_shaped
                || account.len() != 12
                || !account.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            resource.strip_prefix("flow/")?
        }
    };
    is_flow_id(id).then_some(id)
}

/// The most knowledge bases this store holds; past it `CreateKnowledgeBase`
/// answers the modelled `ServiceQuotaExceededException` instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on knowledge bases per account
/// per Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store, set far above
/// any workflow an emulator serves, and it is what stops the knowledge-base
/// state growing without end.
pub const KNOWLEDGE_BASE_QUOTA: usize = 1_000;

/// How long after `CreateKnowledgeBase` a knowledge base leaves `CREATING` for
/// `ACTIVE`.
///
/// CHOSEN, not documented: AWS publishes no duration for knowledge-base
/// creation. It is deliberately non-zero so a caller that reads the knowledge
/// base back immediately still observes the transient `CREATING` status the
/// modelled `KnowledgeBaseStatus` declares ("CREATING – The knowledge base is
/// being created"), and short enough that a client script converges promptly.
pub const KNOWLEDGE_BASE_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// The `KnowledgeBaseType` discriminator of a self-managed vector knowledge
/// base: the one `CreateKnowledgeBase` documents a vector store for ("For
/// self-managed knowledge bases, provide the configuration for your vector
/// store in the storageConfiguration object").
pub const VECTOR_KNOWLEDGE_BASE_TYPE: &str = "VECTOR";

/// Every modelled `KnowledgeBaseType` value, paired with the
/// `KnowledgeBaseConfiguration` member that carries that kind's details.
///
/// Source: the `KnowledgeBaseType` enum and the `KnowledgeBaseConfiguration`
/// structure of this service's Smithy model
/// (`smithy_query(query="shape",shape="KnowledgeBaseType,KnowledgeBaseConfiguration")`).
pub const KNOWLEDGE_BASE_CONFIGURATION_MEMBERS: [(&str, &str); 4] = [
    ("VECTOR", "vectorKnowledgeBaseConfiguration"),
    ("KENDRA", "kendraKnowledgeBaseConfiguration"),
    ("SQL", "sqlKnowledgeBaseConfiguration"),
    ("MANAGED", "managedKnowledgeBaseConfiguration"),
];

/// Why a `knowledgeBaseConfiguration` (with its `storageConfiguration`) cannot
/// back a knowledge base. Each variant is a bad request, answered with the
/// modelled `ValidationException`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnowledgeBaseConfigurationProblem {
    /// `type` is none of the modelled `KnowledgeBaseType` values.
    UnknownType(String),
    /// `type` names a kind whose own configuration member was not supplied.
    MissingConfigurationMember {
        knowledge_base_type: String,
        member: &'static str,
    },
    /// A self-managed vector knowledge base with no vector store configured.
    MissingStorageConfiguration,
}

impl KnowledgeBaseConfigurationProblem {
    /// The refusal's message: it names the member and the constraint the
    /// request failed, so it is built from the request rather than quoted.
    pub fn message(&self) -> String {
        match self {
            Self::UnknownType(value) => {
                let admitted: Vec<&str> = KNOWLEDGE_BASE_CONFIGURATION_MEMBERS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect();
                format!(
                    "Value '{value}' at 'knowledgeBaseConfiguration.type' failed to satisfy \
                     constraint: Member must satisfy enum value set: [{}]",
                    admitted.join(", ")
                )
            }
            Self::MissingConfigurationMember {
                knowledge_base_type,
                member,
            } => format!(
                "knowledgeBaseConfiguration.{member} is required for a \
                 knowledgeBaseConfiguration.type of {knowledge_base_type}"
            ),
            Self::MissingStorageConfiguration => format!(
                "storageConfiguration is required for a knowledgeBaseConfiguration.type of \
                 {VECTOR_KNOWLEDGE_BASE_TYPE}"
            ),
        }
    }
}

/// Whether `configuration` names a kind this service can store, and whether the
/// request supplied everything that kind needs. `None` when it did.
///
/// The `type` member is a discriminator: it names which of
/// `KnowledgeBaseConfiguration`'s sibling members carries that kind's details,
/// and for a self-managed vector knowledge base it also makes
/// `storageConfiguration` — the vector store — a requirement, which a managed,
/// Kendra or SQL knowledge base has no use for.
pub fn knowledge_base_configuration_problem(
    configuration: &KnowledgeBaseConfiguration,
    storage_configuration: Option<&StorageConfiguration>,
) -> Option<KnowledgeBaseConfigurationProblem> {
    let knowledge_base_type = configuration.r#type.as_str();
    let member = KNOWLEDGE_BASE_CONFIGURATION_MEMBERS
        .iter()
        .find(|(name, _)| *name == knowledge_base_type)
        .map(|(_, member)| *member);
    let Some(member) = member else {
        return Some(KnowledgeBaseConfigurationProblem::UnknownType(
            knowledge_base_type.to_string(),
        ));
    };

    let supplied = match knowledge_base_type {
        "VECTOR" => configuration.vector_knowledge_base_configuration.is_some(),
        "KENDRA" => configuration.kendra_knowledge_base_configuration.is_some(),
        "SQL" => configuration.sql_knowledge_base_configuration.is_some(),
        _ => configuration.managed_knowledge_base_configuration.is_some(),
    };
    if !supplied {
        return Some(
            KnowledgeBaseConfigurationProblem::MissingConfigurationMember {
                knowledge_base_type: knowledge_base_type.to_string(),
                member,
            },
        );
    }

    if knowledge_base_type == VECTOR_KNOWLEDGE_BASE_TYPE && storage_configuration.is_none() {
        return Some(KnowledgeBaseConfigurationProblem::MissingStorageConfiguration);
    }

    None
}

/// The `KnowledgeBaseStatus` values this slice of the service can hold, named as
/// the model's `KnowledgeBaseStatus` enum names them on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgeBaseStatus {
    /// `CreateKnowledgeBase` accepted; the knowledge base reaches
    /// [`KnowledgeBaseStatus::Active`] once [`KNOWLEDGE_BASE_ACTIVATION_SPAN`]
    /// has elapsed.
    Creating,
    /// The knowledge base is ready to be queried.
    Active,
}

impl KnowledgeBaseStatus {
    /// The member value for the modelled `KnowledgeBaseStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
        }
    }
}

/// One stored knowledge base: the single source of truth for every
/// knowledge-base operation.
#[derive(Debug, Clone)]
pub struct KnowledgeBaseRecord {
    pub knowledge_base_id: String,
    pub name: String,
    pub knowledge_base_arn: String,
    pub description: Option<String>,
    pub role_arn: String,
    pub knowledge_base_configuration: KnowledgeBaseConfiguration,
    pub storage_configuration: Option<StorageConfiguration>,
    pub status: KnowledgeBaseStatus,
    /// A status the CLOCK owes this knowledge base, and the instant it comes
    /// due. A reporting operation may materialise it once that instant has
    /// passed; it never invents a transition of its own.
    pub pending_status: Option<(SystemTime, KnowledgeBaseStatus)>,
    /// Reasons the last API operation on the knowledge base failed; empty while
    /// nothing has failed.
    pub failure_reasons: Vec<String>,
    pub tags: HashMap<String, String>,
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl KnowledgeBaseRecord {
    /// This record as the modelled `KnowledgeBase` shape: every member is
    /// populated from the stored record, never from the request being served.
    pub fn to_knowledge_base(&self) -> KnowledgeBase {
        KnowledgeBase {
            knowledge_base_id: self.knowledge_base_id.clone(),
            name: self.name.clone(),
            knowledge_base_arn: self.knowledge_base_arn.clone(),
            description: self.description.clone(),
            role_arn: self.role_arn.clone(),
            knowledge_base_configuration: self.knowledge_base_configuration.clone(),
            storage_configuration: self.storage_configuration.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            failure_reasons: (!self.failure_reasons.is_empty())
                .then(|| self.failure_reasons.clone()),
        }
    }
}

/// The most data sources one knowledge base holds; past it `CreateDataSource`
/// answers the modelled `ServiceQuotaExceededException` instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on data sources per knowledge
/// base, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store, set far above
/// any workflow an emulator serves, and it is what stops the data-source state
/// growing without end.
pub const DATA_SOURCE_QUOTA: usize = 1_000;

/// The `KnowledgeBaseType` of a service-managed knowledge base: the one kind
/// whose data sources have a documented `dataDeletionPolicy` default.
pub const MANAGED_KNOWLEDGE_BASE_TYPE: &str = "MANAGED";

/// The `dataDeletionPolicy` a data source of a service-managed knowledge base
/// takes when the request names none.
///
/// Documented: `CreateDataSource`'s `dataDeletionPolicy` says "For managed
/// knowledge bases, the only supported option is DELETE, which is also the
/// default". No default is published for any other kind, so a data source of
/// one keeps whatever the request supplied — including nothing.
pub const MANAGED_DATA_DELETION_POLICY: &str = "DELETE";

/// Every modelled `DataSourceType` value, paired with the
/// `DataSourceConfiguration` member that carries that kind's connection
/// details — `None` for the kinds the model gives no member for, which
/// therefore need none.
///
/// Source: the `DataSourceType` enum and the `DataSourceConfiguration`
/// structure of this service's Smithy model
/// (`smithy_query(query="shape",shape="DataSourceType,DataSourceConfiguration")`).
pub const DATA_SOURCE_CONFIGURATION_MEMBERS: [(&str, Option<&str>); 8] = [
    ("S3", Some("s3Configuration")),
    ("WEB", Some("webConfiguration")),
    ("CONFLUENCE", Some("confluenceConfiguration")),
    ("SALESFORCE", Some("salesforceConfiguration")),
    ("SHAREPOINT", Some("sharePointConfiguration")),
    (
        "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
        Some("managedKnowledgeBaseConnectorConfiguration"),
    ),
    ("CUSTOM", None),
    ("REDSHIFT_METADATA", None),
];

/// Why a `dataSourceConfiguration` cannot back a data source. Each variant is a
/// bad request, answered with the modelled `ValidationException`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataSourceConfigurationProblem {
    /// `type` is none of the modelled `DataSourceType` values.
    UnknownType(String),
    /// `type` names a kind whose own connection member was not supplied.
    MissingConfigurationMember {
        data_source_type: String,
        member: &'static str,
    },
}

impl DataSourceConfigurationProblem {
    /// The refusal's message: it names the member and the constraint the
    /// request failed, so it is built from the request rather than quoted.
    pub fn message(&self) -> String {
        match self {
            Self::UnknownType(value) => {
                let admitted: Vec<&str> = DATA_SOURCE_CONFIGURATION_MEMBERS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect();
                format!(
                    "Value '{value}' at 'dataSourceConfiguration.type' failed to satisfy \
                     constraint: Member must satisfy enum value set: [{}]",
                    admitted.join(", ")
                )
            }
            Self::MissingConfigurationMember {
                data_source_type,
                member,
            } => format!(
                "dataSourceConfiguration.{member} is required for a \
                 dataSourceConfiguration.type of {data_source_type}"
            ),
        }
    }
}

/// Whether `configuration` names a kind this service can store, and whether the
/// request supplied the connection details that kind needs. `None` when it did.
///
/// The `type` member is a discriminator: it names which of
/// `DataSourceConfiguration`'s sibling members carries that kind's connection
/// details, and the kinds the model pairs with no member need none.
pub fn data_source_configuration_problem(
    configuration: &DataSourceConfiguration,
) -> Option<DataSourceConfigurationProblem> {
    let data_source_type = configuration.r#type.as_str();
    let Some((_, member)) = DATA_SOURCE_CONFIGURATION_MEMBERS
        .iter()
        .find(|(name, _)| *name == data_source_type)
    else {
        return Some(DataSourceConfigurationProblem::UnknownType(
            data_source_type.to_string(),
        ));
    };
    // The kinds the model pairs with no sibling member need none.
    let member = (*member)?;

    let supplied = match data_source_type {
        "S3" => configuration.s3_configuration.is_some(),
        "WEB" => configuration.web_configuration.is_some(),
        "CONFLUENCE" => configuration.confluence_configuration.is_some(),
        "SALESFORCE" => configuration.salesforce_configuration.is_some(),
        "SHAREPOINT" => configuration.share_point_configuration.is_some(),
        _ => configuration
            .managed_knowledge_base_connector_configuration
            .is_some(),
    };
    if supplied {
        return None;
    }
    Some(DataSourceConfigurationProblem::MissingConfigurationMember {
        data_source_type: data_source_type.to_string(),
        member,
    })
}

/// The `DataSourceStatus` values this slice of the service can hold, named as
/// the model's `DataSourceStatus` enum names them on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSourceStatus {
    /// Created and ready to be ingested from — what `CreateDataSource` answers
    /// with, since it completes synchronously (HTTP 200) and the modelled
    /// `DataSource$status` documents this as the status of a data source that
    /// "has been created and is ready for ingestion into the knowledge base".
    Available,
}

impl DataSourceStatus {
    /// The member value for the modelled `DataSourceStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Available => "AVAILABLE",
        }
    }
}

/// One stored data source: the single source of truth for every data-source
/// operation.
#[derive(Debug, Clone)]
pub struct DataSourceRecord {
    pub knowledge_base_id: String,
    pub data_source_id: String,
    pub name: String,
    pub description: Option<String>,
    pub data_source_configuration: DataSourceConfiguration,
    pub server_side_encryption_configuration: Option<ServerSideEncryptionConfiguration>,
    pub vector_ingestion_configuration: Option<VectorIngestionConfiguration>,
    pub data_deletion_policy: Option<String>,
    pub status: DataSourceStatus,
    /// Reasons the last API operation on the data source failed; empty while
    /// nothing has failed.
    pub failure_reasons: Vec<String>,
    pub client_token: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl DataSourceRecord {
    /// This record as the modelled `DataSource` shape: every member is
    /// populated from the stored record, never from the request being served.
    pub fn to_data_source(&self) -> DataSource {
        DataSource {
            knowledge_base_id: self.knowledge_base_id.clone(),
            data_source_id: self.data_source_id.clone(),
            name: self.name.clone(),
            status: self.status.as_wire().to_string(),
            description: self.description.clone(),
            data_source_configuration: self.data_source_configuration.clone(),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            vector_ingestion_configuration: self.vector_ingestion_configuration.clone(),
            data_deletion_policy: self.data_deletion_policy.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            failure_reasons: (!self.failure_reasons.is_empty())
                .then(|| self.failure_reasons.clone()),
        }
    }
}

/// A `ListAgents` resume cursor, decoded back into the ordering it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    created_at_nanos: u128,
    agent_id: String,
}

impl PageCursor {
    /// Decode a `nextToken` a previous page handed out. `None` when the token was
    /// not produced by [`AgentRecord::page_cursor`].
    pub fn parse(token: &str) -> Option<Self> {
        let (nanos, agent_id) = token.split_once('-')?;
        if agent_id.is_empty() {
            return None;
        }
        Some(Self {
            created_at_nanos: nanos.parse().ok()?,
            agent_id: agent_id.to_string(),
        })
    }

    /// Whether `record` sorts after this cursor, i.e. belongs on the next page.
    fn precedes(&self, record: &AgentRecord) -> bool {
        (self.created_at_nanos, self.agent_id.as_str()) < record.sort_key()
    }
}

/// Per-service state for com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// Every agent that exists, keyed by its 10-character `agentId`.
    pub agents: HashMap<String, AgentRecord>,
    /// `agentName` -> `agentId`, so `CreateAgent` can refuse a duplicate name.
    pub agent_names: HashMap<String, String>,
    /// `clientToken` -> `agentId`, so a retried `CreateAgent` carrying the same
    /// `@idempotencyToken` returns the agent it already created.
    pub client_tokens: HashMap<String, String>,
    /// Every flow that exists, keyed by its 10-character `flowId`.
    pub flows: HashMap<String, FlowRecord>,
    /// `clientToken` -> `flowId`, so a retried `CreateFlow` carrying the same
    /// `@idempotencyToken` returns the flow it already created.
    pub flow_client_tokens: HashMap<String, String>,
    /// Every knowledge base that exists, keyed by its 10-character
    /// `knowledgeBaseId`.
    pub knowledge_bases: HashMap<String, KnowledgeBaseRecord>,
    /// `clientToken` -> `knowledgeBaseId`, so a retried `CreateKnowledgeBase`
    /// carrying the same `@idempotencyToken` returns the knowledge base it
    /// already created.
    pub knowledge_base_client_tokens: HashMap<String, String>,
    /// Every data source that exists, keyed by its 10-character `dataSourceId`.
    /// Each record names the knowledge base it belongs to, so a read of one
    /// knowledge base never reports another's data source.
    pub data_sources: HashMap<String, DataSourceRecord>,
    /// `clientToken` -> `dataSourceId`, so a retried `CreateDataSource`
    /// carrying the same `@idempotencyToken` returns the data source it already
    /// created.
    pub data_source_client_tokens: HashMap<String, String>,
    /// Feeds id minting; never returned to a caller.
    id_sequence: u64,
}

/// The alphabet an `agentId` is drawn from: the modelled `Id` pattern is
/// `^[0-9a-zA-Z]{10}$`.
const ID_ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Characters in a minted `agentId`, from that same pattern.
const ID_LEN: usize = 10;

impl ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// The next candidate id drawn from the modelled `^[0-9a-zA-Z]{10}$` alphabet.
    /// Whether it is free is the caller's question: every resource family keys its
    /// own map.
    fn next_id_candidate(&mut self, now: SystemTime) -> String {
        self.id_sequence = self.id_sequence.wrapping_add(1);
        let mut hasher = DefaultHasher::new();
        self.id_sequence.hash(&mut hasher);
        epoch_nanos(now).hash(&mut hasher);
        let mut bits = hasher.finish();
        let mut id = String::with_capacity(ID_LEN);
        for _ in 0..ID_LEN {
            id.push(char::from(ID_ALPHABET[(bits % 62) as usize]));
            bits /= 62;
        }
        id
    }

    /// Mint an unused `agentId` matching the modelled `^[0-9a-zA-Z]{10}$`, or
    /// `None` when every one of [`MINT_ATTEMPTS`] draws collided — answered with a
    /// modelled server error rather than by looping.
    pub fn mint_agent_id(&mut self, now: SystemTime) -> Option<String> {
        for _ in 0..MINT_ATTEMPTS {
            let id = self.next_id_candidate(now);
            if !self.agents.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// Mint an unused `flowId`, which the modelled `FlowId` shape constrains to the
    /// same `^[0-9a-zA-Z]{10}$` as an `agentId`. `None` when every one of
    /// [`MINT_ATTEMPTS`] draws collided.
    pub fn mint_flow_id(&mut self, now: SystemTime) -> Option<String> {
        for _ in 0..MINT_ATTEMPTS {
            let id = self.next_id_candidate(now);
            if !self.flows.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// Mint an unused `knowledgeBaseId`, which the modelled `Id` shape
    /// constrains to the same `^[0-9a-zA-Z]{10}$` as an `agentId`. `None` when
    /// every one of [`MINT_ATTEMPTS`] draws collided.
    pub fn mint_knowledge_base_id(&mut self, now: SystemTime) -> Option<String> {
        for _ in 0..MINT_ATTEMPTS {
            let id = self.next_id_candidate(now);
            if !self.knowledge_bases.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// Mint an unused `dataSourceId`, which the modelled `Id` shape constrains
    /// to the same `^[0-9a-zA-Z]{10}$` as an `agentId`. `None` when every one of
    /// [`MINT_ATTEMPTS`] draws collided.
    pub fn mint_data_source_id(&mut self, now: SystemTime) -> Option<String> {
        for _ in 0..MINT_ATTEMPTS {
            let id = self.next_id_candidate(now);
            if !self.data_sources.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// The stored data source `data_source_id` names, but only when it belongs
    /// to `knowledge_base_id`: a data source is identified by the pair, so one
    /// knowledge base never reports another's.
    pub fn data_source_of(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
    ) -> Option<&DataSourceRecord> {
        self.data_sources
            .get(data_source_id)
            .filter(|record| record.knowledge_base_id == knowledge_base_id)
    }

    /// How many data sources `knowledge_base_id` already holds, the count the
    /// per-knowledge-base bound is checked against.
    pub fn data_source_count(&self, knowledge_base_id: &str) -> usize {
        self.data_sources
            .values()
            .filter(|record| record.knowledge_base_id == knowledge_base_id)
            .count()
    }

    /// Apply every status transition the CLOCK has already made due.
    ///
    /// Called by the reporting operations as well as the mutating ones: a change
    /// that was owed at this instant whether or not anyone called is the only
    /// write a read may perform.
    pub fn materialise_due_transitions(&mut self, now: SystemTime) {
        for agent in self.agents.values_mut() {
            if let Some((due, next)) = agent.pending_status {
                if now >= due {
                    agent.agent_status = next;
                    agent.pending_status = None;
                }
            }
        }
        for knowledge_base in self.knowledge_bases.values_mut() {
            if let Some((due, next)) = knowledge_base.pending_status {
                if now >= due {
                    knowledge_base.status = next;
                    knowledge_base.pending_status = None;
                }
            }
        }
    }

    /// One `ListAgents` page: the agents after `cursor` in creation order, at most
    /// `page_size` of them, plus the cursor to resume from when more remain.
    pub fn page_of_agents(
        &self,
        cursor: Option<&PageCursor>,
        page_size: usize,
    ) -> (Vec<AgentSummary>, Option<String>) {
        let mut ordered: Vec<&AgentRecord> = self
            .agents
            .values()
            .filter(|record| cursor.is_none_or(|cursor| cursor.precedes(record)))
            .collect();
        ordered.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));

        let page: Vec<&AgentRecord> = ordered.iter().take(page_size).copied().collect();
        let next_token = (ordered.len() > page.len())
            .then(|| page.last().map(|record| record.page_cursor()))
            .flatten();
        (page.iter().map(|record| record.to_summary()).collect(), next_token)
    }

    /// Forget an agent and every index that pointed at it.
    pub fn remove_agent(&mut self, agent_id: &str) -> Option<AgentRecord> {
        let record = self.agents.remove(agent_id)?;
        self.agent_names.remove(&record.agent_name);
        self.client_tokens.retain(|_, id| id != agent_id);
        Some(record)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState>)
}

/// The region to build an ARN in: the request's SigV4 credential-scope region
/// when it names one the `agentArn` pattern admits, else [`DEFAULT_REGION`].
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 20
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A stored instant as the modelled `date-time` timestamp.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = i64::try_from(epoch_nanos(at) / 1_000_000).unwrap_or(i64::MAX);
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Nanoseconds since the Unix epoch, the total order `ListAgents` pages over.
fn epoch_nanos(at: SystemTime) -> u128 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
