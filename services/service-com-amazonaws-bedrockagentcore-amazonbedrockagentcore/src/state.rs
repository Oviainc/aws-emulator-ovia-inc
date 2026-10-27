// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockagentcore-amazonbedrockagentcore service.
//!
//! The AgentCore Memory *event* data plane is stored here: a memory container holds
//! actors, an actor holds sessions, and a session holds its events in the order they
//! were created. Every one of `CreateEvent` / `ListSessions` / `ListEvents` /
//! `GetEvent` / `DeleteEvent` computes its answer from these maps; nothing is canned.
//!
//! The AgentCore *tools* data plane keeps its own map: a code interpreter holds the
//! sessions `StartCodeInterpreterSession` started on it, and
//! `GetCodeInterpreterSession` reports one of them back — its status read off the
//! clock against the time-to-live the request asked for.
//!
//! # The store has an explicit ceiling
//!
//! `CreateEvent` is the only growth site, and a caller chooses every key it inserts
//! under (`memoryId`, `actorId`, `sessionId`, `clientToken`), so request volume alone
//! must not be able to grow the process heap without limit. Each level therefore
//! carries a maximum and evicts its least recently written record once that maximum is
//! passed ([`MAX_MEMORIES`], [`MAX_ACTORS_PER_MEMORY`], [`MAX_SESSIONS_PER_ACTOR`],
//! [`MAX_EVENTS_PER_SESSION`], [`MAX_IDEMPOTENCY_RECORDS_PER_MEMORY`]), and the
//! payload-carrying records are additionally bounded ACROSS the whole store by
//! [`MAX_STORED_EVENTS`], a ring over every stored event in insertion order. So the
//! store holds at most `MAX_STORED_EVENTS` events (the only records whose size a
//! caller influences, each itself bounded by the modelled `payload` <= 100 items and
//! `metadata` <= 15 entries) and at most
//! `MAX_MEMORIES * MAX_ACTORS_PER_MEMORY * MAX_SESSIONS_PER_ACTOR` session records,
//! whatever a caller sends. AWS publishes no retention for any of this, so these are
//! the EMULATOR's own bounds, set far above any test or client working set.
//!
//! `StartCodeInterpreterSession` is the tools plane's only growth site, and a caller
//! chooses its `codeInterpreterIdentifier` and `clientToken`, so it is bounded the
//! same way: [`MAX_CODE_INTERPRETERS`], [`MAX_CODE_INTERPRETER_SESSIONS`] and
//! [`MAX_IDEMPOTENCY_RECORDS_PER_CODE_INTERPRETER`], each evicting its least recently
//! written record.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Branch, Certificate, Event, MetadataValue, PayloadType, SessionSummary,
    ToolsFileSystemConfiguration,
};

/// The most memory containers this emulator retains; the least recently written one
/// is evicted once the store passes it.
pub const MAX_MEMORIES: usize = 32;

/// The most actors this emulator retains inside one memory container.
pub const MAX_ACTORS_PER_MEMORY: usize = 32;

/// The most sessions this emulator retains for one actor.
pub const MAX_SESSIONS_PER_ACTOR: usize = 64;

/// The most events this emulator retains in one session. It also bounds the work a
/// single `ListEvents` does: filtering and sorting walk one session's events.
pub const MAX_EVENTS_PER_SESSION: usize = 1_000;

/// The most events this emulator retains across EVERY session, as a ring in
/// insertion order: the oldest event (and the now-empty session, actor and memory it
/// was the last of) is dropped once the store passes it.
pub const MAX_STORED_EVENTS: usize = 10_000;

/// The most `clientToken` records this emulator retains for one memory container.
pub const MAX_IDEMPOTENCY_RECORDS_PER_MEMORY: usize = 1_000;

/// The most code interpreters this emulator retains sessions for; the least recently
/// used one is evicted once the store passes it.
pub const MAX_CODE_INTERPRETERS: usize = 32;

/// The most sessions this emulator retains for one code interpreter.
pub const MAX_CODE_INTERPRETER_SESSIONS: usize = 256;

/// The most `clientToken` records this emulator retains for one code interpreter.
pub const MAX_IDEMPOTENCY_RECORDS_PER_CODE_INTERPRETER: usize = 1_000;

/// `CodeInterpreterSessionStatus` for a session that is still inside its
/// time-to-live, so it can run code (source: smithy_query
/// query="shape" shape="CodeInterpreterSessionStatus", values READY | TERMINATED).
pub const CODE_INTERPRETER_SESSION_READY: &str = "READY";

/// `CodeInterpreterSessionStatus` for a session whose time-to-live has run out, from
/// the same enum.
pub const CODE_INTERPRETER_SESSION_TERMINATED: &str = "TERMINATED";

/// Per-service state for com-amazonaws-bedrockagentcore-amazonbedrockagentcore. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// Memory containers this data plane has observed, keyed by the memory id as
    /// [`normalize_memory_id`] spells it. Bounded by [`MAX_MEMORIES`].
    pub memories: HashMap<String, MemoryStore>,
    /// Every stored event, in insertion order, as a ring bounded by
    /// [`MAX_STORED_EVENTS`]. An entry whose event is already gone (deleted, or
    /// evicted by a per-level bound) is simply skipped when it reaches the front.
    pub order: VecDeque<EventKey>,
    /// Monotonic counter feeding the hex suffix of every generated `eventId` and
    /// `sessionId`, so two records minted inside the same millisecond still get
    /// distinct ids, and ordering the records each bound evicts by.
    pub event_seq: u64,
    /// Code interpreters this data plane has started a session on, keyed by the
    /// `codeInterpreterIdentifier` the caller named. Bounded by
    /// [`MAX_CODE_INTERPRETERS`].
    ///
    /// A code interpreter is registered here by the first `StartCodeInterpreterSession`
    /// that names it, for the same reason a memory container is registered by its first
    /// event: the interpreter itself is created through the CONTROL plane, a different
    /// service.
    pub code_interpreters: HashMap<String, CodeInterpreterStore>,
}

impl ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// The next value of the event sequence. Never returns the same value twice for
    /// one engine, which is what makes a generated `eventId` unique.
    pub fn next_seq(&mut self) -> u64 {
        self.event_seq += 1;
        self.event_seq
    }

    /// The stored event addressed by (memory, actor, session, event), or `None` when
    /// any step of that path is absent.
    pub fn find_event(
        &self,
        memory_id: &str,
        actor_id: &str,
        session_id: &str,
        event_id: &str,
    ) -> Option<&StoredEvent> {
        self.session(memory_id, actor_id, session_id)?
            .events
            .iter()
            .find(|event| event.event_id == event_id)
    }

    /// The stored session addressed by (memory, actor, session).
    pub fn session(
        &self,
        memory_id: &str,
        actor_id: &str,
        session_id: &str,
    ) -> Option<&SessionStore> {
        self.memories
            .get(memory_id)?
            .actors
            .get(actor_id)?
            .sessions
            .get(session_id)
    }

    /// Persist one event — registering its memory, actor and session on the way — and
    /// hold every bound in [the module's ceiling](self#the-store-has-an-explicit-ceiling).
    ///
    /// The memory container is registered here rather than created by its own call
    /// because the container is created through the CONTROL plane, a different service.
    pub fn insert_event(&mut self, event: StoredEvent, client_token: Option<String>) {
        let key = EventKey {
            memory_id: event.memory_id.clone(),
            actor_id: event.actor_id.clone(),
            session_id: event.session_id.clone(),
            event_id: event.event_id.clone(),
        };
        let seq = event.seq;

        let memory = self.memories.entry(key.memory_id.clone()).or_default();
        memory.last_seq = seq;
        if let Some(token) = client_token {
            memory.idempotency.insert(
                token,
                EventLocator {
                    actor_id: key.actor_id.clone(),
                    session_id: key.session_id.clone(),
                    event_id: key.event_id.clone(),
                    seq,
                },
            );
            while memory.idempotency.len() > MAX_IDEMPOTENCY_RECORDS_PER_MEMORY {
                let Some(oldest) =
                    least_recently_written(&memory.idempotency, |locator| locator.seq)
                else {
                    break;
                };
                memory.idempotency.remove(&oldest);
            }
        }

        let actor = memory.actors.entry(key.actor_id.clone()).or_default();
        actor.last_seq = seq;
        let session = actor
            .sessions
            .entry(key.session_id.clone())
            .or_insert_with(|| SessionStore {
                session_id: key.session_id.clone(),
                actor_id: key.actor_id.clone(),
                // The session came into being with its first event, so that event's
                // `eventTimestamp` is the session's `createdAt`.
                created_at: event.event_timestamp,
                last_seq: seq,
                events: VecDeque::new(),
            });
        session.last_seq = seq;
        session.events.push_back(event);
        while session.events.len() > MAX_EVENTS_PER_SESSION {
            session.events.pop_front();
        }

        while actor.sessions.len() > MAX_SESSIONS_PER_ACTOR {
            let Some(oldest) = least_recently_written(&actor.sessions, |session| session.last_seq)
            else {
                break;
            };
            actor.sessions.remove(&oldest);
        }
        while memory.actors.len() > MAX_ACTORS_PER_MEMORY {
            let Some(oldest) = least_recently_written(&memory.actors, |actor| actor.last_seq) else {
                break;
            };
            memory.actors.remove(&oldest);
        }
        while self.memories.len() > MAX_MEMORIES {
            let Some(oldest) = least_recently_written(&self.memories, |memory| memory.last_seq)
            else {
                break;
            };
            self.memories.remove(&oldest);
        }

        self.order.push_back(key);
        while self.order.len() > MAX_STORED_EVENTS {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.evict_event(&oldest);
        }
    }

    /// Drop an evicted event and every container it was the last record of, so a
    /// container cannot outlive the events that created it.
    ///
    /// This is the EVICTION path, not `DeleteEvent`: a caller's delete leaves the
    /// session in place (AWS reports a session until its own cleanup removes an empty
    /// one), while an evicted event leaves nothing behind to report.
    fn evict_event(&mut self, key: &EventKey) {
        let Some(memory) = self.memories.get_mut(&key.memory_id) else {
            return;
        };
        if let Some(actor) = memory.actors.get_mut(&key.actor_id) {
            if let Some(session) = actor.sessions.get_mut(&key.session_id) {
                session.events.retain(|event| event.event_id != key.event_id);
                if session.events.is_empty() {
                    actor.sessions.remove(&key.session_id);
                }
            }
            if actor.sessions.is_empty() {
                memory.actors.remove(&key.actor_id);
            }
        }
        memory
            .idempotency
            .retain(|_, locator| locator.event_id != key.event_id);
        if memory.actors.is_empty() {
            self.memories.remove(&key.memory_id);
        }
    }

    /// The stored code interpreter session addressed by (code interpreter, session),
    /// or `None` when this emulator holds no such session.
    pub fn code_interpreter_session(
        &self,
        code_interpreter_identifier: &str,
        session_id: &str,
    ) -> Option<&CodeInterpreterSessionRecord> {
        self.code_interpreters
            .get(code_interpreter_identifier)?
            .sessions
            .get(session_id)
    }

    /// The session a `clientToken` already started on this code interpreter, or `None`
    /// when that token started none (or the session it started is no longer held).
    pub fn code_interpreter_session_for_token(
        &self,
        code_interpreter_identifier: &str,
        client_token: &str,
    ) -> Option<&CodeInterpreterSessionRecord> {
        let interpreter = self.code_interpreters.get(code_interpreter_identifier)?;
        let session_id = &interpreter.idempotency.get(client_token)?.session_id;
        interpreter.sessions.get(session_id)
    }

    /// Persist one code interpreter session — registering its code interpreter on the
    /// way — and hold every bound in
    /// [the module's ceiling](self#the-store-has-an-explicit-ceiling).
    pub fn insert_code_interpreter_session(
        &mut self,
        session: CodeInterpreterSessionRecord,
        client_token: Option<String>,
    ) {
        let seq = session.seq;
        let session_id = session.session_id.clone();
        let interpreter = self
            .code_interpreters
            .entry(session.code_interpreter_identifier.clone())
            .or_default();
        interpreter.last_seq = seq;
        interpreter.sessions.insert(session_id.clone(), session);
        if let Some(token) = client_token {
            interpreter
                .idempotency
                .insert(token, SessionLocator { session_id, seq });
            while interpreter.idempotency.len() > MAX_IDEMPOTENCY_RECORDS_PER_CODE_INTERPRETER {
                let Some(oldest) =
                    least_recently_written(&interpreter.idempotency, |locator| locator.seq)
                else {
                    break;
                };
                interpreter.idempotency.remove(&oldest);
            }
        }

        while interpreter.sessions.len() > MAX_CODE_INTERPRETER_SESSIONS {
            let Some(oldest) = least_recently_written(&interpreter.sessions, |session| session.seq)
            else {
                break;
            };
            interpreter.sessions.remove(&oldest);
            interpreter
                .idempotency
                .retain(|_, locator| locator.session_id != oldest);
        }
        while self.code_interpreters.len() > MAX_CODE_INTERPRETERS {
            let Some(oldest) =
                least_recently_written(&self.code_interpreters, |interpreter| interpreter.last_seq)
            else {
                break;
            };
            self.code_interpreters.remove(&oldest);
        }
    }
}

/// The least recently written entry of `map`, by the sequence `seq_of` reads off a
/// record, with the key breaking a tie so eviction is deterministic.
fn least_recently_written<T>(
    map: &HashMap<String, T>,
    seq_of: impl Fn(&T) -> u64,
) -> Option<String> {
    map.iter()
        .min_by(|left, right| {
            (seq_of(left.1), left.0.as_str()).cmp(&(seq_of(right.1), right.0.as_str()))
        })
        .map(|(key, _)| key.clone())
}

/// Where one stored event lives, as the whole-store ring records it.
#[derive(Debug, Clone)]
pub struct EventKey {
    pub memory_id: String,
    pub actor_id: String,
    pub session_id: String,
    pub event_id: String,
}

/// One AgentCore Memory container's data-plane contents.
#[derive(Debug, Default)]
pub struct MemoryStore {
    /// Actors that have produced at least one event in this memory. Bounded by
    /// [`MAX_ACTORS_PER_MEMORY`].
    pub actors: HashMap<String, ActorStore>,
    /// `clientToken` -> the event that token already created. `CreateEvent` is
    /// `@idempotent`, so a replayed token returns the first event rather than
    /// creating a second one. Bounded by [`MAX_IDEMPOTENCY_RECORDS_PER_MEMORY`], and
    /// a record is dropped with the event it points at.
    pub idempotency: HashMap<String, EventLocator>,
    /// The sequence of the last event written into this container, which is what its
    /// eviction order is read from.
    pub last_seq: u64,
}

/// Where an already-created event lives, so a replayed `clientToken` can find it.
#[derive(Debug, Clone)]
pub struct EventLocator {
    pub actor_id: String,
    pub session_id: String,
    pub event_id: String,
    /// The sequence of the event this record points at: its eviction order.
    pub seq: u64,
}

/// One actor's sessions inside a memory container.
#[derive(Debug, Default)]
pub struct ActorStore {
    /// Bounded by [`MAX_SESSIONS_PER_ACTOR`].
    pub sessions: HashMap<String, SessionStore>,
    /// The sequence of the last event written for this actor: its eviction order.
    pub last_seq: u64,
}

/// One session: its identity, when it came into being, and its events in creation
/// order (`ListEvents` reports them newest-first, which is a view over this order,
/// not the order itself).
#[derive(Debug)]
pub struct SessionStore {
    pub session_id: String,
    pub actor_id: String,
    /// The `eventTimestamp` of the first event created in this session, which is the
    /// instant the session came into existence.
    pub created_at: ::aws_smithy_types::DateTime,
    /// The sequence of the last event written into this session: its eviction order.
    pub last_seq: u64,
    /// Creation-ordered, and bounded by [`MAX_EVENTS_PER_SESSION`]: the oldest event
    /// leaves the front once the session passes that bound.
    pub events: VecDeque<StoredEvent>,
}

impl SessionStore {
    /// This session as a `SessionSummary`, the shape `ListSessions` reports.
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            session_id: self.session_id.clone(),
            actor_id: self.actor_id.clone(),
            created_at: self.created_at,
        }
    }
}

/// A stored event: exactly what `CreateEvent` persisted, which is what `GetEvent`
/// and `ListEvents` hand back.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    /// `<epoch-millis>#<hex>`, minted by [`mint_event_id`]; never client-supplied.
    pub event_id: String,
    pub memory_id: String,
    pub actor_id: String,
    pub session_id: String,
    pub event_timestamp: ::aws_smithy_types::DateTime,
    pub payload: Vec<PayloadType>,
    pub branch: Option<Branch>,
    pub metadata: Option<HashMap<String, MetadataValue>>,
    /// The value [`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::next_seq`]
    /// handed this event. It totally orders events created in the same millisecond
    /// and with the same `eventTimestamp`, so a listing is deterministic.
    pub seq: u64,
}

impl StoredEvent {
    /// This event as the modelled `Event`.
    ///
    /// `include_payload` is `ListEvents`' `includePayloads`: `Event$payload` is
    /// `@required`, so a listing that drops the payloads reports each event with an
    /// empty payload list — the closest the modelled shape can come to leaving it out.
    pub fn to_event(&self, include_payload: bool) -> Event {
        Event {
            memory_id: self.memory_id.clone(),
            actor_id: self.actor_id.clone(),
            session_id: self.session_id.clone(),
            event_id: self.event_id.clone(),
            event_timestamp: self.event_timestamp,
            payload: if include_payload {
                self.payload.clone()
            } else {
                Vec::new()
            },
            branch: self.branch.clone(),
            metadata: self.metadata.clone(),
        }
    }

    /// The total order key used to report events newest-first: by the modelled
    /// `eventTimestamp`, then by creation order for events sharing an instant.
    pub fn order_key(&self) -> (i64, u32, u64) {
        (
            self.event_timestamp.secs(),
            self.event_timestamp.subsec_nanos(),
            self.seq,
        )
    }

    /// The name of the branch this event is on, or `None` for the trunk (an event
    /// created without a `branch`).
    pub fn branch_name(&self) -> Option<&str> {
        self.branch.as_ref().map(|branch| branch.name.as_str())
    }
}

/// One code interpreter's sessions, as `StartCodeInterpreterSession` wrote them.
#[derive(Debug, Default)]
pub struct CodeInterpreterStore {
    /// Sessions keyed by their minted `sessionId`. Bounded by
    /// [`MAX_CODE_INTERPRETER_SESSIONS`].
    pub sessions: HashMap<String, CodeInterpreterSessionRecord>,
    /// `clientToken` -> the session that token already started.
    /// `StartCodeInterpreterSession` is `@idempotent` and its `clientToken` is an
    /// `@idempotencyToken`, so a replayed token answers with the first session rather
    /// than starting a second one. Bounded by
    /// [`MAX_IDEMPOTENCY_RECORDS_PER_CODE_INTERPRETER`], and a record is dropped with
    /// the session it points at.
    pub idempotency: HashMap<String, SessionLocator>,
    /// The sequence of the last session started on this code interpreter, which is
    /// what its eviction order is read from.
    pub last_seq: u64,
}

/// Where an already-started session lives, so a replayed `clientToken` can find it.
#[derive(Debug, Clone)]
pub struct SessionLocator {
    pub session_id: String,
    /// The sequence of the session this record points at: its eviction order.
    pub seq: u64,
}

/// One code interpreter session: exactly what `StartCodeInterpreterSession`
/// persisted, which is what `GetCodeInterpreterSession` reports.
#[derive(Debug, Clone)]
pub struct CodeInterpreterSessionRecord {
    /// Minted by [`mint_code_interpreter_session_id`]; never client-supplied.
    pub session_id: String,
    pub code_interpreter_identifier: String,
    pub name: Option<String>,
    pub created_at: SystemTime,
    /// The session's time-to-live in seconds, as the request asked for it (the model
    /// defaults it to 900 when a request omits it).
    pub session_timeout_seconds: i32,
    pub certificates: Option<Vec<Certificate>>,
    pub filesystem_configurations: Option<Vec<ToolsFileSystemConfiguration>>,
    /// The value [`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::next_seq`]
    /// handed this session: its eviction order, and what keeps two sessions started in
    /// the same millisecond apart.
    pub seq: u64,
}

impl CodeInterpreterSessionRecord {
    /// The instant this session's time-to-live runs out, or `None` when that instant
    /// is not representable (so nothing can be past it).
    ///
    /// `sessionTimeoutSeconds` is modelled as "the duration in seconds
    /// (time-to-live) after which the session automatically terminates, regardless of
    /// ongoing activity", so the session's own requested value is the span — nothing
    /// chosen here. The model's range makes it positive; a value that is not is read as
    /// no time at all rather than as a time in the past.
    pub fn expires_at(&self) -> Option<SystemTime> {
        let ttl = Duration::from_secs(u64::try_from(self.session_timeout_seconds).unwrap_or(0));
        self.created_at.checked_add(ttl)
    }

    /// This session's `CodeInterpreterSessionStatus` at `now`: `READY` until its
    /// time-to-live has run out, `TERMINATED` from then on. The clock alone decides,
    /// so a read reports the termination that was already due without writing
    /// anything.
    pub fn status_as_of(&self, now: SystemTime) -> &'static str {
        match self.expires_at() {
            Some(expiry) if now >= expiry => CODE_INTERPRETER_SESSION_TERMINATED,
            _ => CODE_INTERPRETER_SESSION_READY,
        }
    }
}

/// The key a `memoryId` is stored under.
///
/// `MemoryId`'s pattern admits either the bare id or the
/// `arn:<partition>:bedrock-agentcore:<region>:<account>:memory/<id>` form, so both
/// spellings address one container; the bare id is the canonical key.
pub fn normalize_memory_id(memory_id: &str) -> String {
    match memory_id.split_once(":memory/") {
        Some((prefix, id)) if prefix.starts_with("arn:") => id.to_string(),
        _ => memory_id.to_string(),
    }
}

/// Mint an `eventId`.
///
/// `EventId`'s pattern is `^[0-9]+#[a-fA-F0-9]+$`: the decimal creation instant in
/// epoch milliseconds, then a hex discriminator taken from the engine-wide event
/// sequence so two events minted in one millisecond differ.
pub fn mint_event_id(seq: u64) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("{millis}#{seq:016x}")
}

/// Mint a `sessionId` for a `CreateEvent` that supplied none.
///
/// `SessionId`'s pattern is `^[a-zA-Z0-9][a-zA-Z0-9-_]*$` with length 1..100.
pub fn mint_session_id(seq: u64) -> String {
    format!("session-{seq:016x}")
}

/// Mint a `CodeInterpreterSessionId`.
///
/// That shape's pattern is `^[0-9a-zA-Z]{1,40}$` — alphanumerics only, so the id is
/// the creation instant in epoch milliseconds followed by the engine-wide sequence,
/// both in lowercase hex (26 characters), which keeps two sessions started in one
/// millisecond apart.
pub fn mint_code_interpreter_session_id(seq: u64) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("{millis:010x}{seq:016x}")
}

/// Encode an opaque `nextToken` for `kind` resuming at `offset`.
///
/// The token is base64 (`aws_smithy_types::base64`, the codec the protocols already
/// use) over `"<kind>:<offset>"`: opaque to a caller, and tied to the list it came
/// from, so a token minted by one listing cannot silently resume another. It carries
/// the POSITION only, not the request that produced it, so a caller that changes the
/// other inputs between pages (a different `filter`, say) resumes at that offset into
/// the newly described list — the same contract AWS documents for a pagination token,
/// which is only meaningful for an otherwise identical request. An offset past the end
/// of the list is a page with nothing on it and no further token, which is what a
/// caller resuming a list that has since shrunk should see.
pub fn encode_next_token(kind: &str, offset: usize) -> String {
    ::aws_smithy_types::base64::encode(format!("{kind}:{offset}"))
}

/// Decode a `nextToken` minted by [`encode_next_token`] for `kind`, or `None` when
/// it is not one (which a handler answers with a `ValidationException`).
pub fn decode_next_token(kind: &str, token: &str) -> Option<usize> {
    let raw = ::aws_smithy_types::base64::decode(token).ok()?;
    let text = String::from_utf8(raw).ok()?;
    let offset = text.strip_prefix(kind)?.strip_prefix(':')?;
    offset.parse::<usize>().ok()
}

/// One page of `items`, starting at `offset`, plus the `nextToken` that resumes
/// after it (`None` on the last page).
///
/// `render` runs ONLY on the items of the page, so a response never materialises
/// more records than the page it returns, however many the store holds.
pub fn paginate_with<T, U>(
    items: &[T],
    kind: &str,
    offset: usize,
    max_results: usize,
    render: impl FnMut(&T) -> U,
) -> (Vec<U>, Option<String>) {
    let page: Vec<U> = items
        .iter()
        .skip(offset)
        .take(max_results)
        .map(render)
        .collect();
    let next_offset = offset.saturating_add(page.len());
    let next_token = (next_offset < items.len()).then(|| encode_next_token(kind, next_offset));
    (page, next_token)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState>)
}
