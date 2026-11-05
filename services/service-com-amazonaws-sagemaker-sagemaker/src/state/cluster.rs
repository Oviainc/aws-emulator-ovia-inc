// SPDX-License-Identifier: CC0-1.0

//! The HyperPod cluster slice of this service's state.
//!
//! One file for the whole slice: the [`Cluster`] record `CreateCluster` writes with its
//! quota, its clock-owed span and its id minting, and what a reader needs on top of it:
//!
//! * `ComAmazonawsSagemakerSagemakerState::cluster_key`, the one lookup that resolves
//!   the `ClusterNameOrArn` every cluster operation takes to the store key, so no
//!   handler re-implements it;
//! * the `*Details` / `*Output` projections `DescribeCluster` returns, built from the
//!   instance-group SPECIFICATIONS the create stored and the cluster's current status —
//!   so one stored record answers both the "what you asked for" and the "what is
//!   realised" halves of each detail shape, and a later reader cannot disagree with an
//!   earlier one.
//!
//! The module sits under `src/state/` because it holds and projects state, and is
//! declared in `src/state.rs` beside the other state slices of this service.
//!
//! No PROJECTION here writes: the `Creating` -> `InService` transition is
//! [`Cluster::settle_status`]'s, materialising only what the clock already owed.

use super::*;
use crate::state::{ComAmazonawsSagemakerSagemakerState, STATUS_IN_SERVICE};
use std::time::SystemTime;
use crate::ops::types::{
    ClusterAutoPatchConfigDetails, ClusterAutoScalingConfigOutput, ClusterInstanceGroupDetails,
    ClusterInstanceRequirementDetails, ClusterKubernetesConfigDetails,
    ClusterNetworkInterfaceDetails, ClusterPatchScheduleDetails,
    ClusterRestrictedInstanceGroupDetails, ClusterRestrictedInstanceGroupsConfigOutput,
    ClusterSharedEnvironmentConfigDetails, ClusterSlurmConfigDetails, EnvironmentConfigDetails,
};

/// `value`, but only once the cluster has reached `InService`.
///
/// Every `Current*` member of a `*Details` shape reports what is REALISED, against the
/// `Desired*` member that reports what the request declared ("you describe what you want
/// ... and the cluster works to reconcile the actual state with the declared state", per
/// `ClusterInstanceGroupSpecification$KubernetesConfig`). Nothing is realised while the
/// cluster is still provisioning, so every such member is absent until then.
fn realised<T>(in_service: bool, value: Option<T>) -> Option<T> {
    if in_service { value } else { None }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// The store key of the cluster a `ClusterNameOrArn` names, or `None` when no
    /// cluster answers to it.
    ///
    /// Every cluster operation takes `ClusterNameOrArn` ("the string name or the Amazon
    /// Resource Name (ARN)"), so both forms resolve to the one record here rather than
    /// in each handler. A `ClusterName` cannot contain `:`, so the two forms never
    /// collide. The ARN form is matched in FULL rather than through
    /// `crate::state::name_from_arn_or_name`, because a cluster ARN's last segment is
    /// the minted id and not the store key — so an ARN naming another region, account or
    /// partition does not resolve here.
    ///
    /// A cluster whose deletion the clock already completed resolves to nothing, as it
    /// does from every other cluster lookup: past that instant the cluster is gone
    /// whether or not the record has been released yet ([`Cluster::is_released`]).
    pub fn cluster_key(&self, name_or_arn: &str) -> Option<String> {
        let now = SystemTime::now();
        self.clusters
            .get(name_or_arn)
            .or_else(|| {
                self.clusters
                    .values()
                    .find(|cluster| cluster.arn == name_or_arn)
            })
            .filter(|cluster| !cluster.is_released(now))
            .map(|cluster| cluster.name.clone())
    }
}

impl Cluster {
    /// Whether the cluster has finished provisioning, i.e. its instances are realised.
    pub fn is_in_service(&self) -> bool {
        self.status == STATUS_IN_SERVICE
    }

    /// The stored instance-group specifications as the `ClusterInstanceGroupDetails`
    /// every reader returns, in the order the request listed them.
    pub fn instance_group_details(&self) -> Vec<ClusterInstanceGroupDetails> {
        let in_service = self.is_in_service();
        self.instance_groups
            .iter()
            .map(|spec| ClusterInstanceGroupDetails {
                // The instances exist only once provisioning is done; the target is
                // what the request asked for, from the moment it was accepted. A count
                // realises as 0 rather than through `realised()`: `CurrentCount` is "the
                // number of instances that are currently in the instance group", and a
                // provisioning group genuinely has none, where an absent member would
                // read as "unknown".
                current_count: Some(if in_service { spec.instance_count } else { 0 }),
                target_count: Some(spec.instance_count),
                min_count: spec.min_instance_count,
                instance_group_name: Some(spec.instance_group_name.clone()),
                instance_type: spec.instance_type.clone(),
                instance_requirements: spec.instance_requirements.as_ref().map(|requirements| {
                    ClusterInstanceRequirementDetails {
                        current_instance_types: realised(
                            in_service,
                            Some(requirements.instance_types.clone()),
                        ),
                        desired_instance_types: Some(requirements.instance_types.clone()),
                    }
                }),
                // Reported only for a group whose instance types the service itself
                // chose per instance; this emulator provisions exactly what was asked
                // for, so there is no second breakdown to report.
                instance_type_details: None,
                life_cycle_config: spec.life_cycle_config.clone(),
                execution_role: Some(spec.execution_role.clone()),
                threads_per_core: spec.threads_per_core,
                instance_storage_configs: spec.instance_storage_configs.clone(),
                on_start_deep_health_checks: spec.on_start_deep_health_checks.clone(),
                // A group follows its cluster: `InstanceGroupStatus` spells both
                // members exactly as `ClusterStatus` does.
                status: Some(self.status.clone()),
                training_plan_arn: spec.training_plan_arn.clone(),
                // This emulator models no training plan, so a group that names one has
                // no plan status to report.
                training_plan_status: None,
                override_vpc_config: spec.override_vpc_config.clone(),
                scheduled_update_config: spec.scheduled_update_config.clone(),
                auto_patch_config: spec.auto_patch_config.as_ref().map(|config| {
                    let schedule =
                        config
                            .patch_schedule
                            .as_ref()
                            .map(|schedule| ClusterPatchScheduleDetails {
                                next_patch_date: schedule.next_patch_date,
                            });
                    ClusterAutoPatchConfigDetails {
                        patching_strategy: Some(config.patching_strategy.clone()),
                        current_patch_schedule: realised(in_service, schedule.clone()),
                        desired_patch_schedule: schedule,
                        deployment_config: config.deployment_config.clone(),
                    }
                }),
                current_image_id: realised(in_service, spec.image_id.clone()),
                desired_image_id: spec.image_id.clone(),
                current_image_release_version: realised(
                    in_service,
                    spec.image_release_version.clone(),
                ),
                desired_image_release_version: spec.image_release_version.clone(),
                // Nothing in this emulator publishes a newer AMI than the one a group
                // was created with, so there is no version comparison to report.
                image_version_status: None,
                // Set only while an operation (a scaling one, say) is in flight on the
                // group; nothing here runs one.
                active_operations: None,
                kubernetes_config: spec.kubernetes_config.as_ref().map(|config| {
                    ClusterKubernetesConfigDetails {
                        current_labels: realised(in_service, config.labels.clone()),
                        desired_labels: config.labels.clone(),
                        current_taints: realised(in_service, config.taints.clone()),
                        desired_taints: config.taints.clone(),
                    }
                }),
                capacity_requirements: spec.capacity_requirements.clone(),
                // Both belong to a software update; this cluster has never had one
                // requested, so neither has a value yet.
                target_state_count: None,
                software_update_status: None,
                active_software_update_config: None,
                slurm_config: spec
                    .slurm_config
                    .as_ref()
                    .map(|config| ClusterSlurmConfigDetails {
                        node_type: config.node_type.clone(),
                        partition_names: config.partition_names.clone(),
                    }),
                network_interface: spec.network_interface.as_ref().map(|interface| {
                    ClusterNetworkInterfaceDetails {
                        interface_type: interface.interface_type.clone(),
                    }
                }),
            })
            .collect()
    }

    /// The stored restricted instance groups as the details shape readers return.
    pub fn restricted_instance_group_details(&self) -> Vec<ClusterRestrictedInstanceGroupDetails> {
        let in_service = self.is_in_service();
        self.restricted_instance_groups
            .iter()
            .map(|spec| ClusterRestrictedInstanceGroupDetails {
                // Counts realise as 0 while provisioning, for the reason given on
                // `Cluster::instance_group_details`.
                current_count: Some(if in_service { spec.instance_count } else { 0 }),
                target_count: Some(spec.instance_count),
                instance_group_name: Some(spec.instance_group_name.clone()),
                instance_type: Some(spec.instance_type.clone()),
                execution_role: Some(spec.execution_role.clone()),
                threads_per_core: spec.threads_per_core,
                instance_storage_configs: spec.instance_storage_configs.clone(),
                on_start_deep_health_checks: spec.on_start_deep_health_checks.clone(),
                status: Some(self.status.clone()),
                training_plan_arn: spec.training_plan_arn.clone(),
                training_plan_status: None,
                override_vpc_config: spec.override_vpc_config.clone(),
                scheduled_update_config: spec.scheduled_update_config.clone(),
                environment_config: spec.environment_config.as_ref().map(|config| {
                    EnvironmentConfigDetails {
                        f_sx_lustre_config: config.f_sx_lustre_config.clone(),
                        // The RIG's output path is chosen by the service when the
                        // environment is materialised; this emulator runs no
                        // environment, so it reports none.
                        s3_output_path: None,
                    }
                }),
            })
            .collect()
    }

    /// The stored restricted-instance-group configuration as the output shape readers
    /// return, split into the declared and the realised halves.
    pub fn restricted_instance_groups_config_output(
        &self,
    ) -> Option<ClusterRestrictedInstanceGroupsConfigOutput> {
        let in_service = self.is_in_service();
        self.restricted_instance_groups_config
            .as_ref()
            .map(|config| ClusterRestrictedInstanceGroupsConfigOutput {
                shared_environment_config: ClusterSharedEnvironmentConfigDetails {
                    current_f_sx_lustre_config: realised(
                        in_service,
                        Some(config.shared_environment_config.f_sx_lustre_config.clone()),
                    ),
                    desired_f_sx_lustre_config: Some(
                        config.shared_environment_config.f_sx_lustre_config.clone(),
                    ),
                    current_f_sx_lustre_deletion_policy: realised(
                        in_service,
                        Some(
                            config
                                .shared_environment_config
                                .f_sx_lustre_deletion_policy
                                .clone(),
                        ),
                    ),
                    desired_f_sx_lustre_deletion_policy: Some(
                        config
                            .shared_environment_config
                            .f_sx_lustre_deletion_policy
                            .clone(),
                    ),
                },
            })
    }

    /// The stored autoscaling configuration as the output shape readers return. Its
    /// `Status` follows the cluster: `ClusterAutoScalingStatus` spells `Creating` and
    /// `InService` exactly as `ClusterStatus` does, and the autoscaler comes up with
    /// the cluster it scales.
    pub fn auto_scaling_output(&self) -> Option<ClusterAutoScalingConfigOutput> {
        self.auto_scaling
            .as_ref()
            .map(|config| ClusterAutoScalingConfigOutput {
                mode: config.mode.clone(),
                auto_scaler_type: config.auto_scaler_type.clone(),
                status: self.status.clone(),
                // Nothing in this emulator fails an autoscaler, so there is no failure
                // to report; the cluster's own `FailureMessage` is absent for the same
                // reason.
                failure_message: None,
            })
    }
}

/// How long after `CreateCluster` the clock owes the `Creating` -> `InService`
/// transition of a HyperPod cluster. Same compressed, never-zero span as
/// [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that lists clusters straight after creating
/// one still observes `Creating`, one that waits observes `InService`. Real HyperPod
/// provisioning takes many minutes; the span is the emulator's own choice.
pub const CLUSTER_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// How long after `DeleteCluster` the clock owes the `Deleting` -> gone transition of a
/// HyperPod cluster. The same compressed, never-zero span as
/// [`CLUSTER_IN_SERVICE_AFTER`]: a caller that describes the cluster straight after
/// asking for its deletion still observes `Deleting`, one that waits observes the
/// cluster gone. Real HyperPod teardown takes many minutes; the span is the emulator's
/// own choice.
pub const CLUSTER_RELEASED_AFTER: Duration = CLUSTER_IN_SERVICE_AFTER;

/// `ClusterStatus`'s terminal failure member. Nothing in this emulator fails a cluster,
/// so no record carries it — it is named because a delete is admitted for a cluster in
/// it as well as for one `InService` ([`Cluster::accepts_deletion`]), and a handler must
/// not compare a bare string literal.
pub const CLUSTER_STATUS_FAILED: &str = "Failed";

/// See [`MODEL_QUOTA`]; bounds the cluster store, and is released by `DeleteCluster`.
pub const CLUSTER_QUOTA: usize = 1_000;

/// `ClusterSortBy`'s documented default ("The field by which to sort results. The
/// default value is `CREATION_TIME`." — `ListClusters`' member doc). `ListClusters` has
/// its own default and its own enum tokens, which is why it does not share
/// [`DEFAULT_SORT_BY`].
pub const CLUSTER_SORT_BY_CREATION_TIME: &str = "CREATION_TIME";
/// `ClusterSortBy`'s other member.
pub const CLUSTER_SORT_BY_NAME: &str = "NAME";
/// `SortOrder`'s documented default for `ListClusters` ("The sort order for results. The
/// default value is `Ascending`.") — the opposite of [`DEFAULT_SORT_ORDER`], which is
/// what `ListModels` documents. The descending token is [`ORDER_DESCENDING`].
pub const CLUSTER_DEFAULT_SORT_ORDER: &str = ORDER_ASCENDING;

/// A SageMaker HyperPod cluster created by `CreateCluster`, as `ListClusters`
/// summarises it and `DescribeCluster` replays it.
///
/// The whole creation spec is stored, not only the members `ListClusters` reports: the
/// record is the resource, so the readers of the rest (`DescribeCluster`,
/// `ListClusterNodes`) read it from here rather than re-deriving it.
///
/// `in_service_at` is the instant the clock owes the `Creating` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct Cluster {
    pub name: String,
    /// The 12-character id the `ClusterArn` pattern carries
    /// (`…:cluster/[a-z0-9]{12}`), minted once at create time.
    pub id: String,
    pub arn: String,
    pub status: String,
    pub instance_groups: Vec<crate::ops::types::ClusterInstanceGroupSpecification>,
    pub restricted_instance_groups:
        Vec<crate::ops::types::ClusterRestrictedInstanceGroupSpecification>,
    pub restricted_instance_groups_config:
        Option<crate::ops::types::ClusterRestrictedInstanceGroupsConfig>,
    pub vpc_config: Option<VpcConfig>,
    pub orchestrator: Option<crate::ops::types::ClusterOrchestrator>,
    pub node_recovery: Option<String>,
    pub tiered_storage_config: Option<crate::ops::types::ClusterTieredStorageConfig>,
    pub node_provisioning_mode: Option<String>,
    pub cluster_role: Option<String>,
    pub auto_scaling: Option<crate::ops::types::ClusterAutoScalingConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub in_service_at: SystemTime,
    /// The instant the clock owes this cluster's deletion, set by
    /// [`Cluster::begin_deletion`]; `None` while no `DeleteCluster` has asked for one.
    /// Past it the cluster is gone for every caller ([`Cluster::is_released`]), whether
    /// or not a sweep has released the record yet.
    pub removal_due_at: Option<SystemTime>,
    /// The cluster's nodes, keyed by `NodeLogicalId` — the instances its groups were
    /// created asking for plus the ones `BatchAddClusterNodes` added, which
    /// `ListClusterNodes` reports, `DescribeClusterNode` replays and
    /// `AttachClusterNodeVolume` attaches storage inside. A cluster still `Creating`
    /// holds none: its groups' instances are minted when it reaches `InService`, by
    /// [`Cluster::settle_status`].
    pub nodes: BTreeMap<String, ClusterNode>,
    /// What a `BatchAddClusterNodes` `ClientToken` replays: the outcome of the first call
    /// carrying that token. Keyed by the token.
    pub node_additions: BTreeMap<String, NodeAdditionRecord>,
}

impl Cluster {
    /// Whether a delete may begin on this cluster: its status is a settled one.
    ///
    /// A cluster the service is already moving — provisioning it, updating it, rolling
    /// an update back, or tearing it down — cannot take a second change at the same
    /// time, which is the conflict `DeleteCluster` models `ConflictException` for. The
    /// settled statuses are `InService` and [`CLUSTER_STATUS_FAILED`].
    pub fn accepts_deletion(&self) -> bool {
        self.status == STATUS_IN_SERVICE || self.status == CLUSTER_STATUS_FAILED
    }

    /// Start the deletion `DeleteCluster` asked for: the cluster reports `Deleting`, and
    /// the clock owes its release [`CLUSTER_RELEASED_AFTER`] later.
    ///
    /// The record is NOT dropped here: SageMaker answers a cluster in `ClusterStatus`'s
    /// `Deleting` until the teardown finishes, so the transient status is observable and
    /// the removal is the clock's — stored as an instant and never slept on.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = STATUS_DELETING.to_string();
        self.removal_due_at = Some(now + CLUSTER_RELEASED_AFTER);
    }

    /// Whether a `DeleteCluster` has asked for this cluster's deletion, i.e. it is
    /// `Deleting` and on its way out.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the deletion the clock owed has completed, i.e. the cluster is gone.
    ///
    /// A lookup answers `None` for such a cluster and a listing leaves it out, so no
    /// caller sees a cluster whose teardown was already due — the record itself is
    /// dropped by [`ComAmazonawsSagemakerSagemakerState::release_deleted_clusters`],
    /// which frees the name and the quota slot it held.
    pub fn is_released(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due| now >= due)
    }

    /// Materialise the `Creating` -> `InService` transition the clock already owes.
    ///
    /// The one write a reporting operation may make: the transition was due at
    /// `in_service_at` whether or not anybody listed the cluster, so the observer makes
    /// no difference to the result. Answers whether it happened on this call.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == STATUS_CREATING && now >= self.in_service_at {
            self.status = STATUS_IN_SERVICE.to_string();
            self.provision_group_instances(now);
            return true;
        }
        false
    }

    /// Bring every instance group up to the number of instances it declares, minting the
    /// ones it is short of as `Running` nodes of that group.
    ///
    /// `InstanceCount` is "the number of instances to add to the instance group", so a
    /// group that declares N holds N of them once the cluster is provisioned — the same N
    /// that [`Cluster::instance_group_details`] reports as realised in `CurrentCount`, so
    /// the cluster's counts and its node list cannot disagree. They come up WITH the
    /// cluster, which is why this runs at the instant it reaches `InService` and never
    /// before: a caller listing nodes straight after the create sees none, and one that
    /// waits sees the instances the groups asked for.
    ///
    /// A group is short by however many nodes it does not already hold, so the nodes a
    /// call added to it (which raised its count by the same amount) are not minted twice.
    /// The whole cluster stays inside [`CLUSTER_NODE_QUOTA`], the bound every other node
    /// path honours.
    pub(super) fn provision_group_instances(&mut self, now: SystemTime) {
        // The groups' settings are read off first, so the nodes can be written while
        // iterating them. A restricted group's instances are instances of this cluster
        // too, and `ListClusterNodes` reports "the list of instances ... in a SageMaker
        // HyperPod cluster".
        let declared: Vec<(String, i32, Option<String>, Option<i32>)> = self
            .instance_groups
            .iter()
            .map(|group| {
                (
                    group.instance_group_name.clone(),
                    group.instance_count,
                    instance_group_types(group).first().cloned(),
                    group.threads_per_core,
                )
            })
            .chain(self.restricted_instance_groups.iter().map(|group| {
                (
                    group.instance_group_name.clone(),
                    group.instance_count,
                    Some(group.instance_type.clone()),
                    group.threads_per_core,
                )
            }))
            .collect();

        for (group_name, instance_count, instance_type, threads_per_core) in declared {
            let held = self
                .nodes
                .values()
                .filter(|node| node.instance_group_name == group_name)
                .count();
            let declared_count = usize::try_from(instance_count).unwrap_or_default();
            for _ in held..declared_count {
                if self.nodes.len() >= CLUSTER_NODE_QUOTA {
                    return;
                }
                let logical_id = mint_node_logical_id();
                self.nodes.insert(
                    logical_id.clone(),
                    ClusterNode {
                        logical_id: logical_id.clone(),
                        // An instance that is up has its id, the same one the
                        // `Pending` -> `Running` transition mints for an added node.
                        instance_id: Some(mint_instance_id()),
                        instance_group_name: group_name.clone(),
                        instance_type: instance_type.clone(),
                        status: NODE_STATUS_RUNNING.to_string(),
                        // A group's zones follow from the subnets of its VPC
                        // configuration, which this emulator does not resolve.
                        availability_zone: None,
                        threads_per_core,
                        launch_time: now,
                        running_at: now,
                        // Nothing has asked for this node's hardware to be replaced.
                        replacement_due_at: None,
                        volume_attachments: BTreeMap::new(),
                    },
                );
            }
        }
    }

    /// The training plans this cluster's capacity is reserved against, in first-seen
    /// order with duplicates dropped.
    ///
    /// `ClusterSummary::TrainingPlanArns` is "a list of ARNs of the training plans
    /// associated with this cluster", and a cluster is associated with a plan through
    /// the `TrainingPlanArn` of an instance group it was created with — so the list is
    /// computed from the stored groups rather than stored a second time.
    pub fn training_plan_arns(&self) -> Vec<String> {
        let mut arns: Vec<String> = Vec::new();
        let from_groups = self
            .instance_groups
            .iter()
            .map(|group| group.training_plan_arn.as_ref())
            .chain(
                self.restricted_instance_groups
                    .iter()
                    .map(|group| group.training_plan_arn.as_ref()),
            );
        for arn in from_groups.flatten() {
            if !arns.iter().any(|seen| seen == arn) {
                arns.push(arn.clone());
            }
        }
        arns
    }
}

/// The instance types an instance group admits: the one its specification names, or the
/// list its `InstanceRequirements` names.
///
/// The model makes the two mutually exclusive ("The `InstanceType` and
/// `InstanceRequirements` properties are mutually exclusive" —
/// `ClusterInstanceRequirements`' own doc), and a group may name neither, which is why
/// this answers a list that can be empty: a group that names no type admits any.
pub fn instance_group_types(
    group: &crate::ops::types::ClusterInstanceGroupSpecification,
) -> Vec<String> {
    if let Some(instance_type) = &group.instance_type {
        return vec![instance_type.clone()];
    }
    group
        .instance_requirements
        .as_ref()
        .map(|requirements| requirements.instance_types.clone())
        .unwrap_or_default()
}

/// Mint the id a `ClusterArn` carries: the 12 lowercase hex digits
/// [`mint_short_id`] produces, which is exactly the `[a-z0-9]{12}` the
/// modelled `ClusterArn` pattern admits.
///
/// One minter for the service: the entropy, its bounds and the reasons it is not a
/// secret are stated once, on [`mint_opaque_resource_id`]. `CreateCluster` still
/// re-mints on a collision rather than assuming uniqueness.
pub fn mint_cluster_id() -> String {
    mint_short_id()
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another cluster fits, i.e. the store is below [`CLUSTER_QUOTA`]. A second
    /// inherent `impl` block, beside this slice's own types rather than in the one above.
    pub fn clusters_below_quota(&self) -> bool {
        self.clusters.len() < CLUSTER_QUOTA
    }

    /// Drop every cluster whose deletion the clock already completed, answering their
    /// ARNs in store order.
    ///
    /// Materialises only what was already due: the release was owed at the stored instant
    /// whether or not anybody called, and a lookup already answers `None` for such a
    /// cluster ([`Cluster::is_released`]), so this changes nothing a later caller sees —
    /// it frees the name and the quota slot the gone cluster still held.
    pub fn release_deleted_clusters(&mut self, now: SystemTime) -> Vec<String> {
        let released: Vec<(String, String)> = self
            .clusters
            .values()
            .filter(|cluster| cluster.is_released(now))
            .map(|cluster| (cluster.name.clone(), cluster.arn.clone()))
            .collect();
        for (name, _) in &released {
            self.clusters.remove(name);
        }
        released.into_iter().map(|(_, arn)| arn).collect()
    }

    /// The cluster whose `ClusterArn` is `arn`, if this account holds one.
    ///
    /// [`Self::clusters`] is keyed by `ClusterName` — the identifier `CreateCluster`
    /// takes — while another resource that BELONGS to a cluster names it by ARN
    /// (`CreateClusterSchedulerConfig$ClusterArn`), so the lookup is by the stored
    /// `Cluster::arn`. One place for it, so every operation resolving a `ClusterArn`
    /// resolves it against the same records `CreateCluster` writes and `ListClusters`
    /// reports, rather than against a second copy of the cluster.
    pub fn cluster_by_arn(&self, arn: &str) -> Option<&Cluster> {
        self.clusters
            .values()
            .find(|cluster| cluster.arn == arn)
            .filter(|cluster| !cluster.is_released(SystemTime::now()))
    }
}
