// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssmguiconnect-ssmguiconnect service.
//!
//! SSM GUI Connect models exactly ONE resource: the per-account/per-Region
//! *connection recording preferences* singleton. `UpdateConnectionRecordingPreferences`
//! creates-or-replaces it, `GetConnectionRecordingPreferences` reports it, and
//! `DeleteConnectionRecordingPreferences` removes it — so the whole service is the
//! lifecycle of the one [`RecordingPrefsRecord`] held here.
//!
//! Besides the record this module owns the two rules both mutating operations share,
//! so neither handler can drift from the other:
//!
//! * [`refuse_preferences`] — the modeled constraints on
//!   `ConnectionRecordingPreferences` that the framework's boundary validator cannot
//!   enforce (see that function's own docs), reported in the same
//!   `ValidationException` envelope the boundary uses.
//! * [`mint_client_token`] — the service-provided value for an `@idempotencyToken`
//!   member the caller left unset.
//! * [`audit`] — the structured access record every operation emits, so a call to this
//!   service (above all a state-changing one) leaves an attributable trail.

use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

use aws_smithy_schema_server::constraints::{envelope, path, Violation, ViolationKind};

use crate::ops::types::{ConnectionRecordingPreferences, RecordingDestinations, S3Bucket};

/// Most idempotency outcomes retained at once, after which the OLDEST is evicted.
///
/// The map is keyed by a caller-supplied `ClientToken`, so without a cap a client
/// sending a fresh token per request would grow it without bound for the life of the
/// process. The bound is on the number of entries rather than on their age because
/// entry count is what the memory cost is proportional to, and it is set far above any
/// plausible in-flight retry window for a singleton that a caller configures rarely:
/// evicting the oldest entry can only turn a replay that old into a fresh call, which
/// is the same answer the caller would get from a service whose own idempotency window
/// had expired. Eviction is FIFO — strictly by insertion order, tracked in
/// `idempotency_order` — so it never drops a newer outcome to keep an older one.
pub const MAX_IDEMPOTENCY_OUTCOMES: usize = 1024;

/// Per-service state for com-amazonaws-ssmguiconnect-ssmguiconnect. A handler reaches it through [`service_state`].
///
/// Every field is private and reached through the methods below: the singleton and
/// the idempotency record have to move together (an outcome that outlived the record
/// it describes would answer a replay with a state the store no longer holds), and
/// the idempotency store has to stay inside [`MAX_IDEMPOTENCY_OUTCOMES`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSsmguiconnectSsmguiconnectState {
    /// The one per-account/per-Region connection recording preferences record.
    /// `None` means never configured (or deleted), which every read reports as
    /// `ResourceNotFoundException`.
    connection_recording_preferences: Option<RecordingPrefsRecord>,
    /// First outcome per `@idempotencyToken`, so replaying an `@idempotent`
    /// Update/Delete with the same `ClientToken` returns that outcome instead of
    /// mutating twice. Keyed by operation as well as token: the same token
    /// replayed against a *different* operation is a different call. Bounded at
    /// [`MAX_IDEMPOTENCY_OUTCOMES`] entries.
    idempotency_tokens: HashMap<IdempotencyKey, StoredResponse>,
    /// Insertion order of `idempotency_tokens`' keys, which is what makes the
    /// eviction above FIFO. Holds exactly the map's keys, so the two are inserted
    /// and removed together.
    idempotency_order: VecDeque<IdempotencyKey>,
}

/// The stored connection recording preferences singleton.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingPrefsRecord {
    /// `ConnectionRecordingPreferences$KMSKeyArn`.
    pub kms_key_arn: String,
    /// `RecordingDestinations$S3Buckets`, stored in the order it arrived.
    pub s3_buckets: Vec<S3BucketRecord>,
    /// The `ClientToken` of the Update that last wrote this record — what both
    /// that Update and every later Get report.
    pub client_token: String,
    /// When the record was last written. Not on the wire (the model declares no
    /// timestamp member); it is what distinguishes one write of the singleton from
    /// the next, so a replayed token can tell whether the record it wrote is still
    /// the one stored.
    pub last_updated: SystemTime,
}

/// One entry of `RecordingDestinations$S3Buckets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3BucketRecord {
    /// `S3Bucket$BucketOwner`.
    pub bucket_owner: String,
    /// `S3Bucket$BucketName`.
    pub bucket_name: String,
}

/// The operations that carry an `@idempotencyToken`, used as the first half of
/// [`IdempotencyKey`] so one token replayed against two operations is two calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdempotentOperation {
    /// `UpdateConnectionRecordingPreferences`.
    Update,
    /// `DeleteConnectionRecordingPreferences`.
    Delete,
}

/// What [`ComAmazonawsSsmguiconnectSsmguiconnectState`]'s idempotency store is keyed by.
pub type IdempotencyKey = (IdempotentOperation, String);

/// The first outcome of an `@idempotent` call, replayed when the store is still in
/// the state that call left it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredResponse {
    /// An Update that persisted this record.
    Updated(RecordingPrefsRecord),
    /// A Delete that removed the singleton. The response carries only the
    /// `ClientToken`, which is the key's own token, so there is nothing to hold.
    Deleted,
}

impl ComAmazonawsSsmguiconnectSsmguiconnectState {
    /// The stored singleton, or `None` when nothing is configured.
    pub fn preferences(&self) -> Option<&RecordingPrefsRecord> {
        self.connection_recording_preferences.as_ref()
    }

    /// Create-or-REPLACE the singleton, and record the outcome under `record`'s own
    /// `ClientToken`. The stored value is exactly `record`, never a merge with what
    /// was there before.
    pub fn put_preferences(&mut self, record: RecordingPrefsRecord) {
        let key = (IdempotentOperation::Update, record.client_token.clone());
        self.connection_recording_preferences = Some(record.clone());
        self.record_outcome(key, StoredResponse::Updated(record));
    }

    /// Remove the singleton and record the outcome under `client_token`. `false` when
    /// there was nothing to remove, in which case nothing is recorded either — a call
    /// that failed has no outcome to replay.
    pub fn remove_preferences(&mut self, client_token: &str) -> bool {
        if self.connection_recording_preferences.take().is_none() {
            return false;
        }
        let key = (IdempotentOperation::Delete, client_token.to_string());
        self.record_outcome(key, StoredResponse::Deleted);
        true
    }

    /// The outcome a replay of `key` should answer with, or `None` when the call has
    /// to run for real.
    ///
    /// An outcome is replayable only while the store is still in the state that call
    /// left it in: the record an Update wrote is still the stored singleton, or the
    /// singleton a Delete removed is still absent. Otherwise a later call changed the
    /// resource out from under the token, and answering from the cache would report a
    /// state the next reader does not see.
    pub fn replayable_outcome(&self, key: &IdempotencyKey) -> Option<&StoredResponse> {
        let outcome = self.idempotency_tokens.get(key)?;
        let still_current = match outcome {
            StoredResponse::Updated(record) => {
                self.connection_recording_preferences.as_ref() == Some(record)
            }
            StoredResponse::Deleted => self.connection_recording_preferences.is_none(),
        };
        still_current.then_some(outcome)
    }

    /// How many idempotency outcomes are retained. Never above
    /// [`MAX_IDEMPOTENCY_OUTCOMES`].
    pub fn retained_outcome_count(&self) -> usize {
        self.idempotency_tokens.len()
    }

    /// Record `outcome` under `key`, evicting the oldest entries until the store is
    /// back inside [`MAX_IDEMPOTENCY_OUTCOMES`].
    fn record_outcome(&mut self, key: IdempotencyKey, outcome: StoredResponse) {
        if self.idempotency_tokens.insert(key.clone(), outcome).is_none() {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_tokens.len() > MAX_IDEMPOTENCY_OUTCOMES {
            match self.idempotency_order.pop_front() {
                Some(oldest) => {
                    self.idempotency_tokens.remove(&oldest);
                }
                // Unreachable while the two stay in step, and breaking rather than
                // looping is what keeps it unreachable instead of hanging.
                None => break,
            }
        }
    }
}

impl RecordingPrefsRecord {
    /// Build the record a request asks to persist. Every nested member is stored;
    /// none is dropped.
    pub fn from_request(prefs: &ConnectionRecordingPreferences, client_token: String) -> Self {
        Self {
            kms_key_arn: prefs.kms_key_arn.clone(),
            s3_buckets: prefs
                .recording_destinations
                .s3_buckets
                .iter()
                .map(|bucket| S3BucketRecord {
                    bucket_owner: bucket.bucket_owner.clone(),
                    bucket_name: bucket.bucket_name.clone(),
                })
                .collect(),
            client_token,
            last_updated: SystemTime::now(),
        }
    }

    /// The wire shape for what is stored. The ONE place a response is built from
    /// the record, so Update and Get cannot report the same record differently.
    pub fn to_preferences(&self) -> ConnectionRecordingPreferences {
        ConnectionRecordingPreferences {
            recording_destinations: RecordingDestinations {
                s3_buckets: self
                    .s3_buckets
                    .iter()
                    .map(|bucket| S3Bucket {
                        bucket_owner: bucket.bucket_owner.clone(),
                        bucket_name: bucket.bucket_name.clone(),
                    })
                    .collect(),
            },
            kms_key_arn: self.kms_key_arn.clone(),
        }
    }
}

/// The modeled `@pattern` of `S3Bucket$BucketName`, copied verbatim from
/// `smithy_query(query="shape", shape="S3Bucket")` on this service's model.
///
/// Reported in the refusal message so a caller sees the requirement it failed, the
/// way the framework's own `@pattern` violation does.
pub const BUCKET_NAME_PATTERN: &str = r"(?=^.{3,63}$)(?!^(\d+\.)+\d+$)(^(([a-z0-9]|[a-z0-9][a-z0-9\-]*[a-z0-9])\.)*([a-z0-9]|[a-z0-9][a-z0-9\-]*[a-z0-9])$)";

/// The negative lookahead of [`BUCKET_NAME_PATTERN`], `(?!^(\d+\.)+\d+$)`: a name made
/// only of digit labels joined by dots (an IPv4-shaped name) is refused. At least one
/// dot is required by the `+`, so a single all-digit label like `123` is NOT refused.
fn bucket_name_is_all_numeric_labels(name: &str) -> bool {
    name.contains('.')
        && name
            .split('.')
            .all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit()))
}

/// The capturing body of [`BUCKET_NAME_PATTERN`]: dot-separated labels, each one a
/// single `[a-z0-9]` or a `[a-z0-9]`-delimited run that may contain `-`. An empty label
/// (a leading, trailing or doubled dot) satisfies neither alternative, so it is refused.
fn bucket_name_labels_are_well_formed(name: &str) -> bool {
    name.split('.').all(|label| {
        let edge = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
        match label.as_bytes() {
            [] => false,
            [only] => edge(*only),
            [first, inner @ .., last] => {
                edge(*first)
                    && edge(*last)
                    && inner.iter().all(|b| edge(*b) || *b == b'-')
            }
        }
    })
}

/// Longest value echoed back in a refusal message, the same bound the framework's own
/// constraint walk applies: the value is caller-controlled, so an unelided echo would
/// amplify a request into an arbitrarily long response.
const MAX_ECHOED_VALUE: usize = 256;

/// The refused value as it should appear in a MESSAGE: itself when short, otherwise a
/// character-boundary-safe prefix plus the elision the framework's own renderer
/// appends, so a long value reads the same whichever layer refused it.
///
/// Rendering only — the ORIGINAL value is what the check above runs on. Eliding before
/// the check would compare text the caller never sent.
fn echoed(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// True when `name` satisfies the modeled `S3Bucket$BucketName` pattern.
///
/// [`BUCKET_NAME_PATTERN`] uses look-around, which the `regex` crate behind the
/// framework's `@pattern` check cannot compile — an uncompilable model pattern is
/// treated there as matching, so the boundary validator passes every bucket name.
/// The pattern is therefore evaluated here as its three independent parts: the
/// `(?=^.{3,63}$)` length lookahead, the `(?!^(\d+\.)+\d+$)` negative lookahead
/// ([`bucket_name_is_all_numeric_labels`]) and the label body
/// ([`bucket_name_labels_are_well_formed`]).
///
/// Each part is plain string logic rather than a regex: the three are a character-class
/// test over dot-separated labels, which Rust expresses directly, and keeping them out
/// of a regex crate keeps this service crate's dependency set — and therefore the
/// workspace lockfile — untouched by this change.
pub fn bucket_name_matches_model_pattern(name: &str) -> bool {
    (3..=63).contains(&name.chars().count())
        && !bucket_name_is_all_numeric_labels(name)
        && bucket_name_labels_are_well_formed(name)
}

/// Refuse a `ConnectionRecordingPreferences` that violates a modeled constraint the
/// boundary validator let through, as the `ValidationException` message body; `None`
/// when it satisfies every one.
///
/// Only constraints the boundary CANNOT enforce are re-checked here — today that is
/// `S3Bucket$BucketName`, for the reason [`bucket_name_matches_model_pattern`]
/// documents. Every other modeled constraint on this input (`ClientToken`'s
/// `@length`, `S3Buckets`' `@length(1, 1)`, `BucketOwner`'s `^[0-9]{12}$`,
/// `KMSKeyArn`'s `@length(1, 2048)`, and every `@required` member) compiles and is
/// already refused before a handler runs, so duplicating it here would be dead code.
///
/// The message is built from the request — the member path, the value it held and the
/// requirement it failed — through the framework's own `Violation` renderer, so a
/// refusal from here is worded like one from the boundary.
pub fn refuse_preferences(prefs: &ConnectionRecordingPreferences) -> Option<String> {
    let violations: Vec<Violation> = prefs
        .recording_destinations
        .s3_buckets
        .iter()
        .enumerate()
        .filter(|(_, bucket)| !bucket_name_matches_model_pattern(&bucket.bucket_name))
        .map(|(index, bucket)| Violation {
            path: path::join(&[
                path::member("ConnectionRecordingPreferences"),
                path::member("RecordingDestinations"),
                path::member("S3Buckets"),
                path::element(index + 1),
                path::member("BucketName"),
            ]),
            kind: ViolationKind::Pattern(BUCKET_NAME_PATTERN.to_string()),
            value: Some(echoed(&bucket.bucket_name)),
        })
        .collect();
    envelope(&violations)
}

/// The service-provided value for an `@idempotencyToken` member the caller left
/// unset.
///
/// The member is modeled `@idempotencyToken` with `@length(1, 64)` and NO `@pattern`,
/// so what it has to be is a value no other call produces — not a particular shape.
/// This mints 32 lowercase hex characters from two sources that cannot repeat within a
/// process: a monotonic per-process counter, and `RandomState`, whose keys the standard
/// library seeds from the operating system and varies per instance. It is a per-run,
/// server-generated value: nothing about it is derived from the request or from a
/// recording.
///
/// Built on the standard library rather than a UUID crate deliberately. The token has
/// no modeled shape to be faithful to, and adding any dependency to this crate puts an
/// entry in the workspace `Cargo.lock` — the one file every other service's change also
/// rewrites, and the overlap the serialized squash-merge cannot resolve. Keeping this
/// service's dependency set exactly as the scaffold left it keeps the change confined to
/// its own crate.
pub fn mint_client_token() -> String {
    use std::fmt::Write as _;
    use std::hash::{BuildHasher, Hasher};

    /// Calls minted so far in this process; the one input that is guaranteed distinct
    /// per call even if two `RandomState`s ever agreed.
    static MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let sequence = MINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut token = String::with_capacity(32);
    for half in 0..2_u64 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(sequence);
        hasher.write_u64(half);
        let _ = write!(token, "{:016x}", hasher.finish());
    }
    token
}

/// Emit the structured access record for one call to this service.
///
/// Every operation calls this on every branch it can return from, so a reader of the
/// emulator's log can attribute each call — above all the two state-changing ones,
/// whose effect on the singleton is otherwise invisible after the fact. The record
/// carries the operation, the identity the request PRESENTED (the SigV4 access key id,
/// which is an identifier and not a secret), the request target it named, and the
/// outcome; the subscriber supplies the timestamp.
///
/// There is no network peer in the record because a Smithy handler is not given one:
/// `OperationContext::parts()` carries the request's method, URI, headers and
/// extensions, and nothing in this framework puts a peer address in them. The presented
/// credential is therefore what identifies the caller.
///
/// Every interpolated value is request-derived, so each is bounded and stripped of the
/// control characters that would otherwise let one request forge a second log line:
/// [`audit_field`] does both.
///
/// Deliberately metadata only: neither the stored preferences nor the `ClientToken` is
/// logged, so a caller's configuration never reaches the log, and nothing here can be
/// replayed as a credential.
pub fn audit(
    operation: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    outcome: AuditOutcome,
) {
    let parts = ctx.parts();
    tracing::info!(
        target: "emulator::audit",
        service = ctx.identity().signing_name(),
        operation,
        access_key_id = %audit_field(&::engine::extract_akid(&parts.headers)),
        request_target = %audit_field(&format!("{} {}", parts.method, parts.uri.path())),
        outcome = outcome.as_str(),
        "ssm-guiconnect api call"
    );
}

/// Longest request-derived value one audit field carries.
///
/// A SigV4 access key id and a request path are both caller-controlled, so an unbounded
/// echo would let one request emit an arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 128;

/// A request-derived value as it may appear in an audit record: control characters
/// (above all CR and LF, which would forge a second record) replaced, and the whole
/// thing bounded by [`MAX_AUDIT_FIELD`] on a character boundary.
fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '.' } else { c })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// What a call answered, as the one closed set [`audit`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// The singleton was created or replaced.
    PreferencesWritten,
    /// The singleton was removed.
    PreferencesDeleted,
    /// The singleton was reported to a reader.
    PreferencesRead,
    /// An `@idempotent` call replayed an earlier outcome and changed nothing.
    IdempotentReplay,
    /// The request was refused as invalid (400).
    ValidationRefused,
    /// There was no configured singleton to report or remove (404).
    NotFound,
    /// The service's own store was unreachable (500).
    StoreUnavailable,
}

impl AuditOutcome {
    /// The stable token this outcome is logged as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreferencesWritten => "preferences-written",
            Self::PreferencesDeleted => "preferences-deleted",
            Self::PreferencesRead => "preferences-read",
            Self::IdempotentReplay => "idempotent-replay",
            Self::ValidationRefused => "validation-refused",
            Self::NotFound => "not-found",
            Self::StoreUnavailable => "store-unavailable",
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsmguiconnectSsmguiconnectState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsmguiconnectSsmguiconnectState>)
}
