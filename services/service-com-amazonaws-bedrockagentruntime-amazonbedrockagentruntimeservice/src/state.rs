// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice service.
//!
//! The persistent resource this service owns is the **session**: a session holds
//! invocations, and an invocation holds invocation steps. Every session-scoped
//! operation (`GetSession`, `UpdateSession`, `EndSession`, `DeleteSession`,
//! `CreateInvocation`, `PutInvocationStep`, `GetInvocationStep`,
//! `ListInvocationSteps`) resolves its `sessionIdentifier` through the ONE helper
//! [`session_id_from_identifier`], so they all agree on what a bare UUID and a
//! session ARN mean.

use std::collections::BTreeMap;

use ::aws_smithy_types::DateTime;

use crate::ops::types::InvocationStepPayload;

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region every generated emulator defaults to (see
/// `emulator_server::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `bedrock` ARN namespace this service's resources live in
/// (`smithy_query(query="service_info")` -> `awsService.arnNamespace`), which is
/// NOT the `bedrock-agent-runtime` endpoint prefix.
pub const ARN_NAMESPACE: &str = "bedrock";

/// Resource guards, NOT AWS quotas.
///
/// The session store lives in this process's memory, has no TTL and is released
/// only by `DeleteSession`, so an unbounded number of `CreateSession` /
/// `CreateInvocation` / `PutInvocationStep` calls would grow it until the emulator
/// is starved. These are the declared maxima that stop that: past one of them the
/// writing operation answers its modelled `ServiceQuotaExceededException` (HTTP 400,
/// "The number of requests exceeds the service quota") instead of allocating. They
/// are the emulator's own limits, deliberately not presented as the real service's
/// published quotas, and each is far above any plausible test or demo workload.
pub const MAX_SESSIONS: usize = 1_000;

/// Resource guard: the most invocations one session holds. See [`MAX_SESSIONS`].
pub const MAX_INVOCATIONS_PER_SESSION: usize = 1_000;

/// Resource guard: the most steps one invocation holds. See [`MAX_SESSIONS`].
pub const MAX_STEPS_PER_INVOCATION: usize = 1_000;

/// The `maxResults` bounds ListInvocationSteps declares
/// (`smithy_query(query="operation",operation="ListInvocationSteps")` -> `maxResults`
/// has `range {min: 1, max: 1000}` and `default: 10`), named here so the
/// model-derived page bounds have one home.
pub const MIN_LIST_PAGE: i32 = 1;

/// See [`MIN_LIST_PAGE`].
pub const MAX_LIST_PAGE: i32 = 1_000;

/// See [`MIN_LIST_PAGE`].
pub const DEFAULT_LIST_PAGE: i32 = 10;

/// A session's lifecycle status: the three values the model's `SessionStatus` enum
/// declares (`smithy_query(query="shape",shape="SessionStatus")` -> ACTIVE,
/// EXPIRED, ENDED).
///
/// `Expired` is modelled and understood here, but no code path mints it: the model
/// and the service docs state no session lifetime, so there is no instant at which
/// the clock owes that transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Active,
    Expired,
    Ended,
}

impl SessionStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Expired => "EXPIRED",
            Self::Ended => "ENDED",
        }
    }
}

/// One stored invocation step: the identity triple, the caller's step time, and the
/// `InvocationStepPayload` union stored WHOLE so `GetInvocationStep` hands back
/// exactly what `PutInvocationStep` was given.
#[derive(Debug)]
pub struct InvocationStepRecord {
    pub invocation_step_id: String,
    pub invocation_id: String,
    pub session_id: String,
    pub invocation_step_time: DateTime,
    pub payload: InvocationStepPayload,
    /// Position in this service's global store order. Listings sort on it, so a
    /// page boundary is stable even though the maps are keyed by UUID.
    pub order: u64,
}

/// One stored invocation, holding the steps written under it.
#[derive(Debug)]
pub struct Invocation {
    pub invocation_id: String,
    pub session_id: String,
    pub description: Option<String>,
    pub created_at: DateTime,
    pub steps: BTreeMap<String, InvocationStepRecord>,
    /// Store order, as for [`InvocationStepRecord::order`].
    pub order: u64,
}

/// One stored session.
#[derive(Debug)]
pub struct Session {
    pub session_id: String,
    pub session_arn: String,
    pub status: SessionStatus,
    pub created_at: DateTime,
    pub last_updated_at: DateTime,
    /// Exactly what the writer supplied: `None` when the request carried no
    /// `sessionMetadata` member at all, `Some(map)` (possibly empty) when it did, so
    /// `GetSession` can report an absent member and a stored empty map as the two
    /// different answers they are.
    pub session_metadata: Option<BTreeMap<String, String>>,
    pub encryption_key_arn: Option<String>,
    /// The `tags` `CreateSession` was given, stored so nothing the caller supplied is
    /// dropped. WRITE-ONLY for now: `TagResource`, `UntagResource` and
    /// `ListTagsForResource` are still the scaffold's 501, so no implemented
    /// operation reads this map yet — it becomes live when those land.
    pub tags: BTreeMap<String, String>,
    pub invocations: BTreeMap<String, Invocation>,
    /// Store order, as for [`InvocationStepRecord::order`].
    pub order: u64,
}

/// Per-service state for com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState {
    /// Every live session, keyed by its bare `sessionId` UUID.
    pub sessions: BTreeMap<String, Session>,
    /// Monotonic counter handing out [`InvocationStepRecord::order`] values.
    pub order_counter: u64,
}

/// The next store-order position, taken from
/// [`ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState::order_counter`]
/// directly. Never reused, so listing order is insertion order for the lifetime of
/// the process.
///
/// It takes the counter field rather than the whole state, because a handler writing
/// through a `&mut` borrow of one session cannot hold a second borrow of the struct;
/// one function serves every caller either way.
pub fn next_order(counter: &mut u64) -> u64 {
    *counter += 1;
    *counter
}

/// Resolve a `sessionIdentifier` to the bare `sessionId` it names.
///
/// The model's `SessionIdentifier` accepts EITHER a session ARN
/// (`arn:…:bedrock:…:session/<uuid>`) or the bare `<uuid>`
/// (`smithy_query(query="operation",operation="GetSession")` -> the member's
/// pattern), so every session-scoped operation routes its label through here and
/// they cannot disagree.
///
/// Only an ARN this emulator could itself have minted names a session here: the
/// `bedrock` ARN namespace and the canonical emulator account. An ARN for another
/// service or another account is returned unchanged, so the lookup misses and the
/// caller answers `ResourceNotFoundException` — the real service would not resolve
/// another account's resource either. The region segment is NOT checked: one
/// emulator process serves every region from one store, and the region in a minted
/// ARN is whichever one the creating caller signed for.
pub fn session_id_from_identifier(identifier: &str) -> &str {
    let Some((prefix, session_id)) = identifier.rsplit_once(":session/") else {
        // Not an ARN at all: the bare session id.
        return identifier;
    };
    let fields: Vec<&str> = prefix.split(':').collect();
    match fields.as_slice() {
        ["arn", _partition, namespace, _region, account]
            if *namespace == ARN_NAMESPACE && *account == ::engine::EMULATOR_ACCOUNT_ID =>
        {
            session_id
        }
        _ => identifier,
    }
}

/// A freshly minted version-4 UUID in the canonical lowercase hyphenated form every
/// `Uuid`-shaped member of this model requires (`sessionId`, `invocationId`,
/// `invocationStepId`).
///
/// The generator is the audited `uuid` one, reached through a dependency this crate
/// already has rather than a hand-written one: `emulator_telemetry`'s UUID newtypes
/// are `uuid::Uuid::new_v4().as_hyphenated()` behind `random()` (see
/// `vendor/emulator-telemetry/src/types.rs`), which is how the greengrass service
/// crate mints its ids too. Only the id is taken — nothing here emits telemetry.
///
/// A resource id is not a secret and never a credential: created credentials come only
/// from the emulator's credential store.
pub fn new_uuid() -> String {
    ::emulator_telemetry::SessionId::random().as_str().to_string()
}

/// The current wall-clock instant, as the model's `DateTimestamp` carries it.
pub fn now() -> DateTime {
    DateTime::from(::std::time::SystemTime::now())
}

/// The longest region segment this model admits inside a `bedrock` ARN: its own
/// `AWSResourceARN` bounds that segment to `[a-z0-9-]{1,20}`
/// (`smithy_query(query="shape",shape="AWSResourceARN")`), and no AWS region name is
/// longer. A scope region past it cannot be part of a well-formed ARN, so it is not
/// stamped into one.
pub const MAX_REGION_SEGMENT: usize = 20;

/// The region to stamp into an ARN this request mints: the one the caller's SigV4
/// credential scope names, else [`DEFAULT_REGION`].
///
/// A scope value that is not region-shaped (`SessionArn`'s own `[a-z0-9-]+`
/// segment) falls back too, so a minted `sessionArn` always satisfies the pattern
/// the model declares for it.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_SEGMENT
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The scope a pagination cursor is bound to: the session being listed, and the
/// `invocationIdentifier` filter when one is supplied.
///
/// A cursor is only meaningful for the listing that issued it, so the scope goes
/// INTO the token: a token minted while listing one session (or one invocation) is
/// then refused on another rather than silently resuming at an unrelated offset.
pub fn pagination_scope(session_id: &str, invocation_filter: Option<&str>) -> String {
    format!("{session_id}:{}", invocation_filter.unwrap_or(""))
}

/// Encode a pagination cursor: the scope it belongs to, and the store order of the
/// last item already returned.
///
/// Encoded with `aws_smithy_types::base64` — the codec the wire protocols already use
/// for every Smithy blob, so no new one is introduced — which keeps the token opaque
/// to the client and inside the model's `NextToken` constraints (1..2048 chars,
/// `^\S*$`).
pub fn encode_next_token(scope: &str, last_order: u64) -> String {
    ::aws_smithy_types::base64::encode(format!("v1:{scope}:{last_order}"))
}

/// Decode a cursor [`encode_next_token`] issued for `scope`. `None` for anything
/// else — a malformed token, or one issued for a different session or filter —
/// which a handler answers with the modelled `ValidationException`.
pub fn decode_next_token(scope: &str, token: &str) -> Option<u64> {
    let decoded = ::aws_smithy_types::base64::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix("v1:")?
        .strip_prefix(scope)?
        .strip_prefix(':')?
        .parse()
        .ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState>)
}
