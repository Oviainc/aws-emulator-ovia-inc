// SPDX-License-Identifier: CC0-1.0

//! The gateway-capability, dataset, data-segment, workspace and
//! compute-node-execution records of [`super::ComAmazonawsIotsitewiseAwsiotsitewiseState`].

use super::*;

/// The message the per-gateway capability quota is refused with: the ceiling
/// [`MAX_CAPABILITIES_PER_GATEWAY`] states, named as the refusal's own limit.
pub fn capability_limit_message() -> String {
    format!(
        "This emulator holds at most {MAX_CAPABILITIES_PER_GATEWAY} capability configurations \
         per gateway."
    )
}

/// One capability configured on a gateway: the namespace that selects it, the
/// JSON document the last write to it supplied, and the instant the gateway has
/// processed that document.
///
/// The sync status is DERIVED from that instant rather than stored, so a read
/// never has to write for the status to move on.
#[derive(Debug, Clone)]
pub struct GatewayCapabilityRecord {
    pub namespace: String,
    pub configuration: String,
    /// When the capability stops being out of sync — see [`Self::sync_status`].
    pub in_sync_at: SystemTime,
}

impl GatewayCapabilityRecord {
    /// This capability's `CapabilitySyncStatus` at `now`.
    ///
    /// `UpdateGatewayCapabilityConfiguration` documents the rule: after a write
    /// "the sync status becomes OUT_OF_SYNC until the gateway processes the
    /// configuration", which is the owed transition this emulator settles at
    /// [`ACTIVATION_DELAY`] — so the status is `OUT_OF_SYNC` until then and
    /// `IN_SYNC` ("The gateway is running with the latest configuration")
    /// afterwards.
    pub fn sync_status(&self, now: SystemTime) -> &'static str {
        if now < self.in_sync_at {
            CAPABILITY_SYNC_OUT_OF_SYNC
        } else {
            CAPABILITY_SYNC_IN_SYNC
        }
    }
}

/// A stored dataset: everything `DescribeDataset` reports, plus the `tags` the
/// tagging operations read.
///
/// `settling_state` holds a `DatasetState` value, which spells its states exactly
/// as `AssetState` does — `CREATING`, `ACTIVE`, `UPDATING`, `DELETING` (plus
/// `FAILED`, which nothing here produces) — so this record reuses
/// [`STATE_CREATING`] / [`STATE_UPDATING`] / [`STATE_ACTIVE`] and the one
/// [`state_at`] rule rather than a second spelling of the same values.
#[derive(Debug, Clone)]
pub struct DatasetRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// A `DatasetTypeEnum` value — `SESSION`, `CURATED` or `EXTERNAL` — when the
    /// create named one. The member is optional on every shape that carries it,
    /// so a dataset created without one has none rather than a guessed type.
    pub dataset_type: Option<String>,
    pub config: Option<DatasetConfig>,
    pub metadata: HashMap<String, String>,
    pub source: DatasetSource,
    /// The workspace that contains the dataset, for a dataset created in one.
    pub workspace_name: Option<String>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `DatasetState` this dataset reports until [`Self::active_at`] falls
    /// due: [`STATE_CREATING`] for a freshly created dataset, [`STATE_UPDATING`]
    /// while a mutating call it already accepted settles.
    pub settling_state: String,
    /// How many writes this dataset has accepted, which is the `datasetVersion`
    /// it reports; see [`DATASET_INITIAL_VERSION`].
    pub version: u32,
    pub tags: HashMap<String, String>,
}

impl DatasetRecord {
    /// The dataset's status as of `now`; see [`AssetModelRecord::status`].
    pub fn status(&self, now: SystemTime) -> DatasetStatus {
        DatasetStatus {
            state: self.state(now).to_string(),
            error: None,
        }
    }

    /// The dataset's `DatasetState` as of `now`: its [`Self::settling_state`]
    /// until the owed transition falls due, `ACTIVE` from then on. Derived, so a
    /// read never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// The `datasetVersion` this dataset reports: its accepted-write count as the
    /// decimal string the modelled `Version` type is.
    pub fn version_label(&self) -> String {
        self.version.to_string()
    }

    /// Record that a mutating call landed at `now`: the dataset reports
    /// `UPDATING` until the transition that call owes falls due and `ACTIVE`
    /// after it, and the write it accepted takes the dataset to its next version.
    ///
    /// The due instant is STORED, never slept on, so the caller's own call
    /// returns immediately and the transient state is still what a read taken
    /// straight afterwards observes.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.last_update_date = now;
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
        self.version = self.version.saturating_add(1);
    }
}

/// One data segment a dataset holds: a span of ONE time series' data.
///
/// A session dataset "contains data segments of time series data" and a curated
/// dataset "curates data segments selected from source session datasets" (the
/// `CreateDataset` model doc), so a segment names the dataset it came FROM
/// ([`Self::source_dataset_id`]) beside the time series and the span it covers.
/// The time series and the span are a segment's IDENTITY inside the dataset that
/// holds it: that is exactly what `BatchDeleteDatasetDataSegments` addresses one
/// by, so a dataset holds at most one segment for a given series and span.
///
/// The span is HALF-OPEN — `[start_timestamp, end_timestamp)` — the way a capture
/// source's [`CaptureSegment`] span is read, so two adjacent segments cover a
/// continuous stream and the instant between them belongs to the later one alone.
///
/// The one writer of a dataset's segments is
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::put_dataset_data_segment`] — the
/// entry point every producer path (`BatchAssociateDataSegmentsToDataset`, the
/// bulk import of time series data into a session dataset
/// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::ingest_data_segments`]), or a
/// test seeding a dataset) calls — and the removers are
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::delete_dataset_data_segment`]
/// and
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::disassociate_dataset_data_segment`],
/// which differ only in whether the caller also deletes the data the segment
/// covered.
#[derive(Debug, Clone)]
pub struct DataSegmentRecord {
    /// The dataset the segment's data came from: the holding dataset itself for a
    /// session dataset's own segment, and the source session dataset for a
    /// segment a curated dataset selected.
    pub source_dataset_id: String,
    /// The `TimeSeriesId` of the time series whose data the segment covers.
    pub time_series_id: String,
    /// When the covered data begins.
    pub start_timestamp: TimeInNanos,
    /// When it ends. Always later than [`Self::start_timestamp`]: the writer
    /// refuses a segment covering no time at all.
    pub end_timestamp: TimeInNanos,
}

/// A workspace, by the name that identifies it in this account and region.
#[derive(Debug, Clone)]
pub struct WorkspaceRecord {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    /// The encryption scheme the create stated, kept verbatim so a read reports
    /// what was accepted rather than a default.
    pub encryption: WorkspaceEncryptionConfiguration,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `WorkspaceState` this workspace reports until [`Self::active_at`]
    /// falls due.
    pub settling_state: String,
    pub tags: HashMap<String, String>,
}

impl WorkspaceRecord {
    /// The workspace's `WorkspaceState` as of `now`: its [`Self::settling_state`]
    /// until the owed transition falls due, `ACTIVE` from then on. Derived, so a
    /// read never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// The workspace's status as of `now`.
    pub fn status(&self, now: SystemTime) -> WorkspaceStatus {
        WorkspaceStatus {
            state: self.state(now).to_string(),
            error: None,
        }
    }

    /// Record that an accepted update landed at `now`: the members it leaves the
    /// workspace with replace what it held, and the workspace reports `UPDATING`
    /// until the transition this call owes falls due.
    ///
    /// Every member is passed in resolved, so the caller is the one place that
    /// decides what an omitted member keeps. The due instant is STORED, never
    /// slept on, so the caller's own call returns immediately and the transient
    /// state is still what a read taken straight afterwards observes.
    pub fn record_update(
        &mut self,
        description: Option<String>,
        encryption: WorkspaceEncryptionConfiguration,
        now: SystemTime,
    ) {
        self.description = description;
        self.encryption = encryption;
        self.last_update_date = now;
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
    }
}

/// One compute node of a recorded pipeline execution: which task it runs, the
/// nodes it waits for, and the environment and mounts resolved for it when the
/// execution was accepted.
///
/// Resolved ONCE, at the call that starts the execution, so a later write to the
/// pipeline or to a task cannot change what a past execution reports.
#[derive(Debug, Clone)]
pub struct ComputeNodeExecutionRecord {
    pub compute_node_name: String,
    pub task_name: String,
    /// The ARN the task was minted with, copied rather than rebuilt, so the
    /// execution keeps naming the task it actually resolved.
    pub task_arn: String,
    /// The task version the execution resolved.
    pub task_version: String,
    pub depends_on: Vec<String>,
    /// A `ComputeNodeExecutionState` value.
    pub state: String,
    /// The fully resolved environment variables for this node.
    pub environment_variables: HashMap<String, String>,
    /// The fully resolved mounts for this node.
    pub mounts: Vec<Mount>,
    pub start_time: Option<SystemTime>,
    pub end_time: Option<SystemTime>,
}

impl ComputeNodeExecutionRecord {
    /// This node's execution detail, as `DescribePipelineExecution` reports it.
    pub fn details(&self) -> ComputeNodeExecutionDetails {
        ComputeNodeExecutionDetails {
            compute_node_name: self.compute_node_name.clone(),
            task_name: self.task_name.clone(),
            task_arn: self.task_arn.clone(),
            task_version: self.task_version.clone(),
            depends_on: self.depends_on.clone(),
            status: ComputeNodeExecutionStatus {
                state: self.state.clone(),
                state_details: None,
            },
            start_time: self.start_time.map(to_date_time),
            end_time: self.end_time.map(to_date_time),
            execution_environment_variables: Some(self.environment_variables.clone()),
            execution_mounts: Some(self.mounts.clone()),
        }
    }
}

/// One pipeline execution, by the `pipelineExecutionId` that identifies it.
///
/// A compute node of an execution runs its task's container image, which this
/// emulator has no runtime for, so an accepted execution is RECORDED and no node
/// is reported started, succeeded or failed: a state, a duration or an error for
/// work that nothing ran would be invented rather than observed. What the record
/// does hold is everything the request and the stored pipeline determine — the
/// pipeline version it ran against, the DAG order, and the environment and mounts
/// resolved per node.
///
/// A cancellation IS something a call does to an execution, so it is recorded
/// here ([`Self::cancel_requested_at`]) and the state it takes the execution to
/// is derived from the clock ([`Self::state`]) rather than stored, the way every
/// other owed transition in this store is.
#[derive(Debug, Clone)]
pub struct PipelineExecutionRecord {
    pub id: String,
    pub workspace_name: String,
    pub pipeline_name: String,
    /// The `version` the pipeline carried when the execution was accepted.
    pub pipeline_version: String,
    /// The `PipelineExecutionState` the execution's own RUN has reached.
    /// Nothing runs here, so it stays [`EXECUTION_STATE_NOT_STARTED`]; a cancel
    /// is held separately in [`Self::cancel_requested_at`], so what
    /// [`Self::state`] reports is derived from both rather than overwritten by
    /// either.
    pub run_state: String,
    /// The priority the execution runs at: the one the request named, or
    /// [`DEFAULT_EXECUTION_PRIORITY`].
    pub execution_priority: i32,
    /// The environment variable overrides the request carried, verbatim.
    pub request_environment_variables: ExecutionEnvironmentVariables,
    /// The mount overrides the request carried, when it carried any.
    pub request_mount_overrides: Option<MountOverrides>,
    /// When the execution was accepted. Distinct from [`Self::start_time`], which
    /// is when the execution itself began.
    pub accepted_at: SystemTime,
    pub start_time: Option<SystemTime>,
    pub end_time: Option<SystemTime>,
    /// When a cancel was recorded on this execution, if one was. A cancel is
    /// recorded only on an execution whose run has not reached a terminal
    /// state, and from that instant the rest of the execution's lifecycle is
    /// owed by the clock rather than by any call.
    pub cancel_requested_at: Option<SystemTime>,
    /// The `reason` the recorded cancel carried, if it carried one: "A message
    /// describing why the pipeline execution is being cancelled", which
    /// `CancelPipelineExecution` documents "is returned in the stateDetails
    /// field when you describe the execution".
    pub cancel_reason: Option<String>,
    /// The compute nodes, in the DAG order
    /// [`compute_node_execution_order`] put them in.
    pub compute_nodes: Vec<ComputeNodeExecutionRecord>,
}

impl PipelineExecutionRecord {
    /// The execution's `PipelineExecutionState` at `now`.
    ///
    /// An execution a cancel was recorded on reports
    /// [`EXECUTION_STATE_CANCELLING`] until that cancellation takes effect and
    /// [`EXECUTION_STATE_CANCELLED`] from then on; otherwise it reports the
    /// state its own run has reached. DERIVED rather than stored, because the
    /// second half of the cancellation is owed by the clock and not by any
    /// call, so two readers of one execution cannot disagree about where it has
    /// got to and a read can never move it on.
    pub fn state(&self, now: SystemTime) -> &str {
        match self.cancel_requested_at {
            Some(_) if self.cancelled_at(now).is_some() => EXECUTION_STATE_CANCELLED,
            Some(_) => EXECUTION_STATE_CANCELLING,
            None => self.run_state.as_str(),
        }
    }

    /// The instant the recorded cancellation took effect, and `None` until it
    /// has — or when no cancel was recorded at all. The transition falls due
    /// [`ACTIVATION_DELAY`] after the cancel was recorded, the span this
    /// service owes every other transition it settles, and the instant is
    /// DERIVED from the recorded one rather than slept for.
    pub fn cancelled_at(&self, now: SystemTime) -> Option<SystemTime> {
        let due = self.cancel_requested_at? + ACTIVATION_DELAY;
        (now >= due).then_some(due)
    }

    /// The execution's status at `now`: the state [`Self::state`] derives, and
    /// beside it the `stateDetails` — "additional information about the current
    /// execution status. Populated when the execution has terminated", whose
    /// `message` for a cancelled execution "is the reason you supplied when
    /// calling CancelPipelineExecution". So the details appear with the terminal
    /// state the cancellation reaches, and only when the cancel carried a
    /// reason; the `code` and the per-step entries beside it are a FAILED
    /// execution's ("present when the execution failed"), which a cancelled one
    /// is not.
    pub fn status(&self, now: SystemTime) -> PipelineExecutionStatus {
        let state = self.state(now).to_string();
        let state_details = self
            .cancel_reason
            .clone()
            .filter(|_| EXECUTION_TERMINAL_STATES.contains(&state.as_str()))
            .map(|message| PipelineExecutionStateDetails {
                code: None,
                message,
                details: None,
            });
        PipelineExecutionStatus {
            state,
            state_details,
        }
    }

    /// When the execution ended, and `None` while it has not: the instant its
    /// run finished, or the instant a recorded cancellation took effect, since
    /// that is when a cancelled execution reached the state it stays in.
    pub fn ended_at(&self, now: SystemTime) -> Option<SystemTime> {
        self.end_time.or_else(|| self.cancelled_at(now))
    }

    /// Whether this execution is still ACTIVE at `now`: in a
    /// `PipelineExecutionState` that is not one of
    /// [`EXECUTION_TERMINAL_STATES`], so the execution has not finished. An
    /// execution waiting to begin counts as active — it has not completed
    /// either — and one whose cancellation is still in flight does too.
    pub fn is_active(&self, now: SystemTime) -> bool {
        !EXECUTION_TERMINAL_STATES.contains(&self.state(now))
    }

    /// Whether a cancel may still be RECORDED on this execution: one whose run
    /// has not reached a terminal state. An execution a cancel was already
    /// recorded on is not cancelled again — the repeat is a no-op reporting the
    /// state it stands in — and one whose run ended cannot be cancelled at all.
    pub fn is_cancelable(&self) -> bool {
        self.cancel_requested_at.is_none()
            && !EXECUTION_TERMINAL_STATES.contains(&self.run_state.as_str())
    }

    /// Record a cancellation on this execution at `now`, carrying the `reason`
    /// the request supplied. The due instant of the `CANCELLING` ->
    /// `CANCELLED` transition is STORED (as the instant it is owed from), never
    /// slept on, so the caller's own call returns immediately and the transient
    /// state is still what a read taken straight afterwards observes.
    pub fn record_cancel(&mut self, reason: Option<String>, now: SystemTime) {
        self.cancel_requested_at = Some(now);
        self.cancel_reason = reason;
    }
}

/// The canonical ARN of a workspace in `region`.
pub fn workspace_arn(region: &str, workspace_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{WORKSPACE_RESOURCE_TYPE}/{workspace_name}"),
    )
}

/// The order a pipeline's compute nodes run in: the DAG order their `dependsOn`
/// edges define, as the positions of [`PipelineVersion::computations`].
///
/// "Nodes without dependencies start immediately, while dependent nodes wait for
/// all upstream nodes to complete successfully" — so a node is ordered after
/// every node it depends on. Ties keep declaration order, which makes the order
/// one request produces stable across reads and resumable by a page token.
///
/// `Err` carries the refusal message for a `computations` list that is not a DAG
/// at all, from [`check_compute_nodes`] — the one validator every write to a
/// pipeline's compute nodes already uses, so one list is judged by one rule. The
/// caller lifts it into its own modelled refusal.
///
/// Linear in the nodes and their dependencies, like that validator: the lookups
/// are hashed and each edge is walked once (Kahn's algorithm over a ready set
/// ordered by declaration position), so no shape of list — a chain declared
/// backwards included — makes the walk cost more than the list itself. How long
/// the list itself can be is bounded by the model: `ComputeNodeList` is
/// `@length(min: 0, max: 50)` and `ComputeNodeNameList` `@length(min: 0, max:
/// 10)`, both refused at the boundary before a handler sees the request, so no
/// request can grow this walk.
pub fn compute_node_execution_order(computations: &[ComputeNode]) -> Result<Vec<usize>, String> {
    check_compute_nodes(computations)?;

    // How many dependencies each node is still waiting on, by position, and
    // which positions wait on each node.
    let mut waiting: Vec<usize> = Vec::with_capacity(computations.len());
    let mut dependents: HashMap<&str, Vec<usize>> = HashMap::with_capacity(computations.len());
    for (at, node) in computations.iter().enumerate() {
        let mut outstanding = 0usize;
        for dependency in node.depends_on.iter().flatten() {
            outstanding += 1;
            dependents.entry(dependency.as_str()).or_default().push(at);
        }
        waiting.push(outstanding);
    }

    // The ready set is ordered BY POSITION, so of the nodes that could run next
    // the one the write declared first is ordered first.
    let mut ready: BTreeSet<usize> = waiting
        .iter()
        .enumerate()
        .filter(|(_, outstanding)| **outstanding == 0)
        .map(|(at, _)| at)
        .collect();
    let mut order = Vec::with_capacity(computations.len());
    while let Some(at) = ready.pop_first() {
        order.push(at);
        let released = computations[at].compute_node_name.as_str();
        for dependent in dependents.get(released).into_iter().flatten().copied() {
            // Decremented through a guard rather than in place: the count is a
            // `usize`, and a list where two nodes answer to one name would
            // release the same dependent twice, so the guard is what keeps that
            // count from wrapping and the dependent from being ordered twice.
            // Such a list is already refused by `check_compute_nodes` above;
            // this holds whatever reaches here.
            let Some(outstanding) = waiting.get_mut(dependent) else {
                continue;
            };
            if *outstanding == 0 {
                continue;
            }
            *outstanding -= 1;
            if *outstanding == 0 {
                ready.insert(dependent);
            }
        }
    }
    // Every node is placed: a list that would leave one waiting holds a cycle,
    // which `check_compute_nodes` above already refused. Asserted rather than
    // assumed, so a regression in that validator shows up as a failing test
    // instead of an execution quietly missing compute nodes.
    debug_assert_eq!(
        order.len(),
        computations.len(),
        "the DAG walk placed {} of {} compute nodes",
        order.len(),
        computations.len()
    );
    Ok(order)
}
