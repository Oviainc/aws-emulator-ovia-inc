// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pcs-awsparallelcomputingservice service.
//!
//! PCS is a control plane over three nested resources: a cluster, and the compute node
//! groups and queues that live inside one. Every operation computes its answer from the
//! records held here — a create mints an id and an ARN and stores the record, a read
//! looks it up, a delete takes it out.
//!
//! Besides the records, this module owns the three things more than one handler needs,
//! so no two operations can disagree about them:
//!
//! * [`ComAmazonawsPcsAwsparallelcomputingserviceState::settle`] — the lifecycle
//!   transitions the CLOCK owes. A created resource is reported `CREATING` and becomes
//!   `ACTIVE` once [`TRANSITION_SPAN`] has elapsed; a deleted cluster is reported
//!   `DELETING` and then disappears. The due instant is stored and checked on the next
//!   call, so nothing ever sleeps.
//! * the identifier rules: minting a `pcs_...` id, building an ARN, resolving a
//!   `clusterIdentifier`/`queueIdentifier`/`computeNodeGroupIdentifier` by name OR id,
//!   and refusing a resource name the boundary validator cannot refuse
//!   ([`resource_name_violation`]).
//! * paginating a creation-ordered list behind an opaque `nextToken`
//!   ([`paginate`]).
//!
//! # Every store here is bounded
//!
//! Nothing a caller can repeat grows this state without a ceiling:
//!
//! * record COUNT — [`MAX_CLUSTERS`], [`MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER`] and
//!   [`MAX_QUEUES_PER_CLUSTER`] are checked before the insert in each create, which
//!   refuses the overflowing call with the modelled `ServiceQuotaExceededException`
//!   ([`quota_exceeded`]).
//! * record SIZE — the caller-sized lists a create persists are capped at
//!   [`MAX_STORED_LIST_ENTRIES`] ([`stored_list_violation`]); `tags` needs no check
//!   here because `RequestTagMap` is modelled `@length(min: 1, max: 200)` with
//!   `TagKey` `@length(..128)` and `TagValue` `@length(..256)`, all enforced at the
//!   boundary before a handler runs.
//! * the idempotency index — [`MAX_REMEMBERED_CLIENT_TOKENS`] entries, evicted
//!   oldest-first, and every entry pointing into a cluster is dropped when that
//!   cluster is removed ([`ComAmazonawsPcsAwsparallelcomputingserviceState::settle`]),
//!   so a create/delete loop cannot grow it.
//! * response size — [`page_size`] clamps a caller's `maxResults` to the modelled
//!   `1..=100` before [`paginate`] truncates.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    Accounting, AccountingRequest, Cluster, ClusterSlurmConfiguration,
    ClusterSlurmConfigurationRequest, ClusterSummary, ComputeNodeGroup,
    ComputeNodeGroupConfiguration, ComputeNodeGroupSlurmConfiguration,
    ComputeNodeGroupSlurmConfigurationRequest, CustomLaunchTemplate,
    Endpoint, ErrorInfo, InstanceConfig, Networking, NetworkingRequest, NodeLifecycleActions,
    NodeLifecycleActionsRequest, Queue, QueueSlurmConfiguration, QueueSlurmConfigurationRequest,
    QueueSummary, ScalingConfiguration, ScalingConfigurationRequest, Scheduler, SchedulerRequest,
    ServiceQuotaExceededException, SlurmRest, SlurmRestRequest, SpotOptions, ValidationException,
    ValidationExceptionField,
};

/// The `ClusterStatus`/`QueueStatus`/`ComputeNodeGroupStatus` member a just-created
/// resource carries (every one of the three enums declares it; see
/// `CLUSTER_STATUS_VALUES` in `crate::ops::types`).
pub const STATUS_CREATING: &str = "CREATING";

/// The status a resource reaches once its creation has come due.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// The status a cluster carries between `DeleteCluster` and its removal.
pub const STATUS_DELETING: &str = "DELETING";

/// How long after the call that starts it a PCS lifecycle transition the clock owes
/// comes due: a created resource leaving `CREATING`, a deleted cluster leaving
/// `DELETING` and disappearing. PCS itself takes minutes to provision a cluster; the
/// emulator keeps the transient state observable to the caller that acts straight after
/// the call, and never sleeps for it — the due instant is stored and read on the next
/// call.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The `maxResults` a list operation serves when the request names none: the
/// `@default(10)` the model declares on `MaxResults`.
pub const DEFAULT_PAGE_SIZE: usize = 10;

/// The region an ARN carries when the request carries no SigV4 credential scope to read
/// one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The prefix PCS reserves for the ids it generates (`pcs_...`); a caller-chosen
/// resource name can never take it.
pub const GENERATED_ID_PREFIX: &str = "pcs_";

/// The most clusters this emulator holds. PCS caps clusters per account per Region with
/// an account-adjustable service quota and answers `ServiceQuotaExceededException` once
/// it is reached ("The max number of clusters or queues has been reached for the
/// account"); the NUMBER here is the emulator's own ceiling, not a published AWS
/// default, and exists so a repeated create cannot grow this process without bound.
pub const MAX_CLUSTERS: usize = 50;

/// The most compute node groups one cluster holds, refused the same way ("The max number
/// of compute node groups has been reached for the associated cluster"). The number is
/// the emulator's ceiling.
pub const MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER: usize = 20;

/// The most queues one cluster holds, refused the same way. The number is the emulator's
/// ceiling.
pub const MAX_QUEUES_PER_CLUSTER: usize = 50;

/// The most entries a caller-sized list a create PERSISTS may carry (`subnetIds`,
/// `instanceConfigs`, `computeNodeGroupConfigurations`, and the Slurm custom-setting
/// lists). The model bounds none of them, so without this one request could size one
/// record arbitrarily; the ceiling matches the 200 the model allows for `RequestTagMap`,
/// the one caller-sized collection it does bound.
pub const MAX_STORED_LIST_ENTRIES: usize = 200;

/// The most `@idempotencyToken` values remembered at once. Beyond this the oldest entry
/// is evicted, so a caller minting fresh tokens cannot grow the index without bound; a
/// retry whose token has been evicted is served as a fresh create, which is what a
/// caller retrying long after the fact would get from the real service too.
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = 1024;

/// What the clock owes a resource once its pending transition comes due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingChange {
    /// `CREATING` -> [`STATUS_ACTIVE`].
    Activate,
    /// `DELETING` -> gone from state.
    Remove,
}

/// A transition the clock owes a resource, and the instant it comes due.
#[derive(Debug, Clone, Copy)]
pub struct Pending {
    /// The instant from which the change is owed. Never read by sleeping on it.
    pub due_at: Instant,
    /// The change the clock owes.
    pub change: PendingChange,
}

impl Pending {
    /// The change, owed [`TRANSITION_SPAN`] after now.
    pub fn in_a_span(change: PendingChange) -> Self {
        Self {
            due_at: Instant::now() + TRANSITION_SPAN,
            change,
        }
    }

    /// Whether `now` has reached the due instant.
    fn is_due(self, now: Instant) -> bool {
        now >= self.due_at
    }
}

/// A persisted cluster: everything `GetCluster`, `ListClusters` and the child creates
/// read.
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    /// Creation order, which is the order a list walks and what a `nextToken` resumes
    /// from.
    pub seq: u64,
    /// The generated `pcs_...` id.
    pub id: String,
    /// The caller-chosen `clusterName`.
    pub name: String,
    /// The ARN built by [`cluster_arn`].
    pub arn: String,
    /// A `ClusterStatus` member.
    pub status: String,
    /// When the record was created; never changes afterwards.
    pub created_at: DateTime,
    /// When the record last changed.
    pub modified_at: DateTime,
    /// The scheduler the request asked for.
    pub scheduler: Scheduler,
    /// A `Size` member.
    pub size: String,
    /// The control-plane networking the request asked for.
    pub networking: Networking,
    /// Slurm options, when the request carried any.
    pub slurm_configuration: Option<ClusterSlurmConfiguration>,
    /// Scheduler endpoints. Empty: the emulator serves no Slurm control plane, so it
    /// reports no endpoint for one.
    pub endpoints: Vec<Endpoint>,
    /// Provisioning errors. Empty on the path the emulator implements.
    pub error_info: Vec<ErrorInfo>,
    /// The tags the create request carried, keyed for a later `TagResource`.
    pub tags: HashMap<String, String>,
    /// The cluster's compute node groups, by generated id.
    pub compute_node_groups: HashMap<String, ComputeNodeGroupRecord>,
    /// The cluster's queues, by generated id.
    pub queues: HashMap<String, QueueRecord>,
    /// The transition the clock owes this cluster, if any.
    pub pending: Option<Pending>,
}

/// A persisted compute node group, stored under its parent cluster.
#[derive(Debug, Clone)]
pub struct ComputeNodeGroupRecord {
    /// Creation order within the cluster.
    pub seq: u64,
    /// The generated `pcs_...` id.
    pub id: String,
    /// The caller-chosen `computeNodeGroupName`.
    pub name: String,
    /// The ARN built by [`compute_node_group_arn`].
    pub arn: String,
    /// The id of the cluster this group belongs to.
    pub cluster_id: String,
    /// A `ComputeNodeGroupStatus` member.
    pub status: String,
    /// When the record was created.
    pub created_at: DateTime,
    /// When the record last changed.
    pub modified_at: DateTime,
    /// The AMI the request named, if any.
    pub ami_id: Option<String>,
    /// The subnets instances are provisioned in.
    pub subnet_ids: Vec<String>,
    /// A `PurchaseOption` member, when the request named one.
    pub purchase_option: Option<String>,
    /// The launch template the request named.
    pub custom_launch_template: CustomLaunchTemplate,
    /// The instance profile the request named.
    pub iam_instance_profile_arn: String,
    /// The scaling bounds the request asked for.
    pub scaling_configuration: ScalingConfiguration,
    /// The instance types the request listed.
    pub instance_configs: Vec<InstanceConfig>,
    /// Spot options, when the request carried any.
    pub spot_options: Option<SpotOptions>,
    /// Slurm options, when the request carried any.
    pub slurm_configuration: Option<ComputeNodeGroupSlurmConfiguration>,
    /// Lifecycle actions, when the request carried any.
    pub node_lifecycle_actions: Option<NodeLifecycleActions>,
    /// Provisioning errors. Empty on the path the emulator implements.
    pub error_info: Vec<ErrorInfo>,
    /// The tags the create request carried.
    pub tags: HashMap<String, String>,
    /// The transition the clock owes this group, if any.
    pub pending: Option<Pending>,
}

/// A persisted queue, stored under its parent cluster.
#[derive(Debug, Clone)]
pub struct QueueRecord {
    /// Creation order within the cluster.
    pub seq: u64,
    /// The generated `pcs_...` id.
    pub id: String,
    /// The caller-chosen `queueName`.
    pub name: String,
    /// The ARN built by [`queue_arn`].
    pub arn: String,
    /// The id of the cluster this queue belongs to.
    pub cluster_id: String,
    /// A `QueueStatus` member.
    pub status: String,
    /// When the record was created.
    pub created_at: DateTime,
    /// When the record last changed.
    pub modified_at: DateTime,
    /// The compute node groups the queue assigns jobs to.
    pub compute_node_group_configurations: Vec<ComputeNodeGroupConfiguration>,
    /// Slurm options, when the request carried any.
    pub slurm_configuration: Option<QueueSlurmConfiguration>,
    /// Provisioning errors. Empty on the path the emulator implements.
    pub error_info: Vec<ErrorInfo>,
    /// The tags the create request carried.
    pub tags: HashMap<String, String>,
    /// The transition the clock owes this queue, if any.
    pub pending: Option<Pending>,
}

impl ClusterRecord {
    /// The modelled `Cluster`, read off the stored record.
    pub fn to_api(&self) -> Cluster {
        Cluster {
            name: self.name.clone(),
            id: self.id.clone(),
            arn: self.arn.clone(),
            status: self.status.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            scheduler: self.scheduler.clone(),
            size: self.size.clone(),
            slurm_configuration: self.slurm_configuration.clone(),
            networking: self.networking.clone(),
            endpoints: non_empty(&self.endpoints),
            error_info: non_empty(&self.error_info),
        }
    }

    /// The modelled `ClusterSummary` `ListClusters` returns.
    pub fn to_summary(&self) -> ClusterSummary {
        ClusterSummary {
            name: self.name.clone(),
            id: self.id.clone(),
            arn: self.arn.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            status: self.status.clone(),
        }
    }

    /// The compute node group `identifier` names — its generated id or its name.
    pub fn compute_node_group(&self, identifier: &str) -> Option<&ComputeNodeGroupRecord> {
        self.compute_node_groups.get(identifier).or_else(|| {
            self.compute_node_groups
                .values()
                .find(|group| group.name == identifier)
        })
    }

    /// The queue `identifier` names — its generated id or its name.
    pub fn queue(&self, identifier: &str) -> Option<&QueueRecord> {
        self.queues
            .get(identifier)
            .or_else(|| self.queues.values().find(|queue| queue.name == identifier))
    }

    /// Apply every transition the clock owes this cluster and its children at `now`.
    /// `true` means the cluster itself is now owed removal.
    fn settle(&mut self, now: Instant) -> bool {
        self.compute_node_groups
            .retain(|_, group| !group.settle(now));
        self.queues.retain(|_, queue| !queue.settle(now));
        apply_pending(&mut self.pending, &mut self.status, &mut self.modified_at, now)
    }
}

impl ComputeNodeGroupRecord {
    /// The modelled `ComputeNodeGroup`, read off the stored record.
    pub fn to_api(&self) -> ComputeNodeGroup {
        ComputeNodeGroup {
            name: self.name.clone(),
            id: self.id.clone(),
            arn: self.arn.clone(),
            cluster_id: self.cluster_id.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            status: self.status.clone(),
            ami_id: self.ami_id.clone(),
            subnet_ids: self.subnet_ids.clone(),
            purchase_option: self.purchase_option.clone(),
            custom_launch_template: self.custom_launch_template.clone(),
            iam_instance_profile_arn: self.iam_instance_profile_arn.clone(),
            scaling_configuration: self.scaling_configuration.clone(),
            instance_configs: self.instance_configs.clone(),
            spot_options: self.spot_options.clone(),
            slurm_configuration: self.slurm_configuration.clone(),
            node_lifecycle_actions: self.node_lifecycle_actions.clone(),
            error_info: non_empty(&self.error_info),
        }
    }

    /// Apply the transition the clock owes this group at `now`; `true` means removal.
    fn settle(&mut self, now: Instant) -> bool {
        apply_pending(&mut self.pending, &mut self.status, &mut self.modified_at, now)
    }
}

impl QueueRecord {
    /// The modelled `Queue`, read off the stored record.
    pub fn to_api(&self) -> Queue {
        Queue {
            name: self.name.clone(),
            id: self.id.clone(),
            arn: self.arn.clone(),
            cluster_id: self.cluster_id.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            status: self.status.clone(),
            compute_node_group_configurations: self.compute_node_group_configurations.clone(),
            slurm_configuration: self.slurm_configuration.clone(),
            error_info: non_empty(&self.error_info),
        }
    }

    /// The modelled `QueueSummary` `ListQueues` returns.
    pub fn to_summary(&self) -> QueueSummary {
        QueueSummary {
            name: self.name.clone(),
            id: self.id.clone(),
            arn: self.arn.clone(),
            cluster_id: self.cluster_id.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            status: self.status.clone(),
        }
    }

    /// Apply the transition the clock owes this queue at `now`; `true` means removal.
    fn settle(&mut self, now: Instant) -> bool {
        apply_pending(&mut self.pending, &mut self.status, &mut self.modified_at, now)
    }
}

/// What a previous create with an `@idempotencyToken` produced, so a retry can be
/// answered with it rather than creating a second resource.
#[derive(Debug, Clone)]
pub struct RememberedCreate {
    /// The cluster the resource was created in; `None` for a cluster itself.
    pub cluster_id: Option<String>,
    /// The id the first call created.
    pub resource_id: String,
}

/// Per-service state for com-amazonaws-pcs-awsparallelcomputingservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPcsAwsparallelcomputingserviceState {
    /// Every cluster, by its generated id. Compute node groups and queues are nested
    /// inside their cluster, so deleting a cluster cannot orphan one. At most
    /// [`MAX_CLUSTERS`] entries; each cluster holds at most
    /// [`MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER`] groups and [`MAX_QUEUES_PER_CLUSTER`]
    /// queues, and every caller-sized collection inside a record is bounded (see the
    /// module docs).
    pub clusters: HashMap<String, ClusterRecord>,
    /// `clusterName` -> cluster id, so a `clusterIdentifier` resolves by name or by id
    /// and a duplicate name is a conflict. One entry per cluster, removed with it.
    pub cluster_ids_by_name: HashMap<String, String>,
    /// Source of both the generated ids and the creation-order sequence a list walks.
    pub next_seq: u64,
    /// `<operation>:<clientToken>` -> what the first call created, so repeating an
    /// `@idempotencyToken` create returns that call's result instead of creating a
    /// second resource. Bounded at [`MAX_REMEMBERED_CLIENT_TOKENS`] by the FIFO order
    /// below, and pruned when the cluster an entry points into is removed.
    pub client_tokens: HashMap<String, RememberedCreate>,
    /// The keys of [`Self::client_tokens`] in insertion order, so the oldest entry can
    /// be evicted once the index is full. Holds exactly the map's keys.
    pub client_token_order: VecDeque<String>,
}

impl ComAmazonawsPcsAwsparallelcomputingserviceState {
    /// Allocate the next creation sequence and the `pcs_...` id that goes with it.
    pub fn mint_id(&mut self) -> (u64, String) {
        self.next_seq += 1;
        let seq = self.next_seq;
        (seq, format!("{GENERATED_ID_PREFIX}{:016x}", scramble(seq)))
    }

    /// The cluster `identifier` names — its generated id or its `clusterName`, the two
    /// forms `ClusterIdentifier`'s pattern allows.
    pub fn cluster(&self, identifier: &str) -> Option<&ClusterRecord> {
        let id = self.cluster_id_for(identifier)?;
        self.clusters.get(&id)
    }

    /// [`Self::cluster`], mutably.
    pub fn cluster_mut(&mut self, identifier: &str) -> Option<&mut ClusterRecord> {
        let id = self.cluster_id_for(identifier)?;
        self.clusters.get_mut(&id)
    }

    /// The id of the cluster `identifier` names.
    pub fn cluster_id_for(&self, identifier: &str) -> Option<String> {
        if self.clusters.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.cluster_ids_by_name.get(identifier).cloned()
    }

    /// What a previous call with this `@idempotencyToken` created, if it is still
    /// remembered.
    pub fn remembered_create(
        &self,
        operation: &str,
        token: Option<&String>,
    ) -> Option<&RememberedCreate> {
        self.client_tokens.get(&client_token_key(operation, token?))
    }

    /// Remember what `operation` with `token` created, so a retry replays it.
    ///
    /// An existing entry is never overwritten — the token names the FIRST call's
    /// resource for as long as it is remembered — and the index is kept at
    /// [`MAX_REMEMBERED_CLIENT_TOKENS`] entries by evicting the oldest.
    pub fn remember_create(
        &mut self,
        operation: &str,
        token: Option<&String>,
        cluster_id: Option<&str>,
        resource_id: &str,
    ) {
        let Some(token) = token else { return };
        let key = client_token_key(operation, token);
        if self.client_tokens.contains_key(&key) {
            return;
        }
        self.client_tokens.insert(
            key.clone(),
            RememberedCreate {
                cluster_id: cluster_id.map(str::to_string),
                resource_id: resource_id.to_string(),
            },
        );
        self.client_token_order.push_back(key);
        while self.client_token_order.len() > MAX_REMEMBERED_CLIENT_TOKENS {
            if let Some(oldest) = self.client_token_order.pop_front() {
                self.client_tokens.remove(&oldest);
            }
        }
    }

    /// Drop every remembered create that points at `cluster_id` or at a resource inside
    /// it, so removing a cluster releases its share of the index.
    fn forget_creates_for_cluster(&mut self, cluster_id: &str) {
        let Self {
            client_tokens,
            client_token_order,
            ..
        } = self;
        client_tokens.retain(|_, remembered| {
            remembered.resource_id != cluster_id
                && remembered.cluster_id.as_deref() != Some(cluster_id)
        });
        client_token_order.retain(|key| client_tokens.contains_key(key));
    }

    /// Apply every lifecycle transition the clock owes at this instant. Called at the
    /// top of every handler, including the reporting ones: it only materialises a change
    /// that was already due, so a read never moves a status the clock had not reached.
    pub fn settle(&mut self) {
        let now = Instant::now();
        let removable: Vec<String> = self
            .clusters
            .iter_mut()
            .filter_map(|(id, cluster)| cluster.settle(now).then(|| id.clone()))
            .collect();
        for id in removable {
            if let Some(cluster) = self.clusters.remove(&id) {
                if self.cluster_ids_by_name.get(&cluster.name) == Some(&cluster.id) {
                    self.cluster_ids_by_name.remove(&cluster.name);
                }
            }
            self.forget_creates_for_cluster(&id);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPcsAwsparallelcomputingserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPcsAwsparallelcomputingserviceState>)
}

/// The current wall clock, as the `date-time` `Timestamp` the model declares.
pub fn now_date_time() -> DateTime {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// `None` for an empty list, so an absent member stays absent on the wire.
fn non_empty<T: Clone>(items: &[T]) -> Option<Vec<T>> {
    (!items.is_empty()).then(|| items.to_vec())
}

/// Apply the transition the clock owes one record at `now`; `true` means the record is
/// owed removal and its owner should drop it.
///
/// Shared by all three record types so a new [`PendingChange`] variant cannot end up
/// handled three different ways.
fn apply_pending(
    pending: &mut Option<Pending>,
    status: &mut String,
    modified_at: &mut DateTime,
    now: Instant,
) -> bool {
    match *pending {
        Some(owed) if owed.is_due(now) => match owed.change {
            PendingChange::Activate => {
                *status = STATUS_ACTIVE.to_string();
                *modified_at = now_date_time();
                *pending = None;
                false
            }
            PendingChange::Remove => true,
        },
        _ => false,
    }
}

/// The key an idempotency token is remembered under, scoped by operation so two
/// operations cannot collide on one token.
fn client_token_key(operation: &str, token: &str) -> String {
    format!("{operation}:{token}")
}

/// SplitMix64's finaliser. A BIJECTION on `u64` — every step (an odd multiply, an
/// xor-shift) is invertible — so distinct creation sequences always produce distinct
/// ids. That makes the generated id opaque, the way PCS's own ids are, without a random
/// source or a collision check.
///
/// Deliberately a permutation of the creation index rather than a hash or a UUID: the
/// uniqueness has to be guaranteed, not merely probable, and `pcs_[a-zA-Z0-9]+` leaves
/// no room for a 36-character UUID. A hash crate would give neither property.
fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The region the request was signed for. PCS resources are regional, so every ARN the
/// service mints carries it; a request that carries no SigV4 credential scope gets
/// [`DEFAULT_REGION`].
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:pcs:<region>:<account>:cluster/<cluster id>`.
pub fn cluster_arn(region: &str, cluster_id: &str) -> String {
    ::engine::emulator_arn("pcs", region, &format!("cluster/{cluster_id}"))
}

/// `arn:aws:pcs:<region>:<account>:cluster/<cluster id>/computenodegroup/<group id>`.
pub fn compute_node_group_arn(region: &str, cluster_id: &str, group_id: &str) -> String {
    ::engine::emulator_arn(
        "pcs",
        region,
        &format!("cluster/{cluster_id}/computenodegroup/{group_id}"),
    )
}

/// `arn:aws:pcs:<region>:<account>:cluster/<cluster id>/queue/<queue id>`.
pub fn queue_arn(region: &str, cluster_id: &str, queue_id: &str) -> String {
    ::engine::emulator_arn(
        "pcs",
        region,
        &format!("cluster/{cluster_id}/queue/{queue_id}"),
    )
}

/// Why the service refuses this resource name, or `None` when it accepts it.
///
/// `clusterName`, `computeNodeGroupName` and `queueName` all carry the same
/// `@pattern`, `^(?!pcs_)^[A-Za-z][A-Za-z0-9-]+$`: a leading ASCII letter, then only
/// ASCII letters, digits and hyphens — which also rules out the `pcs_` prefix the
/// service reserves for the ids it generates, because `_` is not in the class.
///
/// The rule is enforced HERE rather than at the boundary because that regex carries a
/// negative lookahead, which the boundary validator's regex engine cannot compile, and
/// a pattern it cannot compile is treated as matching (see
/// `aws_smithy_schema_server::constraints::pattern_matches`). The `@length` bounds on
/// the same members compile and are enforced at the boundary, so they are not repeated.
pub fn resource_name_violation(member: &str, value: &str) -> Option<String> {
    let mut chars = value.chars();
    let starts_with_letter = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest: Vec<char> = chars.collect();
    // `[A-Za-z0-9-]+` is one-or-more, so the remainder must be non-empty as well as
    // inside the class; the `@length(min: 3)` the boundary enforces makes that
    // unreachable through the wire, but the helper stands on its own.
    let rest_is_allowed = !rest.is_empty()
        && rest
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == '-');
    if starts_with_letter && rest_is_allowed {
        return None;
    }
    Some(format!(
        "{member} must satisfy regular expression pattern: ^(?!pcs_)^[A-Za-z][A-Za-z0-9-]+$"
    ))
}

/// The modelled `ValidationException` for a member the service refuses: reason
/// `fieldValidationFailed` (a `ValidationExceptionReason` member) and a `fieldList`
/// naming the member, so a client can fix the request from the error alone.
pub fn field_validation(member: &str, message: String) -> ValidationException {
    ValidationException {
        message: message.clone(),
        reason: "fieldValidationFailed".to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message,
        }]),
    }
}

/// Why the service refuses a caller-sized list a create would PERSIST, or `None` when
/// it accepts it.
///
/// The model bounds none of these lists, so this is the ceiling that keeps one request
/// from sizing one stored record arbitrarily — see [`MAX_STORED_LIST_ENTRIES`].
pub fn stored_list_violation(member: &str, entries: usize) -> Option<String> {
    (entries > MAX_STORED_LIST_ENTRIES).then(|| {
        format!("{member} must carry at most {MAX_STORED_LIST_ENTRIES} entries, not {entries}")
    })
}

/// The modelled `ServiceQuotaExceededException` (HTTP 402) for a create that would take
/// a store past its ceiling. `quota_code` is left absent rather than invented: the
/// emulator stands in for the quota, it does not reproduce Service Quotas' codes.
pub fn quota_exceeded(resource_type: &str, limit: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "the maximum number of {resource_type} resources ({limit}) has been reached"
        ),
        service_code: "pcs".to_string(),
        resource_id: None,
        resource_type: Some(resource_type.to_string()),
        quota_code: None,
    }
}

/// The page size a list serves for this `maxResults`.
///
/// `MaxResults` is `@default(10)` with `@range(min: 1, max: 100)`: the boundary refuses
/// anything outside the range, and a request that omits the member asks for the default,
/// which is also what a zero (an absent member that did not pick the default up) means.
pub fn page_size(max_results: i32) -> usize {
    match usize::try_from(max_results) {
        Ok(size @ 1..=100) => size,
        _ => DEFAULT_PAGE_SIZE,
    }
}

/// The listing kind [`ListClusters`](crate::ops::list_clusters)' page tokens carry.
pub const PAGE_KIND_CLUSTERS: &str = "clusters";

/// The listing kind [`ListQueues`](crate::ops::list_queues)' page tokens carry, for one
/// cluster. Scoped by cluster id, so a token issued for one cluster's queues cannot
/// resume another cluster's.
pub fn page_kind_queues(cluster_id: &str) -> String {
    format!("queues:{cluster_id}")
}

/// The creation sequence a `nextToken` for the listing `kind` resumes after, or `None`
/// when the token is not one THAT listing issued.
///
/// The kind is part of the token because the creation sequence is service-wide: a bare
/// sequence would decode in every listing, so a token `ListClusters` handed out would
/// silently resume a `ListQueues` page (and one cluster's token another cluster's
/// queues) instead of being refused.
pub fn decode_page_token(kind: &str, token: &str) -> Option<u64> {
    let bytes = hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    decoded.strip_prefix(&format!("{kind}#"))?.parse().ok()
}

/// The opaque `nextToken` that resumes a listing of `kind` after sequence `seq`.
fn encode_page_token(kind: &str, seq: u64) -> String {
    hex::encode(format!("{kind}#{seq}"))
}

/// One page of a creation-ordered list.
///
/// `kind` is the listing the token belongs to (see [`decode_page_token`]), `rows` every
/// candidate paired with its creation sequence, `after` the sequence the previous page
/// ended on, and `size` the page size. The returned token resumes after the last row of
/// this page and is absent on the final page.
pub fn paginate<T>(
    kind: &str,
    mut rows: Vec<(u64, T)>,
    after: Option<u64>,
    size: usize,
) -> (Vec<T>, Option<String>) {
    rows.sort_by_key(|(seq, _)| *seq);
    let mut remaining = rows
        .into_iter()
        .filter(|(seq, _)| after.is_none_or(|last| *seq > last));
    let page: Vec<(u64, T)> = remaining.by_ref().take(size).collect();
    let token = match (remaining.next(), page.last()) {
        (Some(_), Some((seq, _))) => Some(encode_page_token(kind, *seq)),
        _ => None,
    };
    (page.into_iter().map(|(_, row)| row).collect(), token)
}

impl From<SchedulerRequest> for Scheduler {
    fn from(request: SchedulerRequest) -> Self {
        Self {
            r#type: request.r#type,
            version: request.version,
        }
    }
}

impl From<NetworkingRequest> for Networking {
    fn from(request: NetworkingRequest) -> Self {
        Self {
            subnet_ids: request.subnet_ids,
            security_group_ids: request.security_group_ids,
            network_type: request.network_type,
        }
    }
}

impl From<AccountingRequest> for Accounting {
    fn from(request: AccountingRequest) -> Self {
        Self {
            default_purge_time_in_days: request.default_purge_time_in_days,
            mode: request.mode,
        }
    }
}

impl From<SlurmRestRequest> for SlurmRest {
    fn from(request: SlurmRestRequest) -> Self {
        Self { mode: request.mode }
    }
}

impl From<ClusterSlurmConfigurationRequest> for ClusterSlurmConfiguration {
    fn from(request: ClusterSlurmConfigurationRequest) -> Self {
        Self {
            scale_down_idle_time_in_seconds: request.scale_down_idle_time_in_seconds,
            slurm_custom_settings: request.slurm_custom_settings,
            slurmdbd_custom_settings: request.slurmdbd_custom_settings,
            cgroup_custom_settings: request.cgroup_custom_settings,
            // PCS reports the Secrets Manager secret it generated for the Slurm auth key
            // and for JWT auth. The emulator generates no secret, so it reports neither
            // rather than inventing an ARN.
            auth_key: None,
            jwt_auth: None,
            accounting: request.accounting.map(Accounting::from),
            slurm_rest: request.slurm_rest.map(SlurmRest::from),
        }
    }
}

impl From<ScalingConfigurationRequest> for ScalingConfiguration {
    fn from(request: ScalingConfigurationRequest) -> Self {
        Self {
            min_instance_count: request.min_instance_count,
            max_instance_count: request.max_instance_count,
        }
    }
}

impl From<ComputeNodeGroupSlurmConfigurationRequest> for ComputeNodeGroupSlurmConfiguration {
    fn from(request: ComputeNodeGroupSlurmConfigurationRequest) -> Self {
        Self {
            scale_down_idle_time_in_seconds: request.scale_down_idle_time_in_seconds,
            slurm_custom_settings: request.slurm_custom_settings,
            gres_custom_settings: request.gres_custom_settings,
        }
    }
}

impl From<NodeLifecycleActionsRequest> for NodeLifecycleActions {
    fn from(request: NodeLifecycleActionsRequest) -> Self {
        Self {
            stages: request.stages,
            script_caching_policy: request.script_caching_policy,
        }
    }
}

impl From<QueueSlurmConfigurationRequest> for QueueSlurmConfiguration {
    fn from(request: QueueSlurmConfigurationRequest) -> Self {
        Self {
            slurm_custom_settings: request.slurm_custom_settings,
        }
    }
}
