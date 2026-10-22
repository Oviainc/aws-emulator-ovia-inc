// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-qconnect-wisdomservice service.
//!
//! The service's root resource is the Amazon Q in Connect *assistant*; a session is
//! created inside one assistant and is reachable only through its parent. Everything a
//! handler answers with is derived from the records kept here, so what one request
//! writes is what the next request reads.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    AIAgentConfigurationData, AssistantData, AssistantSummary, ServerSideEncryptionConfiguration,
    SessionData, TagFilter,
};

/// The only `AssistantType` the model declares
/// (`smithy_query(query="shape",shape="AssistantType")`).
pub const ASSISTANT_TYPE_AGENT: &str = "AGENT";

/// The `AssistantStatus` a freshly created assistant carries: creation is asynchronous,
/// which is why the modelled enum has a `CREATE_IN_PROGRESS` member at all
/// (`smithy_query(query="shape",shape="AssistantStatus")`).
pub const ASSISTANT_STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// The terminal `AssistantStatus` of a usable assistant.
pub const ASSISTANT_STATUS_ACTIVE: &str = "ACTIVE";

/// The `Origin` of a session created through the `CreateSession` API — the model's own
/// wording: "`SYSTEM` for a default Session created by Amazon Q in Connect or
/// `CUSTOMER` for a Session created by calling CreateSession API".
pub const SESSION_ORIGIN_CUSTOMER: &str = "CUSTOMER";

/// How long a freshly created assistant stays `CREATE_IN_PROGRESS` before the clock
/// owes it `ACTIVE`.
///
/// CHOSEN, not documented: AWS publishes no provisioning duration for an assistant. The
/// span is deliberately non-zero, so a caller reading straight after the create still
/// observes the transient status and only a later read sees the assistant settled.
pub const CREATE_TRANSITION: Duration = Duration::from_millis(500);

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

/// This service's ARN for a session, which names its parent assistant too:
/// `arn:aws:wisdom:<region>:<account>:session/<assistantId>/<sessionId>`.
pub fn session_arn(region: &str, assistant_id: &str, session_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("session/{assistant_id}/{session_id}"),
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

/// A map field rendered as the modelled map shape, or `None` when it is empty: an
/// untagged resource reports no `tags` member rather than an empty object.
fn map_out<V: Clone>(map: &HashMap<String, V>) -> Option<HashMap<String, V>> {
    (!map.is_empty()).then(|| map.clone())
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
            tags: map_out(&self.tags),
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
    /// The instant the `CREATE_IN_PROGRESS` -> `ACTIVE` transition falls due.
    pub active_due_at: Instant,
    /// This assistant's sessions, keyed by `sessionId`. A session is reachable only
    /// through its parent, so a session id does not resolve under another assistant.
    pub sessions: HashMap<String, Session>,
}

impl Assistant {
    /// Materialise the status change the clock already owes this assistant. A read
    /// calls this because the transition was due at that instant whether or not
    /// anybody asked.
    fn settle(&mut self, now: Instant) {
        if self.status == ASSISTANT_STATUS_CREATE_IN_PROGRESS && now >= self.active_due_at {
            self.status = ASSISTANT_STATUS_ACTIVE.to_string();
        }
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
            tags: map_out(&self.tags),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            // No topic integration, no capability override and no AI agent is bound to
            // an assistant by the operations implemented so far, so each of these
            // reports nothing rather than an invented value.
            integration_configuration: None,
            capability_configuration: None,
            ai_agent_configuration: None,
            orchestrator_configuration_list: None,
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
            tags: map_out(&self.tags),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            integration_configuration: None,
            capability_configuration: None,
            ai_agent_configuration: None,
            orchestrator_configuration_list: None,
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

/// Per-service state for com-amazonaws-qconnect-wisdomservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsQconnectWisdomserviceState {
    /// Every live assistant, keyed by its `assistantId` — the service's primary index.
    pub assistants: HashMap<String, Assistant>,
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

    /// Store an assistant and index it by name.
    pub fn insert_assistant(&mut self, assistant: Assistant) {
        self.assistant_name_index
            .insert(assistant.name.clone(), assistant.assistant_id.clone());
        self.assistants
            .insert(assistant.assistant_id.clone(), assistant);
    }

    /// The assistant a `UuidOrArn` label names, with any clock-owed status change
    /// materialised.
    pub fn assistant(&mut self, label: &str) -> Option<&Assistant> {
        self.assistant_mut(label).map(|assistant| &*assistant)
    }

    /// The assistant a `UuidOrArn` label names, for mutation, with any clock-owed
    /// status change materialised first.
    pub fn assistant_mut(&mut self, label: &str) -> Option<&mut Assistant> {
        let assistant = self.assistants.get_mut(resolve_id(label))?;
        assistant.settle(Instant::now());
        Some(assistant)
    }

    /// Forget an assistant, its sessions and every index entry that named them, as
    /// `DeleteAssistant` does.
    pub fn remove_assistant(&mut self, label: &str) -> Option<Assistant> {
        let assistant = self.assistants.remove(resolve_id(label))?;
        self.assistant_name_index.remove(&assistant.name);
        // A token whose resource is gone must not resurrect it on a replay.
        let gone: Vec<String> = self
            .idempotency
            .iter()
            .filter(|(_, created)| {
                *created == &assistant.assistant_id
                    || assistant.sessions.contains_key(created.as_str())
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

    /// Every stored assistant's id, settled and in `assistantId` order — the stable
    /// order `ListAssistants` pages over. Ids only, so a list call projects just the
    /// page it answers with instead of cloning every record (and its sessions).
    pub fn assistant_ids_in_order(&mut self) -> Vec<String> {
        let now = Instant::now();
        let mut ids: Vec<String> = self
            .assistants
            .values_mut()
            .map(|assistant| {
                assistant.settle(now);
                assistant.assistant_id.clone()
            })
            .collect();
        ids.sort();
        ids
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
