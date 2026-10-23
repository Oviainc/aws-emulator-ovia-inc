// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-neptunegraph-amazonneptunegraph service.
//!
//! The graph resource is the root of this service's control plane: snapshots,
//! import/export tasks, private endpoints and queries all hang off a graph, so the
//! store below is the one every later slice extends rather than duplicates.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{ResourceNotFoundException, VectorSearchConfiguration};

/// The ARN namespace (and SigV4 signing name) of this service, from the model's
/// `aws.api#service` trait:
/// `smithy_query(query="service_info")` -> `awsService.arnNamespace == "neptune-graph"`.
pub const ARN_NAMESPACE: &str = "neptune-graph";

/// How long after the call that starts it a graph's provisioning (or a
/// reconfiguration) is owed: the graph answers `CREATING`/`UPDATING` until this span
/// has passed, then the next read settles it to `AVAILABLE`.
///
/// The real service takes minutes; this emulator picks a span inside the
/// emulator-wide 250ms..750ms band for a transition the clock owes, so a caller that
/// acts immediately after `CreateGraph` still observes the transient status (and the
/// state errors that go with it) without any test having to wait minutes. No handler
/// ever sleeps for it — the due instant is stored and compared on read.
pub const PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// The lifecycle statuses this slice of the emulator puts a graph into.
///
/// Every variant's wire form is one of the modelled `GraphStatus` values
/// (`crate::ops::types::GRAPH_STATUS_VALUES`). The statuses this slice never
/// produces (`FAILED`, `IMPORTING`, `RESETTING`, `STARTING`, `STOPPED`,
/// `STOPPING`) belong to operations that are still scaffolded, and are added with
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphStatus {
    /// `CreateGraph` has accepted the graph; it is not usable yet.
    Creating,
    /// The graph is provisioned and serving.
    Available,
    /// `UpdateGraph` has accepted a reconfiguration.
    Updating,
    /// `CreateGraphSnapshot` has accepted a snapshot of the graph; the graph is
    /// being snapshotted and is not reconfigurable until that finishes.
    Snapshotting,
    /// `DeleteGraph` has accepted the deletion.
    Deleting,
}

impl GraphStatus {
    /// This status as the modelled `GraphStatus` enum value.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Updating => "UPDATING",
            Self::Snapshotting => "SNAPSHOTTING",
            Self::Deleting => "DELETING",
        }
    }
}

/// One stored Neptune Analytics graph: every member `GetGraph` reports, plus the
/// instant at which a pending provisioning transition becomes due.
#[derive(Debug, Clone)]
pub struct Graph {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub status: GraphStatus,
    pub status_reason: Option<String>,
    pub create_time: SystemTime,
    pub provisioned_memory: i32,
    pub endpoint: String,
    pub public_connectivity: bool,
    pub replica_count: i32,
    pub kms_key_identifier: Option<String>,
    pub vector_search_configuration: Option<VectorSearchConfiguration>,
    pub deletion_protection: bool,
    pub source_snapshot_id: Option<String>,
    pub build_number: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// When the pending `CREATING`/`UPDATING` -> `AVAILABLE` transition falls due.
    /// `None` once there is nothing outstanding.
    pub settles_at: Option<Instant>,
}

impl Graph {
    /// Materialise the transition the clock has already made due.
    ///
    /// This is the one write a reporting operation (`GetGraph`, `ListGraphs`) is
    /// allowed to make: the graph would have become `AVAILABLE` at `settles_at`
    /// whether or not anybody called, so applying it on read is bookkeeping, not a
    /// state change caused by the read.
    pub fn settle(&mut self, now: Instant) {
        if self.settles_at.is_some_and(|due| now >= due) {
            self.status = GraphStatus::Available;
            self.settles_at = None;
        }
    }

    /// Start a provisioning transition to `AVAILABLE`, due [`PROVISIONING_SPAN`]
    /// from `now`, and report `status` until then.
    pub fn begin_transition(&mut self, status: GraphStatus, now: Instant) {
        self.status = status;
        self.settles_at = Some(now + PROVISIONING_SPAN);
    }
}

/// The lifecycle statuses this slice of the emulator puts a graph snapshot into.
///
/// Every variant's wire form is one of the modelled `SnapshotStatus` values
/// (`crate::ops::types::SNAPSHOT_STATUS_VALUES`). The statuses this slice never
/// produces (`DELETING`, `FAILED`) belong to operations that are still scaffolded
/// (`DeleteGraphSnapshot`), and are added with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotStatus {
    /// `CreateGraphSnapshot` has accepted the snapshot; it is not restorable yet.
    Creating,
    /// The snapshot is complete and restorable.
    Available,
}

impl SnapshotStatus {
    /// This status as the modelled `SnapshotStatus` enum value.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
        }
    }
}

/// One stored graph snapshot: every member `GetGraphSnapshot` reports, plus the
/// instant at which the pending `CREATING` -> `AVAILABLE` transition becomes due.
///
/// A snapshot is a resource in its own right — it outlives the graph it was taken
/// from — so it is stored beside the graphs rather than inside the graph record.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The graph the snapshot was taken from. Kept as the graph's id (not a
    /// borrow), so deleting that graph leaves the snapshot's provenance intact.
    pub source_graph_id: String,
    pub status: SnapshotStatus,
    pub snapshot_create_time: SystemTime,
    /// The key the source graph's data is encrypted with: a snapshot of an
    /// encrypted graph is encrypted with the same key.
    pub kms_key_identifier: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// When the pending `CREATING` -> `AVAILABLE` transition falls due. `None` once
    /// there is nothing outstanding.
    pub settles_at: Option<Instant>,
}

impl Snapshot {
    /// Materialise the transition the clock has already made due.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: the
    /// snapshot would have become `AVAILABLE` at `settles_at` whether or not anybody
    /// called, so applying it on read is bookkeeping rather than a state change
    /// caused by the read.
    pub fn settle(&mut self, now: Instant) {
        if self.settles_at.is_some_and(|due| now >= due) {
            self.status = SnapshotStatus::Available;
            self.settles_at = None;
        }
    }
}

/// Per-service state for com-amazonaws-neptunegraph-amazonneptunegraph. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNeptunegraphAmazonneptunegraphState {
    /// Every stored graph, keyed by its minted `g-xxxxxxxxxx` id. A `BTreeMap` so
    /// `ListGraphs` has one stable total order to page over.
    pub graphs: BTreeMap<String, Graph>,
    /// `graphName` -> graph id: the uniqueness index behind `CreateGraph`'s
    /// `ConflictException`.
    pub graph_names: HashMap<String, String>,
    /// Every stored snapshot, keyed by its minted `gs-xxxxxxxxxx` id. A `BTreeMap`
    /// so `ListGraphSnapshots` has one stable total order to page over.
    pub snapshots: BTreeMap<String, Snapshot>,
    /// `snapshotName` -> snapshot id: the uniqueness index behind
    /// `CreateGraphSnapshot`'s `ConflictException`.
    pub snapshot_names: HashMap<String, String>,
    /// Feeds the graph-id generator, so ids are unique within a store.
    pub id_counter: u64,
    /// Feeds the snapshot-id generator, so ids are unique within a store.
    pub snapshot_id_counter: u64,
}

impl ComAmazonawsNeptunegraphAmazonneptunegraphState {
    /// Mint an id no graph in this store holds, matching the modelled `GraphId`
    /// pattern `^g-[a-z0-9]{10}$`.
    pub fn next_graph_id(&mut self) -> String {
        loop {
            self.id_counter = self.id_counter.wrapping_add(1);
            let id = encode_resource_id("g-", scramble(self.id_counter) % BASE36_POW10);
            if !self.graphs.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an id no snapshot in this store holds, matching the modelled
    /// `SnapshotId` pattern `^gs-[a-z0-9]{10}$`.
    pub fn next_snapshot_id(&mut self) -> String {
        loop {
            self.snapshot_id_counter = self.snapshot_id_counter.wrapping_add(1);
            let id = encode_resource_id("gs-", scramble(self.snapshot_id_counter) % BASE36_POW10);
            if !self.snapshots.contains_key(&id) {
                return id;
            }
        }
    }

    /// Store a new graph and index its name.
    pub fn insert_graph(&mut self, graph: Graph) {
        self.graph_names.insert(graph.name.clone(), graph.id.clone());
        self.graphs.insert(graph.id.clone(), graph);
    }

    /// Store a new snapshot and index its name.
    pub fn insert_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshot_names
            .insert(snapshot.name.clone(), snapshot.id.clone());
        self.snapshots.insert(snapshot.id.clone(), snapshot);
    }

    /// Drop a graph and its name index entry.
    pub fn remove_graph(&mut self, id: &str) -> Option<Graph> {
        let graph = self.graphs.remove(id)?;
        self.graph_names.remove(&graph.name);
        Some(graph)
    }

    /// Apply every provisioning transition the clock has made due, then borrow one
    /// graph.
    pub fn settled_graph(&mut self, id: &str, now: Instant) -> Option<&Graph> {
        let graph = self.graphs.get_mut(id)?;
        graph.settle(now);
        Some(graph)
    }

    /// Apply every provisioning transition the clock has made due, then borrow one
    /// graph mutably.
    pub fn settled_graph_mut(&mut self, id: &str, now: Instant) -> Option<&mut Graph> {
        let graph = self.graphs.get_mut(id)?;
        graph.settle(now);
        Some(graph)
    }

    /// Apply every provisioning transition the clock has made due across the store.
    pub fn settle_all(&mut self, now: Instant) {
        for graph in self.graphs.values_mut() {
            graph.settle(now);
        }
        for snapshot in self.snapshots.values_mut() {
            snapshot.settle(now);
        }
    }

    /// Apply the snapshot transition the clock has made due, then borrow one
    /// snapshot.
    pub fn settled_snapshot(&mut self, id: &str, now: Instant) -> Option<&Snapshot> {
        let snapshot = self.snapshots.get_mut(id)?;
        snapshot.settle(now);
        Some(snapshot)
    }

    /// The tag map of the resource `arn` names, or `None` when this store holds no
    /// resource by that ARN.
    ///
    /// The lookup is on the ARN the service MINTED for the resource, so a request
    /// naming the right resource id in the wrong region, account or partition names
    /// no resource here — an ARN identifies one resource in one region of one
    /// account.
    ///
    /// Tags live on the resource record itself, which is what makes a tag supplied at
    /// create time and one written later by `TagResource` the same bytes, read back by
    /// this one lookup. Graphs and snapshots are the taggable resources this slice
    /// stores; private endpoints and tasks join this lookup with the operations that
    /// mint them.
    pub fn tags_of(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        self.graphs
            .values()
            .find(|graph| graph.arn == arn)
            .map(|graph| &graph.tags)
            .or_else(|| {
                self.snapshots
                    .values()
                    .find(|snapshot| snapshot.arn == arn)
                    .map(|snapshot| &snapshot.tags)
            })
    }

    /// [`Self::tags_of`], mutably: the one place `TagResource`/`UntagResource` write.
    pub fn tags_of_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        // Two disjoint maps and one `&mut self`, so which map holds the ARN is
        // decided by an immutable scan before either is borrowed mutably.
        if self.graphs.values().any(|graph| graph.arn == arn) {
            return self
                .graphs
                .values_mut()
                .find(|graph| graph.arn == arn)
                .map(|graph| &mut graph.tags);
        }
        self.snapshots
            .values_mut()
            .find(|snapshot| snapshot.arn == arn)
            .map(|snapshot| &mut snapshot.tags)
    }
}

// ---------------------------------------------------------------------------
// Store bounds
//
// A graph is created by a caller and reclaimed only by an explicit `DeleteGraph`, so
// the growth site carries a cap and nothing caller-sized reaches the store unchecked.
// The product is finite: at most `MAX_GRAPHS` graphs, each holding a `graphName` of at
// most 63 characters, a `kmsKeyIdentifier` of at most 1024, and at most `MAX_TAGS` tags
// of at most `MAX_TAG_KEY_CHARS` / `MAX_TAG_VALUE_CHARS` characters.
// ---------------------------------------------------------------------------

/// The most graphs one store holds.
///
/// Emulator resource guard, NOT a quota AWS publishes: the store is shared
/// process-wide and grows by one entry per `CreateGraph`, so without a ceiling
/// repeated calls grow the emulator's memory without bound. `CreateGraph` reports
/// reaching it with the `ServiceQuotaExceededException` (402) the model declares for
/// exactly this condition. Set well above any realistic use of a local emulator.
pub const MAX_GRAPHS: usize = 100;

/// The most snapshots one store holds.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the same bound
/// [`MAX_GRAPHS`] puts on the graph map: the store is shared process-wide and grows
/// by one entry per `CreateGraphSnapshot`, so without a ceiling repeated calls grow
/// the emulator's memory without bound. `CreateGraphSnapshot` reports reaching it
/// with the `ServiceQuotaExceededException` (402) the model declares for exactly
/// this condition. Set well above any realistic use of a local emulator.
pub const MAX_SNAPSHOTS: usize = 100;

/// AWS's tagging limit of 50 tags per resource, which this service models as
/// `TagMap`'s `@length(0..50)`.
pub const MAX_TAGS: usize = 50;

/// From the modelled `TagKey`: `@length(1..128)`.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// From the modelled `TagValue`: `@length(0..256)`.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Whether `value` matches the modelled `GraphId` pattern `^g-[a-z0-9]{10}$`.
///
/// Shared by the id generator's own test and by `ListGraphs`, which refuses a
/// pagination token that does not name a position in the graph-id space.
pub fn is_graph_id(value: &str) -> bool {
    is_resource_id(value, "g-")
}

/// Whether `value` matches the modelled `SnapshotId` pattern `^gs-[a-z0-9]{10}$`.
pub fn is_snapshot_id(value: &str) -> bool {
    is_resource_id(value, "gs-")
}

/// Whether `value` is `prefix` followed by exactly ten `[a-z0-9]` characters — the
/// shape every resource id of this service has, differing only in its prefix.
fn is_resource_id(value: &str, prefix: &str) -> bool {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return false;
    };
    suffix.len() == 10
        && suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Whether `name` satisfies this service's resource-name pattern
/// `^(?!<forbidden_prefix>)[a-z][a-z0-9]*(-[a-z0-9]+)*$`, read literally: not the
/// prefix the service reserves for its own ids, a lowercase letter first, then
/// lowercase letters, digits and single interior hyphens, and no trailing hyphen.
///
/// `GraphName` and `SnapshotName` spell the same expression with a different
/// reserved prefix (`g-`, `gs-`), and both open with a negative lookahead the
/// boundary validator's regex engine cannot compile — an uncompilable `@pattern` is
/// treated as matching — so this is where that constraint is enforced for every
/// operation that accepts a name.
pub fn resource_name_is_well_formed(name: &str, forbidden_prefix: &str) -> bool {
    if name.starts_with(forbidden_prefix) {
        return false;
    }
    let mut chars = name.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_lowercase()) {
        return false;
    }
    let mut after_hyphen = false;
    for c in chars {
        if c == '-' {
            // `(-[a-z0-9]+)*` requires at least one alphanumeric between hyphens.
            if after_hyphen {
                return false;
            }
            after_hyphen = true;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            after_hyphen = false;
        } else {
            return false;
        }
    }
    // A trailing hyphen would leave the last group empty.
    !after_hyphen
}

/// The region this request was signed for — the region that names the ARN (and, for
/// a graph, the endpoint) of every resource this service mints. Falls back to the
/// emulator-wide default region when the request carries no SigV4 credential scope
/// (request authentication may be off).
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| "us-east-1".to_string(), |scope| scope.region)
}

// ---------------------------------------------------------------------------
// How this service words a failure
//
// Both error messages below are built from the REQUEST — the identity it named, the
// value it sent and the constraint that value broke — and each is written in exactly
// one place, reached by every operation and by the framework-level refusal renderer in
// `crate::service`, so no two call sites can drift into different wording for the same
// failure.
// ---------------------------------------------------------------------------

/// The account every request this emulator serves is made by: the one canonical
/// emulator account, which is also the account segment of every ARN this service mints
/// (`::engine::emulator_arn`). Never a literal — the account contract has one source of
/// truth.
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The modelled `ResourceNotFoundException` for a `graphIdentifier` that names no
/// stored graph.
///
/// The graph namespace is per-account, so "there is no such graph" is a statement about
/// one account: the failure names the account the lookup was resolved in as well as the
/// identifier the request asked for, which is what lets a caller tell a graph that was
/// deleted apart from a call that reached the wrong account or partition. The modelled
/// shape carries only `message`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), so the message is
/// the only place either fact can go.
pub fn graph_not_found(graph_identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "No graph found for customer [{}] with graph id [{graph_identifier}]",
            caller_account_id()
        ),
    }
}

/// The modelled `ResourceNotFoundException` for a `snapshotIdentifier` that names no
/// stored snapshot.
///
/// The snapshot namespace is per-account like the graph namespace, so the failure
/// names the account the lookup was resolved in beside the identifier the request
/// asked for — the same two facts [`graph_not_found`] reports, and for the same
/// reason.
pub fn snapshot_not_found(snapshot_identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "No snapshot found for customer [{}] with snapshot id [{snapshot_identifier}]",
            caller_account_id()
        ),
    }
}

/// The modelled `ResourceNotFoundException` for a `resourceArn` that names no stored
/// resource.
///
/// The tagging operations identify their subject by ARN rather than by graph id, so the
/// failure names the ARN the request asked for, beside the account the lookup was
/// resolved in — the same two facts [`graph_not_found`] reports, which is what lets a
/// caller tell a deleted resource apart from a call that reached the wrong account or
/// partition. The modelled shape carries only `message`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), so the message is
/// the only place either fact can go.
pub fn resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "No resource found for customer [{}] with resource arn [{resource_arn}]",
            caller_account_id()
        ),
    }
}

/// Refuse more tag entries on one resource than it may hold ([`MAX_TAGS`]).
///
/// `total` is the number of entries the record would hold AFTER the call, so
/// `TagResource` checks the merge of what is stored with what it was sent rather than
/// the request alone. `Err` carries the message, naming the bound it broke, for the
/// caller to fix the request from.
pub fn bound_tag_count(total: usize) -> Result<(), String> {
    if total > MAX_TAGS {
        return Err(format!(
            "tags may carry at most {MAX_TAGS} entries, got {total}"
        ));
    }
    Ok(())
}

/// Refuse a `tags` map that would put more into one stored record than it may hold
/// ([`MAX_TAGS`] entries of at most [`MAX_TAG_KEY_CHARS`] / [`MAX_TAG_VALUE_CHARS`]
/// characters).
///
/// Reached by every operation that accepts a `tags` map (`CreateGraph`, `TagResource`),
/// so the bound on what one record stores is stated once. The model's own `@length`
/// traits on `TagMap`/`TagKey`/`TagValue` refuse these at the boundary too; the check is
/// repeated at the growth site so the bound lives beside the write that stores it.
pub fn bound_tags(tags: &HashMap<String, String>) -> Result<(), String> {
    bound_tag_count(tags.len())?;
    for (key, value) in tags {
        let key_chars = key.chars().count();
        if key_chars > MAX_TAG_KEY_CHARS {
            return Err(format!(
                "a tag key may be at most {MAX_TAG_KEY_CHARS} characters, got {key_chars}"
            ));
        }
        let value_chars = value.chars().count();
        if value_chars > MAX_TAG_VALUE_CHARS {
            return Err(format!(
                "the value of tag '{key}' may be at most {MAX_TAG_VALUE_CHARS} characters, \
                 got {value_chars}"
            ));
        }
    }
    Ok(())
}

/// One clause of a `ValidationException` message: the value that was rejected, the
/// member path it sat at, and the requirement it failed.
///
/// `value` is `None` for a member that was ABSENT, which is reported as the null it was
/// — a fact of the request's shape rather than a rendering of something the caller sent.
pub fn constraint_clause(member_path: &str, value: Option<&str>, requirement: &str) -> String {
    let subject = match value {
        Some(shown) => format!("Value '{shown}'"),
        None => "Value null".to_string(),
    };
    format!("{subject} at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// The requirement clause for a `@pattern` violation: the expression the value had to
/// satisfy, rendered WITHOUT the anchors the model spells it with (see [`unanchored`]).
pub fn pattern_requirement(pattern: &str) -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        unanchored(pattern)
    )
}

/// The `ValidationException` message over one or more clauses: how many constraints were
/// violated, then the clauses themselves.
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses {
        [only] => format!("1 validation error detected: {only}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// A `@pattern` expression without the anchors a model may spell it with: the
/// requirement reported is the expression a value must satisfy, not the anchoring the
/// model uses to make the match total.
///
/// Only the expression's OUTER anchors go: an anchor inside the expression (one per
/// branch of an alternation, say) is part of its own structure, and dropping it would
/// change which strings the reported expression describes. A `$` escaped by a backslash
/// is a literal dollar sign rather than the end anchor, so it stays.
pub fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// `36^10` — the number of distinct values ten `[a-z0-9]` characters can spell.
const BASE36_POW10: u64 = 36u64.pow(10);

/// Spread consecutive counter values over the id space (SplitMix64's finalizer), so
/// ids are not guessable neighbours while still being a pure function of the
/// counter — the same sequence of calls against a fresh store mints the same ids.
fn scramble(seed: u64) -> u64 {
    let mut z = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0x6A09_E667_F3BC_C909);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Spell `value` (already reduced below [`BASE36_POW10`]) as `prefix` plus exactly
/// ten base-36 digits, matching this service's resource-id patterns (`GraphId`'s
/// `^g-[a-z0-9]{10}$`, `SnapshotId`'s `^gs-[a-z0-9]{10}$`).
fn encode_resource_id(prefix: &str, mut value: u64) -> String {
    let mut digits = ['0'; 10];
    for slot in digits.iter_mut().rev() {
        // `from_digit` is total for radix 36 and a remainder below 36, and yields
        // lowercase letters above 9; the fallback keeps a handler panic-free.
        *slot = char::from_digit((value % 36) as u32, 36).unwrap_or('0');
        value /= 36;
    }
    let mut id = String::with_capacity(prefix.len() + digits.len());
    id.push_str(prefix);
    id.extend(digits);
    id
}

/// The five `*Output` shapes that carry a whole graph record are structurally
/// identical, so one macro renders the stored graph into each rather than five
/// hand-copied field lists that could drift apart.
macro_rules! graph_record_output {
    ($($ty:ident),+ $(,)?) => {$(
        impl From<&Graph> for crate::ops::types::$ty {
            fn from(graph: &Graph) -> Self {
                Self {
                    id: graph.id.clone(),
                    name: graph.name.clone(),
                    arn: graph.arn.clone(),
                    status: Some(graph.status.as_wire().to_string()),
                    status_reason: graph.status_reason.clone(),
                    create_time: Some(::aws_smithy_types::DateTime::from(graph.create_time)),
                    provisioned_memory: Some(graph.provisioned_memory),
                    endpoint: Some(graph.endpoint.clone()),
                    public_connectivity: Some(graph.public_connectivity),
                    vector_search_configuration: graph.vector_search_configuration.clone(),
                    replica_count: Some(graph.replica_count),
                    kms_key_identifier: graph.kms_key_identifier.clone(),
                    source_snapshot_id: graph.source_snapshot_id.clone(),
                    deletion_protection: Some(graph.deletion_protection),
                    build_number: graph.build_number.clone(),
                }
            }
        }
    )+};
}

graph_record_output!(
    CreateGraphOutput,
    GetGraphOutput,
    UpdateGraphOutput,
    DeleteGraphOutput,
);

/// `GraphSummary` is the listing projection of the same record: it carries no
/// `createTime`, `statusReason`, `sourceSnapshotId`, `buildNumber` or
/// `vectorSearchConfiguration`.
impl From<&Graph> for crate::ops::types::GraphSummary {
    fn from(graph: &Graph) -> Self {
        Self {
            id: graph.id.clone(),
            name: graph.name.clone(),
            arn: graph.arn.clone(),
            status: Some(graph.status.as_wire().to_string()),
            provisioned_memory: Some(graph.provisioned_memory),
            public_connectivity: Some(graph.public_connectivity),
            endpoint: Some(graph.endpoint.clone()),
            replica_count: Some(graph.replica_count),
            kms_key_identifier: graph.kms_key_identifier.clone(),
            deletion_protection: Some(graph.deletion_protection),
        }
    }
}

/// The two `*Output` shapes that carry a whole snapshot record are structurally
/// identical, so one macro renders the stored snapshot into each rather than two
/// hand-copied field lists that could drift apart. `GraphSnapshotSummary` joins this
/// list with `ListGraphSnapshots`.
macro_rules! snapshot_record_output {
    ($($ty:ident),+ $(,)?) => {$(
        impl From<&Snapshot> for crate::ops::types::$ty {
            fn from(snapshot: &Snapshot) -> Self {
                Self {
                    id: snapshot.id.clone(),
                    name: snapshot.name.clone(),
                    arn: snapshot.arn.clone(),
                    source_graph_id: Some(snapshot.source_graph_id.clone()),
                    snapshot_create_time: Some(::aws_smithy_types::DateTime::from(
                        snapshot.snapshot_create_time,
                    )),
                    status: Some(snapshot.status.as_wire().to_string()),
                    kms_key_identifier: snapshot.kms_key_identifier.clone(),
                }
            }
        }
    )+};
}

snapshot_record_output!(CreateGraphSnapshotOutput, GetGraphSnapshotOutput);

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNeptunegraphAmazonneptunegraphState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNeptunegraphAmazonneptunegraphState>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every minted id matches the modelled `GraphId` pattern and no id repeats.
    #[test]
    fn minted_ids_match_the_modelled_pattern_and_are_unique() {
        let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..2_000 {
            let id = state.next_graph_id();
            assert!(
                is_graph_id(&id),
                "id {id:?}: expected a match for ^g-[a-z0-9]{{10}}$"
            );
            assert!(seen.insert(id.clone()), "id {id:?}: minted twice");
        }
    }

    /// The id shape test, over values that break each clause of the pattern.
    #[test]
    fn is_graph_id_rejects_anything_the_modelled_pattern_rejects() {
        for (candidate, accepted) in [
            ("g-0123456789", true),
            ("g-abcdefghij", true),
            ("g-zz99zz99zz", true),
            ("g-tooshort", false),
            ("g-waytoolongforten", false),
            ("g-ABCDEFGHIJ", false),
            ("gs-0123456789", false),
            ("0123456789", false),
            ("", false),
        ] {
            let actual = is_graph_id(candidate);
            assert_eq!(
                actual, accepted,
                "case {candidate:?}: expected accepted={accepted}, got {actual}"
            );
        }
    }

    /// Only the OUTER anchors of a `@pattern` are dropped, over a row per form an
    /// expression can take: anchored at both ends, at one end, not at all, an escaped
    /// trailing `$`, and anchors inside an alternation.
    #[test]
    fn unanchored_drops_only_the_outer_anchors() {
        // (case, pattern, the expression the requirement must report)
        let cases: &[(&str, &str, &str)] = &[
            // The modelled `GraphIdentifier` / `GraphId` pattern.
            ("anchored at both ends", "^g-[a-z0-9]{10}$", "g-[a-z0-9]{10}"),
            // The modelled `SnapshotId` pattern.
            ("another anchored pattern", "^gs-[a-z0-9]{10}$", "gs-[a-z0-9]{10}"),
            ("anchored at the start only", "^g-[a-z0-9]+", "g-[a-z0-9]+"),
            ("anchored at the end only", "[a-z0-9]+$", "[a-z0-9]+"),
            ("no anchors at all", "[a-z0-9]+", "[a-z0-9]+"),
            ("an escaped trailing dollar", "^cost\\$", "cost\\$"),
            ("an escaped backslash before the anchor", "^path\\\\$", "path\\\\"),
            ("anchors inside an alternation", "^(^a$|^b$)$", "(^a$|^b$)"),
            ("empty", "", ""),
        ];
        for (case, pattern, expected) in cases {
            let actual = unanchored(pattern);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The `ValidationException` envelope counts the clauses it carries, singular for
    /// one and plural beyond.
    #[test]
    fn the_validation_envelope_counts_its_clauses() {
        let clause = |index: usize| {
            constraint_clause(
                &format!("member{index}"),
                Some("bad"),
                &pattern_requirement("^[a-z]+$"),
            )
        };
        for count in 1..=3 {
            let clauses: Vec<String> = (0..count).map(clause).collect();
            let actual = validation_envelope(&clauses);
            let expected_prefix = if count == 1 {
                "1 validation error detected: ".to_string()
            } else {
                format!("{count} validation errors detected: ")
            };
            assert!(
                actual.starts_with(&expected_prefix),
                "case {count} clauses: expected a message opening {expected_prefix:?}, \
                 got {actual:?}"
            );
            assert_eq!(
                actual.matches("failed to satisfy constraint").count(),
                count,
                "case {count} clauses: expected {count} clauses, got {actual:?}"
            );
        }
    }

    /// An absent member is reported as the null it was; a supplied one quotes its value.
    #[test]
    fn a_constraint_clause_states_the_value_it_rejected() {
        // (case, the value the request carried, the subject the clause must open with)
        let cases: &[(&str, Option<&str>, &str)] = &[
            ("a supplied value", Some("not-a-valid-id"), "Value 'not-a-valid-id' at 'graphIdentifier'"),
            ("an empty value", Some(""), "Value '' at 'graphIdentifier'"),
            ("an absent member", None, "Value null at 'graphIdentifier'"),
        ];
        for (case, value, expected_subject) in cases {
            let actual = constraint_clause(
                "graphIdentifier",
                *value,
                &pattern_requirement("^g-[a-z0-9]{10}$"),
            );
            assert!(
                actual.starts_with(expected_subject),
                "case {case:?}: expected a clause opening {expected_subject:?}, got {actual:?}"
            );
        }
    }

    /// The bound on what one stored record's tags may hold, at the limit and one past
    /// it for the entry count, the key length and the value length.
    #[test]
    fn tag_bounds_accept_the_limit_and_refuse_one_past_it() {
        let map = |pairs: Vec<(String, String)>| -> HashMap<String, String> {
            pairs.into_iter().collect()
        };
        let many = |count: usize| {
            map((0..count)
                .map(|index| (format!("key{index}"), "v".to_string()))
                .collect())
        };

        // (case, tags, accepted)
        let cases: Vec<(&str, HashMap<String, String>, bool)> = vec![
            ("no tags", map(Vec::new()), true),
            ("at the entry limit", many(MAX_TAGS), true),
            ("one entry past the limit", many(MAX_TAGS + 1), false),
            (
                "a key at the length limit",
                map(vec![("k".repeat(MAX_TAG_KEY_CHARS), "v".to_string())]),
                true,
            ),
            (
                "a key one character past the limit",
                map(vec![("k".repeat(MAX_TAG_KEY_CHARS + 1), "v".to_string())]),
                false,
            ),
            (
                "a value at the length limit",
                map(vec![("k".to_string(), "v".repeat(MAX_TAG_VALUE_CHARS))]),
                true,
            ),
            (
                "a value one character past the limit",
                map(vec![("k".to_string(), "v".repeat(MAX_TAG_VALUE_CHARS + 1))]),
                false,
            ),
        ];
        for (case, tags, accepted) in cases {
            let actual = bound_tags(&tags);
            assert_eq!(
                actual.is_ok(),
                accepted,
                "case {case:?}: expected accepted={accepted}, got {actual:?}"
            );
        }
    }

    /// The post-call entry count `TagResource` checks, at the limit and one past it.
    #[test]
    fn the_tag_entry_bound_accepts_the_limit_and_refuses_one_past_it() {
        for total in [0, 1, MAX_TAGS - 1, MAX_TAGS, MAX_TAGS + 1, MAX_TAGS * 2] {
            let actual = bound_tag_count(total);
            let accepted = total <= MAX_TAGS;
            assert_eq!(
                actual.is_ok(),
                accepted,
                "case total={total}: expected accepted={accepted}, got {actual:?}"
            );
        }
    }

    /// A missing graph is reported scoped to the account the lookup ran in, for every
    /// well-formed identifier.
    #[test]
    fn graph_not_found_names_the_account_and_the_requested_id() {
        for id in ["g-0000000000", "g-zzzzzzzzzz", "g-a1b2c3d4e5"] {
            let actual = graph_not_found(id).message;
            let expected = format!(
                "No graph found for customer [{}] with graph id [{id}]",
                caller_account_id()
            );
            assert_eq!(
                actual, expected,
                "case {id:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The ARN lookup the tagging operations resolve their subject through is EXACT:
    /// only the ARN the service minted for a resource names it, so the same resource id
    /// under another region, account or partition names no resource in this store.
    #[test]
    fn tags_of_resolves_only_the_minted_arn() {
        let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        let id = "g-0123456789";
        let arn = ::engine::emulator_arn(ARN_NAMESPACE, "us-east-1", &format!("graph/{id}"));
        let mut graph = sample_graph(id, &arn);
        graph.tags.insert("env".to_string(), "prod".to_string());
        state.insert_graph(graph);

        // (case, the ARN a request names, whether it resolves to the stored graph)
        let cases: Vec<(&str, String, bool)> = vec![
            ("the minted ARN", arn.clone(), true),
            (
                "another region",
                ::engine::emulator_arn(ARN_NAMESPACE, "eu-west-1", &format!("graph/{id}")),
                false,
            ),
            (
                "another account",
                format!("arn:aws:{ARN_NAMESPACE}:us-east-1:999999999999:graph/{id}"),
                false,
            ),
            (
                "another resource type",
                ::engine::emulator_arn(ARN_NAMESPACE, "us-east-1", &format!("graph-snapshot/{id}")),
                false,
            ),
            (
                "another graph id",
                ::engine::emulator_arn(ARN_NAMESPACE, "us-east-1", "graph/g-9999999999"),
                false,
            ),
            ("not an ARN of this service", "arn:aws:s3:::bucket".to_string(), false),
        ];
        for (case, candidate, resolves) in cases {
            let actual = state.tags_of(&candidate).is_some();
            assert_eq!(
                actual, resolves,
                "case {case:?} ({candidate:?}): expected resolves={resolves}, got {actual}"
            );
            let actual_mut = state.tags_of_mut(&candidate).is_some();
            assert_eq!(
                actual_mut, resolves,
                "case {case:?} ({candidate:?}) mutably: expected resolves={resolves}, \
                 got {actual_mut}"
            );
        }
    }

    /// A stored graph with the members every test here needs and nothing else set.
    fn sample_graph(id: &str, arn: &str) -> Graph {
        Graph {
            id: id.to_string(),
            name: format!("name-for-{id}"),
            arn: arn.to_string(),
            status: GraphStatus::Available,
            status_reason: None,
            create_time: SystemTime::UNIX_EPOCH,
            provisioned_memory: 16,
            endpoint: "endpoint".to_string(),
            public_connectivity: false,
            replica_count: 1,
            kms_key_identifier: None,
            vector_search_configuration: None,
            deletion_protection: false,
            source_snapshot_id: None,
            build_number: None,
            tags: BTreeMap::new(),
            settles_at: None,
        }
    }

    /// The transition is owed at a future instant, so a read taken immediately
    /// still sees the transient status.
    #[test]
    fn a_pending_transition_settles_only_once_it_is_due() {
        let now = Instant::now();
        let mut graph = Graph {
            id: "g-0123456789".to_string(),
            name: "example".to_string(),
            arn: "arn".to_string(),
            status: GraphStatus::Creating,
            status_reason: None,
            create_time: SystemTime::UNIX_EPOCH,
            provisioned_memory: 16,
            endpoint: "endpoint".to_string(),
            public_connectivity: false,
            replica_count: 1,
            kms_key_identifier: None,
            vector_search_configuration: None,
            deletion_protection: false,
            source_snapshot_id: None,
            build_number: None,
            tags: BTreeMap::new(),
            settles_at: Some(now + PROVISIONING_SPAN),
        };

        graph.settle(now);
        assert_eq!(
            graph.status,
            GraphStatus::Creating,
            "case not-yet-due: expected CREATING, got {:?}",
            graph.status
        );

        graph.settle(now + PROVISIONING_SPAN);
        assert_eq!(
            graph.status,
            GraphStatus::Available,
            "case due: expected AVAILABLE, got {:?}",
            graph.status
        );
        assert!(
            graph.settles_at.is_none(),
            "case due: the settled transition must be cleared, got {:?}",
            graph.settles_at
        );
    }

    /// Every minted snapshot id matches the modelled `SnapshotId` pattern, no id
    /// repeats, and the graph and snapshot id spaces stay disjoint.
    #[test]
    fn minted_snapshot_ids_match_the_modelled_pattern_and_are_unique() {
        let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..2_000 {
            let id = state.next_snapshot_id();
            assert!(
                is_snapshot_id(&id),
                "id {id:?}: expected a match for ^gs-[a-z0-9]{{10}}$"
            );
            assert!(
                !is_graph_id(&id),
                "id {id:?}: a snapshot id must not also be a graph id"
            );
            assert!(seen.insert(id.clone()), "id {id:?}: minted twice");
        }
    }

    /// The snapshot-id shape test, over values that break each clause of the pattern.
    #[test]
    fn is_snapshot_id_rejects_anything_the_modelled_pattern_rejects() {
        for (candidate, accepted) in [
            ("gs-0123456789", true),
            ("gs-abcdefghij", true),
            ("gs-zz99zz99zz", true),
            ("gs-tooshort", false),
            ("gs-waytoolongforten", false),
            ("gs-ABCDEFGHIJ", false),
            ("g-0123456789", false),
            ("0123456789", false),
            ("", false),
        ] {
            let actual = is_snapshot_id(candidate);
            assert_eq!(
                actual, accepted,
                "case {candidate:?}: expected accepted={accepted}, got {actual}"
            );
        }
    }

    /// The shared resource-name rule, with a row per clause of the modelled pattern
    /// `^(?!<prefix>)[a-z][a-z0-9]*(-[a-z0-9]+)*$` and a column per reserved prefix
    /// (`GraphName`'s `g-`, `SnapshotName`'s `gs-`): the lookahead is the only clause
    /// the two names differ on.
    #[test]
    fn the_resource_name_rule_differs_only_in_the_reserved_prefix() {
        // (candidate, accepted as a graph name, accepted as a snapshot name)
        let cases: &[(&str, bool, bool)] = &[
            ("a", true, true),
            ("g", true, true),
            ("gs", true, true),
            ("my-snapshot-1", true, true),
            ("snapshot9", true, true),
            // `g-` is reserved for graph ids only, `gs-` for snapshot ids only.
            ("g-abc", false, true),
            ("gs-abc", true, false),
            ("1snapshot", false, false),
            ("-snapshot", false, false),
            ("My-Snapshot", false, false),
            ("my_snapshot", false, false),
            ("my--snapshot", false, false),
            ("my-snapshot-", false, false),
            ("", false, false),
        ];
        for (name, as_graph, as_snapshot) in cases {
            let actual_graph = resource_name_is_well_formed(name, "g-");
            assert_eq!(
                actual_graph, *as_graph,
                "case {name:?} as a graph name: expected accepted={as_graph}, \
                 got {actual_graph}"
            );
            let actual_snapshot = resource_name_is_well_formed(name, "gs-");
            assert_eq!(
                actual_snapshot, *as_snapshot,
                "case {name:?} as a snapshot name: expected accepted={as_snapshot}, \
                 got {actual_snapshot}"
            );
        }
    }

    /// The ARN lookup resolves a snapshot as well as a graph: both are taggable, and
    /// each resolves only through the ARN the service minted for it.
    #[test]
    fn tags_of_resolves_a_stored_snapshot_by_its_minted_arn() {
        let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        let graph_id = "g-0123456789";
        let graph_arn =
            ::engine::emulator_arn(ARN_NAMESPACE, "us-east-1", &format!("graph/{graph_id}"));
        state.insert_graph(sample_graph(graph_id, &graph_arn));

        let snapshot_id = "gs-0123456789";
        let snapshot_arn = ::engine::emulator_arn(
            ARN_NAMESPACE,
            "us-east-1",
            &format!("graph-snapshot/{snapshot_id}"),
        );
        state.insert_snapshot(Snapshot {
            id: snapshot_id.to_string(),
            name: "my-snapshot".to_string(),
            arn: snapshot_arn.clone(),
            source_graph_id: graph_id.to_string(),
            status: SnapshotStatus::Available,
            snapshot_create_time: SystemTime::UNIX_EPOCH,
            kms_key_identifier: None,
            tags: [("env".to_string(), "prod".to_string())].into_iter().collect(),
            settles_at: None,
        });

        // (case, the ARN a request names, the tag entries it must resolve to)
        let cases: Vec<(&str, String, Option<usize>)> = vec![
            ("the snapshot's minted ARN", snapshot_arn.clone(), Some(1)),
            ("the graph's minted ARN", graph_arn, Some(0)),
            (
                "the snapshot id under the graph resource type",
                ::engine::emulator_arn(ARN_NAMESPACE, "us-east-1", &format!("graph/{snapshot_id}")),
                None,
            ),
            (
                "the snapshot ARN in another region",
                ::engine::emulator_arn(
                    ARN_NAMESPACE,
                    "eu-west-1",
                    &format!("graph-snapshot/{snapshot_id}"),
                ),
                None,
            ),
        ];
        for (case, candidate, expected) in cases {
            let actual = state.tags_of(&candidate).map(BTreeMap::len);
            assert_eq!(
                actual, expected,
                "case {case:?} ({candidate:?}): expected {expected:?} tag entries, \
                 got {actual:?}"
            );
            let actual_mut = state.tags_of_mut(&candidate).map(|tags| tags.len());
            assert_eq!(
                actual_mut, expected,
                "case {case:?} ({candidate:?}) mutably: expected {expected:?} tag \
                 entries, got {actual_mut:?}"
            );
        }
    }

    /// A snapshot's pending transition is owed at a future instant too, so a read
    /// taken immediately still sees `CREATING`.
    #[test]
    fn a_pending_snapshot_transition_settles_only_once_it_is_due() {
        let now = Instant::now();
        let mut snapshot = Snapshot {
            id: "gs-0123456789".to_string(),
            name: "example".to_string(),
            arn: "arn".to_string(),
            source_graph_id: "g-0123456789".to_string(),
            status: SnapshotStatus::Creating,
            snapshot_create_time: SystemTime::UNIX_EPOCH,
            kms_key_identifier: None,
            tags: BTreeMap::new(),
            settles_at: Some(now + PROVISIONING_SPAN),
        };

        snapshot.settle(now);
        assert_eq!(
            snapshot.status,
            SnapshotStatus::Creating,
            "case not-yet-due: expected CREATING, got {:?}",
            snapshot.status
        );

        snapshot.settle(now + PROVISIONING_SPAN);
        assert_eq!(
            snapshot.status,
            SnapshotStatus::Available,
            "case due: expected AVAILABLE, got {:?}",
            snapshot.status
        );
        assert!(
            snapshot.settles_at.is_none(),
            "case due: the settled transition must be cleared, got {:?}",
            snapshot.settles_at
        );
    }

    /// Every status this slice stores renders as one of the values the model
    /// declares, so no handler can answer a status outside the modelled enum.
    #[test]
    fn every_stored_status_is_a_modelled_enum_value() {
        for status in [
            GraphStatus::Creating,
            GraphStatus::Available,
            GraphStatus::Updating,
            GraphStatus::Snapshotting,
            GraphStatus::Deleting,
        ] {
            let wire = status.as_wire();
            assert!(
                crate::ops::types::GRAPH_STATUS_VALUES.contains(&wire),
                "case {status:?}: {wire:?} is not a modelled GraphStatus value"
            );
        }
        for status in [SnapshotStatus::Creating, SnapshotStatus::Available] {
            let wire = status.as_wire();
            assert!(
                crate::ops::types::SNAPSHOT_STATUS_VALUES.contains(&wire),
                "case {status:?}: {wire:?} is not a modelled SnapshotStatus value"
            );
        }
    }
}
