// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-qconnect-wisdomservice service.
//!
//! The service's root resource is the Amazon Q in Connect *assistant*; a session is
//! created inside one assistant and is reachable only through its parent. Everything a
//! handler answers with is derived from the records kept here, so what one request
//! writes is what the next request reads.

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::{
    AIAgentConfigurationData, AssistantAssociationData, AssistantAssociationInputData,
    AssistantAssociationOutputData, AssistantCapabilityConfiguration, AssistantData,
    AssistantIntegrationConfiguration, AssistantSummary, ExternalBedrockKnowledgeBaseConfig,
    KnowledgeBaseAssociationData, KnowledgeBaseData, OrchestratorConfigurationEntry,
    RenderingConfiguration, ServerSideEncryptionConfiguration, SessionData, SourceConfiguration,
    TagFilter, VectorIngestionConfiguration,
};

/// The only `AssistantType` the model declares
/// (`smithy_query(query="shape",shape="AssistantType")`).
pub const ASSISTANT_TYPE_AGENT: &str = "AGENT";

/// The terminal `AssistantStatus` of a usable assistant, and the one a create answers
/// with: the service reports the assistant ready in the response to the call that made
/// it, so no later status change is owed and every read reports the same value
/// (`smithy_query(query="shape",shape="AssistantStatus")` also declares
/// `CREATE_IN_PROGRESS`, `CREATE_FAILED`, `DELETE_IN_PROGRESS`, `DELETE_FAILED` and
/// `DELETED`, which this emulator reaches for no assistant it holds: a delete removes
/// the record outright).
pub const ASSISTANT_STATUS_ACTIVE: &str = "ACTIVE";

/// The `AssistantCapabilityType` an assistant created through the Amazon Q in Connect
/// API carries (`smithy_query(query="shape",shape="AssistantCapabilityType")` declares
/// `V1` and `V2`): `V1` is the legacy Amazon Connect Wisdom capability, `V2` the one
/// this model's AI agent, AI prompt and AI guardrail surface needs. `CreateAssistant`
/// takes no capability member, so the service chooses it.
pub const ASSISTANT_CAPABILITY_V2: &str = "V2";

/// The `Origin` of a session created through the `CreateSession` API — the model's own
/// wording: "`SYSTEM` for a default Session created by Amazon Q in Connect or
/// `CUSTOMER` for a Session created by calling CreateSession API".
pub const SESSION_ORIGIN_CUSTOMER: &str = "CUSTOMER";

/// The page size `ListAssistants` serves when a call names no `maxResults`: the
/// modelled maximum of the `maxResults` range (1..100).
pub const DEFAULT_MAX_RESULTS: usize = 100;

/// The most assistants one account may hold at a time; a create beyond it is refused
/// with the modelled `ServiceQuotaExceededException` (HTTP 402).
///
/// CHOSEN: AWS quotas assistants per account but does not publish the figure. The bound
/// is also what keeps this store finite — a record is released only by
/// `DeleteAssistant`.
pub const MAX_ASSISTANTS: usize = 100;

/// The most live sessions one assistant may hold at a time, with the same contract as
/// [`MAX_ASSISTANTS`]. `CreateSession` models no quota error, so a create beyond this
/// bound is refused with its modelled `ValidationException` (HTTP 400).
pub const MAX_SESSIONS_PER_ASSISTANT: usize = 200;

/// The `KnowledgeBaseStatus` a knowledge base this service holds carries
/// (`smithy_query(query="shape",shape="KnowledgeBaseStatus")` also declares
/// `CREATE_IN_PROGRESS`, `CREATE_FAILED`, `DELETE_IN_PROGRESS`, `DELETE_FAILED` and
/// `DELETED`).
///
/// A `CreateKnowledgeBase` answer is recorded with `status` ACTIVE, so the create owes no
/// later transition: a resource the service returns ready stays ready. The assistant
/// above is recorded the same way — see [`ASSISTANT_STATUS_ACTIVE`].
pub const KNOWLEDGE_BASE_STATUS_ACTIVE: &str = "ACTIVE";

/// The sentence this service answers a request addressed to an assistant this account
/// does not hold with.
///
/// Every message constant below is a FIXED service text: it is the same sentence for
/// every request that meets the condition and it quotes nothing from the request. The
/// recorded wire responses show exactly that — the not-found answer to a request for an
/// assistant id names no id — so a message built by interpolating the requested
/// identifier is a message this service never sends. The requested label still travels,
/// in `ResourceNotFoundException`'s own `resourceName` member, which the model provides
/// for naming it.
pub const MESSAGE_ASSISTANT_NOT_FOUND: &str = "Assistant does not exist.";

/// The fixed text for a request addressed to a knowledge base this account does not
/// hold. See [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";

/// The fixed text for a request addressed to a session its named assistant does not
/// hold. See [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_SESSION_NOT_FOUND: &str = "Session does not exist.";

/// The fixed text for a request addressed to an assistant association its named
/// assistant does not hold. See [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_ASSOCIATION_NOT_FOUND: &str = "Assistant Association does not exist.";

/// The fixed text of the modelled `ConflictException` a create whose `name` a live
/// resource already holds is refused with. See [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_NAME_IN_USE: &str = "Name is already in use";

/// The fixed text of the modelled `ConflictException` a create that reuses an
/// `@idempotencyToken` with different parameters is refused with. See
/// [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_IDEMPOTENCY_CONFLICT: &str = "Idempotency conflict";

/// The fixed text of the modelled `UnauthorizedException`. See
/// [`MESSAGE_ASSISTANT_NOT_FOUND`].
pub const MESSAGE_UNAUTHORIZED: &str = "Unauthorized";

/// The `Origin` of an AI agent the service created for an assistant itself, as the
/// model words it: "`SYSTEM` for a default AI Agent created by Q in Connect or
/// `CUSTOMER` for an AI Agent created by calling AI Agent creation APIs"
/// (`smithy_query(query="shape",shape="AIAgentData")`, member `origin`).
pub const AI_AGENT_ORIGIN_SYSTEM: &str = "SYSTEM";

/// The `VisibilityStatus` of a default AI agent: it is in service for the assistant the
/// moment the assistant exists, which is what `PUBLISHED` means
/// (`smithy_query(query="shape",shape="VisibilityStatus")` declares `SAVED` and
/// `PUBLISHED`).
pub const AI_AGENT_VISIBILITY_PUBLISHED: &str = "PUBLISHED";

/// The terminal `Status` of an AI agent in service
/// (`smithy_query(query="shape",shape="Status")`).
pub const AI_AGENT_STATUS_ACTIVE: &str = "ACTIVE";

/// The one AI agent type whose binding can be turned off, which is why its binding is
/// the one that reports `enabled`: the model's `AIAgentConfigurationData$enabled` doc
/// says "Setting this value to false is currently supported only for the
/// `ANSWER_RECOMMENDATION` AI Agent type".
///
/// Its default binding is reported DISABLED, which is what the recorded wire answer for
/// a fresh assistant shows (and what the model's wording describes as "the AI Agent ID
/// is retained but no AI Agent runs for the AI Agent type"): answer recommendation runs
/// against knowledge-base content, and a fresh assistant is associated with none yet.
/// Every other type's binding reports no `enabled` at all, since no other type's can be
/// turned off.
pub const AI_AGENT_TYPE_ANSWER_RECOMMENDATION: &str = "ANSWER_RECOMMENDATION";

/// The AI agent types an assistant gets a default `SYSTEM` AI agent for when the service
/// creates it, which is what a fresh assistant's `aiAgentConfiguration` reports a
/// binding for.
///
/// Every `AIAgentType` the model declares
/// (`smithy_query(query="shape",shape="AIAgentType")`) except `ORCHESTRATION`, which has
/// no default agent: the recorded wire answer for a fresh assistant carries a binding
/// for each of the eight below and none for `ORCHESTRATION`. Sorted, so the list reads
/// as the set it is.
pub const DEFAULT_AI_AGENT_TYPES: [&str; 8] = [
    "ANSWER_RECOMMENDATION",
    "CASE_SUMMARIZATION",
    "EMAIL_GENERATIVE_ANSWER",
    "EMAIL_OVERVIEW",
    "EMAIL_RESPONSE",
    "MANUAL_SEARCH",
    "NOTE_TAKING",
    "SELF_SERVICE",
];

/// The most knowledge bases one account may hold at a time, with the same contract as
/// [`MAX_ASSISTANTS`]: a create beyond it is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402), which is also what keeps this store
/// finite.
///
/// CHOSEN: AWS quotas knowledge bases per account but does not publish the figure.
pub const MAX_KNOWLEDGE_BASES: usize = 100;

/// The most assistant associations one assistant may hold at a time. A create beyond it
/// is refused with the modelled `ServiceQuotaExceededException` (HTTP 402), which
/// `CreateAssistantAssociation` declares for exactly this, and which is also what keeps
/// this store finite: an association is released only by `DeleteAssistantAssociation`
/// or by the delete of its assistant.
///
/// CHOSEN: AWS quotas associations per assistant but does not publish the figure.
pub const MAX_ASSOCIATIONS_PER_ASSISTANT: usize = 100;

/// The `AssociationType` of an association with a knowledge base this account holds
/// (`smithy_query(query="shape",shape="AssociationType")`).
pub const ASSOCIATION_TYPE_KNOWLEDGE_BASE: &str = "KNOWLEDGE_BASE";

/// The `AssociationType` of an association with a Bedrock knowledge base outside this
/// service (`smithy_query(query="shape",shape="AssociationType")`).
pub const ASSOCIATION_TYPE_EXTERNAL_BEDROCK_KNOWLEDGE_BASE: &str =
    "EXTERNAL_BEDROCK_KNOWLEDGE_BASE";

/// How many ids a mint tries before giving up, so no request can spin here. Each
/// candidate comes from the store's own sequence, so the first one is free unless the
/// store already holds that exact id.
const MINT_ATTEMPTS: usize = 8;

/// The ARN namespace (and SigV4 signing name) of this service: Amazon Q in Connect is
/// still `wisdom` on the wire.
pub const ARN_NAMESPACE: &str = "wisdom";

/// The region a request that carries no SigV4 credential scope is attributed to —
/// `emulator_arn`'s own documented default.
const DEFAULT_REGION: &str = "us-east-1";

/// The region the caller signed this request for: an ARN names the region the request
/// was addressed to. Falls back to [`DEFAULT_REGION`] for an unsigned request, and for
/// a scope region the modelled `Arn` pattern (`[a-z0-9-]*?`) would not admit.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's ARN for an assistant:
/// `arn:aws:wisdom:<region>:<account>:assistant/<assistantId>`.
pub fn assistant_arn(region: &str, assistant_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("assistant/{assistant_id}"))
}

/// This service's ARN for a knowledge base:
/// `arn:aws:wisdom:<region>:<account>:knowledge-base/<knowledgeBaseId>`.
pub fn knowledge_base_arn(region: &str, knowledge_base_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("knowledge-base/{knowledge_base_id}"),
    )
}

/// This service's ARN for an assistant association, which names its parent assistant
/// too: `arn:aws:wisdom:<region>:<account>:association/<assistantId>/<assistantAssociationId>`.
///
/// Two id segments, which is the form the modelled `Arn` pattern admits for a nested
/// resource (`<label>/<uuid>(/<uuid>){0,2}`), and the form [`resolve_id`] reads the
/// resource's own id back out of.
pub fn assistant_association_arn(
    region: &str,
    assistant_id: &str,
    assistant_association_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("association/{assistant_id}/{assistant_association_id}"),
    )
}

/// This service's ARN for a session, which names its parent assistant too:
/// `arn:aws:wisdom:<region>:<account>:session/<assistantId>/<sessionId>`.
pub fn session_arn(region: &str, assistant_id: &str, session_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("session/{assistant_id}/{session_id}"),
    )
}

/// This service's ARN for an AI agent, which names its parent assistant too:
/// `arn:aws:wisdom:<region>:<account>:ai-agent/<assistantId>/<aiAgentId>`.
pub fn ai_agent_arn(region: &str, assistant_id: &str, ai_agent_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("ai-agent/{assistant_id}/{ai_agent_id}"),
    )
}

/// The ARN of the SNS topic an assistant streams chat messages through, which the
/// service provisions with the assistant: an Amazon SNS topic ARN
/// (`arn:aws:sns:<region>:<account>:<topic>`), named after the assistant it belongs to,
/// so one assistant's topic is never another's and a read reports the same ARN every
/// time.
pub fn topic_integration_arn(region: &str, assistant_id: &str) -> String {
    ::engine::emulator_arn(
        "sns",
        region,
        &format!("qconnect-assistant-{assistant_id}"),
    )
}

/// One id candidate for the mint block the store's sequence number `seq` owns.
///
/// The form is the modelled `Uuid` shape
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`). UNIQUENESS comes
/// from the sequence, which no two mints share — two resources created in the same
/// nanosecond still get different ids; the digest only spreads that counter (and the
/// wall clock, so two runs of the same process do not repeat ids) over the 32 hex digits
/// the pattern wants, so an id carries no readable structure and no sequence to guess.
fn mint_candidate(seq: u64, attempt: u64) -> String {
    use ::sha2::Digest;

    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let block = seq
        .wrapping_mul(MINT_ATTEMPTS as u64)
        .wrapping_add(attempt);
    let digest = ::sha2::Sha256::digest(format!("{ARN_NAMESPACE}:{block}:{nanos}").as_bytes());
    let hex = ::hex::encode(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The bare UUID a `UuidOrArn` label names: the model's pattern admits either the id or
/// the resource's full ARN, and every ARN this service mints ends in the resource's own
/// UUID, so the trailing path segment is the id.
///
/// An ARN this emulator could not have minted — another partition, service or account —
/// resolves to NOTHING: the label comes back unchanged, so the lookup misses and the
/// caller gets the operation's not-found error instead of a resource the ARN does not
/// name.
pub fn resolve_id(label: &str) -> &str {
    let Some((prefix, id)) = label.rsplit_once('/') else {
        // No path segment at all: a bare UUID.
        return label;
    };
    let mut fields = prefix.split(':');
    let minted_here = fields.next() == Some("arn")
        && fields.next() == Some("aws")
        && fields.next() == Some(ARN_NAMESPACE)
        && fields.next().is_some_and(|region| !region.is_empty())
        && fields.next() == Some(::engine::emulator_account_id())
        // The resource segment, plus any parent id a nested resource's ARN carries.
        && fields.next().is_some();
    if minted_here {
        id
    } else {
        label
    }
}

/// A non-tag map field rendered as the modelled map shape, or `None` when it is empty:
/// a resource with nothing to report under it reports no member rather than an empty
/// object. The `tags` member follows the opposite rule — see [`tags_out`], which is what
/// the recorded responses show for it.
fn map_out<V: Clone>(map: &HashMap<String, V>) -> Option<HashMap<String, V>> {
    (!map.is_empty()).then(|| map.clone())
}

/// The `tags` member of a resource this service reports: ALWAYS present, and an empty
/// object when the resource carries no tag.
///
/// This is what the service answers on the wire — a recorded create of an untagged
/// resource carries the `tags` member all the same — so a client reading
/// `response["tags"]` finds a map for every resource rather than having to tell an
/// untagged resource's absent member from a tagged one's map.
fn tags_out(tags: &HashMap<String, String>) -> Option<HashMap<String, String>> {
    Some(tags.clone())
}

/// One session of an assistant, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub session_arn: String,
    pub assistant_id: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub tag_filter: Option<TagFilter>,
    pub ai_agent_configuration: HashMap<String, AIAgentConfigurationData>,
    /// The contact this session was opened for, as the caller gave it. Not a member of
    /// `SessionData`, so it is persisted but not reported.
    pub contact_arn: Option<String>,
    pub origin: String,
    pub created_at: SystemTime,
}

impl Session {
    /// The `SessionData` every operation that reports this session answers with.
    pub fn to_session_data(&self) -> SessionData {
        SessionData {
            session_arn: self.session_arn.clone(),
            session_id: self.session_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            tags: tags_out(&self.tags),
            // The emulator integrates a session with no external topic, so the
            // integration configuration has nothing to report.
            integration_configuration: None,
            tag_filter: self.tag_filter.clone(),
            ai_agent_configuration: map_out(&self.ai_agent_configuration),
            origin: Some(self.origin.clone()),
            orchestrator_configuration_list: None,
        }
    }
}

/// One AI agent of an assistant, as this emulator persists it.
///
/// The default agents the service creates with the assistant are `SYSTEM`-origin
/// records; an agent a caller creates through `CreateAIAgent` is `CUSTOMER`-origin and
/// carries the configuration the caller gave. Nothing reports these yet — `GetAIAgent`
/// and `ListAIAgents` are still scaffolded — but the assistant's
/// `aiAgentConfiguration` names them, so the ids it reports are the ids of records this
/// store holds rather than numbers with nothing behind them.
#[derive(Debug, Clone)]
pub struct AiAgent {
    pub ai_agent_id: String,
    pub ai_agent_arn: String,
    pub assistant_id: String,
    pub name: String,
    pub r#type: String,
    pub origin: String,
    pub visibility_status: String,
    pub status: String,
    pub created_at: SystemTime,
}

/// One Amazon Q in Connect assistant, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Assistant {
    pub assistant_id: String,
    pub assistant_arn: String,
    pub name: String,
    pub r#type: String,
    pub status: String,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub server_side_encryption_configuration: Option<ServerSideEncryptionConfiguration>,
    pub created_at: SystemTime,
    /// The `AssistantCapabilityType` this assistant was provisioned with. Chosen by the
    /// service — `CreateAssistant` models no capability member — so it is stored once
    /// and reported unchanged.
    pub capability_type: String,
    /// The ARN of the SNS topic this assistant streams chat messages through. The
    /// service provisions one with the assistant — see [`topic_integration_arn`] — so
    /// every read of the assistant reports the same topic.
    pub topic_integration_arn: Option<String>,
    /// The AI agent bound to this assistant per `AIAgentType`, which is what
    /// `UpdateAssistantAIAgent`/`RemoveAssistantAIAgent` write. Every entry names an
    /// agent in [`Self::ai_agents`].
    pub ai_agent_configuration: HashMap<String, AIAgentConfigurationData>,
    /// The AI agents this assistant holds, keyed by `aiAgentId`: the default `SYSTEM`
    /// ones the create provisions, and any a caller adds later. An AI agent is
    /// reachable only through its assistant, as a session is.
    pub ai_agents: HashMap<String, AiAgent>,
    /// This assistant's orchestrator configurations, with the same contract as
    /// [`Self::ai_agent_configuration`].
    pub orchestrator_configuration_list: Vec<OrchestratorConfigurationEntry>,
    /// This assistant's sessions, keyed by `sessionId`. A session is reachable only
    /// through its parent, so a session id does not resolve under another assistant.
    pub sessions: HashMap<String, Session>,
}

impl Assistant {
    /// The assistant's integration configuration, as every operation that reports this
    /// assistant renders it: the member is always present, and carries a
    /// `topicIntegrationArn` only once a topic is integrated with the assistant.
    fn integration_configuration(&self) -> Option<AssistantIntegrationConfiguration> {
        Some(AssistantIntegrationConfiguration {
            topic_integration_arn: self.topic_integration_arn.clone(),
        })
    }

    /// The assistant's capability configuration, rendered from the stored capability.
    fn capability_configuration(&self) -> Option<AssistantCapabilityConfiguration> {
        Some(AssistantCapabilityConfiguration {
            r#type: Some(self.capability_type.clone()),
        })
    }

    /// The `AssistantData` every operation that reports this assistant answers with.
    pub fn to_assistant_data(&self) -> AssistantData {
        AssistantData {
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            name: self.name.clone(),
            r#type: self.r#type.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            tags: tags_out(&self.tags),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            // Every configuration member of the stored record is reported, each out of
            // what the record holds: the capability the assistant was provisioned with,
            // the topic it streams through, the AI agent bound per `AIAgentType` and its
            // orchestrator configurations — the last reported empty, not omitted, the
            // way `tags` is.
            integration_configuration: self.integration_configuration(),
            capability_configuration: self.capability_configuration(),
            ai_agent_configuration: Some(self.ai_agent_configuration.clone()),
            orchestrator_configuration_list: Some(self.orchestrator_configuration_list.clone()),
        }
    }

    /// The `AssistantSummary` `ListAssistants` reports for this assistant — the same
    /// stored values `GetAssistant` answers with.
    pub fn to_summary(&self) -> AssistantSummary {
        AssistantSummary {
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            name: self.name.clone(),
            r#type: self.r#type.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            tags: tags_out(&self.tags),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            // The same record, rendered the same way: a summary reports what
            // `GetAssistant` reports for the assistant it summarises.
            integration_configuration: self.integration_configuration(),
            capability_configuration: self.capability_configuration(),
            ai_agent_configuration: Some(self.ai_agent_configuration.clone()),
            orchestrator_configuration_list: Some(self.orchestrator_configuration_list.clone()),
        }
    }

    /// A session id no session of this assistant holds, or `None` when
    /// [`MINT_ATTEMPTS`] candidates of this sequence number's block were all taken.
    pub fn mint_session_id(&self, seq: u64) -> Option<String> {
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.sessions.contains_key(candidate))
    }
}

/// One knowledge base, as this emulator persists it.
///
/// The knowledge base is the root of the content/quick-response/message-template side of
/// the service, the way [`Assistant`] is the root of the session side.
#[derive(Debug, Clone)]
pub struct KnowledgeBase {
    pub knowledge_base_id: String,
    pub knowledge_base_arn: String,
    pub name: String,
    pub knowledge_base_type: String,
    pub status: String,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub source_configuration: Option<SourceConfiguration>,
    pub rendering_configuration: Option<RenderingConfiguration>,
    pub vector_ingestion_configuration: Option<VectorIngestionConfiguration>,
    pub server_side_encryption_configuration: Option<ServerSideEncryptionConfiguration>,
    /// When the create that minted this record ran, as [`Assistant`] and [`Session`]
    /// keep it too. `KnowledgeBaseData` models no creation-time member, so nothing
    /// reports it yet; it is the record's own age, not a reported value.
    pub created_at: SystemTime,
}

impl KnowledgeBase {
    /// The `KnowledgeBaseData` every operation that reports this knowledge base answers
    /// with.
    pub fn to_knowledge_base_data(&self) -> KnowledgeBaseData {
        KnowledgeBaseData {
            knowledge_base_id: self.knowledge_base_id.clone(),
            knowledge_base_arn: self.knowledge_base_arn.clone(),
            name: self.name.clone(),
            knowledge_base_type: self.knowledge_base_type.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            tags: tags_out(&self.tags),
            source_configuration: self.source_configuration.clone(),
            rendering_configuration: self.rendering_configuration.clone(),
            vector_ingestion_configuration: self.vector_ingestion_configuration.clone(),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            // The model's own wording: "If no content exists in a knowledge base, this
            // value is unset". No content operation is implemented yet, so no knowledge
            // base this service holds has content to have modified.
            last_content_modification_time: None,
            // Ingestion runs against a data source, and nothing in this emulator ingests
            // one, so there is no sync status or failure to report rather than an
            // invented one.
            ingestion_status: None,
            ingestion_failure_reasons: None,
        }
    }
}

/// What one assistant association associates the assistant WITH: the modelled
/// `AssistantAssociationInputData` union, as this emulator persists it.
///
/// The variant is the one the create named, and it is stored RESOLVED: a knowledge base
/// of this account is kept by id together with the ARN it carried when the association
/// was made, so a later read of the association reports the knowledge base it was made
/// against even once that knowledge base is gone (`DeleteKnowledgeBase` does not refuse
/// over a live association).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssociationTarget {
    /// A knowledge base this account holds — `associationType` `KNOWLEDGE_BASE`.
    KnowledgeBase {
        knowledge_base_id: String,
        knowledge_base_arn: String,
    },
    /// A Bedrock knowledge base outside this service — `associationType`
    /// `EXTERNAL_BEDROCK_KNOWLEDGE_BASE`. Both ARNs name resources of OTHER services
    /// (Bedrock, IAM), which this emulator does not hold, so they are stored as the
    /// caller gave them; their form is already refused by the modelled `@pattern`/
    /// `@length` of `ExternalBedrockKnowledgeBaseConfig` before a handler runs.
    ExternalBedrockKnowledgeBase {
        bedrock_knowledge_base_arn: String,
        access_role_arn: String,
    },
}

impl AssociationTarget {
    /// This target as the modelled `AssistantAssociationOutputData` union every
    /// operation that reports an association answers with.
    fn to_output_data(&self) -> AssistantAssociationOutputData {
        match self {
            Self::KnowledgeBase {
                knowledge_base_id,
                knowledge_base_arn,
            } => AssistantAssociationOutputData::knowledgeBaseAssociation(
                KnowledgeBaseAssociationData {
                    knowledge_base_id: Some(knowledge_base_id.clone()),
                    knowledge_base_arn: Some(knowledge_base_arn.clone()),
                },
            ),
            Self::ExternalBedrockKnowledgeBase {
                bedrock_knowledge_base_arn,
                access_role_arn,
            } => AssistantAssociationOutputData::externalBedrockKnowledgeBaseConfig(
                ExternalBedrockKnowledgeBaseConfig {
                    bedrock_knowledge_base_arn: bedrock_knowledge_base_arn.clone(),
                    access_role_arn: access_role_arn.clone(),
                },
            ),
        }
    }
}

/// One association between an assistant and the data it answers out of, as this
/// emulator persists it.
///
/// An association is reachable only through its assistant — the modelled routes are all
/// `/assistants/{assistantId}/associations/…` — so [`Self::assistant_id`] is what makes
/// a lookup under another assistant miss.
#[derive(Debug, Clone)]
pub struct AssistantAssociation {
    pub assistant_association_id: String,
    pub assistant_association_arn: String,
    pub assistant_id: String,
    pub assistant_arn: String,
    pub association_type: String,
    pub target: AssociationTarget,
    pub tags: HashMap<String, String>,
    /// The fingerprint of the create that minted this record — see
    /// [`association_fingerprint`]. What a replayed `@idempotencyToken` is compared
    /// against, so a token reused with different parameters is told it conflicts instead
    /// of being handed a resource it did not ask for.
    pub fingerprint: String,
    /// When the create that minted this record ran, as every other record here keeps it.
    /// `AssistantAssociationData` models no creation-time member, so nothing reports it.
    pub created_at: SystemTime,
}

impl AssistantAssociation {
    /// The `AssistantAssociationData` every operation that reports this association
    /// answers with.
    pub fn to_assistant_association_data(&self) -> AssistantAssociationData {
        AssistantAssociationData {
            assistant_association_id: self.assistant_association_id.clone(),
            assistant_association_arn: self.assistant_association_arn.clone(),
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            association_type: self.association_type.clone(),
            association_data: self.target.to_output_data(),
            tags: tags_out(&self.tags),
        }
    }
}

/// The fingerprint of a `CreateAssistantAssociation` request: what its
/// `@idempotencyToken` identifies.
///
/// An idempotency token identifies ONE request, so a replay carrying the identical
/// request is a retry and answers with the resource the first call created, while the
/// same token carrying a DIFFERENT request is an idempotency conflict and is refused.
/// The fingerprint therefore covers every member the created record is made of — the
/// association type, the association target ([`input_target_key`]) and the tags — and
/// nothing else; the tag pairs are sorted, so the same request written in another order
/// is one request.
pub fn association_fingerprint(
    association_type: &str,
    target: &str,
    tags: &HashMap<String, String>,
) -> String {
    let mut pairs: Vec<String> = tags.iter().map(|(key, tag)| format!("{key}={tag}")).collect();
    pairs.sort();
    format!(
        "{association_type}\u{1f}{target}\u{1f}{}",
        pairs.join("\u{1e}")
    )
}

/// What the association target a request names comes down to: the member of the
/// `AssistantAssociationInputData` union it carries, and the values that identify the
/// resource under it.
///
/// Taken from the REQUEST, so [`association_fingerprint`] can be computed before the
/// target is resolved: a retry whose token is known is answered out of the record the
/// first call made even when the resource it named has since been deleted.
pub fn input_target_key(association: &AssistantAssociationInputData) -> String {
    match association {
        AssistantAssociationInputData::knowledgeBaseId(knowledge_base_id) => {
            format!("knowledgeBase={knowledge_base_id}")
        }
        AssistantAssociationInputData::externalBedrockKnowledgeBaseConfig(config) => format!(
            "externalBedrock={}|{}",
            config.bedrock_knowledge_base_arn, config.access_role_arn
        ),
    }
}

/// Per-service state for com-amazonaws-qconnect-wisdomservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsQconnectWisdomserviceState {
    /// Every live assistant, keyed by its `assistantId` — the service's primary index.
    pub assistants: HashMap<String, Assistant>,
    /// Every live knowledge base, keyed by its `knowledgeBaseId`.
    pub knowledge_bases: HashMap<String, KnowledgeBase>,
    /// Every live assistant association, keyed by its `assistantAssociationId`. Each
    /// record names the assistant it belongs to, so a lookup is always scoped to the
    /// parent the route named.
    pub assistant_associations: HashMap<String, AssistantAssociation>,
    /// `name` -> `knowledgeBaseId`: a knowledge base's name identifies it, so a second
    /// create under a live name is the conflict `ConflictException` describes.
    pub knowledge_base_name_index: HashMap<String, String>,
    /// `name` -> `assistantId`: an assistant's name identifies it, so a second create
    /// under a live name is the conflict `ConflictException` describes.
    pub assistant_name_index: HashMap<String, String>,
    /// `clientToken` -> the id of the resource that token created, so a replayed
    /// `@idempotencyToken` answers with the resource it already made instead of
    /// creating a second one. Keyed through [`idempotency_key`], so one token used on
    /// two different creates does not collide.
    pub idempotency: HashMap<String, String>,
    /// The id mint's sequence: every mint takes the next number, so no two minted ids
    /// can be the same candidate.
    next_id_seq: u64,
}

impl ComAmazonawsQconnectWisdomserviceState {
    /// The next sequence number for the id mint. Hand it to [`Self::mint_assistant_id`]
    /// or [`Assistant::mint_session_id`]; each call owns its own block of candidates.
    pub fn next_seq(&mut self) -> u64 {
        self.next_id_seq = self.next_id_seq.wrapping_add(1);
        self.next_id_seq
    }

    /// An assistant id no stored assistant holds, or `None` when [`MINT_ATTEMPTS`]
    /// candidates of this sequence number's block were all taken.
    pub fn mint_assistant_id(&mut self) -> Option<String> {
        let seq = self.next_seq();
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.assistants.contains_key(candidate))
    }

    /// The default AI agents an assistant comes with, and the bindings that name them:
    /// one `SYSTEM`-origin agent per [`DEFAULT_AI_AGENT_TYPES`] entry, which is what the
    /// service provisions when it creates an assistant. Returned rather than stored, so
    /// the create puts them on the record it inserts.
    pub fn default_ai_agents(
        &mut self,
        region: &str,
        assistant_id: &str,
    ) -> (
        HashMap<String, AiAgent>,
        HashMap<String, AIAgentConfigurationData>,
    ) {
        let created_at = SystemTime::now();
        let mut agents: HashMap<String, AiAgent> = HashMap::new();
        let mut bindings: HashMap<String, AIAgentConfigurationData> = HashMap::new();
        for agent_type in DEFAULT_AI_AGENT_TYPES {
            let seq = self.next_seq();
            let Some(ai_agent_id) = (0..MINT_ATTEMPTS as u64)
                .map(|attempt| mint_candidate(seq, attempt))
                .find(|candidate| !agents.contains_key(candidate))
            else {
                // Every candidate of this block was taken by an agent minted a moment
                // ago: the assistant gets no default agent of this type rather than a
                // second record under one id.
                continue;
            };
            agents.insert(
                ai_agent_id.clone(),
                AiAgent {
                    ai_agent_arn: ai_agent_arn(region, assistant_id, &ai_agent_id),
                    ai_agent_id: ai_agent_id.clone(),
                    assistant_id: assistant_id.to_string(),
                    name: agent_type.to_string(),
                    r#type: agent_type.to_string(),
                    origin: AI_AGENT_ORIGIN_SYSTEM.to_string(),
                    visibility_status: AI_AGENT_VISIBILITY_PUBLISHED.to_string(),
                    status: AI_AGENT_STATUS_ACTIVE.to_string(),
                    created_at,
                },
            );
            bindings.insert(
                agent_type.to_string(),
                AIAgentConfigurationData {
                    // The binding names the stored agent by its id — the model's own
                    // wording for the member is "The ID of the AI Agent to be
                    // configured" — so the id this assistant reports is the id of a
                    // record this store holds.
                    ai_agent_id,
                    // The model's own wording: "Indicates whether the AI Agent
                    // configured for this AI Agent type is enabled … Setting this value
                    // to false is currently supported only for the
                    // `ANSWER_RECOMMENDATION` AI Agent type" — see
                    // [`AI_AGENT_TYPE_ANSWER_RECOMMENDATION`] for why that one default
                    // is bound disabled and why no other type reports the member.
                    enabled: (agent_type == AI_AGENT_TYPE_ANSWER_RECOMMENDATION)
                        .then_some(false),
                },
            );
        }
        (agents, bindings)
    }

    /// A knowledge-base id no stored knowledge base holds, or `None` when
    /// [`MINT_ATTEMPTS`] candidates of this sequence number's block were all taken.
    pub fn mint_knowledge_base_id(&mut self) -> Option<String> {
        let seq = self.next_seq();
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.knowledge_bases.contains_key(candidate))
    }

    /// Store a knowledge base and index it by name.
    pub fn insert_knowledge_base(&mut self, knowledge_base: KnowledgeBase) {
        self.knowledge_base_name_index.insert(
            knowledge_base.name.clone(),
            knowledge_base.knowledge_base_id.clone(),
        );
        self.knowledge_bases
            .insert(knowledge_base.knowledge_base_id.clone(), knowledge_base);
    }

    /// The knowledge base a `UuidOrArn` label names, or `None` when this account holds
    /// none under that label — including an ARN this emulator could not have minted,
    /// which names nothing.
    pub fn knowledge_base(&self, label: &str) -> Option<&KnowledgeBase> {
        self.knowledge_bases.get(resolve_id(label))
    }

    /// Forget a knowledge base and every index entry that named it, as
    /// `DeleteKnowledgeBase` does. `None` — nothing removed — when this account holds no
    /// knowledge base under that label, which is the caller's not-found.
    ///
    /// The name the record held is released by the same call, so a create may take it
    /// again: the conflict a duplicate name is refused with is a conflict with a LIVE
    /// knowledge base. The `clientToken` entries that created it go too — a token whose
    /// resource is gone must not resurrect it on a replay — exactly as
    /// [`Self::remove_assistant`] does for an assistant.
    pub fn remove_knowledge_base(&mut self, label: &str) -> Option<KnowledgeBase> {
        let knowledge_base = self.knowledge_bases.remove(resolve_id(label))?;
        self.knowledge_base_name_index.remove(&knowledge_base.name);
        let gone: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, created)| *created == &knowledge_base.knowledge_base_id)
            .map(|(token, _)| token.clone())
            .collect();
        for token in gone {
            self.idempotency.remove(&token);
        }
        Some(knowledge_base)
    }

    /// The id of the live knowledge base already using this name, if any.
    pub fn knowledge_base_id_with_name(&self, name: &str) -> Option<&str> {
        self.knowledge_base_name_index
            .get(name)
            .map(String::as_str)
            .filter(|id| self.knowledge_bases.contains_key(*id))
    }

    /// Store an assistant and index it by name.
    pub fn insert_assistant(&mut self, assistant: Assistant) {
        self.assistant_name_index
            .insert(assistant.name.clone(), assistant.assistant_id.clone());
        self.assistants
            .insert(assistant.assistant_id.clone(), assistant);
    }

    /// The assistant a `UuidOrArn` label names, or `None` when this account holds none
    /// under that label — including an ARN this emulator could not have minted, which
    /// names nothing.
    pub fn assistant(&self, label: &str) -> Option<&Assistant> {
        self.assistants.get(resolve_id(label))
    }

    /// The assistant a `UuidOrArn` label names, for mutation.
    pub fn assistant_mut(&mut self, label: &str) -> Option<&mut Assistant> {
        self.assistants.get_mut(resolve_id(label))
    }

    /// Forget an assistant, its sessions, its associations and every index entry that
    /// named them, as `DeleteAssistant` does.
    pub fn remove_assistant(&mut self, label: &str) -> Option<Assistant> {
        let assistant = self.assistants.remove(resolve_id(label))?;
        self.assistant_name_index.remove(&assistant.name);
        // An association exists only inside its assistant, so it goes with it.
        let associations: Vec<String> = self
            .assistant_associations
            .iter()
            .filter(|(_, association)| association.assistant_id == assistant.assistant_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &associations {
            self.assistant_associations.remove(id);
        }
        // A token whose resource is gone must not resurrect it on a replay.
        let gone: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, created)| {
                *created == &assistant.assistant_id
                    || assistant.sessions.contains_key(created.as_str())
                    || associations.contains(*created)
            })
            .map(|(token, _)| token.clone())
            .collect();
        for token in gone {
            self.idempotency.remove(&token);
        }
        Some(assistant)
    }

    /// The id of the live assistant already using this name, if any.
    pub fn assistant_id_with_name(&self, name: &str) -> Option<&str> {
        self.assistant_name_index
            .get(name)
            .map(String::as_str)
            .filter(|id| self.assistants.contains_key(*id))
    }

    /// Every stored assistant's id, in `assistantId` order — the stable order
    /// `ListAssistants` pages over. Ids only, so a list call projects just the page it
    /// answers with instead of cloning every record (and its sessions).
    pub fn assistant_ids_in_order(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.assistants.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// An assistant-association id no stored association holds, or `None` when
    /// [`MINT_ATTEMPTS`] candidates of this sequence number's block were all taken.
    pub fn mint_assistant_association_id(&mut self) -> Option<String> {
        let seq = self.next_seq();
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.assistant_associations.contains_key(candidate))
    }

    /// Store an assistant association under the assistant it names.
    pub fn insert_assistant_association(&mut self, association: AssistantAssociation) {
        self.assistant_associations
            .insert(association.assistant_association_id.clone(), association);
    }

    /// The association a (resolved `assistantId`, `UuidOrArn` label) pair names, or
    /// `None` when this assistant holds none under that label — including an id that
    /// names an association of ANOTHER assistant, which this assistant does not hold.
    pub fn assistant_association(
        &self,
        assistant_id: &str,
        label: &str,
    ) -> Option<&AssistantAssociation> {
        self.assistant_associations
            .get(resolve_id(label))
            .filter(|association| association.assistant_id == assistant_id)
    }

    /// Every association of one assistant, in `assistantAssociationId` order, so what a
    /// read reports does not depend on map iteration order.
    pub fn assistant_associations_of(&self, assistant_id: &str) -> Vec<&AssistantAssociation> {
        let mut associations: Vec<&AssistantAssociation> = self
            .assistant_associations
            .values()
            .filter(|association| association.assistant_id == assistant_id)
            .collect();
        associations.sort_by(|left, right| {
            left.assistant_association_id
                .cmp(&right.assistant_association_id)
        });
        associations
    }

    /// Forget one association, as `DeleteAssistantAssociation` does, together with the
    /// `clientToken` entry that created it: a token whose resource is gone must not
    /// resurrect it on a replay.
    pub fn remove_assistant_association(
        &mut self,
        assistant_id: &str,
        label: &str,
    ) -> Option<AssistantAssociation> {
        let id = self.assistant_association(assistant_id, label)?.assistant_association_id.clone();
        let association = self.assistant_associations.remove(&id)?;
        let gone: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, created)| *created == &id)
            .map(|(token, _)| token.clone())
            .collect();
        for token in gone {
            self.idempotency.remove(&token);
        }
        Some(association)
    }
}

/// The key a `clientToken` is remembered under: the token scoped to the call it was
/// given to (and, for a nested resource, to its parent), so the same token handed to two
/// different creates identifies two different resources rather than colliding.
pub fn idempotency_key(scope: &str, token: &str) -> String {
    format!("{scope}\u{1f}{token}")
}

/// What a pagination cursor this service minted carries, so a token it did not mint is
/// refused rather than read as some position by accident.
const CURSOR_SCOPE: &str = "qconnect-assistants\u{1f}";

/// The pagination cursor for a page that starts at `id`.
///
/// Hex of a scoped key rather than the key itself, so a caller cannot read structure
/// into the token and has nothing to build one from: a `nextToken` is opaque and only
/// ever travels back unchanged. Well inside the modelled `NextToken` length (1..2048).
pub fn encode_cursor(id: &str) -> String {
    ::hex::encode(format!("{CURSOR_SCOPE}{id}").as_bytes())
}

/// The inclusive start key a cursor minted by [`encode_cursor`] carries, or `None` when
/// the token was not minted by this service.
pub fn decode_cursor(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(CURSOR_SCOPE).map(str::to_string)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsQconnectWisdomserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsQconnectWisdomserviceState>)
}
