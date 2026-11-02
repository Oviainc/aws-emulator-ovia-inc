// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudcontrol-cloudapiservice service.
//!
//! Cloud Control API is a uniform control plane over resources of a named type: a
//! request names a `TypeName` and the resource's primary identifier, and every answer
//! is rendered from one of two records this store holds.
//!
//! * A [`StoredResource`] is a provisioned resource — the properties a `GetResource` or
//!   `ListResources` reports, and the properties an `UpdateResource`'s patch is applied
//!   to. It is keyed by the pair that identifies it, `(TypeName, Identifier)`, so two
//!   types may hold the same identifier and one type's listing never sees another's.
//! * A [`RequestRecord`] is one resource operation request — the `ProgressEvent` the
//!   mutating operations answer with and `GetResourceRequestStatus` resolves a
//!   `RequestToken` to. A request is answered while the operation is still running, so
//!   the record also carries the work the operation still owes
//!   ([`ResourceEffect`]) and the instant that work falls due: it is applied by
//!   whichever call arrives at or after that instant, and nothing waits for it.
//!
//! Every read goes through [`ComAmazonawsCloudcontrolCloudapiserviceState::materialise_due`]
//! first, so no two operations can disagree about which requests have completed or which
//! resources exist.

use std::collections::{BTreeMap, VecDeque};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The secret this service's `NextToken`s are authenticated under — the shared one every
/// paginated read of this emulator uses, so no two services disagree about what makes a
/// continuation token one the service ISSUED.
pub use ::service_common::page_tokens::PageTokenKey;

// ---------------------------------------------------------------------------
// The wire vocabulary
// ---------------------------------------------------------------------------

/// The `Operation` a create request reports (`Operation` enum member).
pub const OPERATION_CREATE: &str = "CREATE";
/// The `Operation` an update request reports (`Operation` enum member).
pub const OPERATION_UPDATE: &str = "UPDATE";
/// The `Operation` a delete request reports (`Operation` enum member).
pub const OPERATION_DELETE: &str = "DELETE";

/// "`IN_PROGRESS`: The resource operation is currently in progress" — the status every
/// resource operation request is answered in, because the operation it starts is still
/// running when the call returns.
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// "`SUCCESS`: The resource operation has successfully completed" — the status a request
/// reaches once the work it owes has been applied.
pub const STATUS_SUCCESS: &str = "SUCCESS";

/// "`FAILED`: The resource operation has failed. Refer to the error code and status
/// message for more information."
pub const STATUS_FAILED: &str = "FAILED";

/// How long after the call that starts it a resource operation completes.
///
/// A resource operation request is returned in a non-terminal status, so the clock owes
/// it the rest of the progression; the span is this emulator's and is deliberately not
/// zero, so a caller that polls straight away still observes the `IN_PROGRESS` status
/// and the states that go with it.
pub const RESOURCE_OPERATION_SPAN: Duration = Duration::from_secs(3);

/// How long a `ClientToken` keeps naming the request it first carried.
///
/// `CreateResource$ClientToken` states the rule: "A client token is valid for 36 hours
/// once used. After that, a resource request with the same client token is treated as a
/// new request."
pub const CLIENT_TOKEN_LIFETIME: Duration = Duration::from_secs(36 * 60 * 60);

/// The most characters an identifier may carry — the maximum length the model gives the
/// `Identifier` shape, so a value longer than this could not be reported back at all.
pub const MAX_IDENTIFIER_CHARACTERS: usize = 1024;

/// The most characters a rendered resource model may carry — the maximum length the
/// model gives the `Properties` shape, which both `ResourceDescription$Properties` and
/// `ProgressEvent$ResourceModel` target.
pub const MAX_PROPERTIES_CHARACTERS: usize = 262_144;

/// The largest page a paginated read answers — the maximum the model gives
/// `ListResources$MaxResults` (`@range(min: 1, max: 100)`), which is therefore also the
/// page a read that names no `MaxResults` answers.
pub const MAX_PAGE_SIZE: usize = 100;

/// How many resources this store holds before a create is refused.
///
/// A create is caller-driven growth, so the store is bounded; the bound is reported as
/// the service-side resource limit the model declares for exactly this.
pub const MAX_RESOURCES: usize = 10_000;

/// How many resource operation requests this store keeps.
///
/// At the bound the oldest COMPLETED record leaves to make room — a record still owing
/// work is never evicted, or the work it owes would never be applied.
pub const MAX_REQUESTS: usize = 1_000;

/// How many `ClientToken` records this store keeps, so a caller cannot grow the store by
/// inventing tokens. At the bound the oldest leaves, which only costs that token its
/// idempotent replay.
pub const MAX_CLIENT_TOKENS: usize = 1_000;

/// How many candidates [`ComAmazonawsCloudcontrolCloudapiserviceState::allocate_request_token`]
/// draws before giving up, so the allocation cannot spin.
const TOKEN_ALLOCATION_ATTEMPTS: usize = 8;

/// What a handler says when this service's store is not mounted on the engine it was
/// built from. Reported as the modelled server-side failure, never as a fresh store.
pub const STORE_NOT_MOUNTED: &str =
    "the Cloud Control API resource store is not mounted on this engine";

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// The pair that identifies a provisioned resource: its type, then its primary
/// identifier. Ordered type-first, so one type's resources are contiguous and a listing
/// of that type is a range scan in identifier order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceKey {
    /// The `TypeName` the resource was created under.
    pub type_name: String,
    /// The resource's primary identifier.
    pub identifier: String,
}

impl ResourceKey {
    /// The key of `identifier` under `type_name`.
    pub fn new(type_name: &str, identifier: &str) -> Self {
        Self {
            type_name: type_name.to_string(),
            identifier: identifier.to_string(),
        }
    }
}

/// One provisioned resource: the properties the reads report, and the property its
/// primary identifier is carried by.
#[derive(Debug, Clone)]
pub struct StoredResource {
    /// The `TypeName` this resource belongs to.
    pub type_name: String,
    /// The primary identifier, which is also the value of [`Self::identifier_property`]
    /// inside [`Self::properties`].
    pub identifier: String,
    /// The property of the resource model that carries the primary identifier.
    pub identifier_property: String,
    /// The current resource model: a JSON object of each property and its value.
    pub properties: Value,
    /// The `TypeVersionId` the create named, when it named one.
    pub type_version_id: Option<String>,
    /// When the create that provisioned this resource completed.
    pub created_at: SystemTime,
    /// When the last operation that changed this resource completed.
    pub updated_at: SystemTime,
}

/// The work a resource operation request still owes the store.
#[derive(Debug, Clone)]
pub enum ResourceEffect {
    /// Provision the resource with these properties.
    Create {
        /// The resource model to store, identifier property included.
        properties: Value,
        /// The property that model carries the primary identifier in.
        identifier_property: String,
        /// The `TypeVersionId` the create named, when it named one.
        type_version_id: Option<String>,
    },
    /// Replace the resource's properties with the patched model.
    Update {
        /// The resource model the patch produced.
        properties: Value,
    },
    /// Remove the resource from the store.
    Delete,
}

/// The work a request owes, and the instant it falls due.
#[derive(Debug, Clone)]
struct PendingWork {
    /// When the work becomes due. Stored, never waited for.
    due: SystemTime,
    /// What is applied then.
    effect: ResourceEffect,
}

/// One resource operation request, as `ProgressEvent` reports it.
#[derive(Debug, Clone)]
pub struct RequestRecord {
    /// The token the client was handed, and resolves this record by.
    pub request_token: String,
    /// The `TypeName` the request named.
    pub type_name: String,
    /// The primary identifier the request acts on.
    pub identifier: Option<String>,
    /// `CREATE`, `UPDATE` or `DELETE`.
    pub operation: String,
    /// Where the request has got to.
    pub operation_status: String,
    /// "When the resource operation request was initiated" — set once, at the call that
    /// started it, and never moved.
    pub event_time: SystemTime,
    /// The rendered resource model the request left behind, once it completed.
    pub resource_model: Option<String>,
    /// Any message explaining the current status.
    pub status_message: Option<String>,
    /// For a `FAILED` request, the handler error code.
    pub error_code: Option<String>,
    /// "When to next request the status of this resource operation request" — the
    /// instant the work below falls due, while there is work left.
    pub retry_after: Option<SystemTime>,
    /// The work still owed, or `None` once it has been applied.
    pending: Option<PendingWork>,
}

impl RequestRecord {
    /// A request that has just been accepted: `IN_PROGRESS`, owing `effect` one
    /// [`RESOURCE_OPERATION_SPAN`] from `now`.
    pub fn accepted(
        request_token: String,
        type_name: String,
        identifier: Option<String>,
        operation: &str,
        now: SystemTime,
        effect: ResourceEffect,
    ) -> Self {
        let due = now + RESOURCE_OPERATION_SPAN;
        Self {
            request_token,
            type_name,
            identifier,
            operation: operation.to_string(),
            operation_status: STATUS_IN_PROGRESS.to_string(),
            event_time: now,
            // The resource model the request leaves behind is what the store holds once
            // the work has been applied, so there is none to report yet.
            resource_model: None,
            status_message: None,
            error_code: None,
            retry_after: Some(due),
            pending: Some(PendingWork { due, effect }),
        }
    }

    /// Whether this request still owes the store work.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// What one `ClientToken` has been used for.
#[derive(Debug, Clone)]
pub struct ClientTokenEntry {
    /// The request this token carried.
    pub request_token: String,
    /// A digest of that request's caller-owned members ([`request_fingerprint`]), so a
    /// reuse with different parameters is told from a retry.
    pub request_fingerprint: String,
    /// When the token was first used, which is what [`CLIENT_TOKEN_LIFETIME`] runs from.
    pub used_at: SystemTime,
}

/// What this store says about a `ClientToken` a mutating operation was handed.
#[derive(Debug, PartialEq, Eq)]
pub enum ClientTokenVerdict {
    /// This store has never served this token (or the request carries none), so the
    /// operation goes ahead.
    Fresh,
    /// This token already carried THIS request: the named request is replayed rather
    /// than a second one started.
    Replay(String),
    /// This token already carried a DIFFERENT request.
    Conflict,
}

/// Why a request could not be recorded.
#[derive(Debug)]
pub struct RequestStoreFull;

/// Why a request's owed work could not be applied when it fell due.
///
/// A request is accepted against the store as it was at the call and its work applied
/// when the clock makes it due; this is what a record reports when the store has moved in
/// a way that leaves that work nothing to do.
#[derive(Debug, PartialEq, Eq)]
enum EffectFailure {
    /// The resource the request was accepted against is no longer in the store.
    ResourceGone,
}

impl EffectFailure {
    /// The `HandlerErrorCode` the record reports (`ProgressEvent$ErrorCode`): `NotFound`
    /// is the handler error code for a resource that could not be found, which is what a
    /// request accepted against a resource since removed finds.
    fn error_code(&self) -> &'static str {
        match self {
            Self::ResourceGone => "NotFound",
        }
    }

    /// The `StatusMessage` the record reports, naming the resource the work was owed on.
    fn status_message(&self, record: &RequestRecord) -> String {
        match self {
            Self::ResourceGone => resource_not_found_message(
                &record.type_name,
                record.identifier.as_deref().unwrap_or_default(),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// Per-service state for com-amazonaws-cloudcontrol-cloudapiservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsCloudcontrolCloudapiserviceState {
    /// Provisioned resources, by type then primary identifier.
    resources: BTreeMap<ResourceKey, StoredResource>,
    /// Every resource operation request, by the token handed to the client.
    requests: BTreeMap<String, RequestRecord>,
    /// Those tokens in the order the requests were accepted, so the oldest completed
    /// record is the one that leaves at [`MAX_REQUESTS`].
    request_order: VecDeque<String>,
    /// `ClientToken` -> what it has been used for.
    client_tokens: BTreeMap<String, ClientTokenEntry>,
    /// Those client tokens in first-use order, so the oldest leaves at
    /// [`MAX_CLIENT_TOKENS`].
    client_token_order: VecDeque<String>,
    /// The secret this store's `NextToken`s are authenticated under. PRIVATE, and
    /// reached only through [`Self::page_token_key`]: a key a caller could read or
    /// replace would no longer tell a token this service minted from one it did not.
    page_token_key: PageTokenKey,
}

impl Default for ComAmazonawsCloudcontrolCloudapiserviceState {
    fn default() -> Self {
        Self {
            // Nothing exists until a create provisions it.
            resources: BTreeMap::new(),
            requests: BTreeMap::new(),
            request_order: VecDeque::new(),
            client_tokens: BTreeMap::new(),
            client_token_order: VecDeque::new(),
            // One key per store, from the system's randomness: see `PageTokenKey`.
            page_token_key: PageTokenKey::new(),
        }
    }
}

impl ComAmazonawsCloudcontrolCloudapiserviceState {
    /// Apply everything the CLOCK already owes this store at `now`.
    ///
    /// Two clock-driven changes, neither of which depends on which call arrives first:
    /// every accepted request whose work has fallen due is applied and its status moves
    /// to `SUCCESS`, and every `ClientToken` past [`CLIENT_TOKEN_LIFETIME`] is forgotten.
    /// Both were owed at `now` whether or not anybody called, which is why a reporting
    /// operation may make them.
    ///
    /// Every operation of this service calls this before it reads anything, so no two of
    /// them can disagree about which requests have completed or which resources exist.
    pub fn materialise_due(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .requests
            .iter()
            .filter(|(_, record)| {
                record
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.due <= now)
            })
            .map(|(token, _)| token.clone())
            .collect();
        for token in due {
            // Taken out of the map so the effect below can write the resource store
            // while the record is being settled, and put back once it is.
            let Some(mut record) = self.requests.remove(&token) else {
                continue;
            };
            if let Some(pending) = record.pending.take() {
                match self.apply(&record, pending.effect, now) {
                    Ok(resource_model) => {
                        record.resource_model = resource_model;
                        record.operation_status = STATUS_SUCCESS.to_string();
                    }
                    Err(failure) => {
                        // The work could not be applied, so the record reports what
                        // actually happened rather than a success nothing performed.
                        record.operation_status = STATUS_FAILED.to_string();
                        record.error_code = Some(failure.error_code().to_string());
                        record.status_message = Some(failure.status_message(&record));
                    }
                }
            } else {
                record.operation_status = STATUS_SUCCESS.to_string();
            }
            // Nothing is left to poll for.
            record.retry_after = None;
            self.requests.insert(token, record);
        }
        self.forget_expired_client_tokens(now);
    }

    /// Apply one request's owed work, answering the resource model the request leaves
    /// behind (none, for a delete: there is no resource left to report) — or why the work
    /// could not be applied.
    fn apply(
        &mut self,
        record: &RequestRecord,
        effect: ResourceEffect,
        now: SystemTime,
    ) -> Result<Option<String>, EffectFailure> {
        let identifier = record.identifier.clone().unwrap_or_default();
        let key = ResourceKey::new(&record.type_name, &identifier);
        match effect {
            ResourceEffect::Create {
                properties,
                identifier_property,
                type_version_id,
            } => {
                let model = rendered_properties(&properties);
                self.resources.insert(
                    key,
                    StoredResource {
                        type_name: record.type_name.clone(),
                        identifier,
                        identifier_property,
                        properties,
                        type_version_id,
                        created_at: now,
                        updated_at: now,
                    },
                );
                Ok(Some(model))
            }
            ResourceEffect::Update { properties } => {
                // The resource this update was accepted against is gone, so there is
                // nothing to patch: the request FAILED, and reporting it as anything else
                // would claim a write that was discarded.
                let stored = self
                    .resources
                    .get_mut(&key)
                    .ok_or(EffectFailure::ResourceGone)?;
                stored.properties = properties;
                stored.updated_at = now;
                Ok(Some(rendered_properties(&stored.properties)))
            }
            ResourceEffect::Delete => {
                self.resources.remove(&key);
                Ok(None)
            }
        }
    }

    /// Forget every `ClientToken` whose [`CLIENT_TOKEN_LIFETIME`] has elapsed, so a
    /// request carrying it again is treated as a new request.
    fn forget_expired_client_tokens(&mut self, now: SystemTime) {
        self.client_tokens
            .retain(|_, entry| entry.used_at + CLIENT_TOKEN_LIFETIME > now);
        self.client_token_order
            .retain(|token| self.client_tokens.contains_key(token));
    }

    /// The resource `identifier` of `type_name`, or `None` when this store holds none —
    /// a resource of ANOTHER type never answers here, which is what makes the same
    /// identifier under two types two resources.
    pub fn resource(&self, type_name: &str, identifier: &str) -> Option<&StoredResource> {
        self.resources.get(&ResourceKey::new(type_name, identifier))
    }

    /// Every resource of `type_name`, in identifier order.
    ///
    /// A range scan bounded by the type: one type's keys are contiguous, so this reads
    /// exactly that type's entries and stops at the first key belonging to another.
    pub fn resources_of_type<'a>(
        &'a self,
        type_name: &'a str,
    ) -> impl Iterator<Item = &'a StoredResource> + 'a {
        // `Identifier` is at least one character long, so the empty identifier sorts
        // before every real one and excluding it excludes nothing.
        let start = ResourceKey::new(type_name, "");
        self.resources
            .range((Bound::Excluded(start), Bound::Unbounded))
            .take_while(move |(key, _)| key.type_name == type_name)
            .map(|(_, stored)| stored)
    }

    /// How many resources this store holds — what [`MAX_RESOURCES`] bounds.
    pub fn resource_count(&self) -> usize {
        self.resources.len()
    }

    /// Whether a resource operation is still running against `identifier` of
    /// `type_name`, which is what makes a second operation on the same resource the
    /// modelled concurrent-operation refusal.
    pub fn operation_in_flight(&self, type_name: &str, identifier: &str) -> bool {
        self.requests.values().any(|record| {
            record.is_pending()
                && record.type_name == type_name
                && record.identifier.as_deref() == Some(identifier)
        })
    }

    /// The request `request_token` names, or `None` when this store holds none.
    pub fn request(&self, request_token: &str) -> Option<&RequestRecord> {
        self.requests.get(request_token)
    }

    /// A `RequestToken` this store holds no request under.
    ///
    /// Answers `None` when [`TOKEN_ALLOCATION_ATTEMPTS`] candidates were all taken, so
    /// the allocation cannot spin.
    pub fn allocate_request_token(&self) -> Option<String> {
        (0..TOKEN_ALLOCATION_ATTEMPTS)
            .map(|_| mint_token())
            .find(|candidate| !self.requests.contains_key(candidate))
    }

    /// Record `record`, evicting the oldest COMPLETED request when the store is at
    /// [`MAX_REQUESTS`].
    ///
    /// A record that still owes work is never evicted, or that work would never be
    /// applied; a store whose every record is still running therefore has no room, which
    /// is the throttling answer rather than a silently dropped operation.
    pub fn record_request(&mut self, record: RequestRecord) -> Result<(), RequestStoreFull> {
        while self.request_order.len() >= MAX_REQUESTS {
            let evictable = self.request_order.iter().position(|token| {
                self.requests
                    .get(token)
                    .is_none_or(|held| !held.is_pending())
            });
            let Some(position) = evictable else {
                return Err(RequestStoreFull);
            };
            if let Some(evicted) = self.request_order.remove(position) {
                self.requests.remove(&evicted);
            }
        }
        self.request_order.push_back(record.request_token.clone());
        self.requests.insert(record.request_token.clone(), record);
        Ok(())
    }

    /// What this store says about `client_token` for a request whose caller-owned
    /// members digest to `fingerprint`.
    ///
    /// The one rule all three mutating operations apply: a token that already carried
    /// the same request replays THAT request instead of starting a second one, and a
    /// token reused for a different request is refused rather than answered with an
    /// unrelated record. A token whose request record has since been evicted names
    /// nothing to replay, so it is treated as a new request.
    pub fn judge_client_token(
        &self,
        client_token: Option<&str>,
        fingerprint: &str,
    ) -> ClientTokenVerdict {
        let Some(token) = client_token else {
            return ClientTokenVerdict::Fresh;
        };
        let Some(entry) = self.client_tokens.get(token) else {
            return ClientTokenVerdict::Fresh;
        };
        if entry.request_fingerprint != fingerprint {
            return ClientTokenVerdict::Conflict;
        }
        if self.requests.contains_key(&entry.request_token) {
            return ClientTokenVerdict::Replay(entry.request_token.clone());
        }
        ClientTokenVerdict::Fresh
    }

    /// Record that `client_token` carried the request `request_token`, so a retry
    /// carrying it again replays that request.
    pub fn remember_client_token(
        &mut self,
        client_token: Option<&str>,
        fingerprint: String,
        request_token: &str,
        now: SystemTime,
    ) {
        let Some(token) = client_token else {
            return;
        };
        while self.client_token_order.len() >= MAX_CLIENT_TOKENS {
            let Some(oldest) = self.client_token_order.pop_front() else {
                break;
            };
            self.client_tokens.remove(&oldest);
        }
        if self
            .client_tokens
            .insert(
                token.to_string(),
                ClientTokenEntry {
                    request_token: request_token.to_string(),
                    request_fingerprint: fingerprint,
                    used_at: now,
                },
            )
            .is_none()
        {
            self.client_token_order.push_back(token.to_string());
        }
    }

    /// The key this store's paginated read mints and recognises its `NextToken`s under.
    /// Copied out rather than borrowed, so a read can check the token it was handed and
    /// go on to borrow the resources it is about to report in the same call.
    pub(crate) fn page_token_key(&self) -> PageTokenKey {
        self.page_token_key
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudcontrolCloudapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudcontrolCloudapiserviceState>)
}

// ---------------------------------------------------------------------------
// The audit record a state-changing call leaves
// ---------------------------------------------------------------------------

/// The region a request is signed for, read from its SigV4 credential scope.
///
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, which carries no scope. The
/// scope is caller-controlled text, so anything that is not an AWS region token is
/// refused here and the default used instead: a record never carries arbitrary text a
/// caller chose.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_CHARACTERS
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The most characters a credential scope's region may carry to be read as one:
/// comfortably above every published AWS region name.
pub const MAX_REGION_CHARACTERS: usize = 64;

/// The region an unsigned request is taken to have addressed — the region the emulator's
/// generated client configuration uses.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The tracing target every audit record this service emits carries.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// How a record names the resource a call acted on: the pair that identifies it.
pub fn resource_label(type_name: &str, identifier: &str) -> String {
    format!("{type_name}/{identifier}")
}

/// Emit the structured audit record a state-changing call owes: the operation, the
/// resource it acts on, the request it started, how the call ended, and which credential
/// was presented for it.
///
/// Only IDENTIFIERS are recorded. The resource model and the patch document a request
/// carries are values the caller may hold secrets in — the model marks both members
/// sensitive — so neither appears here; nor does any header beyond the access key id
/// that names the caller. The subscriber stamps the time.
pub fn audit(
    parts: &::http::request::Parts,
    operation: &str,
    resource: &str,
    request_token: &str,
    outcome: &str,
) {
    ::tracing::info!(
        target: AUDIT_TARGET,
        operation,
        resource,
        request_token,
        outcome,
        region = %request_region(parts),
        access_key_id = %::engine::extract_akid(&parts.headers),
        "Cloud Control API resource operation"
    );
}

/// The outcome a record carries for a call that started a resource operation.
pub const OUTCOME_ACCEPTED: &str = "accepted";

/// The outcome a record carries for a call whose `ClientToken` replayed a request this
/// service had already started, so the call changed nothing.
pub const OUTCOME_REPLAYED: &str = "replayed";

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// A fresh token, in the form Cloud Control API hands one out: a random UUID written in
/// the canonical hyphenated form, which satisfies the `RequestToken` pattern
/// (`^[-A-Za-z0-9+/=]+$`, 1..128) and the `Identifier` pattern (`^.+$`, 1..1024) alike.
///
/// Every `RequestToken` the service hands out, and every primary identifier it assigns
/// itself, comes from here.
pub fn mint_token() -> String {
    ::uuid::Uuid::new_v4().hyphenated().to_string()
}

/// The resource's own name inside a `TypeName` — the third `::` segment of
/// `<Organization>::<Service>::<Resource>`.
pub fn resource_segment(type_name: &str) -> &str {
    type_name.split("::").nth(2).unwrap_or(type_name)
}

/// The properties a resource type's primary identifier can be carried by, most specific
/// first.
///
/// A resource type's schema names its own primary identifier, and across the registry
/// that property is named after the resource the type describes — `BucketName` for
/// `AWS::S3::Bucket`, `LogGroupName` for `AWS::Logs::LogGroup`, `VpcId` for
/// `AWS::EC2::VPC` — falling back to the bare `Name`/`Id`/`Arn` a type uses when the
/// resource's name would only repeat itself. The order is most specific first, so a
/// model carrying both `RoleName` and `Arn` is identified by the first.
///
/// A candidate is matched against the model's property names IGNORING ASCII case
/// ([`derive_identifier`]): a type name spells an acronym in full where the property
/// spells it as a word (`AWS::EC2::VPC` carries `VpcId`), and that is a difference in
/// spelling rather than a different property.
pub fn identifier_property_candidates(type_name: &str) -> Vec<String> {
    let resource = resource_segment(type_name);
    vec![
        format!("{resource}Name"),
        format!("{resource}Id"),
        format!("{resource}Arn"),
        "Name".to_string(),
        "Id".to_string(),
        "Arn".to_string(),
    ]
}

/// The property a SERVER-ASSIGNED primary identifier is recorded under, for a type whose
/// desired state names none: the type's own id property, which is how the registry names
/// an identifier the service allocates rather than the caller.
pub fn assigned_identifier_property(type_name: &str) -> String {
    format!("{}Id", resource_segment(type_name))
}

/// Where a resource's primary identifier came from.
#[derive(Debug, PartialEq, Eq)]
pub enum DerivedIdentifier {
    /// The desired state names it, under `property`.
    Carried {
        /// The property that carries it.
        property: String,
        /// The identifier itself.
        identifier: String,
    },
    /// The desired state names none, so the service assigned one under `property`.
    Assigned {
        /// The property it is recorded under.
        property: String,
        /// The identifier itself.
        identifier: String,
    },
    /// `property` is the type's primary identifier but holds something that cannot be
    /// one: not a scalar, empty, or longer than [`MAX_IDENTIFIER_CHARACTERS`].
    Unusable {
        /// The offending property.
        property: String,
    },
}

/// The identifier text a property's value carries, or `None` when its value cannot be
/// an identifier: not a scalar, empty, or longer than [`MAX_IDENTIFIER_CHARACTERS`].
///
/// An identifier travels on the wire as text, so a numeric or boolean property still
/// names the resource — and this is the ONE rule that says so, applied both where a
/// create derives the identifier and where an update checks the patch left it alone.
pub fn identifier_text(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => return None,
    };
    (!text.is_empty() && text.chars().count() <= MAX_IDENTIFIER_CHARACTERS).then_some(text)
}

/// The primary identifier of the resource `properties` describes, by the one rule every
/// create applies ([`identifier_property_candidates`]).
pub fn derive_identifier(properties: &Value, type_name: &str) -> DerivedIdentifier {
    for candidate in identifier_property_candidates(type_name) {
        let Some((property, value)) = property_named(properties, &candidate) else {
            continue;
        };
        return match identifier_text(value) {
            Some(identifier) => DerivedIdentifier::Carried {
                property,
                identifier,
            },
            None => DerivedIdentifier::Unusable { property },
        };
    }
    DerivedIdentifier::Assigned {
        property: assigned_identifier_property(type_name),
        identifier: mint_token(),
    }
}

/// The property of `properties` named `candidate` ignoring ASCII case, with the name as
/// the model actually spells it — so a refusal and the record both name the property the
/// caller sent rather than the candidate that matched it.
fn property_named<'a>(properties: &'a Value, candidate: &str) -> Option<(String, &'a Value)> {
    properties
        .as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(candidate))
        .map(|(name, value)| (name.clone(), value))
}

/// A digest over the caller-owned members of one request, length-prefixed so no two
/// different member sets can digest alike and absence is distinguished from the empty
/// string.
///
/// This is what a `ClientToken`'s record holds, so a reuse with different parameters is
/// told from a retry without the store keeping the request bodies themselves.
pub fn request_fingerprint(members: &[Option<&str>]) -> String {
    let mut digest = Sha256::new();
    for member in members {
        match member {
            Some(text) => {
                digest.update([1_u8]);
                digest.update((text.len() as u64).to_be_bytes());
                digest.update(text.as_bytes());
            }
            None => digest.update([0_u8]),
        }
    }
    ::hex::encode(digest.finalize())
}

// ---------------------------------------------------------------------------
// The resource model
// ---------------------------------------------------------------------------

/// The resource model `desired_state` describes, or the detail of why it is not one.
///
/// `DesiredState` is "structured data format representing the desired state of the
/// resource, consisting of that resource's properties and their desired values. Cloud
/// Control API currently supports JSON as a structured data format", so it is read by
/// the JSON reader and has to be an object of properties. The reader's own recursion
/// limit is what bounds how deeply nested a caller's document may be.
pub fn parse_properties(member: &str, desired_state: &str) -> Result<Value, String> {
    let parsed: Value = ::serde_json::from_str(desired_state)
        .map_err(|_| format!("{member} is not valid JSON"))?;
    if !parsed.is_object() {
        return Err(format!("{member} must be a JSON object of resource properties"));
    }
    Ok(parsed)
}

/// One resource model as the wire carries it: a JSON string of each property and its
/// current value.
pub fn rendered_properties(properties: &Value) -> String {
    // `Value`'s own rendering is infallible, so a resource model is never unreportable.
    properties.to_string()
}

/// The most operations one `PatchDocument` may carry.
///
/// A patch document is bounded by the model at 262144 characters, but what RFC 6902's
/// `copy` and `move` operations can DUPLICATE is not bounded by that: each one can clone
/// a whole subtree, so a short document can describe a document that grows
/// multiplicatively. The work one request can drive is therefore bounded here by the
/// operation count as well as by the size of the document between every pair of them
/// ([`apply_patch`]).
///
/// This limit is the EMULATOR's, not the service's: nothing AWS publishes caps a patch
/// document's operation count, so a document past this is refused as an invalid request
/// rather than with a code or wording the service has for it.
pub const MAX_PATCH_OPERATIONS: usize = 100;

/// Why a patch document was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum PatchRefusal {
    /// The document carries more operations than a patch may.
    TooManyOperations,
    /// One of its operations could not be applied to the model.
    NotApplicable,
    /// The model it produced is not an object of properties.
    NotAnObject,
    /// The model it produced is longer than one this service can report.
    TooLarge,
}

impl PatchRefusal {
    /// What the refusal states, behind the invalid-request prefix
    /// ([`invalid_request_message`]).
    pub fn detail(&self) -> String {
        match self {
            Self::TooManyOperations => format!(
                "PatchDocument must carry at most {MAX_PATCH_OPERATIONS} patch operations"
            ),
            Self::NotApplicable => {
                "the patch document could not be applied to the resource model".to_string()
            }
            Self::NotAnObject => {
                "the patch document would leave a resource model that is not a JSON object"
                    .to_string()
            }
            Self::TooLarge => format!(
                "the patch document would leave a resource model longer than \
                 {MAX_PROPERTIES_CHARACTERS} characters"
            ),
        }
    }
}

/// Apply `patch` to `properties`, ONE operation at a time, holding the document to the
/// `Properties` bound after every one of them.
///
/// The bound is checked BETWEEN operations rather than only at the end, because that is
/// the only place it bounds the work: `copy` and `move` duplicate a whole subtree, so a
/// document measured only once could have grown by a factor per operation on the way
/// there. With the check between each pair, the largest document that can exist is one
/// operation's growth past the bound, and the operation count caps how many such steps a
/// request can ask for ([`MAX_PATCH_OPERATIONS`]).
///
/// On a refusal `properties` may hold the operations applied before it, so a caller
/// applies this to a COPY of the stored model and discards it unless the whole patch
/// succeeded.
pub fn apply_patch(properties: &mut Value, patch: &::json_patch::Patch) -> Result<(), PatchRefusal> {
    if patch.0.len() > MAX_PATCH_OPERATIONS {
        return Err(PatchRefusal::TooManyOperations);
    }
    for operation in &patch.0 {
        let step = ::json_patch::Patch(vec![operation.clone()]);
        ::json_patch::patch(properties, &step).map_err(|_| PatchRefusal::NotApplicable)?;
        if !properties_within_bound(properties) {
            return Err(PatchRefusal::TooLarge);
        }
    }
    // Only the WHOLE patch has to leave a resource model: an intermediate value of any
    // shape is RFC 6902's business, and a later operation may put an object back.
    if !properties.is_object() {
        return Err(PatchRefusal::NotAnObject);
    }
    Ok(())
}

/// Whether `properties` renders inside [`MAX_PROPERTIES_CHARACTERS`].
///
/// Measured through a counting sink that refuses the first byte past the bound, so a
/// document far past it is abandoned where it is crossed instead of rendered whole, and
/// measuring one allocates nothing.
fn properties_within_bound(properties: &Value) -> bool {
    let mut sink = BoundedSink {
        written: 0,
        limit: MAX_PROPERTIES_CHARACTERS,
    };
    ::serde_json::to_writer(&mut sink, properties).is_ok()
}

/// A sink that counts what is written to it and stops accepting at `limit`.
struct BoundedSink {
    /// How many bytes have been offered so far.
    written: usize,
    /// The most it accepts.
    limit: usize,
}

impl ::std::io::Write for BoundedSink {
    fn write(&mut self, buf: &[u8]) -> ::std::io::Result<usize> {
        self.written = self.written.saturating_add(buf.len());
        if self.written > self.limit {
            return Err(::std::io::Error::other("past the size bound"));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> ::std::io::Result<()> {
        Ok(())
    }
}

/// Whether `properties` carries every member of `selector` with the same value — the
/// rule `ListResources$ResourceModel` ("the resource model to use to select the
/// resources to return") selects by, so a listing narrowed by a partial model reports
/// the resources that match it and no others.
pub fn properties_match_selector(properties: &Value, selector: &Value) -> bool {
    let Some(selector) = selector.as_object() else {
        return true;
    };
    selector
        .iter()
        .all(|(property, wanted)| properties.get(property) == Some(wanted))
}

// ---------------------------------------------------------------------------
// Refusal wording
// ---------------------------------------------------------------------------

/// How every operation of this service words "a resource with the specified identifier
/// can't be found", with the type and identifier the request named.
pub fn resource_not_found_message(type_name: &str, identifier: &str) -> String {
    format!("Resource of type '{type_name}' with identifier '{identifier}' was not found.")
}

/// How a create words "the resource with the name requested already exists".
pub fn already_exists_message(type_name: &str, identifier: &str) -> String {
    format!("Resource of type '{type_name}' with identifier '{identifier}' already exists.")
}

/// How a mutating operation words a refusal caused by the request itself, with the
/// prefix Cloud Control API states an invalid request behind.
pub fn invalid_request_message(detail: &str) -> String {
    format!("Invalid request provided: {detail}")
}

/// How a mutating operation words an attempt to change a create-only property, naming
/// the property the request would have changed.
pub fn not_updatable_message(property: &str) -> String {
    format!("Cannot update create-only property '{property}'.")
}

/// How a mutating operation words "another resource operation is currently being
/// performed on this resource".
pub const CONCURRENT_OPERATION_MESSAGE: &str =
    "Another resource operation is currently being performed on this resource.";

/// How a mutating operation words "the resource is temporarily unavailable to be acted
/// upon".
pub const RESOURCE_CONFLICT_MESSAGE: &str =
    "The resource is temporarily unavailable to be acted upon.";

/// How a mutating operation words "the specified client token has already been used in
/// another resource request".
pub const CLIENT_TOKEN_CONFLICT_MESSAGE: &str =
    "The specified client token has already been used in another resource request.";

/// How `GetResourceRequestStatus` words a token no request of this store answers to.
pub const REQUEST_TOKEN_NOT_FOUND_MESSAGE: &str =
    "A resource operation with the specified request token can't be found.";

/// How a mutating operation words a request it has no room to accept.
pub const THROTTLING_MESSAGE: &str = "Rate exceeded";

/// How a create words the service-side resource limit this store enforces.
pub fn resource_limit_message(type_name: &str) -> String {
    format!(
        "The account already holds the maximum of {MAX_RESOURCES} resources, so no resource of \
         type '{type_name}' can be created."
    )
}

// ---------------------------------------------------------------------------
// Wire rendering
// ---------------------------------------------------------------------------

/// The `ProgressEvent` one request record reports as.
///
/// Shared by the three mutating operations and `GetResourceRequestStatus`, so the event
/// a create answers with and the event a later status call reports cannot disagree about
/// one request.
pub fn progress_event(record: &RequestRecord) -> crate::ops::types::ProgressEvent {
    crate::ops::types::ProgressEvent {
        type_name: Some(record.type_name.clone()),
        identifier: record.identifier.clone(),
        request_token: Some(record.request_token.clone()),
        // Hooks are configured on the CloudFormation registry, and this emulator holds
        // none, so no request drives a Hook invocation to report a token for.
        hooks_request_token: None,
        operation: Some(record.operation.clone()),
        operation_status: Some(record.operation_status.clone()),
        event_time: Some(::aws_smithy_types::DateTime::from(record.event_time)),
        resource_model: record.resource_model.clone(),
        status_message: record.status_message.clone(),
        error_code: record.error_code.clone(),
        retry_after: record
            .retry_after
            .map(::aws_smithy_types::DateTime::from),
    }
}

/// The `ResourceDescription` one stored resource reports as.
///
/// Shared by `GetResource` and `ListResources`, so the two cannot report different
/// properties for one resource.
pub fn resource_description(stored: &StoredResource) -> crate::ops::types::ResourceDescription {
    crate::ops::types::ResourceDescription {
        identifier: Some(stored.identifier.clone()),
        properties: Some(rendered_properties(&stored.properties)),
    }
}

// ---------------------------------------------------------------------------
// Pagination tokens
// ---------------------------------------------------------------------------

/// The byte a page cursor puts between the type a listing is of and the identifier it
/// resumes after. A unit separator cannot occur in a `TypeName`, whose pattern admits
/// only alphanumerics and `::`.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// The cursor that resumes a listing of `type_name` after `identifier`.
pub fn page_cursor(type_name: &str, identifier: &str) -> String {
    format!("{type_name}{CURSOR_SEPARATOR}{identifier}")
}

/// Encode `cursor` as an opaque `NextToken` this store can later recognise as its own:
/// the authenticated bytes of the cursor, hex-encoded.
pub fn encode_page_token(key: &PageTokenKey, cursor: &str) -> String {
    ::hex::encode(key.mint(cursor))
}

/// The identifier a `NextToken` resumes a listing of `type_name` after, or `None` for
/// any token THIS store did not mint for THAT type — one that is not hex, one too short
/// to carry an authenticator, one whose authenticator does not match the cursor it
/// travels with, and one minted for another type's listing.
pub fn decode_page_token(key: &PageTokenKey, type_name: &str, token: &str) -> Option<String> {
    let cursor = key.cursor_of(&::hex::decode(token).ok()?)?;
    let (cursor_type, identifier) = cursor.split_once(CURSOR_SEPARATOR)?;
    (cursor_type == type_name).then(|| identifier.to_string())
}

/// The position a request's `NextToken` names, or `None` when it names none.
///
/// The framework binds an absent member to `None` and an empty one to `Some("")`; an
/// empty token names no position, so a read takes it for no token at all and answers the
/// first page rather than refusing it.
pub fn requested_page_token(token: Option<&str>) -> Option<&str> {
    token.filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The constants this service writes onto the wire are values the model's own enums
    /// declare, so a typo cannot reach a client.
    #[test]
    fn the_wire_vocabulary_is_the_models_own() {
        for operation in [OPERATION_CREATE, OPERATION_UPDATE, OPERATION_DELETE] {
            assert!(
                crate::ops::types::OPERATION_VALUES.contains(&operation),
                "operation {operation:?}: not a value the model's Operation enum declares"
            );
        }
        for status in [STATUS_IN_PROGRESS, STATUS_SUCCESS, STATUS_FAILED] {
            assert!(
                crate::ops::types::OPERATION_STATUS_VALUES.contains(&status),
                "status {status:?}: not a value the model's OperationStatus enum declares"
            );
        }
        let code = EffectFailure::ResourceGone.error_code();
        assert!(
            crate::ops::types::HANDLER_ERROR_CODE_VALUES.contains(&code),
            "error code {code:?}: not a value the model's HandlerErrorCode enum declares"
        );
    }

    /// A page token round-trips under the key and the type that minted it, and under
    /// nothing else.
    #[test]
    fn a_page_cursor_round_trips_only_for_the_type_it_was_minted_for() {
        let key = PageTokenKey::new();
        let token = encode_page_token(&key, &page_cursor("AWS::S3::Bucket", "demo-bucket"));
        assert_eq!(
            decode_page_token(&key, "AWS::S3::Bucket", &token).as_deref(),
            Some("demo-bucket"),
            "the cursor must round-trip for the type it was minted for"
        );
        assert_eq!(
            decode_page_token(&key, "AWS::Logs::LogGroup", &token),
            None,
            "a token minted for one type must not resume another type's listing"
        );
        assert_eq!(
            decode_page_token(&PageTokenKey::new(), "AWS::S3::Bucket", &token),
            None,
            "a token minted under another key must not decode"
        );
        assert_eq!(
            decode_page_token(&key, "AWS::S3::Bucket", "not-hex"),
            None,
            "a token a caller wrote must not decode"
        );
    }

    /// Absence and the empty string are different member sets, so a retry that drops a
    /// member is not mistaken for the request that sent it empty.
    #[test]
    fn a_fingerprint_distinguishes_absence_from_emptiness() {
        let absent = request_fingerprint(&[Some("CreateResource"), None]);
        let empty = request_fingerprint(&[Some("CreateResource"), Some("")]);
        assert_ne!(
            absent, empty,
            "an absent member must not digest like an empty one"
        );
        assert_eq!(
            absent,
            request_fingerprint(&[Some("CreateResource"), None]),
            "the same member set must digest alike"
        );
    }
}
