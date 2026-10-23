// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networkflowmonitor-networkflowmonitor service.
//!
//! The records here are the ONLY place a Network Flow Monitor scope or monitor
//! exists: every implemented handler in `src/ops/` mints into this store or reads
//! out of it, so a create is observable by the next get/list and a delete by the
//! next reader.
//!
//! Two things this slice deliberately does NOT emulate, so a reader does not
//! mistake a modelled error for reachable behaviour: authorization (every
//! operation models `AccessDeniedException`, and the emulator holds no principals
//! to evaluate, so no handler constructs it) and throttling
//! (`ThrottlingException`, likewise). The emulator verifies request signatures in
//! the framework, before any handler runs.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ConflictException, CreateMonitorInput, CreateScopeInput, InternalServerException,
    MonitorLocalResource, MonitorRemoteResource, ResourceNotFoundException,
    ServiceQuotaExceededException, TargetId, TargetResource, UpdateMonitorInput,
    ValidationException,
};

/// The Region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service namespace for every resource this service mints
/// (`OPERATIONS.md`: "ARN namespace: `networkflowmonitor`").
pub const ARN_SERVICE: &str = "networkflowmonitor";

/// The ARN resource type of a scope.
pub const SCOPE_RESOURCE: &str = "scope";

/// The ARN resource type of a monitor.
pub const MONITOR_RESOURCE: &str = "monitor";

/// How long a freshly created resource stays in the non-terminal status the model
/// documents for it (`ScopeStatus` `IN_PROGRESS`, `MonitorStatus` `PENDING`: "The
/// monitor is in the process of being created") before the clock owes it the
/// terminal one (`SUCCEEDED`, `ACTIVE`).
///
/// AWS does not document how long either takes, so this span is chosen rather than
/// cited: long enough that a caller acting straight after the create still observes
/// the transient status, short enough that a client script does not wait. The due
/// instant is stored on the record ([`Scope::succeeded_at`], [`Monitor::active_at`])
/// and materialized by whichever call runs after it falls due — nothing ever sleeps.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// The most items a `@paginated` list in this service returns in one page,
/// including when `maxResults` is absent. It is `MaxResults`' own `@range` maximum
/// (1..25), so no page can exceed what the model lets a caller ask for.
pub const MAX_PAGE_SIZE: i32 = 25;

/// The most tags one resource carries, from `TagMap`'s own `@length` maximum
/// (0..200, `smithy_query(query="shape",shape="TagMap")`). It bounds what
/// `TagResource` can MERGE onto a resource: the model refuses a single request
/// carrying more than 200 entries at the boundary, and this is what keeps repeated
/// requests from growing a resource's tag set past what `ListTagsForResource` is
/// allowed to answer with.
pub const MAX_TAGS: usize = 200;

/// What a `resourceArn` names in the modelled errors of the tagging operations:
/// they address a scope or a monitor through one member, so the refusal names the
/// taggable resource rather than either store.
pub const TAGGED_RESOURCE: &str = "resource";

/// The most scopes this emulator holds at once.
///
/// A bound on how far the store can grow under repeated creates: a create past it
/// is refused with the modelled `ServiceQuotaExceededException` (HTTP 402) rather
/// than accepted, so an unauthenticated caller cannot grow the process's memory
/// without limit. AWS publishes its own quota and it is not this number; this is
/// the emulator's own ceiling, chosen well above what a client script needs.
pub const MAX_SCOPES: usize = 100;

/// The most monitors this emulator holds at once, for the same reason and with the
/// same refusal. Deleting a monitor frees its slot (and the idempotency record
/// that named it), so the store's size is bounded by the live resources: at most
/// `MAX_MONITORS` records, each holding at most [`MAX_MONITOR_RESOURCES`] entries
/// per resource list and [`MAX_UPDATE_TOKENS`] remembered client tokens.
pub const MAX_MONITORS: usize = 100;

/// The most entries one monitor's `localResources` or `remoteResources` list
/// holds, and the most entries one request may name in any ONE of its resource
/// lists (`CreateMonitor`'s two, `UpdateMonitor`'s four).
///
/// Neither `MonitorLocalResources` nor `MonitorRemoteResources` carries a
/// `@length`, so nothing before a handler caps how long these lists grow, and an
/// entry's `identifier` is caller-chosen free text: without a ceiling here a
/// caller repeating `UpdateMonitor` with fresh identifiers grows one record — and
/// the work every later call does over it — without limit. A request that would
/// leave a list past this ceiling is refused with the modelled
/// `ValidationException` (HTTP 400, naming the member), which is the refusal both
/// operations model for a member they will not accept. AWS publishes its own
/// quota and it is not this number; this is the emulator's own ceiling, chosen
/// well above what a client script needs.
pub const MAX_MONITOR_RESOURCES: usize = 100;

/// How many `clientToken`s of already-applied updates a monitor remembers, so a
/// replayed `@idempotent` update is recognized as a replay instead of applying
/// twice ([`Monitor::update_replayed`]).
///
/// AWS does not document how long a client token is remembered, so this window is
/// chosen rather than cited: deep enough that a retry — even behind a few other
/// updates to the same monitor — is still recognized, and bounded so that a caller
/// issuing updates forever cannot grow this process's memory without limit. The
/// oldest token is forgotten first.
pub const MAX_UPDATE_TOKENS: usize = 64;

/// `ScopeStatus`, the lifecycle status of a scope. The wire values are the model's
/// (SUCCEEDED | IN_PROGRESS | FAILED | DEACTIVATING | DEACTIVATED); only the ones
/// this slice can produce are modelled here, and the others arrive with the
/// operations that move a scope into them. `DeleteScope` is not one of them: it
/// removes the scope outright, so no reader sees a deactivation status for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeStatus {
    /// The scope is being set up: the status a create answers with.
    InProgress,
    /// The scope is ready, and metrics can be requested for its topology.
    Succeeded,
}

impl ScopeStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Succeeded => "SUCCEEDED",
        }
    }
}

/// `MonitorStatus`, the lifecycle status of a monitor. As with [`ScopeStatus`],
/// only the values this slice produces are modelled; `INACTIVE`, `ERROR` and
/// `DELETING` arrive with the operations that produce them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorStatus {
    /// "The monitor is in the process of being created" — the status a create
    /// answers with.
    Pending,
    /// "The monitor is active."
    Active,
}

impl MonitorStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Active => "ACTIVE",
        }
    }
}

/// What an ARN in [`ComAmazonawsNetworkflowmonitorNetworkflowmonitorState::arn_index`]
/// names: the store it lives in and its key there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// A scope, by `scopeId`.
    Scope(String),
    /// A monitor, by `monitorName`.
    Monitor(String),
}

/// A Network Flow Monitor scope, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct Scope {
    /// `scopeId`.
    pub scope_id: String,
    /// `scopeArn`.
    pub scope_arn: String,
    /// `status`.
    pub status: ScopeStatus,
    /// `targets` — exactly the Region/account pairs the create request carried.
    pub targets: Vec<TargetResource>,
    /// The tags the create request carried.
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this scope its terminal status.
    pub succeeded_at: SystemTime,
    /// When this scope was created.
    pub created_at: SystemTime,
}

impl Scope {
    /// Materialize the status change the clock has already made due. Called by
    /// every handler before it reads or writes, so a reporting operation reports
    /// what a caller would see at this instant without changing anything a later
    /// caller would not already have seen.
    fn settle(&mut self, now: SystemTime) {
        if self.status == ScopeStatus::InProgress && now >= self.succeeded_at {
            self.status = ScopeStatus::Succeeded;
        }
    }

    /// Whether this record is the one `CreateScope` would mint for `input` — the
    /// comparison an idempotent replay makes before answering with it, so a
    /// `clientToken` reused with DIFFERENT parameters conflicts instead of quietly
    /// answering with the first call's scope ("Don't reuse the same client token
    /// for other API requests").
    pub fn matches_create(&self, input: &CreateScopeInput) -> bool {
        same_targets(&self.targets, &input.targets) && self.tags == tags_of(input.tags.as_ref())
    }
}

/// A Network Flow Monitor monitor, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct Monitor {
    /// `monitorName` — unique within the store, and this monitor's key.
    pub monitor_name: String,
    /// `monitorArn`.
    pub monitor_arn: String,
    /// `monitorStatus`.
    pub monitor_status: MonitorStatus,
    /// The `scopeArn` this monitor was created under.
    pub scope_arn: String,
    /// `localResources`.
    pub local_resources: Vec<MonitorLocalResource>,
    /// `remoteResources` — empty when the create request carried none.
    pub remote_resources: Vec<MonitorRemoteResource>,
    /// The tags the create request carried.
    pub tags: BTreeMap<String, String>,
    /// `createdAt`.
    pub created_at: SystemTime,
    /// `modifiedAt`.
    pub modified_at: SystemTime,
    /// When the clock owes this monitor its terminal status.
    pub active_at: SystemTime,
    /// The `clientToken`s of the updates already applied to this monitor, each
    /// with the fingerprint of the change it carried ([`update_fingerprint`]),
    /// oldest first and at most [`MAX_UPDATE_TOKENS`] of them. A deque rather than
    /// a map because the eviction order is what bounds it; the tokens go away with
    /// the monitor.
    pub update_tokens: VecDeque<(String, String)>,
}

/// What a `clientToken` on an `UpdateMonitor` request means for the monitor it
/// addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateReplay {
    /// No token, or a token this monitor has not applied: the request is a new
    /// update and is applied.
    New,
    /// The token already applied THIS change: the request is a replay, answered
    /// with the record as it stands without applying anything again.
    Replay,
    /// The token already applied a DIFFERENT change ("Don't reuse the same client
    /// token for other API requests"), so the request is refused rather than
    /// answered with a success for a write that was discarded.
    Mismatch,
}

impl Monitor {
    /// The monitor counterpart of [`Scope::settle`]. Reaching `ACTIVE` is a change
    /// to the monitor, so `modifiedAt` moves to the instant that change fell due —
    /// the stored due instant, never "now", so two readers see one value.
    fn settle(&mut self, now: SystemTime) {
        if self.monitor_status == MonitorStatus::Pending && now >= self.active_at {
            self.monitor_status = MonitorStatus::Active;
            self.modified_at = self.active_at;
        }
    }

    /// What `token` means for this monitor, given the change `fingerprint` the
    /// request carries ([`update_fingerprint`]) — the check an `@idempotent`
    /// replay makes before anything is changed, so a retried update neither
    /// applies twice nor moves `modifiedAt` again. It compares the REQUEST against
    /// the remembered update, the way [`Monitor::matches_create`] does for a
    /// create, so a token reused for a different change is not answered with a
    /// success for a write nothing applied. A request without a `clientToken` is
    /// never a replay.
    pub fn update_replay(&self, token: Option<&str>, fingerprint: &str) -> UpdateReplay {
        let Some(token) = token else {
            return UpdateReplay::New;
        };
        match self
            .update_tokens
            .iter()
            .find(|(seen, _)| seen == token)
            .map(|(_, applied)| applied == fingerprint)
        {
            None => UpdateReplay::New,
            Some(true) => UpdateReplay::Replay,
            Some(false) => UpdateReplay::Mismatch,
        }
    }

    /// Remember `token` as having applied the change `fingerprint` describes,
    /// forgetting the oldest once the window is full.
    pub fn remember_update(&mut self, token: String, fingerprint: String) {
        if self.update_tokens.iter().any(|(seen, _)| seen == &token) {
            return;
        }
        if self.update_tokens.len() >= MAX_UPDATE_TOKENS {
            self.update_tokens.pop_front();
        }
        self.update_tokens.push_back((token, fingerprint));
    }

    /// Apply an `UpdateMonitor` request's additions and removals to this monitor's
    /// resource lists, answering whether the stored record actually changed (which
    /// is what decides whether `modifiedAt` moves).
    ///
    /// Each list is edited as a SET keyed by an entry's identity (`type` +
    /// `identifier`): the removals land first and then the additions, so one
    /// request naming a resource in both ends with that resource present, and
    /// repeating either is a no-op rather than a duplicate or an error.
    ///
    /// A request that would leave a stored list past [`MAX_MONITOR_RESOURCES`] is
    /// refused: the answer is the member to name in a `ValidationException` and
    /// the reason, and NOTHING is applied, so a refused update leaves the record
    /// exactly as it was. The request's own list lengths are refused earlier, by
    /// [`request_overflow`], before this monitor is touched or the request is
    /// fingerprinted.
    pub fn apply_update(
        &mut self,
        input: &UpdateMonitorInput,
    ) -> Result<bool, (&'static str, String)> {
        let local_to_add = input.local_resources_to_add.as_deref().unwrap_or_default();
        let local_to_remove = input
            .local_resources_to_remove
            .as_deref()
            .unwrap_or_default();
        let remote_to_add = input.remote_resources_to_add.as_deref().unwrap_or_default();
        let remote_to_remove = input
            .remote_resources_to_remove
            .as_deref()
            .unwrap_or_default();

        let local = merged_resources(&self.local_resources, local_to_remove, local_to_add);
        if local.len() > MAX_MONITOR_RESOURCES {
            return Err(("localResourcesToAdd", too_many_resources(local.len())));
        }
        let remote = merged_resources(&self.remote_resources, remote_to_remove, remote_to_add);
        if remote.len() > MAX_MONITOR_RESOURCES {
            return Err(("remoteResourcesToAdd", too_many_resources(remote.len())));
        }

        // "The last date and time that the monitor was modified": the record
        // changed only if a list really ended up different, so a request whose
        // removals and additions cancel out is a no-op.
        let changed = !same_resources(&local, &self.local_resources)
            || !same_resources(&remote, &self.remote_resources);
        self.local_resources = local;
        self.remote_resources = remote;
        Ok(changed)
    }

    /// The monitor counterpart of [`Scope::matches_create`], including the scope:
    /// a token reused under a different scope is a mismatch, not a replay. The
    /// request's lists are compared in the form a create STORES them (deduped), so
    /// a replay of a request carrying a duplicate entry still matches its monitor.
    pub fn matches_create(&self, input: &CreateMonitorInput) -> bool {
        self.monitor_name == input.monitor_name
            && self.scope_arn == input.scope_arn
            && same_resources(
                &self.local_resources,
                &deduped_resources(&input.local_resources),
            )
            && same_resources(
                &self.remote_resources,
                &deduped_resources(input.remote_resources.as_deref().unwrap_or_default()),
            )
            && self.tags == tags_of(input.tags.as_ref())
    }
}

/// A create request's `tags` as a record holds them: ordered, and empty when the
/// member is absent.
pub fn tags_of(tags: Option<&HashMap<String, String>>) -> BTreeMap<String, String> {
    tags.map(|tags| {
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
    .unwrap_or_default()
}

/// The stored tags as the wire carries them: present only when the resource has
/// any, so a resource created without tags reports none rather than an empty map.
pub fn tags_for_output(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    if tags.is_empty() {
        None
    } else {
        Some(
            tags.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }
}

/// The stored tags as a TAG LISTING carries them: always a map, and empty when
/// the resource has none.
///
/// `tags` is the whole of `ListTagsForResource`'s output ("Returns all the tags for
/// a resource"), so an untagged resource answers `{}` — the listing says "this
/// resource has no tags" rather than leaving the one member it has unset. The
/// create/read shapes use [`tags_for_output`] instead, where the record's other
/// members already say what was read.
pub fn tags_as_map(tags: &BTreeMap<String, String>) -> HashMap<String, String> {
    tags.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The account id a `TargetId` union carries. `accountId` is its only member
/// today ("currently, a target is always an Amazon Web Services account"), so an
/// id that is not one compares equal to nothing.
fn target_account_id(id: &TargetId) -> &str {
    match id {
        TargetId::accountId(account_id) => account_id.as_str(),
    }
}

/// Whether two `targets` lists name the same Region/account pairs in the same
/// order. `TargetResource` is a generated wire shape and derives no `PartialEq`,
/// so the comparison is spelled out over its members.
fn same_targets(stored: &[TargetResource], requested: &[TargetResource]) -> bool {
    stored.len() == requested.len()
        && stored.iter().zip(requested).all(|(left, right)| {
            left.region == right.region
                && left.target_identifier.target_type == right.target_identifier.target_type
                && target_account_id(&left.target_identifier.target_id)
                    == target_account_id(&right.target_identifier.target_id)
        })
}

/// The two members a monitor resource list entry carries on the wire.
///
/// `MonitorLocalResource` and `MonitorRemoteResource` are generated wire shapes
/// that derive no `PartialEq` and share no trait, so this is how ONE comparison
/// serves both lists (and any further resource list the service grows). It is
/// `pub` only because the handlers' entry points ([`deduped_resources`]) are
/// generic over it; nothing outside this module implements it.
pub trait MonitorResource {
    /// The entry's `type` member.
    fn resource_type(&self) -> &str;
    /// The entry's `identifier` member.
    fn identifier(&self) -> &str;
}

impl MonitorResource for MonitorLocalResource {
    fn resource_type(&self) -> &str {
        self.r#type.as_str()
    }

    fn identifier(&self) -> &str {
        self.identifier.as_str()
    }
}

impl MonitorResource for MonitorRemoteResource {
    fn resource_type(&self) -> &str {
        self.r#type.as_str()
    }

    fn identifier(&self) -> &str {
        self.identifier.as_str()
    }
}

/// The list `stored` becomes when a request's removals and additions are applied:
/// every entry `to_remove` does not name, in its stored order, followed by every
/// entry of `to_add` the result does not already hold. Entries are compared by
/// identity ([`MonitorResource`]) through a `BTreeSet` of identities, so one call
/// costs O((n + m) log n) rather than a scan per entry.
///
/// One function serves the local and the remote list — and any further monitor
/// resource list the service grows — for the same reason [`same_resources`] does.
/// It never grows the result past what it was asked to merge, so the caller bounds
/// the stored list by checking the length it answers with.
fn merged_resources<T: MonitorResource + Clone>(
    stored: &[T],
    to_remove: &[T],
    to_add: &[T],
) -> Vec<T> {
    let removed: BTreeSet<(String, String)> = to_remove.iter().map(identity).collect();
    let mut merged: Vec<T> = stored
        .iter()
        .filter(|entry| !removed.contains(&identity(*entry)))
        .cloned()
        .collect();
    let mut present: BTreeSet<(String, String)> = merged.iter().map(identity).collect();
    for addition in to_add {
        if present.insert(identity(addition)) {
            merged.push(addition.clone());
        }
    }
    merged
}

/// The reason text of the modelled 400 for a resource list this service will not
/// hold, built from the request so a caller can fix the request from it alone.
/// `CreateMonitor` refuses its own two lists with it, `UpdateMonitor` its four.
pub fn too_many_resources(named: usize) -> String {
    format!(
        "{named} resources, more than the {MAX_MONITOR_RESOURCES} one monitor list \
         holds in this emulator"
    )
}

/// Which member of an `UpdateMonitor` request this service will not accept on the
/// request's own terms, and why — or `None` when every one of its four resource
/// lists is within [`MAX_MONITOR_RESOURCES`].
///
/// This is the FIRST thing the handler checks, before it reads the monitor,
/// fingerprints the request or merges anything: no list a monitor may hold can
/// name more entries than a monitor holds, whether it adds them or removes them,
/// so every later step — the work the merge does, and the digest the remembered
/// `clientToken` keeps — is derived from a request this ceiling already bounds.
pub fn request_overflow(input: &UpdateMonitorInput) -> Option<(&'static str, String)> {
    [
        (
            "localResourcesToAdd",
            input.local_resources_to_add.as_deref().unwrap_or_default().len(),
        ),
        (
            "localResourcesToRemove",
            input
                .local_resources_to_remove
                .as_deref()
                .unwrap_or_default()
                .len(),
        ),
        (
            "remoteResourcesToAdd",
            input
                .remote_resources_to_add
                .as_deref()
                .unwrap_or_default()
                .len(),
        ),
        (
            "remoteResourcesToRemove",
            input
                .remote_resources_to_remove
                .as_deref()
                .unwrap_or_default()
                .len(),
        ),
    ]
    .into_iter()
    .find(|(_, named)| *named > MAX_MONITOR_RESOURCES)
    .map(|(member, named)| (member, too_many_resources(named)))
}

/// The list a create stores for the list a request carried: the same entries in
/// the same order, with any entry naming a resource an earlier one already names
/// dropped. A monitor's resource lists are sets keyed by `type` + `identifier`
/// everywhere else, so they are sets from the moment the record is minted.
pub fn deduped_resources<T: MonitorResource + Clone>(list: &[T]) -> Vec<T> {
    merged_resources(&[], &[], list)
}

/// What an `UpdateMonitor` request asks for, as ONE fixed-size value a later
/// replay can be compared against: the SHA-256 digest, hex encoded, of the
/// identities of its four resource lists in request order.
///
/// A digest rather than the serialized lists themselves, because this value is
/// retained in the store next to the `clientToken` that applied it
/// ([`Monitor::update_tokens`]): the digest is 64 characters whatever the request
/// carried, so what one remembered token costs the store does not depend on the
/// size of the request that minted it. Each piece going into the digest is
/// length-prefixed, so no identifier's own punctuation can make two different
/// requests fingerprint alike.
pub fn update_fingerprint(input: &UpdateMonitorInput) -> String {
    use ::sha2::Digest;

    let mut hasher = ::sha2::Sha256::new();
    hash_list(
        &mut hasher,
        input.local_resources_to_add.as_deref().unwrap_or_default(),
    );
    hash_list(
        &mut hasher,
        input
            .local_resources_to_remove
            .as_deref()
            .unwrap_or_default(),
    );
    hash_list(
        &mut hasher,
        input.remote_resources_to_add.as_deref().unwrap_or_default(),
    );
    hash_list(
        &mut hasher,
        input
            .remote_resources_to_remove
            .as_deref()
            .unwrap_or_default(),
    );
    ::hex::encode(hasher.finalize())
}

/// Feed one resource list's length-prefixed identities into `hasher`.
fn hash_list<T: MonitorResource>(hasher: &mut ::sha2::Sha256, list: &[T]) {
    use ::sha2::Digest;

    hasher.update(list.len().to_le_bytes());
    for entry in list {
        let (resource_type, identifier) = identity_ref(entry);
        hasher.update(resource_type.len().to_le_bytes());
        hasher.update(resource_type.as_bytes());
        hasher.update(identifier.len().to_le_bytes());
        hasher.update(identifier.as_bytes());
    }
}

/// A resource list entry's identity: the two members that say WHICH resource it
/// is, owned so it can be compared against a list that is being rebuilt.
fn identity<T: MonitorResource>(entry: &T) -> (String, String) {
    let (resource_type, identifier) = identity_ref(entry);
    (resource_type.to_string(), identifier.to_string())
}

/// The same identity, borrowed: what the comparison paths use, so comparing two
/// lists allocates nothing.
fn identity_ref<T: MonitorResource>(entry: &T) -> (&str, &str) {
    (entry.resource_type(), entry.identifier())
}

/// Whether two resource lists name the same resources in the same order, for the
/// same reason as [`same_targets`].
fn same_resources<T: MonitorResource>(stored: &[T], requested: &[T]) -> bool {
    stored.len() == requested.len()
        && stored
            .iter()
            .zip(requested)
            .all(|(left, right)| identity_ref(left) == identity_ref(right))
}

/// Per-service state for com-amazonaws-networkflowmonitor-networkflowmonitor. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworkflowmonitorNetworkflowmonitorState {
    /// Every scope, keyed by `scopeId` — a `BTreeMap`, so every list page is in a
    /// stable, resumable order.
    pub scopes: BTreeMap<String, Scope>,
    /// Every monitor, keyed by `monitorName`, for the same reason. The name is the
    /// key because the API addresses a monitor by it (`GET /monitors/{monitorName}`),
    /// which is also what makes a second create of one name a conflict.
    pub monitors: BTreeMap<String, Monitor>,
    /// `clientToken` -> `scopeId`, so an `@idempotent` `CreateScope` replayed with
    /// the same token answers with the scope that token already minted.
    pub scope_idempotency: HashMap<String, String>,
    /// `clientToken` -> `monitorName`, for the same reason. Kept separate from
    /// `scope_idempotency` because the two creates are different operations: one
    /// token reused across both must not make `CreateMonitor` answer with a scope.
    pub monitor_idempotency: HashMap<String, String>,
    /// Every minted ARN -> the record it names, so an operation handed a
    /// `scopeArn` or a `resourceArn` resolves it without scanning the store. This
    /// is how `CreateMonitor` refuses a `scopeArn` that is not a stored scope.
    pub arn_index: HashMap<String, ResourceRef>,
    /// Source of the sequence number behind every minted `scopeId`. Monotonic, so
    /// no two scopes in one store ever share an id.
    pub next_sequence: u64,
}

impl ComAmazonawsNetworkflowmonitorNetworkflowmonitorState {
    /// Materialize every status change the clock has made due, as of `now`.
    pub fn settle(&mut self, now: SystemTime) {
        for scope in self.scopes.values_mut() {
            scope.settle(now);
        }
        for monitor in self.monitors.values_mut() {
            monitor.settle(now);
        }
    }

    /// The next sequence number: unique within this store for the life of the
    /// process.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    /// The scope an ARN names, or `None` when the ARN names nothing in this store
    /// (or names a monitor rather than a scope).
    pub fn scope_by_arn(&self, arn: &str) -> Option<&Scope> {
        match self.arn_index.get(arn) {
            Some(ResourceRef::Scope(scope_id)) => self.scopes.get(scope_id),
            _ => None,
        }
    }

    /// The tags of the resource an ARN names, or `None` when the ARN names nothing
    /// in this store.
    ///
    /// The tagging operations address EVERY taggable resource of this service
    /// through one `resourceArn` member, so they resolve it here rather than each
    /// naming the stores: whatever [`arn_index`](Self::arn_index) holds is
    /// taggable, and a resource type added later is reachable without touching
    /// them.
    pub fn tags_by_arn(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        match self.arn_index.get(arn)? {
            ResourceRef::Scope(scope_id) => self.scopes.get(scope_id).map(|scope| &scope.tags),
            ResourceRef::Monitor(name) => self.monitors.get(name).map(|monitor| &monitor.tags),
        }
    }

    /// The same lookup for a writer. The reference is cloned out of the index
    /// first, so the borrow of the index ends before the record is reached.
    pub fn tags_by_arn_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        match self.arn_index.get(arn)?.clone() {
            ResourceRef::Scope(scope_id) => {
                self.scopes.get_mut(&scope_id).map(|scope| &mut scope.tags)
            }
            ResourceRef::Monitor(name) => self
                .monitors
                .get_mut(&name)
                .map(|monitor| &mut monitor.tags),
        }
    }
}

/// A `scopeId` for sequence number `sequence`.
///
/// `ScopeId` carries no pattern, and the real service's ids are UUIDs ("an
/// internally-generated identifier"), so this is UUID-shaped — 8-4-4-4-12
/// lowercase hex with RFC 4122's version (4) and variant (8) nibbles — with the
/// store's own monotonic sequence in the node field. That keeps every id unique
/// and makes id order creation order, so a list page resumes deterministically.
pub fn scope_id(sequence: u64) -> String {
    format!("00000000-0000-4000-8000-{sequence:012x}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNetworkflowmonitorNetworkflowmonitorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNetworkflowmonitorNetworkflowmonitorState>)
}

/// The Region an ARN this request mints belongs in: the one the request's SigV4
/// credential scope names, else [`DEFAULT_REGION`].
///
/// A scope region that could not be an ARN region segment is not used, so a
/// crafted `Authorization` header can never produce a stored ARN a client cannot
/// parse.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 63
                && region
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of one of this service's resources, through the one ARN builder the
/// framework vends (so the account segment is the canonical emulator account).
pub fn resource_arn(region: &str, resource_type: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{resource_type}/{id}"))
}

/// The modelled 500 for a request that could not reach this service's store.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: Some("the Network Flow Monitor state store is unavailable".to_string()),
    }
}

/// The modelled 404 for an id or a name that names nothing in the store.
pub fn not_found(resource_type: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("{resource_type} {id} was not found")),
    }
}

/// The modelled 409 for a create of a name the store already holds.
pub fn already_exists(resource_type: &str, id: &str) -> ConflictException {
    ConflictException {
        message: Some(format!("{resource_type} {id} already exists")),
    }
}

/// The modelled 409 for an `@idempotencyToken` replayed with parameters that do
/// not match the resource it already minted.
pub fn idempotency_conflict(resource_type: &str, id: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "the clientToken of this request already created {resource_type} {id}, \
             with different parameters"
        )),
    }
}

/// The modelled 402 for a create this store has no room for, naming the ceiling it
/// would pass.
pub fn quota_exceeded(resource_type: &str, limit: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "this emulator holds at most {limit} {resource_type} resources"
        )),
    }
}

/// The modelled 400 for a member this service refuses, naming the member and the
/// rule it broke — the text is built from the request, so a caller can fix the
/// request from it alone.
pub fn field_validation(member: &str, message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: Some(format!("{member}: {}", message.into())),
    }
}

/// The listing a page token belongs to, so a token minted by one list is not
/// accepted by another. `ListMonitors` is the only paginated list implemented
/// today; `ListScopes` adds its own tag when it is.
pub const MONITORS_PAGE_TAG: &str = "monitors/1:";

/// The `nextToken` that resumes the `tag` listing at `key`.
///
/// `nextToken` is a plain `String` in the model with no documented form, so it is
/// this service's to choose: the hex encoding (the `hex` crate, rather than a
/// hand-rolled codec) of the listing's tag followed by the key the next page
/// starts at. The encoding is not a secret and is trivially reversible — it only
/// keeps a client from treating the token as a resource name and building one
/// itself; the TAG is what makes a token this listing did not mint unusable. The
/// service can always resume from a token it issued, including after the resource
/// the previous page stopped at was deleted, because the cursor is compared as a
/// lower bound rather than looked up.
pub fn page_token(tag: &str, key: &str) -> String {
    ::hex::encode(format!("{tag}{key}"))
}

/// The key a `nextToken` resumes the `tag` listing at, or `None` when the token is
/// not one this listing issued.
///
/// The tag is what makes that check real: a token has to decode to this listing's
/// own prefix, so neither a hex string a caller invented nor a token another
/// listing minted resumes a walk from an arbitrary lower bound.
pub fn decode_page_token(tag: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(tag)
        .filter(|key| !key.is_empty())
        .map(ToString::to_string)
}

/// One page of a `@paginated` list over key-ordered `(key, item)` pairs: at most
/// `max_results` items (never more than [`MAX_PAGE_SIZE`]) starting at `after`,
/// and the token that resumes after them.
///
/// An absent `max_results` asks for everything, which this service answers with at
/// most [`MAX_PAGE_SIZE`] items plus a resume token: no single call returns the
/// whole store. The returned token is `None` on the last page, so paging through
/// yields every item exactly once.
pub fn page<'a, T, I>(
    items: I,
    tag: &str,
    after: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<&'a T>, Option<String>)
where
    I: Iterator<Item = (&'a String, &'a T)>,
{
    let cursor = after.unwrap_or_default().to_string();
    let mut remaining = items.skip_while(move |(key, _)| key.as_str() < cursor.as_str());
    // A `maxResults` outside 1..25 is refused by the boundary validator before a
    // handler runs; the clamp is what makes an ABSENT one a bounded page.
    let limit = max_results.map_or(MAX_PAGE_SIZE, |value| value.clamp(1, MAX_PAGE_SIZE)) as usize;
    let page: Vec<&T> = remaining
        .by_ref()
        .take(limit)
        .map(|(_, item)| item)
        .collect();
    let next = remaining.next().map(|(key, _)| page_token(tag, key));
    (page, next)
}
