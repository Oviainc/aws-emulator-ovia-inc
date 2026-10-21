// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-neptunegraph-amazonneptunegraph service.
//!
//! The graph resource is the root of this service's control plane: snapshots,
//! import/export tasks, private endpoints and queries all hang off a graph, so the
//! store below is the one every later slice extends rather than duplicates.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::VectorSearchConfiguration;

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
/// produces (`FAILED`, `IMPORTING`, `RESETTING`, `SNAPSHOTTING`, `STARTING`,
/// `STOPPED`, `STOPPING`) belong to operations that are still scaffolded, and are
/// added with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphStatus {
    /// `CreateGraph` has accepted the graph; it is not usable yet.
    Creating,
    /// The graph is provisioned and serving.
    Available,
    /// `UpdateGraph` has accepted a reconfiguration.
    Updating,
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

/// Per-service state for com-amazonaws-neptunegraph-amazonneptunegraph. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNeptunegraphAmazonneptunegraphState {
    /// Every stored graph, keyed by its minted `g-xxxxxxxxxx` id. A `BTreeMap` so
    /// `ListGraphs` has one stable total order to page over.
    pub graphs: BTreeMap<String, Graph>,
    /// `graphName` -> graph id: the uniqueness index behind `CreateGraph`'s
    /// `ConflictException`.
    pub graph_names: HashMap<String, String>,
    /// Feeds the graph-id generator, so ids are unique within a store.
    pub id_counter: u64,
}

impl ComAmazonawsNeptunegraphAmazonneptunegraphState {
    /// Mint an id no graph in this store holds, matching the modelled `GraphId`
    /// pattern `^g-[a-z0-9]{10}$`.
    pub fn next_graph_id(&mut self) -> String {
        loop {
            self.id_counter = self.id_counter.wrapping_add(1);
            let id = encode_graph_id(scramble(self.id_counter) % BASE36_POW10);
            if !self.graphs.contains_key(&id) {
                return id;
            }
        }
    }

    /// Store a new graph and index its name.
    pub fn insert_graph(&mut self, graph: Graph) {
        self.graph_names.insert(graph.name.clone(), graph.id.clone());
        self.graphs.insert(graph.id.clone(), graph);
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
    let Some(suffix) = value.strip_prefix("g-") else {
        return false;
    };
    suffix.len() == 10
        && suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
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

/// Spell `value` (already reduced below [`BASE36_POW10`]) as `g-` plus exactly ten
/// base-36 digits, matching the modelled `GraphId` pattern `^g-[a-z0-9]{10}$`.
fn encode_graph_id(mut value: u64) -> String {
    let mut digits = ['0'; 10];
    for slot in digits.iter_mut().rev() {
        // `from_digit` is total for radix 36 and a remainder below 36, and yields
        // lowercase letters above 9; the fallback keeps a handler panic-free.
        *slot = char::from_digit((value % 36) as u32, 36).unwrap_or('0');
        value /= 36;
    }
    let mut id = String::with_capacity(12);
    id.push_str("g-");
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
}
