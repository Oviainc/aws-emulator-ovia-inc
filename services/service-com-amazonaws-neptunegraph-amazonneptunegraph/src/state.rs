// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-neptunegraph-amazonneptunegraph service.
//!
//! The graph resource is the root of this service's control plane: snapshots,
//! import/export tasks, private endpoints and queries all hang off a graph, so the
//! store below is the one every later slice extends rather than duplicates.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    ExportFilter, ImportOptions, ResourceNotFoundException, VectorSearchConfiguration,
};

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

/// How long after the `DeleteGraph` that starts it a graph's deletion is owed: the
/// graph answers `DELETING` — and stays listed, with its name still reserved — until
/// this span has passed, after which the next read finds the record gone.
///
/// The real service takes minutes to tear a graph down; this emulator picks a span
/// inside the emulator-wide 250ms..750ms band for a transition the clock owes, so a
/// caller that reads the graph straight after `DeleteGraph` still observes the
/// transient status (and the state errors that go with it). No handler ever sleeps for
/// it — the due instant is stored and compared on read.
pub const DELETION_SPAN: Duration = Duration::from_millis(500);

/// How long after the call that accepts it a cancellation is owed: the cancelled
/// record answers `CANCELLING` until this span has passed, then the next read sees the
/// cancellation through — a query is terminated and no longer reported, and an export
/// task reports `CANCELLED`.
///
/// The real service stops the cancelled work as soon as its engine notices the cancel;
/// this emulator picks a span inside the emulator-wide 250ms..750ms band for a
/// transition the clock owes, so a caller that reads the record straight after
/// cancelling it still observes the transient `CANCELLING` state. No handler ever sleeps
/// for it — the due instant is stored and compared on read.
pub const CANCELLATION_SPAN: Duration = Duration::from_millis(500);

/// How long after its admission a query is over on its own: the span after which an
/// admitted query has finished, so the store stops reporting it even if nobody ever
/// cancels it.
///
/// An in-flight query is a transient record, not a resource a caller deletes: without
/// a due end instant a query admitted by a request would sit in the store for the life
/// of the process. The real service's queries run for as long as their work takes;
/// this emulator picks a span inside the emulator-wide 250ms..750ms band for a
/// transition the clock owes, so a caller that reads a query straight after admitting
/// it still sees it in flight. It is a bound on how long a RECORD is kept, not a claim
/// about work: nothing executes a query here, and no result is ever reported for one.
/// No handler sleeps for it — the due instant is stored and compared on read.
pub const QUERY_RUN_SPAN: Duration = Duration::from_millis(750);

/// How long after the `DeletePrivateGraphEndpoint` that accepts it a private graph
/// endpoint's removal is owed: the endpoint answers `DELETING` until this span has
/// passed, then the next read finds it gone.
///
/// The real service tears a VPC endpoint down over minutes; this emulator picks a span
/// inside the emulator-wide 250ms..750ms band for a transition the clock owes, so a
/// caller that reads the endpoint straight after deleting it still observes the
/// transient `DELETING` status. No handler ever sleeps for it — the due instant is
/// stored and compared on read.
pub const ENDPOINT_DELETION_SPAN: Duration = Duration::from_millis(500);

/// The key a graph (and a snapshot of one) reports when its data is encrypted with a
/// key the service owns rather than one the caller named.
///
/// Every graph of this service is encrypted, so the reported key is never genuinely
/// unknown: a create request that names no `kmsKeyIdentifier` gets a service-owned key,
/// which has no ARN a caller may name, so the record reports this sentinel instead of
/// omitting the member. The model itself says the reported value need not be an ARN:
/// `GraphSummary$kmsKeyIdentifier` targets a plain `String`
/// (`smithy_query(query="shape",shape="GraphSummary")`) where every create INPUT member
/// targets the ARN-patterned `KmsKeyArn`.
pub const SERVICE_OWNED_KMS_KEY: &str = "AWS_OWNED_KEY";

/// The key a graph's data is encrypted with, from the key its create request named:
/// that key, or [`SERVICE_OWNED_KMS_KEY`] when the request named none.
///
/// Stated once here because every operation that mints a graph answers the same
/// question, and a graph record's key member is never absent.
pub fn graph_encryption_key(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| SERVICE_OWNED_KMS_KEY.to_string())
}

/// The build a graph record reports for the software serving it: "The build number of
/// the graph software"
/// (`smithy_query(query="doc",shape="CreateGraphOutput",member="buildNumber")`).
///
/// The software serving these graphs is this emulator, so the build reported is this
/// crate's own version — a real build identifier of the thing answering, rather than a
/// version string of an engine nothing here runs. It is a property of every graph this
/// service holds, so it is the same for all of them and stable across reads.
pub const GRAPH_SOFTWARE_BUILD_NUMBER: &str = env!("CARGO_PKG_VERSION");

/// The lifecycle statuses this slice of the emulator puts a graph into.
///
/// Every variant's wire form is one of the modelled `GraphStatus` values
/// (`crate::ops::types::GRAPH_STATUS_VALUES`). The statuses this slice never
/// produces (`FAILED`, `RESETTING`) belong to operations that are
/// still scaffolded, and are added with them.
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
    /// `StartImportTask` or `CreateGraphUsingImportTask` has accepted a bulk import
    /// into the graph; the graph is loading data and is not reconfigurable until the
    /// task stops.
    Importing,
    /// `StopGraph` has accepted the stop; the graph is winding down.
    Stopping,
    /// The graph is stopped: it exists and keeps its configuration and data, but it
    /// is not serving, so nothing reaches its endpoint until `StartGraph`.
    Stopped,
    /// `StartGraph` has accepted the start; the graph is coming back up.
    Starting,
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
            Self::Importing => "IMPORTING",
            Self::Stopping => "STOPPING",
            Self::Stopped => "STOPPED",
            Self::Starting => "STARTING",
            Self::Deleting => "DELETING",
        }
    }

    /// Whether this is a RESTING status: one the clock owes nothing out of.
    ///
    /// A graph rests either serving (`AVAILABLE`) or stopped (`STOPPED`); every other
    /// status this slice produces is a transition already in flight, during which a
    /// request that would start another transition is a conflict.
    pub const fn is_at_rest(self) -> bool {
        matches!(self, Self::Available | Self::Stopped)
    }
}

/// A status change the clock owes a graph: the status it lands in, and the instant
/// from which it is due.
///
/// The target is carried beside the instant because a graph does not always settle
/// into the same status: provisioning, reconfiguring and starting settle to
/// `AVAILABLE`, while stopping settles to `STOPPED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingTransition {
    /// The status the graph takes once this transition falls due.
    pub settled_status: GraphStatus,
    /// The instant from which the transition is due.
    pub due: Instant,
}

impl PendingTransition {
    /// The transition a call starting at `now` owes: `settled_status`, due
    /// [`PROVISIONING_SPAN`] later.
    pub fn owed(settled_status: GraphStatus, now: Instant) -> Self {
        Self {
            settled_status,
            due: now + PROVISIONING_SPAN,
        }
    }
}

/// One node of a graph's data: the labels it carries and the properties set on it.
///
/// A node is addressed by its own id (openCypher's `~id`), which is the key it sits
/// under in [`GraphData::nodes`], so the id is not repeated here.
#[derive(Debug, Clone, Default)]
pub struct GraphNode {
    /// The node's labels. openCypher lets one node carry several, and a summary of
    /// the data counts DISTINCT labels, so they are held as a set.
    pub labels: BTreeSet<String>,
    /// The properties set on the node, by key. A value is held as the JSON document
    /// the data plane exchanges it as, so a property keeps the type it was written
    /// with; a summary of the data counts keys and usages only.
    pub properties: BTreeMap<String, serde_json::Value>,
}

/// One edge of a graph's data: its label, the nodes it joins, and its properties.
///
/// An edge is addressed by its own id, which is the key it sits under in
/// [`GraphData::edges`], so the id is not repeated here.
#[derive(Debug, Clone)]
pub struct GraphEdge {
    /// The edge's label. openCypher gives an edge exactly one.
    pub label: String,
    /// The id of the node this edge leaves.
    pub from_node: String,
    /// The id of the node this edge enters.
    pub to_node: String,
    /// The properties set on the edge, by key, held as [`GraphNode::properties`]
    /// holds a node's.
    pub properties: BTreeMap<String, serde_json::Value>,
}

/// The DATA one graph holds: its nodes and its edges, plus the instant the
/// statistics over them were last computed.
///
/// It lives INSIDE [`Graph`] rather than in a map of its own, so a graph's data is
/// created, read and reclaimed with the graph that holds it, and no second copy of
/// a graph's contents can drift from this one. Both maps are `BTreeMap`s, so every
/// reading of the data — a count, a label list, a grouping — has one stable total
/// order to walk.
///
/// Writing it belongs to the data plane (an openCypher write, a reset, a bulk
/// load); the mutators below are the one way in, because each of them also moves
/// [`GraphData::statistics_computed_at`].
#[derive(Debug, Clone)]
pub struct GraphData {
    /// Every node of this graph, keyed by its node id.
    pub nodes: BTreeMap<String, GraphNode>,
    /// Every edge of this graph, keyed by its edge id.
    pub edges: BTreeMap<String, GraphEdge>,
    /// When the statistics over this data were last computed.
    ///
    /// Statistics are a function of the data, so they are recomputed when the data
    /// changes and not otherwise: every mutator below sets this to the instant its
    /// caller is writing at, and a graph nobody has written to since it was created
    /// still reports the statistics of its empty data, computed when the graph came
    /// up.
    pub statistics_computed_at: SystemTime,
}

impl GraphData {
    /// The empty data a graph starts out holding, its statistics computed `as_of` —
    /// the instant the graph itself was created.
    pub fn empty(as_of: SystemTime) -> Self {
        Self {
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            statistics_computed_at: as_of,
        }
    }

    /// Store one node under `id`, replacing the node that id already named. The
    /// statistics over the data are recomputed `as_of` the instant the caller is
    /// writing at — the store reads no clock of its own.
    ///
    /// `Err` carries the reason the store refuses the write, for the caller to
    /// report in the wording its own operation uses.
    pub fn insert_node(
        &mut self,
        id: &str,
        node: GraphNode,
        as_of: SystemTime,
    ) -> Result<(), String> {
        if node.properties.len() > MAX_PROPERTIES_PER_ELEMENT {
            return Err(format!(
                "this emulator holds at most {MAX_PROPERTIES_PER_ELEMENT} properties per node"
            ));
        }
        if !self.nodes.contains_key(id) && self.nodes.len() >= MAX_NODES_PER_GRAPH {
            return Err(format!(
                "this emulator holds at most {MAX_NODES_PER_GRAPH} nodes per graph"
            ));
        }
        self.nodes.insert(id.to_string(), node);
        self.statistics_computed_at = as_of;
        Ok(())
    }

    /// Store one edge under `id`, replacing the edge that id already named, with
    /// the statistics recomputed `as_of` the instant the caller is writing at.
    ///
    /// An edge joins two nodes OF THIS GRAPH, so an endpoint the graph does not
    /// hold is refused: an edge dangling off a node nobody can read would be
    /// counted by every later summary of the data.
    pub fn insert_edge(
        &mut self,
        id: &str,
        edge: GraphEdge,
        as_of: SystemTime,
    ) -> Result<(), String> {
        if edge.properties.len() > MAX_PROPERTIES_PER_ELEMENT {
            return Err(format!(
                "this emulator holds at most {MAX_PROPERTIES_PER_ELEMENT} properties per edge"
            ));
        }
        for endpoint in [&edge.from_node, &edge.to_node] {
            if !self.nodes.contains_key(endpoint) {
                return Err(format!(
                    "edge {id} joins node {endpoint}, which this graph does not hold"
                ));
            }
        }
        if !self.edges.contains_key(id) && self.edges.len() >= MAX_EDGES_PER_GRAPH {
            return Err(format!(
                "this emulator holds at most {MAX_EDGES_PER_GRAPH} edges per graph"
            ));
        }
        self.edges.insert(id.to_string(), edge);
        self.statistics_computed_at = as_of;
        Ok(())
    }

    /// Drop every node and edge: the data a graph holds after it is emptied, with
    /// the statistics recomputed `as_of` the instant the caller is emptying it at.
    pub fn clear(&mut self, as_of: SystemTime) {
        self.nodes.clear();
        self.edges.clear();
        self.statistics_computed_at = as_of;
    }

    /// The distinct labels of the edges LEAVING `node_id`.
    ///
    /// Derived from the edges rather than stored on the node, so it cannot disagree
    /// with the edges the graph holds. It SCANS the edges, so it answers about ONE
    /// node: a reading that needs this for every node indexes the edges once
    /// instead (see `crate::ops::get_graph_summary`).
    pub fn outgoing_edge_labels(&self, node_id: &str) -> BTreeSet<String> {
        self.edges
            .values()
            .filter(|edge| edge.from_node == node_id)
            .map(|edge| edge.label.clone())
            .collect()
    }
}

/// One stored Neptune Analytics graph: every member `GetGraph` reports, plus the
/// status change the clock still owes it, if any.
#[derive(Debug, Clone)]
pub struct Graph {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The Region the request that created this graph was signed for.
    ///
    /// neptune-graph is a REGIONAL control plane: a graph exists in exactly one
    /// Region, so this is the lookup and listing predicate every operation applies
    /// beside the identifier (see [`Self::is_in`]) — not merely the Region that names
    /// the record's ARN and endpoint.
    pub region: String,
    pub status: GraphStatus,
    pub status_reason: Option<String>,
    pub create_time: SystemTime,
    pub provisioned_memory: i32,
    pub endpoint: String,
    pub public_connectivity: bool,
    pub replica_count: i32,
    /// The key this graph's data is encrypted with. Every graph is encrypted, so this
    /// is always a key: the customer managed key the create request named, or
    /// [`SERVICE_OWNED_KMS_KEY`] when it named none (see [`graph_encryption_key`]).
    pub kms_key_identifier: String,
    pub vector_search_configuration: Option<VectorSearchConfiguration>,
    pub deletion_protection: bool,
    pub source_snapshot_id: Option<String>,
    /// The build of the graph software serving this graph
    /// ([`GRAPH_SOFTWARE_BUILD_NUMBER`]), which every graph record reports.
    pub build_number: String,
    pub tags: BTreeMap<String, String>,
    /// The status change the clock owes this graph, and when it falls due. `None`
    /// once there is nothing outstanding.
    pub pending: Option<PendingTransition>,
    /// When the accepted deletion of this graph completes, after which the record is
    /// no longer one this service holds. `None` for a graph nobody has deleted.
    ///
    /// A deletion is a transition like any other — the graph reports `DELETING` and
    /// stays observable while it runs — but what it settles into is the ABSENCE of
    /// the record, which no `GraphStatus` can express, so it is carried here rather
    /// than in [`Graph::pending`].
    pub deletes_at: Option<Instant>,
    /// The nodes and edges this graph holds: the data every reading of the graph's
    /// contents (a summary of it, a query over it, an export of it) is computed
    /// from. A freshly created graph holds none.
    pub data: GraphData,
}

impl Graph {
    /// Whether this record belongs to `region`: the predicate that makes a resource
    /// of a regional control plane invisible outside the Region that holds it.
    pub fn is_in(&self, region: &str) -> bool {
        self.region == region
    }

    /// Materialise the transition the clock has already made due.
    ///
    /// This is the one write a reporting operation (`GetGraph`, `ListGraphs`) is
    /// allowed to make: the graph would have reached the pending transition's
    /// `settled_status` at its `due` instant whether or not anybody called, so
    /// applying it on read is bookkeeping, not a state change caused by the read.
    pub fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending else {
            return;
        };
        if now >= pending.due {
            self.status = pending.settled_status;
            self.pending = None;
        }
    }

    /// Report `transient` now and settle into `settled` [`PROVISIONING_SPAN`] after
    /// `now`: the shape of every graph transition an operation accepts.
    pub fn begin_transition(&mut self, transient: GraphStatus, settled: GraphStatus, now: Instant) {
        self.status = transient;
        self.pending = Some(PendingTransition::owed(settled, now));
    }

    /// Report `DELETING` now and complete the deletion [`DELETION_SPAN`] after `now`.
    ///
    /// Any status change that was still outstanding is dropped: the graph is on its way
    /// out, so nothing it was heading for can fall due any more.
    pub fn begin_deletion(&mut self, now: Instant) {
        self.status = GraphStatus::Deleting;
        self.pending = None;
        self.deletes_at = Some(now + DELETION_SPAN);
    }

    /// Whether the deletion this graph is undergoing has fallen due, so the record is
    /// one the service no longer holds.
    pub fn deletion_complete(&self, now: Instant) -> bool {
        self.deletes_at.is_some_and(|due| now >= due)
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
    /// The Region the request that created this snapshot was signed for — the Region
    /// of the graph it was taken from, and the only Region it is visible in. The same
    /// predicate [`Graph::region`] carries, for the same reason.
    pub region: String,
    /// The graph the snapshot was taken from. Kept as the graph's id (not a
    /// borrow), so deleting that graph leaves the snapshot's provenance intact.
    pub source_graph_id: String,
    pub status: SnapshotStatus,
    pub snapshot_create_time: SystemTime,
    /// The key the source graph's data is encrypted with: a snapshot of an
    /// encrypted graph is encrypted with the same key. Always a key, for the reason
    /// [`Graph::kms_key_identifier`] is.
    pub kms_key_identifier: String,
    pub tags: BTreeMap<String, String>,
    /// When the pending `CREATING` -> `AVAILABLE` transition falls due. `None` once
    /// there is nothing outstanding.
    pub settles_at: Option<Instant>,
}

impl Snapshot {
    /// Whether this record belongs to `region`: the same predicate [`Graph::is_in`]
    /// applies to a graph.
    pub fn is_in(&self, region: &str) -> bool {
        self.region == region
    }

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

/// The lifecycle statuses this slice of the emulator puts an import task into.
///
/// Every variant's wire form is one of the modelled `ImportTaskStatus` values
/// (`crate::ops::types::IMPORT_TASK_STATUS_VALUES`). The values that report a STAGE
/// of a running bulk load (`ANALYZING_DATA`, `EXPORTING`, `IMPORTING`,
/// `REPROVISIONING`, `ROLLING_BACK`, `SUCCEEDED`) are deliberately never produced:
/// nothing in this emulator reads a task's `source`, so no task may report a stage —
/// or a success — of a load nothing performed. An accepted task therefore stops in
/// the modelled `FAILED` status carrying [`IMPORT_NOT_RUN_REASON`], and `DELETED`
/// joins this enum with the operation that would produce it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportTaskStatus {
    /// `StartImportTask` has accepted the task; it has not stopped yet.
    Initializing,
    /// `CancelImportTask` has accepted the cancellation; the task is winding down.
    Cancelling,
    /// The task stopped because it was cancelled.
    Cancelled,
    /// The task stopped without importing the data.
    Failed,
}

impl ImportTaskStatus {
    /// This status as the modelled `ImportTaskStatus` enum value.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Initializing => "INITIALIZING",
            Self::Cancelling => "CANCELLING",
            Self::Cancelled => "CANCELLED",
            Self::Failed => "FAILED",
        }
    }

    /// Whether this is a TERMINAL status: the task has stopped, so the clock owes it
    /// nothing more and its outcome can no longer change.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Failed)
    }

    /// Whether a task in this status can still be cancelled: only one whose import is
    /// in flight. A task that has stopped has no work left to stop, and one already
    /// `CANCELLING` is stopping already — each is a conflict rather than a second
    /// cancellation.
    pub const fn is_cancellable(self) -> bool {
        matches!(self, Self::Initializing)
    }
}

/// A status change the clock owes an import task: the status it lands in, the reason
/// that status carries, and the instant from which it is due.
///
/// The reason travels with the target because a task's `statusReason` is a property
/// of the status it settles into — why the load stopped — and the two must never be
/// read apart.
#[derive(Debug, Clone)]
pub struct ImportTaskTransition {
    /// The status the task takes once this transition falls due.
    pub settled_status: ImportTaskStatus,
    /// The `statusReason` the settled status carries, if any.
    pub settled_reason: Option<String>,
    /// The instant from which the transition is due.
    pub due: Instant,
}

/// One stored import task: every member `GetImportTask` reports, plus the status
/// change the clock still owes it, if any.
///
/// A task is a resource in its own right — it is read by its own `t-` id and outlives
/// the load it describes — so it is stored beside the graphs rather than inside the
/// graph record.
#[derive(Debug, Clone)]
pub struct ImportTask {
    pub id: String,
    /// The Region the request that started this task was signed for — the same
    /// predicate [`Graph::region`] carries, for the same reason: this is a regional
    /// control plane, so a task of one Region is not visible from another.
    pub region: String,
    /// The graph the data is imported INTO, kept as the graph's id (not a borrow), so
    /// deleting that graph leaves the task's provenance intact.
    pub graph_id: String,
    pub source: String,
    pub format: Option<String>,
    pub parquet_type: Option<String>,
    pub blank_node_handling: Option<String>,
    pub fail_on_error: bool,
    pub role_arn: String,
    pub import_options: Option<ImportOptions>,
    pub status: ImportTaskStatus,
    pub status_reason: Option<String>,
    /// How many times this task has been run. One per accepted `StartImportTask`:
    /// nothing in this emulator retries a task, so the stored count is what
    /// `GetImportTask` reports rather than a number invented per read.
    pub attempt_number: i32,
    /// The status change the clock owes this task, and when it falls due. `None` once
    /// there is nothing outstanding.
    pub pending: Option<ImportTaskTransition>,
}

impl ImportTask {
    /// Whether this record belongs to `region`: the same predicate [`Graph::is_in`]
    /// applies to a graph.
    pub fn is_in(&self, region: &str) -> bool {
        self.region == region
    }

    /// Materialise the transition the clock has already made due.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: the
    /// task would have reached this status at its `due` instant whether or not
    /// anybody read it, so applying it on read is bookkeeping rather than a state
    /// change caused by the read.
    pub fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if now >= pending.due {
            self.status = pending.settled_status;
            self.status_reason = pending.settled_reason.clone();
            self.pending = None;
        }
    }

    /// Report `transient` now and settle into `settled` (with `reason`)
    /// [`PROVISIONING_SPAN`] after `now`: the shape of every import-task transition
    /// an operation accepts, so a caller that reads the task straight after the call
    /// still observes the transient status.
    pub fn begin_transition(
        &mut self,
        transient: ImportTaskStatus,
        settled: ImportTaskStatus,
        reason: Option<String>,
        now: Instant,
    ) {
        self.status = transient;
        self.pending = Some(ImportTaskTransition {
            settled_status: settled,
            settled_reason: reason,
            due: now + PROVISIONING_SPAN,
        });
    }
}

/// How long after the `StartExportTask` that starts it an export task's terminal
/// status is owed: the task answers `INITIALIZING` until this span has passed, then
/// the next read finds the status the clock settled it into.
///
/// The real service exports for as long as the graph's data takes; this emulator picks
/// a span inside the emulator-wide 250ms..750ms band for a transition the clock owes,
/// so a caller that reads the task straight after starting it still observes the
/// transient `INITIALIZING` status. No handler ever sleeps for it — the due instant is
/// stored and compared on read.
pub const EXPORT_TASK_SPAN: Duration = Duration::from_millis(500);

/// Why an export task this emulator accepts ends `FAILED`.
///
/// `StartExportTask` starts a RUN that reads a graph's vertices and edges and writes
/// them, as CSV or Parquet objects encrypted with the caller's KMS key, under the
/// Amazon S3 URI the request names. This emulator holds no graph data plane and makes
/// no outbound connection to S3 or KMS, so nothing is ever read from the graph and no
/// object is ever written: reporting `EXPORTING` or `SUCCEEDED` would claim a run that
/// did not happen and objects that do not exist, so the task reaches the modelled
/// failure status carrying this reason instead.
pub const EXPORT_NOT_PERFORMED_REASON: &str =
    "this emulator does not export graph data: no data was read from the graph and nothing \
     was written to the destination";

/// The `statusReason` of an export task that stopped because it was cancelled.
pub const EXPORT_CANCELLED_REASON: &str =
    "The export task was cancelled by a CancelExportTask request.";

/// The statuses this slice of the emulator puts an export task into.
///
/// Every variant's wire form is a modelled `ExportTaskStatus` value
/// (`crate::ops::types::EXPORT_TASK_STATUS_VALUES`). `EXPORTING` and `SUCCEEDED` are
/// never produced here because no export run happens (see
/// [`EXPORT_NOT_PERFORMED_REASON`]), and `DELETED` joins this enum with an operation
/// that would produce it — this service models none that deletes an export task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportTaskStatus {
    /// `StartExportTask` has accepted the task; no data has moved yet.
    Initializing,
    /// `CancelExportTask` has accepted the cancellation; the task is winding down.
    Cancelling,
    /// The task stopped because it was cancelled ([`EXPORT_CANCELLED_REASON`]).
    Cancelled,
    /// The task is over and nothing was exported ([`EXPORT_NOT_PERFORMED_REASON`]).
    Failed,
}

impl ExportTaskStatus {
    /// This status as the modelled `ExportTaskStatus` enum value.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Initializing => "INITIALIZING",
            Self::Cancelling => "CANCELLING",
            Self::Cancelled => "CANCELLED",
            Self::Failed => "FAILED",
        }
    }

    /// Whether this is a TERMINAL status: the task has stopped, so the clock owes it
    /// nothing more and its outcome can no longer change.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Failed)
    }

    /// Whether a task in this status can still be cancelled: only one whose export is
    /// in flight. A task that has stopped has no work left to stop, and one already
    /// `CANCELLING` is stopping already — each is a conflict rather than a second
    /// cancellation.
    pub const fn is_cancellable(self) -> bool {
        matches!(self, Self::Initializing)
    }

    /// The status the clock owes a task currently reported in this status, if any: an
    /// accepted task stops without exporting, and an accepted cancellation completes.
    /// A terminal status is owed nothing.
    pub const fn settles_into(self) -> Option<Self> {
        match self {
            Self::Initializing => Some(Self::Failed),
            Self::Cancelling => Some(Self::Cancelled),
            Self::Cancelled | Self::Failed => None,
        }
    }

    /// The `statusReason` this status carries — why the task stopped. It is a property
    /// of the status itself, so the two can never be read apart.
    pub const fn reason(self) -> Option<&'static str> {
        match self {
            Self::Failed => Some(EXPORT_NOT_PERFORMED_REASON),
            Self::Cancelled => Some(EXPORT_CANCELLED_REASON),
            Self::Initializing | Self::Cancelling => None,
        }
    }
}

/// One stored export task: every member `GetExportTask` reports, plus the instant at
/// which the terminal status the clock owes it becomes due.
///
/// An export task is a resource in its own right — it is addressed by its own
/// `^t-[a-z0-9]{10}$` id and outlives the graph it reads — so it is stored beside the
/// graphs rather than inside the graph record, the way a snapshot is.
#[derive(Debug, Clone)]
pub struct ExportTask {
    pub id: String,
    /// The ARN the tagging surface reads this task's tags back through. Spelled with
    /// the same hyphenated resource segment convention as the other resources this
    /// service mints (`graph/`, `graph-snapshot/`).
    pub arn: String,
    /// The Region the request that started this task was signed for — the same
    /// predicate [`Graph::region`] carries, for the same reason: this is a regional
    /// control plane, so a task of one Region is not visible from another.
    pub region: String,
    /// The graph the export reads, kept as that graph's id (not a borrow) so deleting
    /// the graph leaves the task's provenance intact.
    pub source_graph_id: String,
    pub role_arn: String,
    pub format: String,
    pub parquet_type: Option<String>,
    pub destination: String,
    pub kms_key_identifier: String,
    pub export_filter: Option<ExportFilter>,
    pub status: ExportTaskStatus,
    pub status_reason: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// When the task was accepted: what `ExportTaskDetails$startTime` reports.
    pub start_time: SystemTime,
    /// The monotonic counterpart of [`Self::start_time`], which
    /// `ExportTaskDetails$timeElapsedSeconds` is measured from.
    pub started_at: Instant,
    /// When the terminal status falls due. `None` once there is nothing outstanding.
    pub settles_at: Option<Instant>,
}

impl ExportTask {
    /// Whether this record belongs to `region`: the same predicate [`Graph::is_in`]
    /// applies to a graph.
    pub fn is_in(&self, region: &str) -> bool {
        self.region == region
    }

    /// Materialise the transition the clock has already made due.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: the task
    /// would have ended at `settles_at` whether or not anybody called, so applying it
    /// on read is bookkeeping rather than a state change caused by the read.
    ///
    /// Which status falls due is a property of the status the task is in
    /// ([`ExportTaskStatus::settles_into`]), so an accepted task settles into the
    /// failure no export run can avoid while an accepted cancellation settles into
    /// `CANCELLED`.
    pub fn settle(&mut self, now: Instant) {
        if self.settles_at.is_none_or(|due| now < due) {
            return;
        }
        if let Some(settled) = self.status.settles_into() {
            self.status = settled;
            self.status_reason = settled.reason().map(str::to_string);
        }
        self.settles_at = None;
    }

    /// Accept a cancellation of this task: report the modelled `CANCELLING` status now
    /// ("The status is CANCELLING when the export task is cancelled") and let the clock
    /// owe the move to `CANCELLED` [`CANCELLATION_SPAN`] later, so a caller that reads
    /// the task straight after cancelling it still sees it winding down.
    pub fn begin_cancellation(&mut self, now: Instant) {
        self.status = ExportTaskStatus::Cancelling;
        self.status_reason = None;
        self.settles_at = Some(now + CANCELLATION_SPAN);
    }

    /// How long the task has been running, in whole seconds: what
    /// `ExportTaskDetails$timeElapsedSeconds` reports.
    pub fn elapsed_seconds(&self, now: Instant) -> i64 {
        i64::try_from(now.saturating_duration_since(self.started_at).as_secs())
            .unwrap_or(i64::MAX)
    }

    /// The task's `ExportTaskDetails`, as of `now`.
    ///
    /// `progressPercentage` is zero and the written-element counts are ABSENT because
    /// no export run happens here (see [`EXPORT_NOT_PERFORMED_REASON`]): no vertex or
    /// edge was written, so there is no count to report rather than a count of zero
    /// that would claim a run which wrote nothing.
    pub fn details(&self, now: Instant) -> crate::ops::types::ExportTaskDetails {
        crate::ops::types::ExportTaskDetails {
            start_time: ::aws_smithy_types::DateTime::from(self.start_time),
            time_elapsed_seconds: self.elapsed_seconds(now),
            progress_percentage: 0,
            num_vertices_written: None,
            num_edges_written: None,
        }
    }

    /// This task as `GetExportTask`'s output, as of `now`: the stored record plus the
    /// details derived from the clock, so that read cannot drift from what
    /// `StartExportTask` reported.
    pub fn get_output(&self, now: Instant) -> crate::ops::types::GetExportTaskOutput {
        crate::ops::types::GetExportTaskOutput {
            graph_id: self.source_graph_id.clone(),
            role_arn: self.role_arn.clone(),
            task_id: self.id.clone(),
            status: self.status.as_wire().to_string(),
            format: self.format.clone(),
            destination: self.destination.clone(),
            kms_key_identifier: self.kms_key_identifier.clone(),
            parquet_type: self.parquet_type.clone(),
            status_reason: self.status_reason.clone(),
            export_task_details: Some(self.details(now)),
            export_filter: self.export_filter.clone(),
        }
    }
}

/// The `minProvisionedMemory` a request that names none gets, from the member's own
/// documentation: "The minimum provisioned memory-optimized Neptune Capacity Units
/// (m-NCUs) to use for the graph. Default: 16"
/// (`smithy_query(query="doc",shape="CreateGraphUsingImportTaskInput",
/// member="minProvisionedMemory")`).
pub const DEFAULT_MIN_PROVISIONED_MEMORY: i32 = 16;

/// The provisioned memory a graph gets when the request bounds it NEITHER way, from
/// `maxProvisionedMemory`'s own documentation: "If neither value is specified, 128
/// m-NCUs are used" (`smithy_query(query="doc",
/// shape="CreateGraphUsingImportTaskInput",member="maxProvisionedMemory")`).
pub const UNBOUNDED_IMPORT_PROVISIONED_MEMORY: i32 = 128;

/// The provisioned memory a graph created together with an import task gets, from the
/// bounds the request put on it.
///
/// `minProvisionedMemory` and `maxProvisionedMemory` are bounds on a value the service
/// CHOOSES, not the value itself: "If both the minimum and maximum values are
/// specified, the final provisioned-memory will be chosen per the actual size of your
/// imported data. If neither value is specified, 128 m-NCUs are used"
/// (`smithy_query(query="doc",shape="CreateGraphUsingImportTaskInput",
/// member="maxProvisionedMemory")`). So the chosen value is the FLOOR of the range the
/// request allows — the supplied minimum, or [`DEFAULT_MIN_PROVISIONED_MEMORY`] when
/// only a maximum is given — because what would push it above that floor is the size of
/// the imported data, and [`IMPORT_NOT_RUN_REASON`] says no data is ever imported here.
/// With neither bound supplied the documented [`UNBOUNDED_IMPORT_PROVISIONED_MEMORY`]
/// stands.
pub fn import_provisioned_memory(min: Option<i32>, max: Option<i32>) -> i32 {
    match (min, max) {
        (Some(min), _) => min,
        (None, Some(_)) => DEFAULT_MIN_PROVISIONED_MEMORY,
        (None, None) => UNBOUNDED_IMPORT_PROVISIONED_MEMORY,
    }
}

/// The states this slice of the emulator puts an in-flight query into.
///
/// Every variant's wire form is one of the modelled `QueryState` values
/// (`crate::ops::types::QUERY_STATE_VALUES`), which is the whole set: the model names
/// no terminal state, because a query that has finished — on its own or through
/// `CancelQuery` — stops being a reportable query at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryState {
    /// The query has been accepted but has not started executing yet.
    Waiting,
    /// The query is executing.
    Running,
    /// `CancelQuery` has accepted a cancel; the query is stopping.
    Cancelling,
}

impl QueryState {
    /// This state as the modelled `QueryState` enum value.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Waiting => "WAITING",
            Self::Running => "RUNNING",
            Self::Cancelling => "CANCELLING",
        }
    }
}

/// One in-flight query on a graph: what `GetQuery` reports about it, plus the
/// termination the clock owes it once a cancel has been accepted.
///
/// A query is not a resource that outlives its graph: it exists only while it is
/// waiting, running or stopping, so the record carries the graph it runs on — which is
/// also where the store files it — and is dropped both when that graph goes and when
/// the query terminates.
#[derive(Debug, Clone)]
pub struct Query {
    /// The id the query is addressed by. The model puts no pattern on `queryId`
    /// (`smithy_query(query="operation",operation="GetQuery")`: `String`), so whatever
    /// the operation that admitted the query minted is what identifies it here.
    pub id: String,
    /// The graph the query runs on, kept as that graph's id so a query is only ever
    /// found through the graph the request named.
    pub graph_id: String,
    /// The query text, as `GetQuery`'s `queryString` reports it.
    pub query_string: String,
    pub state: QueryState,
    /// When the query was accepted: the start of the span `waited` measures.
    pub submitted_at: Instant,
    /// When the query started executing: the end of the span `waited` measures and the
    /// start of the span `elapsed` measures. `None` while the query is still waiting.
    pub started_at: Option<Instant>,
    /// When the accepted cancellation falls due, after which the query has terminated
    /// and is no longer reported. `None` while nothing has been cancelled.
    pub terminates_at: Option<Instant>,
    /// When the query is over on its own ([`QUERY_RUN_SPAN`] after its admission),
    /// whether or not it is ever cancelled. A query terminates at the EARLIER of this
    /// instant and [`Self::terminates_at`], so no admitted query stays in the store
    /// indefinitely and a cancel can only shorten its life, never extend it.
    pub completes_at: Instant,
}

impl Query {
    /// A query accepted at `now` and executing from `now`: the state a query admitted
    /// on a graph with capacity for it is in.
    pub fn running(
        id: impl Into<String>,
        graph_id: impl Into<String>,
        query_string: impl Into<String>,
        now: Instant,
    ) -> Self {
        Self {
            id: id.into(),
            graph_id: graph_id.into(),
            query_string: query_string.into(),
            state: QueryState::Running,
            submitted_at: now,
            started_at: Some(now),
            terminates_at: None,
            completes_at: now + QUERY_RUN_SPAN,
        }
    }

    /// A query accepted at `now` that has not started executing yet.
    pub fn waiting(
        id: impl Into<String>,
        graph_id: impl Into<String>,
        query_string: impl Into<String>,
        now: Instant,
    ) -> Self {
        Self {
            id: id.into(),
            graph_id: graph_id.into(),
            query_string: query_string.into(),
            state: QueryState::Waiting,
            submitted_at: now,
            started_at: None,
            terminates_at: None,
            completes_at: now + QUERY_RUN_SPAN,
        }
    }

    /// Accept a cancel at `now`: the query reports `CANCELLING` and terminates
    /// [`CANCELLATION_SPAN`] later.
    ///
    /// IDEMPOTENT, because `CancelQuery` is (`smithy_query(query="operation",
    /// operation="CancelQuery")`: `@idempotent`): a cancel of a query already stopping
    /// leaves the termination it is already owed where it is, so repeating the request
    /// cannot keep the query alive.
    pub fn begin_cancellation(&mut self, now: Instant) {
        if self.state == QueryState::Cancelling {
            return;
        }
        self.state = QueryState::Cancelling;
        self.terminates_at = Some(now + CANCELLATION_SPAN);
    }

    /// Whether this query is over: either the cancellation the clock owes it has fallen
    /// due, or it has run out its own [`QUERY_RUN_SPAN`]. Once it is, the query has
    /// stopped and is no longer a query the service reports.
    pub fn has_terminated(&self, now: Instant) -> bool {
        now >= self.completes_at || self.terminates_at.is_some_and(|due| now >= due)
    }

    /// How long the query waited before executing, in milliseconds: the span from its
    /// acceptance to its start, or to `now` while it is still waiting.
    pub fn waited_millis(&self, now: Instant) -> i32 {
        millis_between(self.submitted_at, self.started_at.unwrap_or(now))
    }

    /// How long the query has been executing, in milliseconds. Zero while it is still
    /// waiting: nothing has run yet.
    pub fn elapsed_millis(&self, now: Instant) -> i32 {
        self.started_at
            .map_or(0, |started| millis_between(started, now))
    }
}

/// The milliseconds from `from` to `to`, saturating into the `Integer` the model
/// reports `waited` and `elapsed` as. Zero when `to` precedes `from`, which
/// [`Instant`] arithmetic makes unreachable here but which no caller has to reason
/// about.
fn millis_between(from: Instant, to: Instant) -> i32 {
    i32::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(i32::MAX)
}

/// The lifecycle statuses this slice of the emulator puts a private graph endpoint
/// into.
///
/// Every variant's wire form is one of the modelled `PrivateGraphEndpointStatus`
/// values (`crate::ops::types::PRIVATE_GRAPH_ENDPOINT_STATUS_VALUES`). `FAILED` is
/// deliberately never produced: nothing here can fail to attach an endpoint, so no
/// endpoint may report a failure that did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateGraphEndpointStatus {
    /// `CreatePrivateGraphEndpoint` has accepted the endpoint; it is not reachable
    /// yet.
    Creating,
    /// The endpoint is provisioned: the graph is reachable through it from the VPC.
    Available,
    /// `DeletePrivateGraphEndpoint` has accepted the deletion; the endpoint is on its
    /// way out and is reclaimed once the clock makes its removal due.
    Deleting,
}

impl PrivateGraphEndpointStatus {
    /// This status as the modelled `PrivateGraphEndpointStatus` enum value.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Deleting => "DELETING",
        }
    }
}

/// One stored private graph endpoint: every member `GetPrivateGraphEndpoint` reports,
/// the graph and VPC the pair of identifiers it is addressed by names, the security
/// groups it was created with, and the instant at which the transition the clock owes
/// it falls due.
///
/// An endpoint belongs to ONE graph in ONE VPC — `GetPrivateGraphEndpoint` and
/// `DeletePrivateGraphEndpoint` address it by `(graphIdentifier, vpcId)` and nothing
/// else — so that pair is its identity and the minted `vpcEndpointId` is a property of
/// the record rather than a second key.
#[derive(Debug, Clone)]
pub struct PrivateGraphEndpoint {
    /// The graph the endpoint reaches. Kept as the graph's id, so the endpoint is
    /// reclaimed with the graph it belongs to.
    pub graph_id: String,
    pub vpc_id: String,
    pub subnet_ids: Vec<String>,
    /// The security groups the request attached to the endpoint. Stored because they
    /// are part of the record the service accepted, even though no modelled output
    /// member reports them back.
    pub security_group_ids: Vec<String>,
    /// The minted `^vpce-[0-9a-f]{17}$` id of the VPC endpoint behind this record.
    pub vpc_endpoint_id: String,
    pub status: PrivateGraphEndpointStatus,
    /// When the transition the clock owes this endpoint falls due: the move from
    /// `CREATING` to `AVAILABLE`, or the removal of a `DELETING` endpoint. `None` once
    /// the endpoint rests `AVAILABLE`, which is the only status nothing is owed out of.
    pub due: Option<Instant>,
}

impl PrivateGraphEndpoint {
    /// Materialise the transition the clock has already made due.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: the
    /// endpoint would have become `AVAILABLE` at `due` whether or not anybody called,
    /// so applying it on read is bookkeeping rather than a state change caused by the
    /// read. A `DELETING` endpoint is not settled here — it is RECLAIMED, by
    /// [`ComAmazonawsNeptunegraphAmazonneptunegraphState::materialise_private_endpoints`],
    /// since the record itself goes away.
    pub fn settle(&mut self, now: Instant) {
        if self.status == PrivateGraphEndpointStatus::Creating
            && self.due.is_some_and(|due| now >= due)
        {
            self.status = PrivateGraphEndpointStatus::Available;
            self.due = None;
        }
    }

    /// Whether the accepted deletion of this endpoint has fallen due, so the record is
    /// gone and no read may still report it.
    pub fn is_reclaimed(&self, now: Instant) -> bool {
        self.status == PrivateGraphEndpointStatus::Deleting
            && self.due.is_some_and(|due| now >= due)
    }

    /// Accept this endpoint's deletion: it answers `DELETING` until
    /// [`ENDPOINT_DELETION_SPAN`] has passed, after which it is reclaimed.
    pub fn begin_deletion(&mut self, now: Instant) {
        self.status = PrivateGraphEndpointStatus::Deleting;
        self.due = Some(now + ENDPOINT_DELETION_SPAN);
    }
}

/// Per-service state for com-amazonaws-neptunegraph-amazonneptunegraph. A handler reaches it through [`service_state`].
///
/// One store serves every Region, and every record carries the Region it belongs to
/// ([`Graph::region`], [`Snapshot::region`]), so each lookup, listing and
/// name-uniqueness check below takes the Region of the current request beside the
/// identifier: this is a regional control plane, and a resource of one Region is not
/// visible from another.
#[derive(Debug, Default)]
pub struct ComAmazonawsNeptunegraphAmazonneptunegraphState {
    /// Every stored graph, keyed by its minted `g-xxxxxxxxxx` id. A `BTreeMap` so
    /// `ListGraphs` has one stable total order to page over. Ids are minted unique
    /// across the whole store, so the key needs no Region in it — the Region is the
    /// predicate a lookup applies to the record it finds.
    pub graphs: BTreeMap<String, Graph>,
    /// Every stored snapshot, keyed by its minted `gs-xxxxxxxxxx` id. A `BTreeMap`
    /// so `ListGraphSnapshots` has one stable total order to page over.
    pub snapshots: BTreeMap<String, Snapshot>,
    /// Every stored import task, keyed by its minted `t-xxxxxxxxxx` id. A `BTreeMap`
    /// so `ListImportTasks` has one stable total order to page over.
    pub import_tasks: BTreeMap<String, ImportTask>,
    /// Every in-flight query, keyed by the graph it runs on and then by its own id. A
    /// `BTreeMap` of `BTreeMap`s so `ListQueries` — which lists the queries of ONE
    /// graph — has one stable total order to report, and so a query id is resolved
    /// within the graph that admitted it: the query namespace is per graph, and the
    /// same id on another graph names nothing. A query is admitted by the operation
    /// that runs it and leaves this map when it terminates, so the map holds exactly
    /// the queries `GetQuery`, `ListQueries` and `CancelQuery` can still name. The
    /// graph the queries hang off is itself resolved per Region, so a query is reached
    /// only from the Region that holds its graph.
    pub queries: BTreeMap<String, BTreeMap<String, Query>>,
    /// Every stored export task, keyed by its minted `t-xxxxxxxxxx` id. A `BTreeMap`
    /// so `ListExportTasks` has one stable total order to page over.
    pub export_tasks: BTreeMap<String, ExportTask>,
    /// Every stored private graph endpoint, keyed by the graph it reaches and then by
    /// the VPC it lives in. A `BTreeMap` of `BTreeMap`s so `ListPrivateGraphEndpoints`
    /// — which lists the endpoints of ONE graph — has one stable total order to page
    /// over, and so an endpoint is resolved by the `(graphIdentifier, vpcId)` pair it
    /// is addressed by: the same VPC id against another graph names another endpoint.
    pub private_endpoints: BTreeMap<String, BTreeMap<String, PrivateGraphEndpoint>>,
    /// Feeds the graph-id generator, so ids are unique within a store.
    pub id_counter: u64,
    /// Feeds the snapshot-id generator, so ids are unique within a store.
    pub snapshot_id_counter: u64,
    /// Feeds the import-task-id generator, so ids are unique within a store.
    pub import_task_id_counter: u64,
    /// Feeds the export-task-id generator, so ids are unique within a store.
    pub export_task_id_counter: u64,
    /// Feeds the VPC-endpoint-id generator, so ids are unique within a store.
    pub vpc_endpoint_id_counter: u64,
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

    /// Mint an id no task in this store holds, matching the modelled `TaskId`
    /// pattern `^t-[a-z0-9]{10}$`.
    pub fn next_import_task_id(&mut self) -> String {
        loop {
            self.import_task_id_counter = self.import_task_id_counter.wrapping_add(1);
            let id = encode_resource_id(
                TASK_ID_PREFIX,
                scramble(self.import_task_id_counter) % BASE36_POW10,
            );
            if !self.holds_task_id(&id) {
                return id;
            }
        }
    }

    /// Whether this store already holds a task under `id`, in EITHER task map.
    ///
    /// Import and export tasks share ONE modelled id space (`TaskId`'s
    /// `^t-[a-z0-9]{10}$`), so an id minted for one must not be an id the other
    /// already holds: two resources would otherwise answer to the same identifier,
    /// each through its own read.
    pub fn holds_task_id(&self, id: &str) -> bool {
        self.import_tasks.contains_key(id) || self.export_tasks.contains_key(id)
    }

    /// Store a new graph.
    pub fn insert_graph(&mut self, graph: Graph) {
        self.graphs.insert(graph.id.clone(), graph);
    }

    /// Mint an id no task in this store holds, matching the modelled
    /// `ExportTaskId` pattern `^t-[a-z0-9]{10}$`.
    ///
    /// The task-id space is shared with the import tasks, so an id either map already
    /// holds is skipped (see [`Self::holds_task_id`]).
    pub fn next_export_task_id(&mut self) -> String {
        loop {
            self.export_task_id_counter = self.export_task_id_counter.wrapping_add(1);
            let id = encode_resource_id(
                EXPORT_TASK_ID_PREFIX,
                scramble(self.export_task_id_counter) % BASE36_POW10,
            );
            if !self.holds_task_id(&id) {
                return id;
            }
        }
    }

    /// Store a new export task.
    pub fn insert_export_task(&mut self, task: ExportTask) {
        self.export_tasks.insert(task.id.clone(), task);
    }

    /// Apply the terminal transition the clock has made due, then borrow the export
    /// task `id` names IN `region` — `None` when this Region holds no such task.
    pub fn settled_export_task(
        &mut self,
        id: &str,
        region: &str,
        now: Instant,
    ) -> Option<&ExportTask> {
        self.settled_export_task_mut(id, region, now).map(|t| &*t)
    }

    /// [`Self::settled_export_task`], mutably.
    pub fn settled_export_task_mut(
        &mut self,
        id: &str,
        region: &str,
        now: Instant,
    ) -> Option<&mut ExportTask> {
        let task = self.export_tasks.get_mut(id)?;
        if !task.is_in(region) {
            return None;
        }
        task.settle(now);
        Some(task)
    }

    /// A free `snapshotName` for the FINAL snapshot of the graph named `graph_name`
    /// IN `region`, satisfying the modelled `SnapshotName` constraints
    /// (`@length(1..63)`, pattern `^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`).
    ///
    /// The request that asks for a final snapshot carries no name, so the service names
    /// it: after the graph it is taken of, which is how a caller finds it again. A name
    /// that would break the modelled length, or that the account already holds IN THAT
    /// REGION, falls back to the snapshot's own freshly minted id, which no stored
    /// snapshot shares. Names are per Region, so a name another Region holds is free
    /// here.
    pub fn free_final_snapshot_name(
        &self,
        graph_name: &str,
        snapshot_id: &str,
        region: &str,
    ) -> String {
        let after_the_graph = format!("{graph_name}{FINAL_SNAPSHOT_SUFFIX}");
        if after_the_graph.len() <= MAX_RESOURCE_NAME_CHARS
            && self.snapshot_id_named(&after_the_graph, region).is_none()
        {
            return after_the_graph;
        }
        let mut from_the_id = format!("final-snapshot-{snapshot_id}");
        let mut attempt = 1u32;
        while self.snapshot_id_named(&from_the_id, region).is_some() {
            attempt += 1;
            from_the_id = format!("final-snapshot-{snapshot_id}-{attempt}");
        }
        from_the_id
    }

    /// Store a new snapshot.
    pub fn insert_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshots.insert(snapshot.id.clone(), snapshot);
    }

    /// Store a new import task. Tasks are keyed by their minted id alone: the model
    /// gives a task no name, so there is no second index to keep in step.
    pub fn insert_import_task(&mut self, task: ImportTask) {
        self.import_tasks.insert(task.id.clone(), task);
    }

    /// Drop the graph `id` names IN `region`, leaving another Region's graph of the
    /// same id (and any graph of the same NAME in another Region) in place.
    ///
    /// Every query that was running on it goes too, and every private endpoint that
    /// reached it: a query exists only while the graph it runs on does, so a deleted
    /// graph takes its in-flight queries with it rather than leaving entries a later
    /// `GetQuery` on a re-minted id could find. A private graph endpoint is the same:
    /// it is the private route INTO one graph, addressed as an endpoint of that graph,
    /// so it cannot outlive it. An import task is NOT dropped with its graph: it is
    /// read by its own id and keeps the provenance of the load it describes.
    pub fn remove_graph(&mut self, id: &str, region: &str) -> Option<Graph> {
        if !self.graphs.get(id).is_some_and(|graph| graph.is_in(region)) {
            return None;
        }
        let graph = self.graphs.remove(id)?;
        self.queries.remove(id);
        self.private_endpoints.remove(id);
        Some(graph)
    }

    /// The id of the graph named `name` in `region`, or `None` when that Region holds
    /// no graph by that name: the uniqueness check behind `CreateGraph`'s
    /// `ConflictException`.
    ///
    /// Graph names are unique per account PER REGION, so the same name in two Regions
    /// is two graphs. The record itself carries its Region and its name, so this one
    /// scan over a bounded map ([`MAX_GRAPHS`]) is the whole index — a second map
    /// keyed by name could disagree with it.
    pub fn graph_id_named(&self, name: &str, region: &str) -> Option<&str> {
        self.graphs
            .values()
            .find(|graph| graph.is_in(region) && graph.name == name)
            .map(|graph| graph.id.as_str())
    }

    /// Mint a VPC endpoint id no endpoint in this store holds, matching the modelled
    /// `VpcEndpointId` pattern `^vpce-[0-9a-f]{17}$`.
    pub fn next_vpc_endpoint_id(&mut self) -> String {
        loop {
            self.vpc_endpoint_id_counter = self.vpc_endpoint_id_counter.wrapping_add(1);
            let id = encode_vpc_endpoint_id(self.vpc_endpoint_id_counter);
            if !self.holds_vpc_endpoint_id(&id) {
                return id;
            }
        }
    }

    /// Whether this store already holds a private endpoint under `vpc_endpoint_id`, on
    /// any graph: the id names one VPC endpoint across the whole store.
    pub fn holds_vpc_endpoint_id(&self, vpc_endpoint_id: &str) -> bool {
        self.private_endpoints
            .values()
            .flat_map(BTreeMap::values)
            .any(|endpoint| endpoint.vpc_endpoint_id == vpc_endpoint_id)
    }

    /// Store a newly accepted private graph endpoint, under the graph it reaches.
    ///
    /// Replaces nothing: the growth site refuses a second endpoint in the same VPC
    /// before reaching here, because `(graphIdentifier, vpcId)` is the identity the
    /// read and the delete address an endpoint by.
    pub fn insert_private_endpoint(&mut self, endpoint: PrivateGraphEndpoint) {
        self.private_endpoints
            .entry(endpoint.graph_id.clone())
            .or_default()
            .insert(endpoint.vpc_id.clone(), endpoint);
    }

    /// How many private graph endpoints this store holds, across every graph.
    pub fn private_endpoint_count(&self) -> usize {
        self.private_endpoints.values().map(BTreeMap::len).sum()
    }

    /// Materialise every private-endpoint transition the clock has already made due:
    /// each accepted deletion whose span has passed is reclaimed, and each endpoint
    /// whose provisioning has come due settles to `AVAILABLE`.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: each of
    /// these was owed at that instant whether or not anybody called.
    pub fn materialise_private_endpoints(&mut self, now: Instant) {
        for endpoints in self.private_endpoints.values_mut() {
            endpoints.retain(|_, endpoint| !endpoint.is_reclaimed(now));
            for endpoint in endpoints.values_mut() {
                endpoint.settle(now);
            }
        }
        // A graph with no endpoints left holds no entry, so the outer map does not grow
        // one bucket per graph that ever had one.
        self.private_endpoints.retain(|_, endpoints| !endpoints.is_empty());
    }

    /// Materialise what the clock owes, then borrow the one private endpoint the
    /// `(graph_id, vpc_id)` pair names.
    pub fn settled_private_endpoint(
        &mut self,
        graph_id: &str,
        vpc_id: &str,
        now: Instant,
    ) -> Option<&PrivateGraphEndpoint> {
        self.materialise_private_endpoints(now);
        self.private_endpoints.get(graph_id)?.get(vpc_id)
    }

    /// [`Self::settled_private_endpoint`], mutably: what `DeletePrivateGraphEndpoint`
    /// writes the accepted deletion through.
    pub fn settled_private_endpoint_mut(
        &mut self,
        graph_id: &str,
        vpc_id: &str,
        now: Instant,
    ) -> Option<&mut PrivateGraphEndpoint> {
        self.materialise_private_endpoints(now);
        self.private_endpoints.get_mut(graph_id)?.get_mut(vpc_id)
    }

    /// The private endpoints of ONE graph, in VPC-id order, resuming after
    /// `after_vpc_id` when a pagination token named a position: the listing
    /// `ListPrivateGraphEndpoints` pages over.
    ///
    /// The same shape as [`Self::graphs_in`] and [`Self::snapshots_in`], for the same
    /// reason: the endpoints of a graph are held in a `BTreeMap` keyed by the VPC id an
    /// endpoint is addressed by, so one stable total order underlies every page and a
    /// token can name a position in it. A graph holding no endpoint at all yields
    /// nothing rather than being distinguished from one whose endpoints are all gone.
    pub fn private_endpoints_of<'store>(
        &'store self,
        graph_id: &str,
        after_vpc_id: Option<&str>,
    ) -> impl Iterator<Item = &'store PrivateGraphEndpoint> {
        let lower = match after_vpc_id {
            Some(vpc_id) => ::std::ops::Bound::Excluded(vpc_id.to_string()),
            None => ::std::ops::Bound::Unbounded,
        };
        self.private_endpoints
            .get(graph_id)
            .map(|endpoints| endpoints.range((lower, ::std::ops::Bound::Unbounded)))
            .into_iter()
            .flatten()
            .map(|(_, endpoint)| endpoint)
    }

    /// Store a newly accepted query, under the graph it runs on.
    ///
    /// Fallible at the GROWTH SITE, the way the graph and snapshot caps are enforced by
    /// the operations that mint those: the query map grows by one entry per admitted
    /// query in a store shared process-wide, so an admitting operation has to be told
    /// when the store is full ([`MAX_QUERIES`]) or the text is longer than one record
    /// may carry ([`MAX_QUERY_STRING_CHARS`]) rather than growing the emulator's memory
    /// without bound. `Err` carries the message, naming the bound it broke.
    ///
    /// Replacing a query already filed under the same id on the same graph is not
    /// growth, so it is not refused by the count bound.
    pub fn insert_query(&mut self, query: Query) -> Result<(), String> {
        if query.query_string.chars().count() > MAX_QUERY_STRING_CHARS {
            return Err(format!(
                "queryString may carry at most {MAX_QUERY_STRING_CHARS} characters, got {}",
                query.query_string.chars().count()
            ));
        }
        let replaces_an_entry = self
            .queries
            .get(&query.graph_id)
            .is_some_and(|queries| queries.contains_key(&query.id));
        if !replaces_an_entry && self.query_count() >= MAX_QUERIES {
            return Err(format!(
                "this emulator holds at most {MAX_QUERIES} queries in flight"
            ));
        }
        self.queries
            .entry(query.graph_id.clone())
            .or_default()
            .insert(query.id.clone(), query);
        Ok(())
    }

    /// How many queries this store holds in flight, across every graph.
    pub fn query_count(&self) -> usize {
        self.queries.values().map(BTreeMap::len).sum()
    }

    /// Drop every query the clock has made due: each one whose accepted cancellation
    /// has fallen due, and each one that has run out its own [`QUERY_RUN_SPAN`].
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: each of
    /// those queries was over at that instant whether or not anybody called, so
    /// reclaiming it on read is bookkeeping rather than a state change caused by the
    /// read. It is also what keeps the query map from growing with queries nobody
    /// cancels.
    pub fn reap_terminated_queries(&mut self, now: Instant) {
        for queries in self.queries.values_mut() {
            queries.retain(|_, query| !query.has_terminated(now));
        }
        // A graph with no queries left holds no entry, so the outer map does not grow
        // one bucket per graph that ever ran a query.
        self.queries.retain(|_, queries| !queries.is_empty());
    }

    /// Apply every query termination the clock has made due, then borrow the one query
    /// `query_id` names ON the graph `graph_id` names.
    ///
    /// The lookup is scoped to the graph because a query is addressed as a query of one
    /// graph: the same id against another graph names nothing. The graph itself is
    /// resolved per Region by the caller ([`Self::settled_graph`]), which is what keeps
    /// a query of one Region's graph unreachable from another.
    pub fn live_query(&mut self, graph_id: &str, query_id: &str, now: Instant) -> Option<&Query> {
        self.reap_terminated_queries(now);
        self.queries.get(graph_id)?.get(query_id)
    }

    /// [`Self::live_query`], mutably: what `CancelQuery` writes the accepted
    /// cancellation through.
    pub fn live_query_mut(
        &mut self,
        graph_id: &str,
        query_id: &str,
        now: Instant,
    ) -> Option<&mut Query> {
        self.reap_terminated_queries(now);
        self.queries.get_mut(graph_id)?.get_mut(query_id)
    }

    /// The id of the snapshot named `name` in `region`, or `None` when that Region
    /// holds no snapshot by that name. Snapshot names are unique per account per
    /// Region, exactly as graph names are ([`Self::graph_id_named`]).
    pub fn snapshot_id_named(&self, name: &str, region: &str) -> Option<&str> {
        self.snapshots
            .values()
            .find(|snapshot| snapshot.is_in(region) && snapshot.name == name)
            .map(|snapshot| snapshot.id.as_str())
    }

    /// Drop every graph whose accepted deletion the clock has made due, along with
    /// everything [`Self::remove_graph`] reclaims with a graph.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: each of
    /// those graphs was gone at its `deletes_at` instant whether or not anybody called,
    /// so reclaiming it on read is bookkeeping rather than a state change caused by the
    /// read. Each graph is reclaimed in its OWN Region — the Region the record carries
    /// — so this is the one lookup that is not scoped to the caller's Region: the clock
    /// owes it in every Region at once.
    pub fn reap_deleted_graphs(&mut self, now: Instant) {
        let due: Vec<(String, String)> = self
            .graphs
            .iter()
            .filter(|(_, graph)| graph.deletion_complete(now))
            .map(|(id, graph)| (id.clone(), graph.region.clone()))
            .collect();
        for (id, region) in due {
            self.remove_graph(&id, &region);
        }
    }

    /// Apply every provisioning transition the clock has made due, then borrow the
    /// graph `id` names IN `region` — `None` when this Region holds no such graph,
    /// whatever another Region holds.
    pub fn settled_graph(&mut self, id: &str, region: &str, now: Instant) -> Option<&Graph> {
        self.settled_graph_mut(id, region, now).map(|graph| &*graph)
    }

    /// Apply every provisioning transition the clock has made due, then borrow the
    /// graph `id` names in `region` mutably.
    pub fn settled_graph_mut(
        &mut self,
        id: &str,
        region: &str,
        now: Instant,
    ) -> Option<&mut Graph> {
        self.reap_deleted_graphs(now);
        let graph = self.graphs.get_mut(id)?;
        if !graph.is_in(region) {
            return None;
        }
        graph.settle(now);
        Some(graph)
    }

    /// Apply every provisioning transition the clock has made due across the store.
    pub fn settle_all(&mut self, now: Instant) {
        self.reap_deleted_graphs(now);
        for graph in self.graphs.values_mut() {
            graph.settle(now);
        }
        for snapshot in self.snapshots.values_mut() {
            snapshot.settle(now);
        }
        for task in self.import_tasks.values_mut() {
            task.settle(now);
        }
        for task in self.export_tasks.values_mut() {
            task.settle(now);
        }
        self.materialise_private_endpoints(now);
    }

    /// The graphs `region` holds, in graph-id order, resuming after `after_id` when a
    /// pagination token named a position: the one listing `ListGraphs` pages over.
    pub fn graphs_in<'store>(
        &'store self,
        region: &'store str,
        after_id: Option<&str>,
    ) -> impl Iterator<Item = &'store Graph> {
        let lower = match after_id {
            Some(id) => ::std::ops::Bound::Excluded(id.to_string()),
            None => ::std::ops::Bound::Unbounded,
        };
        self.graphs
            .range((lower, ::std::ops::Bound::Unbounded))
            .map(|(_, graph)| graph)
            .filter(move |graph| graph.is_in(region))
    }

    /// The snapshots `region` holds, in snapshot-id order, resuming after `after_id`
    /// when a pagination token named a position, and restricted to the snapshots taken
    /// from one graph when `of_graph` names one: the listing `ListGraphSnapshots` pages
    /// over.
    ///
    /// The same shape as [`Self::graphs_in`], for the same reason: `snapshots` is a
    /// `BTreeMap`, so one stable total order underlies every page and a token can name
    /// a position in it. `of_graph` is matched against the snapshot's stored
    /// `source_graph_id`, so a snapshot stays listed under the graph it was taken from
    /// even once that graph is gone.
    pub fn snapshots_in<'store>(
        &'store self,
        region: &'store str,
        of_graph: Option<&'store str>,
        after_id: Option<&str>,
    ) -> impl Iterator<Item = &'store Snapshot> {
        let lower = match after_id {
            Some(id) => ::std::ops::Bound::Excluded(id.to_string()),
            None => ::std::ops::Bound::Unbounded,
        };
        self.snapshots
            .range((lower, ::std::ops::Bound::Unbounded))
            .map(|(_, snapshot)| snapshot)
            .filter(move |snapshot| {
                snapshot.is_in(region)
                    && of_graph.is_none_or(|graph_id| snapshot.source_graph_id == graph_id)
            })
    }

    /// Apply the snapshot transition the clock has made due, then borrow the snapshot
    /// `id` names IN `region` — `None` when this Region holds no such snapshot.
    pub fn settled_snapshot(&mut self, id: &str, region: &str, now: Instant) -> Option<&Snapshot> {
        let snapshot = self.snapshots.get_mut(id)?;
        if !snapshot.is_in(region) {
            return None;
        }
        snapshot.settle(now);
        Some(snapshot)
    }

    /// Apply the import-task transition the clock has made due, then borrow the task
    /// `id` names IN `region` — `None` when this Region holds no such task.
    pub fn settled_import_task(
        &mut self,
        id: &str,
        region: &str,
        now: Instant,
    ) -> Option<&ImportTask> {
        self.settled_import_task_mut(id, region, now).map(|t| &*t)
    }

    /// [`Self::settled_import_task`], mutably.
    pub fn settled_import_task_mut(
        &mut self,
        id: &str,
        region: &str,
        now: Instant,
    ) -> Option<&mut ImportTask> {
        let task = self.import_tasks.get_mut(id)?;
        if !task.is_in(region) {
            return None;
        }
        task.settle(now);
        Some(task)
    }

    /// The tag map of the resource `arn` names IN `region`, or `None` when this
    /// Region holds no resource by that ARN.
    ///
    /// The lookup is on the ARN the service MINTED for the resource, so a request
    /// naming the right resource id in the wrong region, account or partition names
    /// no resource here — an ARN identifies one resource in one region of one
    /// account. `region` is the Region the REQUEST was signed for, which the regional
    /// endpoint makes part of the lookup as well: a resource of another Region is not
    /// reachable through this one even by its own full ARN.
    ///
    /// Tags live on the resource record itself, which is what makes a tag supplied at
    /// create time and one written later by `TagResource` the same bytes, read back by
    /// this one lookup. Graphs, snapshots and export tasks are the taggable resources
    /// this slice stores; an import task holds no tags of its own (the `tags` of
    /// `CreateGraphUsingImportTask` are documented as metadata "to the new graph", so
    /// they live on the graph that call creates). A private graph endpoint never joins
    /// this lookup either: the model gives it neither an ARN nor a `tags` member, so the
    /// tagging surface has nothing to address it by.
    pub fn tags_of(&self, arn: &str, region: &str) -> Option<&BTreeMap<String, String>> {
        self.graphs
            .values()
            .find(|graph| graph.is_in(region) && graph.arn == arn)
            .map(|graph| &graph.tags)
            .or_else(|| {
                self.snapshots
                    .values()
                    .find(|snapshot| snapshot.is_in(region) && snapshot.arn == arn)
                    .map(|snapshot| &snapshot.tags)
            })
            .or_else(|| {
                self.export_tasks
                    .values()
                    .find(|task| task.is_in(region) && task.arn == arn)
                    .map(|task| &task.tags)
            })
    }

    /// [`Self::tags_of`], mutably: the one place `TagResource`/`UntagResource` write.
    pub fn tags_of_mut(
        &mut self,
        arn: &str,
        region: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        // Two disjoint maps and one `&mut self`, so which map holds the ARN is
        // decided by an immutable scan before either is borrowed mutably.
        if self
            .graphs
            .values()
            .any(|graph| graph.is_in(region) && graph.arn == arn)
        {
            return self
                .graphs
                .values_mut()
                .find(|graph| graph.is_in(region) && graph.arn == arn)
                .map(|graph| &mut graph.tags);
        }
        if self
            .snapshots
            .values()
            .any(|snapshot| snapshot.is_in(region) && snapshot.arn == arn)
        {
            return self
                .snapshots
                .values_mut()
                .find(|snapshot| snapshot.is_in(region) && snapshot.arn == arn)
                .map(|snapshot| &mut snapshot.tags);
        }
        self.export_tasks
            .values_mut()
            .find(|task| task.is_in(region) && task.arn == arn)
            .map(|task| &mut task.tags)
    }
}

// ---------------------------------------------------------------------------
// Store bounds
//
// A graph is created by a caller and reclaimed only by an explicit `DeleteGraph`, so
// the growth site carries a cap and nothing caller-sized reaches the store unchecked.
// The product is finite: at most `MAX_GRAPHS` graphs, each holding a `graphName` of at
// most 63 characters, a `kmsKeyIdentifier` of at most 1024, and at most `MAX_TAGS` tags
// of at most `MAX_TAG_KEY_CHARS` / `MAX_TAG_VALUE_CHARS` characters. A query is the one
// record no caller has to reclaim: it is capped at `MAX_QUERIES` entries of at most
// `MAX_QUERY_STRING_CHARS` characters AND every entry carries a due end instant, so the
// query map shrinks on its own as well.
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

/// The most characters a `graphName` or a `snapshotName` may carry, from the modelled
/// `@length(1..63)` on both
/// (`smithy_query(query="operation",operation="CreateGraphSnapshot")`).
pub const MAX_RESOURCE_NAME_CHARS: usize = 63;

/// What the name of a graph's FINAL snapshot — the one a `skipSnapshot=false` delete
/// asks for — adds to the graph's own name.
pub const FINAL_SNAPSHOT_SUFFIX: &str = "-final-snapshot";

/// The most import tasks one store holds.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the same bound
/// [`MAX_GRAPHS`] puts on the graph map: the store is shared process-wide and grows
/// by one entry per `StartImportTask` or `CreateGraphUsingImportTask`, and a task is
/// never reclaimed, so without a ceiling repeated calls grow the emulator's memory
/// without bound. `StartImportTask`
/// models no quota error, so reaching the ceiling is reported with the
/// `ConflictException` (409) it does model — the request conflicts with what the store
/// already holds; `CreateGraphUsingImportTask` reports it with the
/// `ServiceQuotaExceededException` (402) its own model declares. Set well above any
/// realistic use of a local emulator.
pub const MAX_IMPORT_TASKS: usize = 100;

/// The most characters an import task's `source` may carry.
///
/// The model puts no `@length` on `source`
/// (`smithy_query(query="operation",operation="StartImportTask")`), so this is the
/// store's own bound on a caller-sized string, set to the 1024 characters the
/// service's other S3-location members are modelled with (`NeptuneImportOptions`'s
/// `s3ExportPath`, `s3ExportKmsKeyId`: `@length(1..1024)`).
pub const MAX_IMPORT_SOURCE_CHARS: usize = 1024;

/// The modelled `Format` value for RDF N-Triples data
/// (`smithy_query(query="shape",shape="Format")`), the one format whose load needs
/// `blankNodeHandling`.
pub const NTRIPLES_FORMAT: &str = "NTRIPLES";

/// The `ValidationException` clause a request that omits `blankNodeHandling` beside an
/// `NTRIPLES` format fails, or `None` when the pair holds.
///
/// Documented member dependency: `blankNodeHandling` "must be provided when format is
/// ntriples" (`smithy_query(query="doc",shape="StartImportTaskInput",
/// member="blankNodeHandling")`), because an N-Triples dataset is the one whose load has
/// to be told what to do with its blank nodes. No modelled trait can express a
/// dependency between two members, so it is refused by hand — stated ONCE here and
/// reached by every operation that accepts an import `format` (`StartImportTask`,
/// `CreateGraphUsingImportTask`), so the two cannot drift into different acceptance
/// sets or different wording for one documented rule.
pub fn blank_node_handling_clause(
    format: Option<&str>,
    blank_node_handling: Option<&str>,
) -> Option<String> {
    if format == Some(NTRIPLES_FORMAT) && blank_node_handling.is_none() {
        return Some(constraint_clause(
            "blankNodeHandling",
            None,
            &format!("Member must be provided when format is {NTRIPLES_FORMAT}"),
        ));
    }
    None
}

/// The requirement an import task's `source` fails, or `None` when it satisfies both
/// bounds the store puts on that member.
///
/// Stated ONCE, and reached by every operation that accepts an import `source`
/// (`StartImportTask`, `CreateGraphUsingImportTask`), so no two of them can drift into
/// different acceptance sets for one documented member:
///
/// * A required LOCATION cannot be blank. The member is modelled as a bare `String` —
///   no `@length`, no `@pattern` — so neither emptiness nor a value of nothing but
///   whitespace is refused at the boundary, and neither locates any data.
/// * The upper bound ([`MAX_IMPORT_SOURCE_CHARS`]) is measured on the value that would
///   be STORED, so no stored record ever holds more than the bound a refusal
///   advertises.
///
/// The SCHEME is deliberately not constrained: `source` is documented as "an Amazon S3
/// path, or ... a Neptune database endpoint or snapshot", so no one scheme is required.
/// The returned requirement never echoes the value — the one that broke the upper bound
/// is exactly the value that must not be sent back.
pub fn import_source_requirement(source: &str) -> Option<String> {
    if source.trim().is_empty() {
        return Some("Member must have length greater than or equal to 1".to_string());
    }
    let stored_chars = source.chars().count();
    if stored_chars > MAX_IMPORT_SOURCE_CHARS {
        return Some(format!(
            "Member must have length less than or equal to {MAX_IMPORT_SOURCE_CHARS}, \
             got {stored_chars}"
        ));
    }
    None
}

/// The prefix the service reserves for import- and export-task ids, from the modelled
/// `TaskId` pattern `^t-[a-z0-9]{10}$`.
pub const TASK_ID_PREFIX: &str = "t-";

/// The `statusReason` of an import task that stopped `FAILED` because nothing read
/// its source.
///
/// This emulator has no bulk loader: no component reads a task's `source`, so an
/// accepted import can never load data. Rather than report a stage, a count or a
/// `SUCCEEDED` status for a load nothing performed, the clock settles every accepted
/// task into the modelled `FAILED` status carrying this reason, which says exactly
/// that.
pub const IMPORT_NOT_RUN_REASON: &str = "This emulator runs no bulk data load: no import engine read the task's source, so the task stopped without importing data.";

/// The `statusReason` of an import task that stopped because it was cancelled.
pub const IMPORT_CANCELLED_REASON: &str =
    "The import task was cancelled by a CancelImportTask request.";

/// The most export tasks one store holds.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the same bound
/// [`MAX_GRAPHS`] puts on the graph map: the store is shared process-wide and grows by
/// one entry per `StartExportTask`, and an export task is never reclaimed (a finished
/// task stays readable for the life of the process), so without a ceiling repeated
/// calls grow the emulator's memory without bound. `StartExportTask` models no quota
/// error (`smithy_query(query="operation",operation="StartExportTask")` declares
/// `ConflictException`, `ValidationException`, `ThrottlingException`,
/// `ResourceNotFoundException` and `InternalServerException` only), so reaching it is
/// reported as the `ValidationException` that names this bound — not as throttling,
/// which would promise that a retry can succeed. Set well above any realistic use of a
/// local emulator.
pub const MAX_EXPORT_TASKS: usize = 100;

/// The most queries one store holds in flight, across every graph.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the same bound
/// [`MAX_GRAPHS`] puts on the graph map: the store is shared process-wide and grows by
/// one entry per admitted query, so without a ceiling repeated query requests grow the
/// emulator's memory without bound. Reaching it is reported by the operation that
/// admits a query, from the message [`ComAmazonawsNeptunegraphAmazonneptunegraphState::insert_query`]
/// hands back. Set well above any realistic use of a local emulator, and queries also
/// leave the store on their own as the clock makes their end due.
pub const MAX_QUERIES: usize = 100;

/// The most nodes one graph's data may hold.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the same kind of bound
/// [`MAX_GRAPHS`] puts on the graph map: a graph's data grows by what the data plane
/// writes into it and is reclaimed only when the graph is emptied or deleted, so
/// without a ceiling repeated writes grow the emulator's memory without bound.
/// [`GraphData::insert_node`] hands the reason back for the operation that was
/// writing to report.
///
/// It also bounds the work ONE reading of the data costs: a report over the data
/// walks the nodes, the edges and the properties on them once each, so the cost of a
/// request is bounded by this ceiling, [`MAX_EDGES_PER_GRAPH`] and
/// [`MAX_PROPERTIES_PER_ELEMENT`] TOGETHER rather than by any of them alone. Set
/// above any realistic use of a local emulator and low enough that the whole product
/// stays small.
pub const MAX_NODES_PER_GRAPH: usize = 10_000;

/// The most edges one graph's data may hold.
///
/// The same guard [`MAX_NODES_PER_GRAPH`] puts on the nodes, set higher because a
/// graph holds more relationships than things: a node can be an endpoint of many
/// edges. [`GraphData::insert_edge`] hands the reason back for the operation that
/// was writing to report.
pub const MAX_EDGES_PER_GRAPH: usize = 20_000;

/// The most properties one node or one edge may carry.
///
/// Emulator resource guard, NOT a quota AWS publishes: without it one element's
/// property map is unbounded, so the number of properties a graph holds — and the
/// work a report over them costs — would not be bounded by the two ceilings above.
/// [`GraphData::insert_node`] and [`GraphData::insert_edge`] hand the reason back for
/// the operation that was writing to report.
pub const MAX_PROPERTIES_PER_ELEMENT: usize = 100;

/// The most characters one stored query's text may carry.
///
/// Emulator resource guard, NOT a quota AWS publishes: `queryString` is modelled as an
/// unconstrained `String` (`smithy_query(query="operation",operation="ExecuteQuery")`),
/// so without a ceiling one request could store an arbitrarily large text that
/// `GetQuery` then keeps reporting. Set well above any query a local emulator is asked
/// to run.
pub const MAX_QUERY_STRING_CHARS: usize = 65_536;

/// AWS's tagging limit of 50 tags per resource, which this service models as
/// `TagMap`'s `@length(0..50)`.
pub const MAX_TAGS: usize = 50;

/// The most private graph endpoints one store holds, across every graph.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the same bound
/// [`MAX_GRAPHS`] puts on the graph map: the store is shared process-wide and grows by
/// one entry per `CreatePrivateGraphEndpoint`, so without a ceiling repeated calls grow
/// the emulator's memory without bound. `CreatePrivateGraphEndpoint` reports reaching
/// it with the `ServiceQuotaExceededException` (402) the model declares for exactly
/// this condition. Set well above any realistic use of a local emulator.
pub const MAX_PRIVATE_GRAPH_ENDPOINTS: usize = 100;

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

/// Whether `value` matches the modelled `VpcId` pattern `^vpc-[a-z0-9]+$`
/// (`smithy_query(query="shape",shape="VpcId")`).
///
/// Unlike this service's own resource ids, a VPC id is minted by another service, so
/// the pattern bounds only its prefix and alphabet and not its length: anything from
/// one character after the prefix upwards is a position in that id space.
pub fn is_vpc_id(value: &str) -> bool {
    value.strip_prefix("vpc-").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// Whether `value` matches the modelled `TaskId` pattern `^t-[a-z0-9]{10}$`.
///
/// One predicate for the whole task-id space: the model spells the same pattern for
/// an import task and an export task, and this service mints both from it.
pub fn is_task_id(value: &str) -> bool {
    is_resource_id(value, TASK_ID_PREFIX)
}

/// Whether `value` matches the modelled `ExportTaskId` pattern `^t-[a-z0-9]{10}$` —
/// the same check as [`is_task_id`], under the name the export surface reads it by.
pub fn is_export_task_id(value: &str) -> bool {
    is_task_id(value)
}

/// The prefix the modelled `ExportTaskId` pattern `^t-[a-z0-9]{10}$` requires: the
/// same task-id space [`TASK_ID_PREFIX`] names, since the model gives an import task
/// and an export task one id pattern.
pub const EXPORT_TASK_ID_PREFIX: &str = TASK_ID_PREFIX;

/// The scheme an Amazon S3 URI is spelled with, which an export `destination` has to
/// carry: the member is documented as "The Amazon S3 URI where data will be exported
/// to" (`smithy_query(query="doc",shape="StartExportTaskInput",member="destination")`)
/// and is modelled as a plain `String` of 1..1024 characters, so the scheme is the one
/// part of it no boundary constraint checks.
pub const S3_URI_SCHEME: &str = "s3://";

/// Whether `value` is an Amazon S3 URI: the `s3://` scheme followed by a bucket name,
/// optionally followed by `/` and a key prefix.
///
/// Only the two facts that make a URI addressable are required — the scheme and a
/// non-empty bucket segment — because S3's own bucket-name rules are enforced by S3,
/// not by the service that is handed the URI.
pub fn is_s3_uri(value: &str) -> bool {
    value
        .strip_prefix(S3_URI_SCHEME)
        .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
}

/// The requirement clause a `destination` that is not an Amazon S3 URI fails, in the
/// wording [`constraint_clause`] frames.
pub fn s3_uri_requirement() -> String {
    format!("Member must be an Amazon S3 URI of the form {S3_URI_SCHEME}bucket[/key-prefix]")
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

/// The modelled `ResourceNotFoundException` for a `taskIdentifier` that names no
/// stored import task.
///
/// The task namespace is per-account like the graph and snapshot namespaces, so the
/// failure names the account the lookup was resolved in beside the identifier the
/// request asked for — the same two facts [`graph_not_found`] reports, and for the
/// same reason.
pub fn import_task_not_found(task_identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "No import task found for customer [{}] with task id [{task_identifier}]",
            caller_account_id()
        ),
    }
}

/// The modelled `ResourceNotFoundException` for a `queryId` that names no query in
/// flight on the graph the request named.
///
/// A query is addressed as a query of one graph, so the failure names the graph the
/// lookup was resolved against beside the query id the request asked for — which is
/// what lets a caller tell a query that has already finished apart from one running on
/// another graph. The modelled shape carries only `message`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), so the message is
/// the only place either fact can go.
pub fn query_not_found(graph_identifier: &str, query_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("No query found on graph [{graph_identifier}] with query id [{query_id}]"),
    }
}

/// The modelled `ResourceNotFoundException` for a `taskIdentifier` that names no
/// stored export task.
///
/// The export-task namespace is per-account like the graph and snapshot namespaces, so
/// the failure names the account the lookup was resolved in beside the identifier the
/// request asked for — the same two facts [`graph_not_found`] reports, and for the same
/// reason.
pub fn export_task_not_found(task_identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "No export task found for customer [{}] with task id [{task_identifier}]",
            caller_account_id()
        ),
    }
}

/// The modelled `ResourceNotFoundException` for a `(graphIdentifier, vpcId)` pair that
/// names no stored private graph endpoint.
///
/// A private graph endpoint is addressed as the endpoint of one graph in one VPC, so
/// the failure names both halves of that pair, in the order the request carries them —
/// which is what lets a caller tell an endpoint that has already been deleted apart
/// from one created in another VPC. The modelled shape carries only `message`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), so the message is
/// the only place either fact can go, and it is rendered the way the service renders
/// it: the graph the endpoint would reach, then the VPC it would be reached from.
pub fn private_endpoint_not_found(graph_identifier: &str, vpc_id: &str) -> ResourceNotFoundException
{
    ResourceNotFoundException {
        message: format!(
            "Private Graph Endpoint to Graph: {graph_identifier} from VPC: {vpc_id} does not exist"
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

/// One clause of a `ValidationException` message for a LENGTH violation: the member
/// path and the requirement it failed, WITHOUT the value.
///
/// A value that broke a length bound is exactly the value that must not be echoed — a
/// refusal would otherwise carry the whole oversized string back — so this clause
/// states the member and the bound and nothing else.
pub fn length_clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
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

/// Spell the VPC endpoint id of the `value`th endpoint this store minted, matching the
/// modelled `VpcEndpointId` pattern `^vpce-[0-9a-f]{17}$`: seventeen lowercase hex
/// digits, scrambled off the counter the same way [`encode_resource_id`]'s input is, so
/// ids are not guessable neighbours while still being a pure function of the counter.
///
/// Seventeen hex digits are 68 bits, one more than a `u64` carries, so the leading digit
/// comes from a second, independently scrambled word.
fn encode_vpc_endpoint_id(counter: u64) -> String {
    let low = scramble(counter);
    let high = scramble(counter ^ 0x5DEE_CE66_D1B5_A1C3);
    format!("vpce-{:x}{low:016x}", high & 0xf)
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

/// The `*Output` shapes that carry a whole graph record are structurally
/// identical, so one macro renders the stored graph into each rather than one
/// hand-copied field list per operation, which could drift apart.
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
                    kms_key_identifier: Some(graph.kms_key_identifier.clone()),
                    source_snapshot_id: graph.source_snapshot_id.clone(),
                    deletion_protection: Some(graph.deletion_protection),
                    build_number: Some(graph.build_number.clone()),
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
    StopGraphOutput,
    StartGraphOutput,
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
            kms_key_identifier: Some(graph.kms_key_identifier.clone()),
            deletion_protection: Some(graph.deletion_protection),
        }
    }
}

/// The shapes that carry a whole snapshot record are structurally identical, so one
/// macro renders the stored snapshot into each rather than one hand-copied field list
/// per operation, which could drift apart. `GraphSnapshotSummary` — the member
/// `ListGraphSnapshots` reports — carries exactly those members too, so it is rendered
/// here rather than by a listing projection of its own.
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
                    kms_key_identifier: Some(snapshot.kms_key_identifier.clone()),
                }
            }
        }
    )+};
}

snapshot_record_output!(
    CreateGraphSnapshotOutput,
    GetGraphSnapshotOutput,
    GraphSnapshotSummary,
);

/// The `*Output` shapes that carry a whole import-task record differ only in the
/// members they add, so the members they SHARE are rendered by one macro rather than
/// one hand-copied field list per operation, which could drift apart.
///
/// Each entry names the binding the stored record is reached through, because macro
/// hygiene puts a binding the macro itself introduces out of reach of the extra
/// members an entry spells.
macro_rules! import_task_record_output {
    ($($ty:ident ($task:ident) { $($extra:tt)* }),+ $(,)?) => {$(
        impl From<&ImportTask> for crate::ops::types::$ty {
            fn from($task: &ImportTask) -> Self {
                Self {
                    task_id: $task.id.clone(),
                    graph_id: Some($task.graph_id.clone()),
                    source: $task.source.clone(),
                    format: $task.format.clone(),
                    parquet_type: $task.parquet_type.clone(),
                    role_arn: $task.role_arn.clone(),
                    status: $task.status.as_wire().to_string(),
                    $($extra)*
                }
            }
        }
    )+};
}

import_task_record_output!(
    // `CancelImportTaskOutput` carries only the shared members.
    CancelImportTaskOutput(task) {},
    StartImportTaskOutput(task) {
        import_options: task.import_options.clone(),
    },
    // `CreateGraphUsingImportTask` reports the task it stored, with the graph it
    // created named by the shared `graphId` member.
    CreateGraphUsingImportTaskOutput(task) {
        import_options: task.import_options.clone(),
    },
    // `GetImportTaskOutput` is the only projection that reports the task's progress.
    // `importTaskDetails` stays ABSENT: it carries the statement, dictionary-entry
    // and error counts of a load, and nothing in this emulator ran one, so there are
    // no counts to report (see `IMPORT_NOT_RUN_REASON`).
    GetImportTaskOutput(task) {
        import_options: task.import_options.clone(),
        import_task_details: None,
        attempt_number: Some(task.attempt_number),
        status_reason: task.status_reason.clone(),
    },
);

/// The `*Output` shapes that carry a whole export-task record differ only in the
/// members they add, so the members they SHARE are rendered by one macro rather than
/// one hand-copied field list per operation, which could drift apart. The listing
/// projection (`ExportTaskSummary`) joins this list with the operation that reports it;
/// `GetExportTaskOutput` is built by [`ExportTask::get_output`] instead, because it also
/// carries the `exportTaskDetails` that are derived from the clock.
///
/// Each entry names the binding the stored record is reached through, because macro
/// hygiene puts a binding the macro itself introduces out of reach of the extra
/// members an entry spells.
macro_rules! export_task_record_output {
    ($($ty:ident ($task:ident) { $($extra:tt)* }),+ $(,)?) => {$(
        impl From<&ExportTask> for crate::ops::types::$ty {
            fn from($task: &ExportTask) -> Self {
                Self {
                    graph_id: $task.source_graph_id.clone(),
                    role_arn: $task.role_arn.clone(),
                    task_id: $task.id.clone(),
                    status: $task.status.as_wire().to_string(),
                    format: $task.format.clone(),
                    destination: $task.destination.clone(),
                    kms_key_identifier: $task.kms_key_identifier.clone(),
                    parquet_type: $task.parquet_type.clone(),
                    status_reason: $task.status_reason.clone(),
                    $($extra)*
                }
            }
        }
    )+};
}

export_task_record_output!(
    StartExportTaskOutput(task) {
        export_filter: task.export_filter.clone(),
    },
    // `CancelExportTaskOutput` carries only the shared members: the model gives it no
    // `exportFilter`.
    CancelExportTaskOutput(task) {},
);

/// The shapes that carry a whole private-graph-endpoint record are structurally
/// identical, so one macro renders the stored endpoint into each rather than one
/// hand-copied field list per operation, which could drift apart. The three `*Output`
/// shapes of the single-endpoint operations are joined here by
/// `PrivateGraphEndpointSummary`, the entry `ListPrivateGraphEndpoints` reports, so a
/// listed endpoint cannot differ from what a read of it answers.
macro_rules! private_graph_endpoint_output {
    ($($ty:ident),+ $(,)?) => {$(
        impl From<&PrivateGraphEndpoint> for crate::ops::types::$ty {
            fn from(endpoint: &PrivateGraphEndpoint) -> Self {
                Self {
                    vpc_id: endpoint.vpc_id.clone(),
                    subnet_ids: endpoint.subnet_ids.clone(),
                    status: endpoint.status.as_wire().to_string(),
                    vpc_endpoint_id: Some(endpoint.vpc_endpoint_id.clone()),
                }
            }
        }
    )+};
}

private_graph_endpoint_output!(
    CreatePrivateGraphEndpointOutput,
    GetPrivateGraphEndpointOutput,
    DeletePrivateGraphEndpointOutput,
    PrivateGraphEndpointSummary,
);

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

    /// Every minted VPC endpoint id matches the modelled `VpcEndpointId` pattern
    /// `^vpce-[0-9a-f]{17}$` and no id repeats.
    #[test]
    fn minted_vpc_endpoint_ids_match_the_modelled_pattern_and_are_unique() {
        let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..2_000 {
            let id = state.next_vpc_endpoint_id();
            let suffix = id
                .strip_prefix("vpce-")
                .unwrap_or_else(|| panic!("id {id:?}: expected the vpce- prefix"));
            assert_eq!(
                suffix.len(),
                17,
                "id {id:?}: expected 17 characters after the prefix, got {}",
                suffix.len()
            );
            assert!(
                suffix
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f')),
                "id {id:?}: expected a match for ^vpce-[0-9a-f]{{17}}$"
            );
            assert!(seen.insert(id.clone()), "id {id:?}: minted twice");
        }
    }

    /// The query store is BOUNDED at its growth site: it refuses a query beyond
    /// [`MAX_QUERIES`] and a text longer than [`MAX_QUERY_STRING_CHARS`], and it
    /// reclaims a query the clock has made due even when nobody cancels it.
    #[test]
    fn the_query_store_is_bounded_at_its_growth_site() {
        let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        let now = Instant::now();

        // Filling the store to the cap is accepted, spread over two graphs so the
        // bound is on the store rather than on one graph's bucket.
        for index in 0..MAX_QUERIES {
            let graph = if index % 2 == 0 { "g-0000000000" } else { "g-0000000001" };
            let result = state.insert_query(Query::running(
                format!("q-{index}"),
                graph,
                "MATCH (n) RETURN n",
                now,
            ));
            assert!(
                result.is_ok(),
                "case query {index}: expected the store to accept it, got {result:?}"
            );
        }
        assert_eq!(
            state.query_count(),
            MAX_QUERIES,
            "case filled to the cap: expected {MAX_QUERIES} queries, got {}",
            state.query_count()
        );

        // One more is refused, and replacing an entry already filed is not growth.
        let over = state.insert_query(Query::running("q-one-too-many", "g-0000000000", "RETURN 1", now));
        assert!(
            over.is_err(),
            "case one past the cap: expected a refusal, got {over:?}"
        );
        let replacement = state.insert_query(Query::running("q-0", "g-0000000000", "RETURN 2", now));
        assert!(
            replacement.is_ok(),
            "case replacing an entry at the cap: expected acceptance, got {replacement:?}"
        );
        assert_eq!(
            state.query_count(),
            MAX_QUERIES,
            "case replacing an entry at the cap: the count must not grow, got {}",
            state.query_count()
        );

        // A text at the bound is accepted and one past it is refused.
        let mut empty = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        for (case, chars, accepted) in [
            ("one character", 1, true),
            ("at the bound", MAX_QUERY_STRING_CHARS, true),
            ("one past the bound", MAX_QUERY_STRING_CHARS + 1, false),
        ] {
            let text = "x".repeat(chars);
            let result = empty.insert_query(Query::running("q-sized", "g-0000000000", text, now));
            assert_eq!(
                result.is_ok(),
                accepted,
                "case {case:?}: expected accepted={accepted}, got {result:?}"
            );
        }

        // And the clock reclaims what nobody cancels: every query admitted at `now` is
        // over a run span later, with no handler having to sleep for it.
        state.reap_terminated_queries(now + QUERY_RUN_SPAN);
        assert_eq!(
            state.query_count(),
            0,
            "case run span out: expected an empty store, got {}",
            state.query_count()
        );
        assert!(
            state.queries.is_empty(),
            "case run span out: the per-graph buckets must go too, got {:?}",
            state.queries.keys().collect::<Vec<_>>()
        );
    }

    /// A cancel can only SHORTEN a query's life: it brings the end forward when it is
    /// accepted early, and a late cancel cannot carry the query past the run span it
    /// was already going to end at.
    #[test]
    fn a_cancel_only_shortens_a_querys_life() {
        let now = Instant::now();
        let tick = Duration::from_millis(1);

        let mut early = Query::running("q-early", "g-0000000000", "RETURN 1", now);
        early.begin_cancellation(now);
        assert!(
            !early.has_terminated(now + CANCELLATION_SPAN - tick),
            "case cancelled at once: the span is never zero, so the query is still stopping"
        );
        assert!(
            early.has_terminated(now + CANCELLATION_SPAN),
            "case cancelled at once: expected the cancellation to end it a span later"
        );

        let mut late = Query::running("q-late", "g-0000000000", "RETURN 1", now);
        late.begin_cancellation(now + QUERY_RUN_SPAN - tick);
        assert!(
            late.has_terminated(now + QUERY_RUN_SPAN),
            "case cancelled late: a cancel must not carry the query past its run span"
        );
    }

    /// Every minted export-task id matches the modelled `ExportTaskId` pattern
    /// `^t-[a-z0-9]{10}$` and no id repeats — the same contract the graph-id generator
    /// has, over the generator this slice added.
    #[test]
    fn minted_export_task_ids_match_the_modelled_pattern_and_are_unique() {
        let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..2_000 {
            let id = state.next_export_task_id();
            assert!(
                is_export_task_id(&id),
                "id {id:?}: expected a match for ^t-[a-z0-9]{{10}}$"
            );
            assert!(seen.insert(id.clone()), "id {id:?}: minted twice");
        }
    }

    /// An export task's terminal status is owed by the CLOCK, not by the read: it
    /// reports `INITIALIZING` until the span has passed, then `FAILED` carrying
    /// [`EXPORT_NOT_PERFORMED_REASON`], and settling again changes nothing.
    ///
    /// The rows span the instants a read can fall on, including the span's own bound
    /// and one tick short of it, so the transient status cannot be skipped.
    #[test]
    fn an_export_task_settles_into_the_modelled_failure_state() {
        let now = Instant::now();
        let tick = Duration::from_millis(1);
        let task_at = |started: Instant| ExportTask {
            id: "t-0123456789".to_string(),
            arn: "arn:aws:neptune-graph:us-east-1:000000000000:export-task/t-0123456789"
                .to_string(),
            region: "us-east-1".to_string(),
            source_graph_id: "g-0000000000".to_string(),
            role_arn: "arn:aws:iam::000000000000:role/NeptuneExportRole".to_string(),
            format: "CSV".to_string(),
            parquet_type: None,
            destination: "s3://export-bucket/nightly".to_string(),
            kms_key_identifier:
                "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012"
                    .to_string(),
            export_filter: None,
            status: ExportTaskStatus::Initializing,
            status_reason: None,
            tags: BTreeMap::new(),
            start_time: SystemTime::now(),
            started_at: started,
            settles_at: Some(started + EXPORT_TASK_SPAN),
        };

        // (case, when the task is read, whether the terminal status is due by then)
        let cases: &[(&str, Duration, bool)] = &[
            ("read at once", Duration::ZERO, false),
            ("read one tick in", tick, false),
            ("read one tick before the span is out", EXPORT_TASK_SPAN - tick, false),
            ("read when the span is out", EXPORT_TASK_SPAN, true),
            ("read a span later", EXPORT_TASK_SPAN * 2, true),
        ];
        for (case, after, terminal) in cases {
            let mut task = task_at(now);
            task.settle(now + *after);
            let expected = if *terminal {
                ExportTaskStatus::Failed
            } else {
                ExportTaskStatus::Initializing
            };
            assert_eq!(
                task.status, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                task.status
            );
            let expected_reason = if *terminal {
                Some(EXPORT_NOT_PERFORMED_REASON.to_string())
            } else {
                None
            };
            assert_eq!(
                task.status_reason, expected_reason,
                "case {case:?}: expected the reason {expected_reason:?}, got {:?}",
                task.status_reason
            );
            assert_eq!(
                task.elapsed_seconds(now + *after),
                after.as_secs() as i64,
                "case {case:?}: expected the whole seconds since the task started, got {}",
                task.elapsed_seconds(now + *after)
            );

            // Settling again at the same instant leaves the record where it is: the
            // transition is owed once, and a read never moves a record on by itself.
            task.settle(now + *after);
            assert_eq!(
                task.status, expected,
                "case {case:?}: a second settle must not change the status, expected \
                 {expected:?}, got {:?}",
                task.status
            );
        }
    }

    /// Which status an export task is owed, whether it can still be cancelled and the
    /// reason it carries are all decided by the status it is in — one row per status
    /// this slice stores.
    #[test]
    fn an_export_task_status_decides_what_the_clock_still_owes_it() {
        /// One row: a stored status and everything it decides.
        struct Row {
            status: ExportTaskStatus,
            cancellable: bool,
            terminal: bool,
            settles_into: Option<ExportTaskStatus>,
            reason: Option<&'static str>,
        }

        let cases = [
            Row {
                status: ExportTaskStatus::Initializing,
                cancellable: true,
                terminal: false,
                settles_into: Some(ExportTaskStatus::Failed),
                reason: None,
            },
            Row {
                status: ExportTaskStatus::Cancelling,
                cancellable: false,
                terminal: false,
                settles_into: Some(ExportTaskStatus::Cancelled),
                reason: None,
            },
            Row {
                status: ExportTaskStatus::Cancelled,
                cancellable: false,
                terminal: true,
                settles_into: None,
                reason: Some(EXPORT_CANCELLED_REASON),
            },
            Row {
                status: ExportTaskStatus::Failed,
                cancellable: false,
                terminal: true,
                settles_into: None,
                reason: Some(EXPORT_NOT_PERFORMED_REASON),
            },
        ];
        for row in cases {
            let status = row.status;
            let actual = status.is_cancellable();
            assert_eq!(
                actual, row.cancellable,
                "case {status:?}: expected cancellable={}, got {actual}",
                row.cancellable
            );
            let actual = status.is_terminal();
            assert_eq!(
                actual, row.terminal,
                "case {status:?}: expected terminal={}, got {actual}",
                row.terminal
            );
            let actual = status.settles_into();
            assert_eq!(
                actual, row.settles_into,
                "case {status:?}: expected it to settle into {:?}, got {actual:?}",
                row.settles_into
            );
            let actual = status.reason();
            assert_eq!(
                actual, row.reason,
                "case {status:?}: expected the reason {:?}, got {actual:?}",
                row.reason
            );
        }
    }

    /// A cancelled export task reports the modelled transient status until the clock
    /// makes its end due, and then stops `CANCELLED` — not the `FAILED` outcome the
    /// uncancelled task would have reached.
    ///
    /// The rows span the instants a read can fall on, including the span's own bound and
    /// one tick short of it, so the transient status cannot be skipped.
    #[test]
    fn a_cancelled_export_task_settles_into_the_modelled_cancelled_state() {
        let now = Instant::now();
        let tick = Duration::from_millis(1);

        // (case, when the task is read, whether the cancellation is through by then)
        let cases: &[(&str, Duration, bool)] = &[
            ("read at once", Duration::ZERO, false),
            ("read one tick in", tick, false),
            (
                "read one tick before the span is out",
                CANCELLATION_SPAN - tick,
                false,
            ),
            ("read when the span is out", CANCELLATION_SPAN, true),
            ("read a span later", CANCELLATION_SPAN * 2, true),
        ];
        for (case, after, through) in cases {
            let mut task = sample_export_task("t-0123456789", "g-0000000000");
            task.settles_at = Some(now + EXPORT_TASK_SPAN);
            task.begin_cancellation(now);
            assert_eq!(
                task.status,
                ExportTaskStatus::Cancelling,
                "case {case:?}: an accepted cancellation reports CANCELLING, got {:?}",
                task.status
            );

            task.settle(now + *after);
            let (expected, expected_reason) = if *through {
                (
                    ExportTaskStatus::Cancelled,
                    Some(EXPORT_CANCELLED_REASON.to_string()),
                )
            } else {
                (ExportTaskStatus::Cancelling, None)
            };
            assert_eq!(
                task.status, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                task.status
            );
            assert_eq!(
                task.status_reason, expected_reason,
                "case {case:?}: expected the reason {expected_reason:?}, got {:?}",
                task.status_reason
            );
        }
    }

    /// `destination` is an Amazon S3 URI: the `s3://` scheme and a bucket. The rows
    /// span the scheme (absent, another one, upper-cased), the bucket segment (named,
    /// missing, empty) and the key prefix.
    #[test]
    fn is_s3_uri_accepts_only_an_amazon_s3_uri() {
        for (candidate, accepted) in [
            ("s3://export-bucket", true),
            ("s3://export-bucket/key", true),
            ("s3://export-bucket/a/b/c", true),
            ("s3://export.bucket.with.dots/prefix", true),
            ("s3://a", true),
            ("s3://", false),
            ("s3:///key", false),
            ("export-bucket/key", false),
            ("https://export-bucket.s3.amazonaws.com/key", false),
            ("file:///tmp/export-bucket", false),
            ("S3://export-bucket", false),
            ("", false),
        ] {
            let actual = is_s3_uri(candidate);
            assert_eq!(
                actual, accepted,
                "case {candidate:?}: expected accepted={accepted}, got {actual}"
            );
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

    /// A missing private graph endpoint is reported from the addressing pair the
    /// request carried, for every well-formed pair.
    #[test]
    fn private_endpoint_not_found_names_the_graph_then_the_vpc() {
        for (graph_id, vpc_id) in [
            ("g-0000000000", "vpc-1"),
            ("g-a1b2c3d4e5", "vpc-0000000000000001e"),
            ("g-zzzzzzzzzz", "vpc-abcdef0123456789"),
        ] {
            let actual = private_endpoint_not_found(graph_id, vpc_id).message;
            let expected =
                format!("Private Graph Endpoint to Graph: {graph_id} from VPC: {vpc_id} does not exist");
            assert_eq!(
                actual, expected,
                "case {graph_id}/{vpc_id}: expected {expected:?}, got {actual:?}"
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
        let mut graph = sample_graph(id, &arn, "us-east-1");
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
            let actual = state.tags_of(&candidate, "us-east-1").is_some();
            assert_eq!(
                actual, resolves,
                "case {case:?} ({candidate:?}): expected resolves={resolves}, got {actual}"
            );
            let actual_mut = state.tags_of_mut(&candidate, "us-east-1").is_some();
            assert_eq!(
                actual_mut, resolves,
                "case {case:?} ({candidate:?}) mutably: expected resolves={resolves}, \
                 got {actual_mut}"
            );
        }
    }

    /// Every lookup and listing is scoped to the Region of the current request: a
    /// record of one Region answers only there, and the same NAME in two Regions is
    /// two records.
    #[test]
    fn every_lookup_is_scoped_to_the_region_of_the_request() {
        let now = Instant::now();
        // One graph per Region, all sharing the name `shared`, plus a second graph in
        // one of them so a Region can hold more than one.
        let placements: &[(&str, &str, &str)] = &[
            ("g-0000000001", "us-east-1", "shared"),
            ("g-0000000002", "eu-west-1", "shared"),
            ("g-0000000003", "ap-southeast-2", "shared"),
            ("g-0000000004", "eu-west-1", "other"),
        ];
        let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        for (id, region, name) in placements {
            let arn = ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("graph/{id}"));
            let mut graph = sample_graph(id, &arn, region);
            graph.name = (*name).to_string();
            state.insert_graph(graph);
            state.insert_snapshot(Snapshot {
                id: format!("gs{}", id.trim_start_matches('g')),
                name: (*name).to_string(),
                arn: ::engine::emulator_arn(
                    ARN_NAMESPACE,
                    region,
                    &format!("graph-snapshot/gs{}", id.trim_start_matches('g')),
                ),
                region: (*region).to_string(),
                source_graph_id: (*id).to_string(),
                status: SnapshotStatus::Available,
                snapshot_create_time: SystemTime::UNIX_EPOCH,
                kms_key_identifier: SERVICE_OWNED_KMS_KEY.to_string(),
                tags: BTreeMap::new(),
                settles_at: None,
            });
        }

        for (id, region, name) in placements {
            let snapshot_id = format!("gs{}", id.trim_start_matches('g'));
            // The id resolves in its own Region ...
            assert!(
                state.settled_graph(id, region, now).is_some(),
                "case {id:?} in {region:?}: expected the graph to resolve in its own Region"
            );
            assert!(
                state.settled_snapshot(&snapshot_id, region, now).is_some(),
                "case {snapshot_id:?} in {region:?}: expected the snapshot to resolve in \
                 its own Region"
            );
            // ... and in no other.
            for (_, other_region, _) in placements.iter().filter(|(_, r, _)| r != region) {
                let graph = state.settled_graph(id, other_region, now);
                assert!(
                    graph.is_none(),
                    "case {id:?} from {other_region:?}: expected no graph, got {:?}",
                    graph.map(|graph| graph.id.clone())
                );
                let snapshot = state.settled_snapshot(&snapshot_id, other_region, now);
                assert!(
                    snapshot.is_none(),
                    "case {snapshot_id:?} from {other_region:?}: expected no snapshot, \
                     got {:?}",
                    snapshot.map(|snapshot| snapshot.id.clone())
                );
            }
            // The name index is per Region: `shared` names a DIFFERENT graph in each.
            let named = state.graph_id_named(name, region);
            assert_eq!(
                named,
                Some(*id),
                "case name {name:?} in {region:?}: expected {id:?}, got {named:?}"
            );
            let named_snapshot = state.snapshot_id_named(name, region);
            assert_eq!(
                named_snapshot,
                Some(snapshot_id.as_str()),
                "case snapshot name {name:?} in {region:?}: expected {snapshot_id:?}, \
                 got {named_snapshot:?}"
            );
        }

        // A listing reports the Region's own graphs, in id order, and nothing else.
        for (region, expected) in [
            ("us-east-1", vec!["g-0000000001"]),
            ("eu-west-1", vec!["g-0000000002", "g-0000000004"]),
            ("ap-southeast-2", vec!["g-0000000003"]),
            ("sa-east-1", Vec::new()),
        ] {
            let actual: Vec<&str> = state
                .graphs_in(region, None)
                .map(|graph| graph.id.as_str())
                .collect();
            assert_eq!(
                actual, expected,
                "case listing {region:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // A delete reaches only the Region it was signed for.
        assert!(
            state.remove_graph("g-0000000002", "us-east-1").is_none(),
            "case deleting an eu-west-1 graph from us-east-1: expected no removal"
        );
        assert!(
            state.remove_graph("g-0000000002", "eu-west-1").is_some(),
            "case deleting an eu-west-1 graph from eu-west-1: expected the removal"
        );
        let named = state.graph_id_named("shared", "us-east-1");
        assert_eq!(
            named,
            Some("g-0000000001"),
            "case after deleting eu-west-1's 'shared': expected us-east-1's graph to \
             keep its name, got {named:?}"
        );
    }

    /// A stored graph with the members every test here needs and nothing else set.
    fn sample_graph(id: &str, arn: &str, region: &str) -> Graph {
        Graph {
            id: id.to_string(),
            name: format!("name-for-{id}"),
            arn: arn.to_string(),
            region: region.to_string(),
            status: GraphStatus::Available,
            status_reason: None,
            create_time: SystemTime::UNIX_EPOCH,
            provisioned_memory: 16,
            endpoint: "endpoint".to_string(),
            public_connectivity: false,
            replica_count: 1,
            kms_key_identifier: SERVICE_OWNED_KMS_KEY.to_string(),
            vector_search_configuration: None,
            deletion_protection: false,
            source_snapshot_id: None,
            build_number: GRAPH_SOFTWARE_BUILD_NUMBER.to_string(),
            tags: BTreeMap::new(),
            pending: None,
            deletes_at: None,
            data: GraphData::empty(SystemTime::UNIX_EPOCH),
        }
    }

    /// A node with the labels and property keys a case names, which is all the data
    /// invariants below look at.
    fn sample_node(labels: &[&str], property_keys: &[&str]) -> GraphNode {
        GraphNode {
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            properties: property_keys
                .iter()
                .map(|key| ((*key).to_string(), serde_json::Value::from(1)))
                .collect(),
        }
    }

    /// The data a graph holds accepts a node and an edge between stored nodes,
    /// refuses an edge with an endpoint the graph does not hold, derives a node's
    /// outgoing edge labels from the edges, and empties on `clear`.
    #[test]
    fn graph_data_writes_keep_the_edges_joined_to_stored_nodes() {
        let created_at = SystemTime::UNIX_EPOCH;
        let written_at = created_at + Duration::from_secs(1);
        let mut data = GraphData::empty(created_at);

        // (case, the edge to store, whether the data accepts it) — an edge is
        // refused for EITHER endpoint, so there is a row for each.
        data.insert_node("n1", sample_node(&["Person"], &["name"]), written_at)
            .expect("case stored node: expected the write to be accepted");
        data.insert_node("n2", sample_node(&["Person", "Author"], &[]), written_at)
            .expect("case second stored node: expected the write to be accepted");
        for (case, from_node, to_node, accepted) in [
            ("both endpoints stored", "n1", "n2", true),
            ("unknown source", "n9", "n2", false),
            ("unknown target", "n1", "n9", false),
        ] {
            let outcome = data.insert_edge(
                "e1",
                GraphEdge {
                    label: "KNOWS".to_string(),
                    from_node: from_node.to_string(),
                    to_node: to_node.to_string(),
                    properties: BTreeMap::new(),
                },
                written_at,
            );
            assert_eq!(
                outcome.is_ok(),
                accepted,
                "case {case:?}: expected accepted={accepted}, got {outcome:?}"
            );
        }

        assert_eq!(
            data.outgoing_edge_labels("n1"),
            BTreeSet::from(["KNOWS".to_string()]),
            "case outgoing labels of the edge's source: got {:?}",
            data.outgoing_edge_labels("n1")
        );
        assert!(
            data.outgoing_edge_labels("n2").is_empty(),
            "case outgoing labels of the edge's target: expected none, got {:?}",
            data.outgoing_edge_labels("n2")
        );

        // A write recomputes the statistics as of the instant it was made; a read
        // leaves them where they are.
        assert_eq!(
            data.statistics_computed_at, written_at,
            "case statistics after a write: expected {written_at:?}, got {:?}",
            data.statistics_computed_at
        );
        assert_eq!(
            data.outgoing_edge_labels("n1").len(),
            1,
            "case reading the data twice: a read must not change it"
        );
        assert_eq!(
            data.statistics_computed_at, written_at,
            "case reading the data: expected the statistics time to stand still"
        );

        let cleared_at = written_at + Duration::from_secs(1);
        data.clear(cleared_at);
        assert!(
            data.nodes.is_empty() && data.edges.is_empty(),
            "case cleared: expected no nodes and no edges, got {} nodes and {} edges",
            data.nodes.len(),
            data.edges.len()
        );
        assert_eq!(
            data.statistics_computed_at, cleared_at,
            "case cleared: expected {cleared_at:?}, got {:?}",
            data.statistics_computed_at
        );
    }

    /// Each map of a graph's data is bounded at its growth site, and a write that
    /// REPLACES an entry is accepted at the bound.
    #[test]
    fn graph_data_is_bounded_at_every_growth_site() {
        let written_at = SystemTime::UNIX_EPOCH;
        let mut data = GraphData::empty(written_at);
        for index in 0..MAX_NODES_PER_GRAPH {
            data.insert_node(&format!("n{index}"), GraphNode::default(), written_at)
                .unwrap_or_else(|reason| panic!("case node {index}: refused with {reason}"));
        }
        let refused = data.insert_node("one-too-many", GraphNode::default(), written_at);
        assert!(
            refused.is_err(),
            "case node past the bound: expected a refusal, got {refused:?}"
        );
        let replacement =
            data.insert_node("n0", sample_node(&["Person"], &["name"]), written_at);
        assert!(
            replacement.is_ok(),
            "case replacing a node at the bound: expected acceptance, got {replacement:?}"
        );
        assert_eq!(
            data.nodes.len(),
            MAX_NODES_PER_GRAPH,
            "case at the bound: expected {MAX_NODES_PER_GRAPH} nodes, got {}",
            data.nodes.len()
        );

        // One element's properties are bounded too, so the properties a graph holds
        // — and the work a report over them costs — stay bounded by the ceilings.
        let keys: Vec<String> = (0..=MAX_PROPERTIES_PER_ELEMENT)
            .map(|index| format!("p{index}"))
            .collect();
        let over_propertied = GraphNode {
            labels: BTreeSet::new(),
            properties: keys
                .iter()
                .map(|key| (key.clone(), serde_json::Value::from(1)))
                .collect(),
        };
        let refused_properties = data.insert_node("n0", over_propertied, written_at);
        assert!(
            refused_properties.is_err(),
            "case a node past the property bound: expected a refusal, got {refused_properties:?}"
        );
    }

    /// The transition is owed at a future instant, so a read taken immediately
    /// still sees the transient status — and it settles into the status the pending
    /// transition names, which is not always `AVAILABLE`.
    #[test]
    fn a_pending_transition_settles_only_once_it_is_due() {
        let now = Instant::now();
        // (case, the transient status reported until the transition falls due, the
        // status it settles into) — one row per transition this service accepts.
        for (case, transient, settled) in [
            ("provisioning", GraphStatus::Creating, GraphStatus::Available),
            ("reconfiguring", GraphStatus::Updating, GraphStatus::Available),
            (
                "snapshotting",
                GraphStatus::Snapshotting,
                GraphStatus::Available,
            ),
            ("stopping", GraphStatus::Stopping, GraphStatus::Stopped),
            ("starting", GraphStatus::Starting, GraphStatus::Available),
        ] {
            let mut graph = sample_graph("g-0123456789", "arn", "us-east-1");
            graph.begin_transition(transient, settled, now);

            graph.settle(now);
            assert_eq!(
                graph.status, transient,
                "case {case:?} not-yet-due: expected {transient:?}, got {:?}",
                graph.status
            );

            graph.settle(now + PROVISIONING_SPAN);
            assert_eq!(
                graph.status, settled,
                "case {case:?} due: expected {settled:?}, got {:?}",
                graph.status
            );
            assert!(
                graph.pending.is_none(),
                "case {case:?} due: the settled transition must be cleared, got {:?}",
                graph.pending
            );
        }
    }

    /// An accepted deletion reports `DELETING` and keeps the record \u2014 and the name it
    /// reserves \u2014 until the deletion the clock owes falls due, whatever status the
    /// graph was resting in when it was deleted.
    #[test]
    fn a_deleted_graph_is_reclaimed_only_once_the_deletion_is_due() {
        let now = Instant::now();
        // (case, the resting status the graph was deleted from) — a deletion is
        // accepted from either status a graph rests in.
        for (case, resting) in [
            ("serving", GraphStatus::Available),
            ("stopped", GraphStatus::Stopped),
        ] {
            let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
            let mut graph = sample_graph("g-0123456789", "arn", "us-east-1");
            graph.status = resting;
            let name = graph.name.clone();
            state.insert_graph(graph);

            state
                .settled_graph_mut("g-0123456789", "us-east-1", now)
                .expect("the stored graph resolves")
                .begin_deletion(now);

            let in_flight = state.settled_graph("g-0123456789", "us-east-1", now);
            assert_eq!(
                in_flight.map(|graph| graph.status),
                Some(GraphStatus::Deleting),
                "case {case:?} not-yet-due: expected a graph reporting DELETING, got {:?}",
                in_flight.map(|graph| graph.status)
            );
            let reserved = state.graph_id_named(&name, "us-east-1");
            assert_eq!(
                reserved,
                Some("g-0123456789"),
                "case {case:?} not-yet-due: the name must stay reserved, got {reserved:?}"
            );

            let settled = state
                .settled_graph("g-0123456789", "us-east-1", now + DELETION_SPAN)
                .cloned();
            assert!(
                settled.is_none(),
                "case {case:?} due: the record must be gone, got {settled:?}"
            );
            let freed = state.graph_id_named(&name, "us-east-1");
            assert!(
                freed.is_none(),
                "case {case:?} due: the name must be free again, got {freed:?}"
            );
        }
    }

    /// The name a final snapshot takes: after the graph where that fits and is free,
    /// otherwise after the snapshot's own id — and always a name the modelled
    /// `SnapshotName` constraints accept. The names it has to avoid are the ones its
    /// OWN Region holds, so a row is checked against a snapshot of another Region too.
    #[test]
    fn a_final_snapshots_name_satisfies_the_modelled_name_constraints() {
        let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        state.insert_snapshot(sample_snapshot(
            "gs-0000000000",
            "taken-graph-final-snapshot",
            "us-east-1",
        ));
        // The same name in ANOTHER Region is not in the way: names are per Region.
        state.insert_snapshot(sample_snapshot(
            "gs-0000000001",
            "elsewhere-graph-final-snapshot",
            "eu-west-1",
        ));
        let longest_graph_name = "a".repeat(MAX_RESOURCE_NAME_CHARS);

        // (case, the graph's name, whether the name may be built from the graph's own)
        let cases: &[(&str, &str, bool)] = &[
            ("a free name", "plain-graph", true),
            ("one character", "a", true),
            (
                "a name already held by a stored snapshot",
                "taken-graph",
                false,
            ),
            (
                "a name held only by a snapshot of another Region",
                "elsewhere-graph",
                true,
            ),
            (
                "a graph name that leaves no room for the suffix",
                longest_graph_name.as_str(),
                false,
            ),
        ];
        for (case, graph_name, after_the_graph) in cases {
            let minted = state.free_final_snapshot_name(graph_name, "gs-1234567890", "us-east-1");
            assert_eq!(
                minted == format!("{graph_name}{FINAL_SNAPSHOT_SUFFIX}"),
                *after_the_graph,
                "case {case:?}: expected named-after-the-graph={after_the_graph}, got {minted:?}"
            );
            assert!(
                !minted.is_empty() && minted.len() <= MAX_RESOURCE_NAME_CHARS,
                "case {case:?}: expected a name within 1..{MAX_RESOURCE_NAME_CHARS} characters, \
                 got {minted:?}"
            );
            assert!(
                resource_name_is_well_formed(&minted, "gs-"),
                "case {case:?}: expected a name the modelled pattern accepts, got {minted:?}"
            );
            let clash = state.snapshot_id_named(&minted, "us-east-1");
            assert!(
                clash.is_none(),
                "case {case:?}: expected a name no stored snapshot of this Region holds, \
                 got {minted:?} ({clash:?})"
            );
        }
    }

    /// A stored snapshot with the members these tests need and nothing else set.
    fn sample_snapshot(id: &str, name: &str, region: &str) -> Snapshot {
        Snapshot {
            id: id.to_string(),
            name: name.to_string(),
            arn: ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("graph-snapshot/{id}")),
            region: region.to_string(),
            source_graph_id: "g-0123456789".to_string(),
            status: SnapshotStatus::Available,
            snapshot_create_time: SystemTime::UNIX_EPOCH,
            kms_key_identifier: SERVICE_OWNED_KMS_KEY.to_string(),
            tags: BTreeMap::new(),
            settles_at: None,
        }
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
        state.insert_graph(sample_graph(graph_id, &graph_arn, "us-east-1"));

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
            region: "us-east-1".to_string(),
            source_graph_id: graph_id.to_string(),
            status: SnapshotStatus::Available,
            snapshot_create_time: SystemTime::UNIX_EPOCH,
            kms_key_identifier: SERVICE_OWNED_KMS_KEY.to_string(),
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
            let actual = state.tags_of(&candidate, "us-east-1").map(BTreeMap::len);
            assert_eq!(
                actual, expected,
                "case {case:?} ({candidate:?}): expected {expected:?} tag entries, \
                 got {actual:?}"
            );
            let actual_mut = state
                .tags_of_mut(&candidate, "us-east-1")
                .map(|tags| tags.len());
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
            region: "us-east-1".to_string(),
            source_graph_id: "g-0123456789".to_string(),
            status: SnapshotStatus::Creating,
            snapshot_create_time: SystemTime::UNIX_EPOCH,
            kms_key_identifier: SERVICE_OWNED_KMS_KEY.to_string(),
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
            GraphStatus::Importing,
            GraphStatus::Stopping,
            GraphStatus::Stopped,
            GraphStatus::Starting,
            GraphStatus::Deleting,
        ] {
            let wire = status.as_wire();
            assert!(
                crate::ops::types::GRAPH_STATUS_VALUES.contains(&wire),
                "case {status:?}: {wire:?} is not a modelled GraphStatus value"
            );
        }
        for status in [
            ExportTaskStatus::Initializing,
            ExportTaskStatus::Cancelling,
            ExportTaskStatus::Cancelled,
            ExportTaskStatus::Failed,
        ] {
            let wire = status.as_wire();
            assert!(
                crate::ops::types::EXPORT_TASK_STATUS_VALUES.contains(&wire),
                "case {status:?}: {wire:?} is not a modelled ExportTaskStatus value"
            );
        }
        for status in [SnapshotStatus::Creating, SnapshotStatus::Available] {
            let wire = status.as_wire();
            assert!(
                crate::ops::types::SNAPSHOT_STATUS_VALUES.contains(&wire),
                "case {status:?}: {wire:?} is not a modelled SnapshotStatus value"
            );
        }
        for status in [
            ImportTaskStatus::Initializing,
            ImportTaskStatus::Cancelling,
            ImportTaskStatus::Cancelled,
            ImportTaskStatus::Failed,
        ] {
            let wire = status.as_wire();
            assert!(
                crate::ops::types::IMPORT_TASK_STATUS_VALUES.contains(&wire),
                "case {status:?}: {wire:?} is not a modelled ImportTaskStatus value"
            );
        }
    }

    /// Every minted import-task id matches the modelled `TaskId` pattern and no id
    /// repeats, which is what lets a task be read back by the id it was minted with.
    #[test]
    fn minted_import_task_ids_match_the_modelled_pattern_and_are_unique() {
        let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..2_000 {
            let id = state.next_import_task_id();
            assert!(
                is_task_id(&id),
                "id {id:?}: expected a match for ^t-[a-z0-9]{{10}}$"
            );
            assert!(seen.insert(id.clone()), "id {id:?}: minted twice");
        }
    }

    /// An import task's pending transition is owed at a FUTURE instant, carries the
    /// reason the settled status reports, and is applied exactly once — over a row
    /// per transition this slice owes a task.
    #[test]
    fn a_pending_import_task_transition_settles_only_once_it_is_due() {
        // (case, the transient status, the status it settles into, its reason)
        let cases: Vec<(&str, ImportTaskStatus, ImportTaskStatus, Option<&str>)> = vec![
            (
                "an accepted task nothing loads",
                ImportTaskStatus::Initializing,
                ImportTaskStatus::Failed,
                Some(IMPORT_NOT_RUN_REASON),
            ),
            (
                "a cancelled task",
                ImportTaskStatus::Cancelling,
                ImportTaskStatus::Cancelled,
                Some(IMPORT_CANCELLED_REASON),
            ),
            (
                "a transition that carries no reason",
                ImportTaskStatus::Cancelling,
                ImportTaskStatus::Cancelled,
                None,
            ),
        ];
        for (case, transient, settled, reason) in cases {
            let now = Instant::now();
            let mut task = sample_import_task("t-0123456789", "g-0123456789");
            task.begin_transition(transient, settled, reason.map(str::to_string), now);

            task.settle(now);
            assert_eq!(
                task.status, transient,
                "case {case:?} not-yet-due: expected {transient:?}, got {:?}",
                task.status
            );

            task.settle(now + PROVISIONING_SPAN);
            assert_eq!(
                task.status, settled,
                "case {case:?} due: expected {settled:?}, got {:?}",
                task.status
            );
            assert_eq!(
                task.status_reason.as_deref(),
                reason,
                "case {case:?} due: expected statusReason {reason:?}, got {:?}",
                task.status_reason
            );
            assert!(
                task.pending.is_none(),
                "case {case:?} due: the settled transition must be cleared"
            );
        }
    }

    /// Only a task whose import is in flight can be cancelled, and only a stopped one
    /// is terminal — one row per status this slice stores.
    #[test]
    fn cancellability_is_decided_by_the_stored_status() {
        // (status, cancellable, terminal)
        for (status, cancellable, terminal) in [
            (ImportTaskStatus::Initializing, true, false),
            (ImportTaskStatus::Cancelling, false, false),
            (ImportTaskStatus::Cancelled, false, true),
            (ImportTaskStatus::Failed, false, true),
        ] {
            let actual = status.is_cancellable();
            assert_eq!(
                actual, cancellable,
                "case {status:?}: expected cancellable={cancellable}, got {actual}"
            );
            let actual_terminal = status.is_terminal();
            assert_eq!(
                actual_terminal, terminal,
                "case {status:?}: expected terminal={terminal}, got {actual_terminal}"
            );
        }
    }

    /// A stored import task with the members every test here needs and nothing else
    /// set.
    fn sample_import_task(id: &str, graph_id: &str) -> ImportTask {
        ImportTask {
            id: id.to_string(),
            region: "us-east-1".to_string(),
            graph_id: graph_id.to_string(),
            source: "s3://example-bucket/data/".to_string(),
            format: None,
            parquet_type: None,
            blank_node_handling: None,
            fail_on_error: true,
            role_arn: "arn:aws:iam::000000000000:role/example".to_string(),
            import_options: None,
            status: ImportTaskStatus::Initializing,
            status_reason: None,
            attempt_number: 1,
            pending: None,
        }
    }

    /// A stored export task with the members every test here needs and nothing else
    /// set.
    fn sample_export_task(id: &str, graph_id: &str) -> ExportTask {
        let now = Instant::now();
        ExportTask {
            id: id.to_string(),
            arn: ::engine::emulator_arn(ARN_NAMESPACE, "us-east-1", &format!("export-task/{id}")),
            region: "us-east-1".to_string(),
            source_graph_id: graph_id.to_string(),
            role_arn: "arn:aws:iam::000000000000:role/example".to_string(),
            format: "CSV".to_string(),
            parquet_type: None,
            destination: "s3://example-bucket/export/".to_string(),
            kms_key_identifier:
                "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012"
                    .to_string(),
            export_filter: None,
            status: ExportTaskStatus::Initializing,
            status_reason: None,
            tags: BTreeMap::new(),
            start_time: SystemTime::UNIX_EPOCH,
            started_at: now,
            settles_at: None,
        }
    }

    /// An import task and an export task are addressed in ONE modelled id space
    /// (`TaskId`'s `^t-[a-z0-9]{10}$`), so no id minted for either is ever an id the
    /// other already holds — asserted by alternating the two generators, which is what
    /// forces each to skip what the other has filed.
    #[test]
    fn minted_task_ids_are_unique_across_the_import_and_export_task_maps() {
        let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        let mut seen = std::collections::BTreeSet::new();
        for index in 0..1_000 {
            let (kind, id) = if index % 2 == 0 {
                let id = state.next_import_task_id();
                state.insert_import_task(sample_import_task(&id, "g-0123456789"));
                ("an import task", id)
            } else {
                let id = state.next_export_task_id();
                state.insert_export_task(sample_export_task(&id, "g-0123456789"));
                ("an export task", id)
            };
            assert!(
                is_task_id(&id),
                "case {kind} ({id:?}): expected a match for ^t-[a-z0-9]{{10}}$"
            );
            assert!(
                seen.insert(id.clone()),
                "case {kind} ({id:?}): minted twice across the shared task-id space"
            );
        }
    }

    /// The provisioned memory a graph created with an import gets, over a row per form
    /// the request's bounds can take (neither, one, both, equal) — including the
    /// published range's own bounds.
    #[test]
    fn import_provisioned_memory_is_the_floor_of_the_range_the_request_allows() {
        // (case, minProvisionedMemory, maxProvisionedMemory, the chosen value)
        let cases: [(&str, Option<i32>, Option<i32>, i32); 7] = [
            (
                "neither bound supplied",
                None,
                None,
                UNBOUNDED_IMPORT_PROVISIONED_MEMORY,
            ),
            ("only a minimum", Some(128), None, 128),
            (
                "only a maximum",
                None,
                Some(2048),
                DEFAULT_MIN_PROVISIONED_MEMORY,
            ),
            ("both bounds", Some(256), Some(1024), 256),
            ("both bounds equal", Some(512), Some(512), 512),
            // The published range of `ProvisionedMemory` is 16..24576
            // (`smithy_query(query="shape",shape="ProvisionedMemory")`).
            ("at the bottom of the published range", Some(16), None, 16),
            (
                "at the top of the published range",
                Some(24_576),
                Some(24_576),
                24_576,
            ),
        ];
        for (case, min, max, expected) in cases {
            let actual = import_provisioned_memory(min, max);
            assert_eq!(
                actual, expected,
                "case {case:?} (min={min:?}, max={max:?}): expected {expected}, \
                 got {actual}"
            );
        }
    }

    /// The one rule every operation that accepts an import `source` enforces, over a
    /// row per clause: what a location may look like, blankness in each of its forms,
    /// and the stored length at the bound and one character past it.
    #[test]
    fn the_import_source_rule_bounds_blankness_and_the_stored_length() {
        let prefix = "s3://example-bucket/";
        let filler = MAX_IMPORT_SOURCE_CHARS - prefix.len();
        // (case, the candidate source, whether the rule accepts it)
        let cases: Vec<(&str, String, bool)> = vec![
            (
                "an S3 prefix",
                "s3://example-bucket/data/".to_string(),
                true,
            ),
            (
                "a Neptune database endpoint",
                "neptune-db://cluster.example.com:8182".to_string(),
                true,
            ),
            (
                "a Neptune snapshot ARN",
                "arn:aws:rds:us-east-1:000000000000:cluster-snapshot:nightly".to_string(),
                true,
            ),
            ("a single character", "s".to_string(), true),
            (
                "at the stored-length bound",
                format!("{prefix}{}", "d".repeat(filler)),
                true,
            ),
            (
                "one character past the stored-length bound",
                format!("{prefix}{}", "d".repeat(filler + 1)),
                false,
            ),
            ("the empty string", String::new(), false),
            ("nothing but spaces", "   ".to_string(), false),
            ("nothing but a tab and a newline", "\t\n".to_string(), false),
        ];
        for (case, candidate, accepted) in cases {
            let requirement = import_source_requirement(&candidate);
            assert_eq!(
                requirement.is_none(),
                accepted,
                "case {case:?} ({} characters): expected accepted={accepted}, got {:?}",
                candidate.chars().count(),
                requirement
            );
        }
    }
}
