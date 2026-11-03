// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssmguiconnect-ssmguiconnect service.
//!
//! SSM GUI Connect models exactly ONE resource: the per-account/per-Region
//! *connection recording preferences* singleton. `UpdateConnectionRecordingPreferences`
//! creates-or-replaces it, `GetConnectionRecordingPreferences` reports it, and
//! `DeleteConnectionRecordingPreferences` removes it — so the whole service is the
//! lifecycle of the one [`RecordingPrefsRecord`] held here.
//!
//! Besides the record this module owns the rules the operations share, so no handler
//! can drift from another:
//!
//! * [`refuse_preferences`] — the modeled constraints on
//!   `ConnectionRecordingPreferences` that the framework's boundary validator cannot
//!   enforce (see that function's own docs), reported in the same
//!   `ValidationException` envelope the boundary uses.
//! * [`constraint_message`] — how THIS service words a `@length` bound the boundary
//!   validator refused, which is not the framework's aggregated envelope.
//! * [`resource_not_found_message`] — the one template every path reports the absent
//!   singleton with.
//! * the account-level just-in-time node access gate
//!   ([`ComAmazonawsSsmguiconnectSsmguiconnectState::just_in_time_node_access_enabled`]),
//!   which `UpdateConnectionRecordingPreferences` is refused by.
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
    /// Whether this account/Region is enrolled in just-in-time node access, the
    /// account-level feature `UpdateConnectionRecordingPreferences` is gated by.
    ///
    /// `false` is the state of an account nobody has enrolled, which is what the
    /// default is: enrollment is performed by SSM Quick Setup's just-in-time node
    /// access configuration — a different API, which this service does not model and
    /// this emulator does not implement — so no operation of THIS service turns it on.
    /// It is state rather than a constant because the gate is a property of the
    /// account, not of the request: whoever enrolls the account (an enrollment path
    /// this emulator grows later, or a test standing the enrolled account up) sets it
    /// through [`ComAmazonawsSsmguiconnectSsmguiconnectState::set_just_in_time_node_access_enabled`]
    /// and every later Update reads it back.
    just_in_time_node_access_enabled: bool,
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

    /// Whether this account/Region is enrolled in just-in-time node access. `false`
    /// until something enrolls it, which is what the Update gate reads.
    pub fn just_in_time_node_access_enabled(&self) -> bool {
        self.just_in_time_node_access_enabled
    }

    /// Enroll (or un-enroll) this account/Region in just-in-time node access.
    ///
    /// The ONE writer of the gate's flag. No modelled operation of this service calls
    /// it — enrollment belongs to SSM Quick Setup — so it exists for whoever stands an
    /// enrolled account up.
    pub fn set_just_in_time_node_access_enabled(&mut self, enabled: bool) {
        self.just_in_time_node_access_enabled = enabled;
    }

    /// Create-or-REPLACE the singleton, and record the outcome under `record`'s own
    /// `ClientToken`. The stored value is exactly `record`, never a merge with what
    /// was there before.
    pub fn put_preferences(&mut self, record: RecordingPrefsRecord) {
        let key = (IdempotentOperation::Update, record.client_token.clone());
        self.connection_recording_preferences = Some(record.clone());
        self.record_outcome(key, StoredResponse::Updated(record));
    }

    /// Remove the singleton and record the outcome under `client_token`.
    ///
    /// SUCCEEDS whether or not anything was configured: Delete is `@idempotent`, and
    /// "there are no preferences now" is the state it is asked for, not a resource it
    /// failed to find. The outcome is therefore recorded on both paths — a replay of
    /// the token answers the same way the first call did.
    ///
    /// The returned flag says whether a record was actually removed, which is what the
    /// caller audits; it is not a success/failure signal.
    pub fn remove_preferences(&mut self, client_token: &str) -> bool {
        let removed = self.connection_recording_preferences.take().is_some();
        let key = (IdempotentOperation::Delete, client_token.to_string());
        self.record_outcome(key, StoredResponse::Deleted);
        removed
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

/// The resource name this service reports the connection recording preferences
/// singleton as, in [`resource_not_found_message`]. It is the resource
/// `GetConnectionRecordingPreferences` and `DeleteConnectionRecordingPreferences`
/// address, and the shape the model names it with
/// (`smithy_query(query="shape", shape="ConnectionRecordingPreferences")`).
pub const CONNECTION_RECORDING_PREFERENCES: &str = "ConnectionRecordingPreferences";

/// This service's `ResourceNotFoundException` message: a fixed template naming the
/// resource that could not be found, never free prose about the account or Region.
///
/// The ONE place the text is built, so Get, Delete and any later reader report an
/// absent resource identically and a client can parse WHICH resource was missing.
///
/// The doubled space is the template's own: the service interpolates a resource-type
/// qualifier before `resource` that is empty for this resource, and the recorded
/// AWS text (`The following  resource cannot be found: '…'`) carries the gap it
/// leaves. Reproduced rather than tidied — a client matching the template sees the
/// service's text, not ours.
pub fn resource_not_found_message(resource: &str) -> String {
    format!("The following  resource cannot be found: '{resource}'")
}

/// Whether a `@length` bound on a member counts characters or collection values,
/// which is what decides the sentence [`constraint_message`] states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundedKind {
    /// A string member: the bound is on its length.
    Characters,
    /// A list member: the bound is on how many values it holds.
    Values,
}

/// Every `@length`-constrained member of this service's input shapes, keyed by the
/// dotted lowerCamel path the framework's boundary walk reports a violation at.
///
/// Source: `smithy_query(query="closure",
/// operation="UpdateConnectionRecordingPreferences,DeleteConnectionRecordingPreferences")`
/// on this service's model — `ClientToken` `@length(1, 64)` (both operations),
/// `ConnectionRecordingPreferences$KMSKeyArn` `@length(1, 2048)` and
/// `RecordingDestinations$S3Buckets` `@length(1, 1)`. The BOUNDS are not repeated
/// here: the violation the framework hands over already carries the bound it broke,
/// so this table cannot drift from the model's numbers.
///
/// The paths are the framework's own spelling (`path::member` lowercases a member's
/// first character only), which is why `KMSKeyArn` appears as `kMSKeyArn`.
const LENGTH_BOUNDED_MEMBERS: &[(&str, BoundedKind)] = &[
    ("clientToken", BoundedKind::Characters),
    (
        "connectionRecordingPreferences.kMSKeyArn",
        BoundedKind::Characters,
    ),
    (
        "connectionRecordingPreferences.recordingDestinations.s3Buckets",
        BoundedKind::Values,
    ),
];

/// How THIS service words a boundary `@length` refusal, or `None` when it words the
/// refusal the way the framework does.
///
/// SSM GuiConnect states a constraint violation as ONE sentence naming the member
/// path and the bound it broke — no `N validation error(s) detected` prefix, and no
/// echo of the offending value, which it withholds. The sentence is built from the
/// request (the member the caller sent and the bound the model declares for it), so
/// it carries no text keyed to any particular request.
///
/// With several violations the first one this service words is stated, because the
/// form is a single sentence rather than an envelope of clauses. A violation of
/// anything other than a `@length` bound (a `@pattern`, an absent `@required` member)
/// keeps the framework's wording: those are not the two forms recorded for this
/// service, so inventing a third would be guessing.
pub fn constraint_message(violations: &[Violation]) -> Option<String> {
    violations.iter().find_map(length_bound_sentence)
}

/// The sentence for one `@length` violation, or `None` when this service does not
/// word that violation itself.
fn length_bound_sentence(violation: &Violation) -> Option<String> {
    let (_, bounded) = LENGTH_BOUNDED_MEMBERS
        .iter()
        .find(|(path, _)| *path == violation.path)?;
    let member = model_member_path(&violation.path);
    match (bounded, &violation.kind) {
        // The recorded forms: a string's upper bound and a list's lower bound. The
        // other side of each bound is the same sentence with the other comparison,
        // which is how the two recorded sentences already differ from each other.
        (BoundedKind::Characters, ViolationKind::MaxLength(max)) => {
            Some(format!("The length for {member} cannot be greater than {max}."))
        }
        (BoundedKind::Characters, ViolationKind::MinLength(min)) => {
            Some(format!("The length for {member} cannot be less than {min}."))
        }
        (BoundedKind::Values, ViolationKind::MinLength(min)) => Some(format!(
            "{member} must contain at least {min} {}.",
            values(*min)
        )),
        (BoundedKind::Values, ViolationKind::MaxLength(max)) => Some(format!(
            "{member} cannot contain more than {max} {}.",
            values(*max)
        )),
        _ => None,
    }
}

/// `value` / `values`, agreeing with the count the sentence states.
fn values(count: i64) -> &'static str {
    if count == 1 {
        "value"
    } else {
        "values"
    }
}

/// A violation path as the MODEL spells the members it names:
/// `connectionRecordingPreferences.recordingDestinations.s3Buckets` ->
/// `ConnectionRecordingPreferences.RecordingDestinations.S3Buckets`.
///
/// The framework builds a path segment by lowercasing a member name's FIRST character
/// (`path::member`), so uppercasing it back recovers the model's own name exactly —
/// including `S3Buckets` and `KMSKeyArn`, whose later characters it never touched.
fn model_member_path(path: &str) -> String {
    path.split('.')
        .map(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(".")
}

/// AWS's denial text for an account that is not enrolled in just-in-time node access.
///
/// It is what distinguishes the FEATURE gate from an IAM denial, which is the only
/// reason the wording matters: the requirement is the rejection class (HTTP 403,
/// `AccessDeniedException`), and this is the recorded example of the gate's own
/// message.
pub const JUST_IN_TIME_NODE_ACCESS_DISABLED: &str = "Just-in-time node access is not enabled. \
     You must set up just-in-time node access to record RDP connections.";

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
            sensitive: false,
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
    /// A Delete found nothing configured, which is the state it was asked for: the
    /// call succeeded and removed nothing.
    PreferencesAlreadyAbsent,
    /// An `@idempotent` call replayed an earlier outcome and changed nothing.
    IdempotentReplay,
    /// The request was refused as invalid (400).
    ValidationRefused,
    /// The caller was refused by the account-level just-in-time node access gate
    /// (403).
    AccessDenied,
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
            Self::PreferencesAlreadyAbsent => "preferences-already-absent",
            Self::IdempotentReplay => "idempotent-replay",
            Self::ValidationRefused => "validation-refused",
            Self::AccessDenied => "access-denied",
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
