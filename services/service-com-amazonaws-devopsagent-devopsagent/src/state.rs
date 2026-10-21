// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-devopsagent-devopsagent service.
//!
//! The records here are the single home for every resource this service persists:
//! an AgentSpace (the control-plane resource `CreateAgentSpace` mints) and the
//! backlog tasks that live inside one. Handlers read and write them through
//! [`service_state`]; the `to_api` methods are the one place a record is rendered
//! into the modelled wire shape, so `Create`, `Get` and `List` cannot disagree
//! about what a stored resource looks like.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{AgentSpace, ReferenceOutput, Task};

/// The most AgentSpaces this emulator stores for the account before
/// `CreateAgentSpace` answers its modelled `ServiceQuotaExceededException`.
///
/// The real service's AgentSpace quota is not stated in the model, so this bound is
/// CHOSEN, not cited: far above any plausible client's count, so normal use never
/// meets it, while the store a caller can grow stays bounded.
pub const MAX_AGENT_SPACES: usize = 1_000;

/// The most backlog tasks one AgentSpace holds before `CreateBacklogTask` answers
/// `ServiceQuotaExceededException`. CHOSEN for the same reason as
/// [`MAX_AGENT_SPACES`]: the model states no task quota.
pub const MAX_TASKS_PER_AGENT_SPACE: usize = 1_000;

/// How long a recorded `@idempotencyToken` replays the result of the create that
/// first used it.
///
/// Cited: `CreateAgentSpaceInput$clientToken`'s own documentation — "Client-provided
/// token to ensure request idempotency. When the same token is provided in
/// subsequent calls, the same response is returned within a 8-hour window." Past
/// the window the entry is expired and pruned, so the token map does not grow for
/// the life of the process.
pub const CLIENT_TOKEN_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// The most idempotency-token entries kept at once. CHOSEN (the model states no
/// such limit) as a hard ceiling on top of [`CLIENT_TOKEN_TTL`]: a create prunes
/// expired entries first and evicts the oldest remaining one if the cap is still
/// met, so a caller cannot grow the map without end inside one TTL window.
pub const MAX_CLIENT_TOKENS: usize = 10_000;

/// The recognised agent-space preference keys.
///
/// Source: the value set of the model's `AgentSpacePreferenceKey` enum —
/// `smithy_query(query="shape", shape="AgentSpacePreferenceKey")`. The
/// `AgentSpacePreferences` map documents that "each key must be a recognized
/// preference key; unrecognized keys are rejected", and the framework's
/// constraint walk does not validate map KEYS, so the handler enforces it.
pub const AGENT_SPACE_PREFERENCE_KEYS: &[&str] = &["elevatedActionsEnabled"];

/// Convert a stored instant into the modelled `Timestamp` representation.
///
/// Every timestamp a response carries is derived from the stored instant, so two
/// reads of one resource render byte-identical values.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// A persisted AgentSpace and the backlog tasks it owns.
#[derive(Debug, Clone)]
pub struct AgentSpaceRecord {
    pub agent_space_id: String,
    pub name: String,
    pub description: Option<String>,
    pub locale: Option<String>,
    pub kms_key_arn: Option<String>,
    /// Only the preferences the caller configured; an unset preference takes its
    /// documented default and is not materialised here.
    pub preferences: HashMap<String, bool>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub tasks: HashMap<String, TaskRecord>,
}

impl AgentSpaceRecord {
    /// The modelled `AgentSpace` for this record.
    pub fn to_api(&self) -> AgentSpace {
        AgentSpace {
            name: self.name.clone(),
            description: self.description.clone(),
            locale: self.locale.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            kms_key_arn: self.kms_key_arn.clone(),
            agent_space_id: self.agent_space_id.clone(),
            preferences: (!self.preferences.is_empty()).then(|| self.preferences.clone()),
        }
    }

    /// The stored tags, omitted entirely when the resource carries none.
    pub fn tags_to_api(&self) -> Option<HashMap<String, String>> {
        (!self.tags.is_empty()).then(|| self.tags.clone())
    }
}

/// A persisted backlog task.
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub task_id: String,
    pub agent_space_id: String,
    pub title: String,
    pub description: Option<String>,
    pub task_type: String,
    pub priority: String,
    pub status: String,
    pub reference: Option<ReferenceOutput>,
    pub execution_id: Option<String>,
    pub metadata: Option<::aws_smithy_types::Document>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub version: i32,
}

impl TaskRecord {
    /// The modelled `Task` for this record.
    pub fn to_api(&self) -> Task {
        Task {
            agent_space_id: self.agent_space_id.clone(),
            task_id: self.task_id.clone(),
            execution_id: self.execution_id.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            reference: self.reference.clone(),
            task_type: self.task_type.clone(),
            priority: self.priority.clone(),
            status: self.status.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
            version: self.version,
            support_metadata: None,
            metadata: self.metadata.clone(),
            primary_task_id: None,
            status_reason: None,
            has_linked_tasks: false,
        }
    }
}

/// What one recorded idempotency token remembers: the resource the first create
/// made, and when that happened (so the entry can expire).
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    pub resource_id: String,
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-devopsagent-devopsagent. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDevopsagentDevopsagentState {
    /// Every AgentSpace, keyed by its minted `agentSpaceId`. Bounded by
    /// [`MAX_AGENT_SPACES`].
    pub agent_spaces: HashMap<String, AgentSpaceRecord>,
    /// `clientToken` -> the resource the first call with that token created, so an
    /// `@idempotent` create replays its first result. Entries expire after
    /// [`CLIENT_TOKEN_TTL`] and the map is capped at [`MAX_CLIENT_TOKENS`].
    pub client_tokens: HashMap<String, ClientTokenRecord>,
    /// Source of minted resource ids; monotonic, so no id is ever reused.
    pub next_id: u64,
}

impl ComAmazonawsDevopsagentDevopsagentState {
    /// Mint a fresh resource id of the form `<prefix>-<hex sequence>`.
    ///
    /// The sequence is monotonic within this emulator, so an id is never handed
    /// out twice. `prefix` must be alphanumeric so the result satisfies both
    /// `AgentSpaceId` (`^[a-zA-Z0-9-]{1,64}$`) and `ResourceId`
    /// (`^[a-zA-Z0-9_.-]+$`, max 128).
    pub fn mint_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{:016x}", self.next_id)
    }

    /// The id a previous `@idempotent` create recorded for `client_token`, when
    /// the entry is still inside [`CLIENT_TOKEN_TTL`] at `now`.
    pub fn replayed_id(&self, scope: &str, client_token: &str, now: SystemTime) -> Option<String> {
        let record = self.client_tokens.get(&token_key(scope, client_token))?;
        (now < record.recorded_at + CLIENT_TOKEN_TTL).then(|| record.resource_id.clone())
    }

    /// Record that `client_token` created `resource_id`.
    ///
    /// Prunes the entries whose TTL the clock has already run out before
    /// inserting, and evicts the oldest remaining entry while the map is still at
    /// [`MAX_CLIENT_TOKENS`], so this map is bounded however many creates arrive.
    pub fn record_client_token(
        &mut self,
        scope: &str,
        client_token: &str,
        resource_id: &str,
        now: SystemTime,
    ) {
        self.client_tokens
            .retain(|_, record| now < record.recorded_at + CLIENT_TOKEN_TTL);
        while self.client_tokens.len() >= MAX_CLIENT_TOKENS {
            let oldest = self
                .client_tokens
                .iter()
                .min_by_key(|(_, record)| record.recorded_at)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.client_tokens.remove(&key);
                }
                None => break,
            }
        }
        self.client_tokens.insert(
            token_key(scope, client_token),
            ClientTokenRecord {
                resource_id: resource_id.to_string(),
                recorded_at: now,
            },
        );
    }

    /// Drop the idempotency-token entries that name any of `resource_ids`.
    ///
    /// Called when those resources are deleted: a token whose resource is gone can
    /// never replay anything, so the entry is reclaimed with the resource instead
    /// of waiting out its TTL.
    pub fn forget_client_tokens_for(&mut self, resource_ids: &[String]) {
        self.client_tokens
            .retain(|_, record| !resource_ids.contains(&record.resource_id));
    }

    /// The stored AgentSpaces in a stable order: by `agentSpaceId`.
    ///
    /// Ids come from the monotonic [`mint_id`](Self::mint_id) sequence and are
    /// fixed-width, so ordering by id IS creation order, and an opaque pagination
    /// token can resume at a position in this sequence by naming the id it stopped
    /// after.
    pub fn agent_spaces_ordered(&self) -> Vec<&AgentSpaceRecord> {
        let mut ordered: Vec<&AgentSpaceRecord> = self.agent_spaces.values().collect();
        ordered.sort_by(|a, b| a.agent_space_id.cmp(&b.agent_space_id));
        ordered
    }
}

/// Idempotency tokens are scoped per create operation, so the same token used on
/// two different creates names two different resources.
fn token_key(scope: &str, client_token: &str) -> String {
    format!("{scope}#{client_token}")
}

/// The prefix every pagination token this service issues carries, so a token it
/// did not issue is rejected instead of being read as a resume point.
const PAGE_TOKEN_PREFIX: &str = "v1:";

/// The prefix of a minted `agentSpaceId`, and the hex width of its sequence.
const AGENT_SPACE_ID_PREFIX: &str = "as-";
const MINTED_SEQUENCE_WIDTH: usize = 16;

/// Encode an opaque pagination token that resumes AFTER `last_id`.
///
/// Opaque to the client (a version marker plus hex of the cursor), and it
/// round-trips through [`decode_next_token`].
pub fn encode_next_token(last_id: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", hex::encode(last_id.as_bytes()))
}

/// The cursor a token carries, or `None` when the token is not one this service
/// issued.
///
/// A cursor this service issued always names a minted `agentSpaceId`, so a token
/// that decodes to anything else is rejected rather than read as a resume point
/// past every stored agent space.
pub fn decode_next_token(token: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let cursor = String::from_utf8(hex::decode(body).ok()?).ok()?;
    let sequence = cursor.strip_prefix(AGENT_SPACE_ID_PREFIX)?;
    let names_an_agent_space = sequence.len() == MINTED_SEQUENCE_WIDTH
        && sequence.chars().all(|c| c.is_ascii_hexdigit());
    names_an_agent_space.then_some(cursor)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDevopsagentDevopsagentState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDevopsagentDevopsagentState>)
}
