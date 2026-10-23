// SPDX-License-Identifier: CC0-1.0

//! The reader half of the HyperPod cluster slice of this service's state.
//!
//! The record itself — [`crate::state::Cluster`], its quota, its clock-owed span and its
//! id minting — lives in the cluster slice of [`crate::state`], beside the other
//! resource slices. What lives HERE is what only a reader needs:
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
//! The module sits under `src/state/` because it projects state, and is declared in
//! `src/state.rs` beside the other state slices of this service.
//!
//! Nothing here writes: the `Creating` -> `InService` transition is
//! [`crate::state::Cluster::settle_status`]'s, materialising only what the clock already
//! owed.

use crate::state::{Cluster, ComAmazonawsSagemakerSagemakerState, STATUS_IN_SERVICE};
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
    pub fn cluster_key(&self, name_or_arn: &str) -> Option<String> {
        if self.clusters.contains_key(name_or_arn) {
            return Some(name_or_arn.to_string());
        }
        self.clusters
            .values()
            .find(|cluster| cluster.arn == name_or_arn)
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
