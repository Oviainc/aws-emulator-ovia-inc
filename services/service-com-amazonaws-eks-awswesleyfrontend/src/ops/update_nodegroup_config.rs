// SPDX-License-Identifier: CC0-1.0

//! @op UpdateNodegroupConfig
//! IMPLEMENTED. Starts the asynchronous configuration update of one managed node
//! group and answers with the `Update` that tracks it: "Your node group continues to
//! function during the update. The response output includes an update ID that you
//! can use to track the status of your node group update with the DescribeUpdate API
//! operation." The requested values are NOT on the node group the instant the call
//! returns — they wait on the record as a
//! [`crate::state::PendingNodegroupConfig`] and are written onto it when the clock
//! completes the update, which is also when the node group leaves `UPDATING` for
//! `ACTIVE`. `DescribeNodegroup` reads that one record; `DescribeUpdate` and
//! `ListUpdates` read the update this mints.
//!
//! The rules it enforces, each from the model's own documentation:
//!
//! * both `clusterName` and `nodegroupName` are `@httpLabel`s addressing ONE
//!   resource, so a miss of either half is the `ResourceNotFoundException`
//!   `DescribeNodegroup` gives for the same pair — "Amazon EKS clusters and node
//!   groups are Amazon Web Services Region specific";
//! * "the Cluster Autoscaler doesn't scale a managed node group lower than minSize or
//!   higher than maxSize" (`NodegroupScalingConfig$desiredSize`), so the three sizes
//!   the node group carries AFTER the change must be ordered, whichever of them this
//!   request names — "When updating a node group, you can specify any or none of the
//!   properties";
//! * "This value or `maxUnavailablePercentage` is required to have a value"
//!   (`NodegroupUpdateConfig$maxUnavailable`), so an `updateConfig` carries exactly
//!   one of the pair;
//! * each `nodeRepairConfig` threshold is a count OR a percentage: "When using this,
//!   you cannot also set <the other> at the same time";
//! * a label key cannot be both added and removed by one request, and neither can a
//!   taint: the change would be ambiguous;
//! * one node group takes one update at a time, so a second update of it while the
//!   first is in flight is `ResourceInUseException`;
//! * `clientRequestToken` is `@idempotencyToken`, so a retry carrying the token of
//!   the call that started the update is answered with that same update.
//!
//! ORDER: the two resources the request addresses are resolved BEFORE any member of
//! the body is judged, so an update naming a cluster that does not exist reports the
//! missing resource (404) rather than a complaint about a sibling member.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateNodegroupConfig`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Updates an Amazon EKS managed node group configuration. Your node group continues to
//! function during the update. The response output includes an update ID that you can use
//! to track the status of your node group update with the DescribeUpdate API operation. You
//! can update the Kubernetes labels and taints for a node group and the scaling and version
//! update configuration.
//!
//! ## Input
//!
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `labels` (UpdateLabelsPayload) — The Kubernetes labels to apply to the nodes in the node
//!   group after the update.
//! * `nodeRepairConfig` (NodeRepairConfig) — The node auto repair configuration for the node
//!   group.
//! * `nodegroupName` (String) **required** — The name of the managed node group to update.
//! * `scalingConfig` (NodegroupScalingConfig) — The scaling configuration details for the
//!   Auto Scaling group after the update.
//! * `taints` (UpdateTaintsPayload) — The Kubernetes taints to be applied to the nodes in the
//!   node group after the update. For more information, see Node taints on managed node
//!   groups.
//! * `updateConfig` (NodegroupUpdateConfig) — The node group update configuration.
//! * `warmPoolConfig` (WarmPoolConfig) — The warm pool configuration to apply to the node
//!   group. You can use this to add a warm pool to an existing node group or modify the
//!   settings of an existing warm pool.
//!
//! ## Output
//!
//! * `update` (Update) — An object representing an asynchronous update.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, PendingNodegroupConfig, UpdateRecord, UpdateScope, DEFAULT_SCALING_DESIRED_SIZE,
    DEFAULT_SCALING_MAX_SIZE, DEFAULT_SCALING_MIN_SIZE, STATUS_ACTIVE, STATUS_UPDATING,
    UPDATE_COMPLETION_DELAY, UPDATE_IN_PROGRESS, UPDATE_PARAM_DESIRED_SIZE,
    UPDATE_PARAM_LABELS_TO_ADD, UPDATE_PARAM_LABELS_TO_REMOVE, UPDATE_PARAM_MAX_SIZE,
    UPDATE_PARAM_MAX_UNAVAILABLE, UPDATE_PARAM_MAX_UNAVAILABLE_PERCENTAGE, UPDATE_PARAM_MIN_SIZE,
    UPDATE_PARAM_NODE_REPAIR_CONFIG, UPDATE_PARAM_TAINTS_TO_ADD, UPDATE_PARAM_TAINTS_TO_REMOVE,
    UPDATE_PARAM_UPDATE_STRATEGY, UPDATE_PARAM_WARM_POOL_ENABLED,
    UPDATE_PARAM_WARM_POOL_MAX_GROUP_PREPARED_CAPACITY, UPDATE_PARAM_WARM_POOL_MIN_SIZE,
    UPDATE_PARAM_WARM_POOL_REUSE_ON_SCALE_IN, UPDATE_PARAM_WARM_POOL_STATE, UPDATE_TYPE_CONFIG,
};
use std::collections::BTreeMap;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateNodegroupConfigError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateNodegroupConfigError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#UpdateNodegroupConfig")]
#[derive(Debug)]
pub enum UpdateNodegroupConfigErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_nodegroup_config::UpdateNodegroupConfigError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateNodegroupConfigError = crate::scaffolded::Scaffolded<UpdateNodegroupConfigErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateNodegroupConfigErrorModeled> for UpdateNodegroupConfigError {
    fn from(modeled: UpdateNodegroupConfigErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateNodegroupConfig` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateNodegroupConfig<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateNodegroupConfig<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateNodegroupConfig<Eng>
{
    type Input = UpdateNodegroupConfigInput;
    type Output = UpdateNodegroupConfigOutput;
    type Error = UpdateNodegroupConfigError;

    const NAME: &'static str = "UpdateNodegroupConfig";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_NODEGROUP_CONFIG_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_NODEGROUP_CONFIG_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();

        let store = state::service_state(&*self.engine).ok_or_else(|| {
            UpdateNodegroupConfigError::from(UpdateNodegroupConfigErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    nodegroup_name: Some(input.nodegroup_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // This call addresses ONE resource — the `(cluster, node group)` pair — so a
        // miss of either half is the service's generic, memberless not-found payload,
        // exactly as `DescribeNodegroup` answers it for the same pair
        // ([`crate::state::resource_not_found`]).
        let not_found = || {
            UpdateNodegroupConfigError::from(
                UpdateNodegroupConfigErrorModeled::ResourceNotFoundException(
                    state::resource_not_found(),
                ),
            )
        };
        let invalid_parameter = |message: String| {
            UpdateNodegroupConfigError::from(
                UpdateNodegroupConfigErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(input.cluster_name.clone()),
                        nodegroup_name: Some(input.nodegroup_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };
        let invalid_request = |message: String| {
            UpdateNodegroupConfigError::from(
                UpdateNodegroupConfigErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        cluster_name: Some(input.cluster_name.clone()),
                        nodegroup_name: Some(input.nodegroup_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };

        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // THE RESOURCES THE REQUEST ADDRESSES FIRST, BEFORE ANY MEMBER OF THE BODY IS
        // JUDGED: a node group lives inside one cluster and a cluster name identifies a
        // cluster only within the region the request is signed for, so both halves are
        // resolved there. A mutation naming a cluster that does not exist therefore
        // reports the missing resource, so the caller's recovery is the one the answer
        // points at.
        let cluster_key = (region.clone(), input.cluster_name.clone());
        let Some(cluster) = guard.clusters.get(&cluster_key) else {
            return Err(not_found());
        };
        let cluster_status = cluster.status.clone();
        let nodegroup_key = (
            region.clone(),
            input.cluster_name.clone(),
            input.nodegroup_name.clone(),
        );
        // A cluster `RegisterCluster` connected has no Amazon EKS-managed node group at
        // all, so this lookup is what reports that — with the same 404 any other
        // missing node group earns — rather than a gate of its own.
        let Some(nodegroup) = guard.nodegroups.get(&nodegroup_key) else {
            return Err(not_found());
        };
        let nodegroup_status = nodegroup.status.clone();
        let stored_scaling = nodegroup.scaling_config.clone();

        // An update of a node group is an update OF THAT NODE GROUP: it is what selects
        // the update in a listing (`ListUpdates$nodegroupName`) and what makes a second
        // update of the same node group a conflict while the first is still running.
        let scope = UpdateScope::Nodegroup(input.nodegroup_name.clone());

        // A retry. `clientRequestToken` is `@idempotencyToken`: an SDK generates it
        // once per call and RESENDS the same value when it retries, so a retry of the
        // call that started an update of this node group is answered with THAT update
        // rather than starting a second one. A blank token identifies nothing, so it
        // never matches, and only an update of this node group can be a retry of this
        // call.
        let replayed_token = input
            .client_request_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());
        if let Some(update) = replayed_token
            .and_then(|token| cluster.update_for_token(token))
            .filter(|update| update.scope == scope)
        {
            return Ok(UpdateNodegroupConfigOutput {
                update: Some(update.to_update()),
            });
        }

        // One node group takes one update at a time: while its own update is running
        // the node group is in use, which is what `ResourceInUseException` reports,
        // worded by [`crate::state::update_in_progress`] for the update type this call
        // would have minted.
        if let Some(in_flight) = cluster.in_flight_update_in_scope(&scope) {
            return Err(
                UpdateNodegroupConfigErrorModeled::ResourceInUseException(
                    state::update_in_progress(
                        UPDATE_TYPE_CONFIG,
                        &input.cluster_name,
                        in_flight,
                    ),
                )
                .into(),
            );
        }

        // "The request is invalid given the state of the cluster": a node group's
        // configuration is changed through a running control plane, and a node group can
        // only take a change once it has SETTLED — one still being created, or being
        // deleted, cannot.
        if cluster_status != STATUS_ACTIVE {
            return Err(invalid_request(format!(
                "Cluster {} is in {cluster_status} status; it must be {STATUS_ACTIVE} to update \
                 a node group.",
                input.cluster_name
            )));
        }
        if nodegroup_status != STATUS_ACTIVE {
            return Err(invalid_request(format!(
                "Nodegroup {} is in {nodegroup_status} status; it must be {STATUS_ACTIVE} to be \
                 updated.",
                input.nodegroup_name
            )));
        }

        // THE BODY'S OWN MEMBERS. Each rule below is one the model cannot express, so
        // the boundary validator passed the request and this is where it is caught.
        //
        // "The Kubernetes labels to add or update" and "the Kubernetes labels to
        // remove" (`UpdateLabelsPayload`) are two halves of ONE change, so a key named
        // by both leaves the label in no defined state.
        let requested_labels = input.labels.clone().unwrap_or_default();
        let add_or_update_labels: BTreeMap<String, String> = requested_labels
            .add_or_update_labels
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let remove_labels = requested_labels.remove_labels.clone().unwrap_or_default();
        if let Some(key) = remove_labels
            .iter()
            .find(|key| add_or_update_labels.contains_key(*key))
        {
            return Err(invalid_parameter(format!(
                "labels names {key:?} in both addOrUpdateLabels and removeLabels"
            )));
        }

        // The same rule for taints, which are identified by the `(key, effect)` pair
        // the scheduler matches on.
        let requested_taints = input.taints.clone().unwrap_or_default();
        let add_or_update_taints = requested_taints
            .add_or_update_taints
            .clone()
            .unwrap_or_default();
        let remove_taints = requested_taints.remove_taints.clone().unwrap_or_default();
        if let Some(taint) = remove_taints.iter().find(|removed| {
            add_or_update_taints
                .iter()
                .any(|added| added.key == removed.key && added.effect == removed.effect)
        }) {
            return Err(invalid_parameter(format!(
                "taints names the taint with key {:?} and effect {:?} in both \
                 addOrUpdateTaints and removeTaints",
                taint.key.clone().unwrap_or_default(),
                taint.effect.clone().unwrap_or_default()
            )));
        }

        // "When updating a node group, you can specify any or none of the properties"
        // (`NodegroupScalingConfig`) — unlike a create, which takes all or none — so
        // the sizes the node group will carry AFTER this change are the requested ones
        // over the stored ones, and it is THAT triple which must satisfy "the Cluster
        // Autoscaler doesn't scale a managed node group lower than minSize or higher
        // than maxSize" (`NodegroupScalingConfig$desiredSize`). Each member's own lower
        // bound is modelled and so already refused before this handler runs.
        let scaling_config = input.scaling_config.clone().map(|requested| {
            NodegroupScalingConfig {
                min_size: requested.min_size.or(stored_scaling.min_size),
                max_size: requested.max_size.or(stored_scaling.max_size),
                desired_size: requested.desired_size.or(stored_scaling.desired_size),
            }
        });
        if let Some(scaling) = scaling_config.as_ref() {
            let min_size = scaling.min_size.unwrap_or(DEFAULT_SCALING_MIN_SIZE);
            let max_size = scaling.max_size.unwrap_or(DEFAULT_SCALING_MAX_SIZE);
            let desired_size = scaling.desired_size.unwrap_or(DEFAULT_SCALING_DESIRED_SIZE);
            // The refusal names the bound the desired size falls outside of, and the
            // capacities it was asked for, the way Amazon EKS words each relation
            // ([`state::scaling_capacity_violation`]).
            if let Some(why) = state::scaling_capacity_violation(min_size, max_size, desired_size) {
                return Err(invalid_parameter(why));
            }
        }

        // "This value or maxUnavailablePercentage is required to have a value"
        // (`NodegroupUpdateConfig$maxUnavailable`), and the percentage member says the
        // same of the count — so exactly one of the pair carries a value, the rule
        // `CreateNodegroup` holds the same shape to.
        if let Some(update_config) = input.update_config.as_ref() {
            let by_count = update_config.max_unavailable.is_some();
            let by_percentage = update_config.max_unavailable_percentage.is_some();
            if by_count == by_percentage {
                return Err(invalid_parameter(
                    "updateConfig must specify exactly one of maxUnavailable and \
                     maxUnavailablePercentage"
                        .to_string(),
                ));
            }
        }

        // Each `nodeRepairConfig` threshold is a count OR a percentage: "When using
        // this, you cannot also set <the other> at the same time"
        // (`NodeRepairConfig$maxUnhealthyNodeThresholdCount` and
        // `NodeRepairConfig$maxParallelNodesRepairedCount`).
        if let Some(repair) = input.node_repair_config.as_ref() {
            for (count, percentage, member) in [
                (
                    repair.max_unhealthy_node_threshold_count,
                    repair.max_unhealthy_node_threshold_percentage,
                    "maxUnhealthyNodeThreshold",
                ),
                (
                    repair.max_parallel_nodes_repaired_count,
                    repair.max_parallel_nodes_repaired_percentage,
                    "maxParallelNodesRepaired",
                ),
            ] {
                if count.is_some() && percentage.is_some() {
                    return Err(invalid_parameter(format!(
                        "nodeRepairConfig must not specify both {member}Count and \
                         {member}Percentage"
                    )));
                }
            }
        }

        // "The minimum number of instances to maintain in the warm pool" and "the
        // maximum total number of instances across the warm pool and Auto Scaling group
        // combined" (`WarmPoolConfig`) — a maximum below the minimum describes no pool.
        if let Some(warm_pool) = input.warm_pool_config.as_ref() {
            if let (Some(min_size), Some(max_prepared)) =
                (warm_pool.min_size, warm_pool.max_group_prepared_capacity)
            {
                if max_prepared < min_size {
                    return Err(invalid_parameter(format!(
                        "warmPoolConfig.maxGroupPreparedCapacity {max_prepared} is below \
                         minSize {min_size}"
                    )));
                }
            }
        }

        let pending = PendingNodegroupConfig {
            update_id: String::new(),
            add_or_update_labels,
            remove_labels,
            add_or_update_taints,
            remove_taints,
            scaling_config,
            update_config: input.update_config.clone(),
            node_repair_config: input.node_repair_config.clone(),
            warm_pool_config: input.warm_pool_config.clone(),
            // This operation changes the node group's CONFIGURATION; its Kubernetes
            // and AMI versions are `UpdateNodegroupVersion`'s to change.
            version: None,
            release_version: None,
        };
        // A request that asks for nothing would mint an update that changes nothing,
        // which is not a change this service records: the caller named no member to
        // update, and that is a bad parameter.
        if pending.is_empty() {
            return Err(invalid_parameter(
                "the request must specify at least one of labels, taints, scalingConfig, \
                 updateConfig, nodeRepairConfig and warmPoolConfig to update"
                    .to_string(),
            ));
        }

        // "A key-value map that contains the parameters associated with the update":
        // one entry per member of the request that asks for a change, so a caller
        // reading the update back sees what it asked for. A member the request did not
        // carry asks for nothing and is not reported.
        let mut params = Vec::new();
        if !pending.add_or_update_labels.is_empty() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_LABELS_TO_ADD.to_string()),
                value: Some(
                    ::serde_json::to_string(&pending.add_or_update_labels)
                        .unwrap_or_else(|_| "{}".to_string()),
                ),
            });
        }
        if !pending.remove_labels.is_empty() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_LABELS_TO_REMOVE.to_string()),
                value: Some(pending.remove_labels.join(",")),
            });
        }
        for (taints, param) in [
            (&pending.add_or_update_taints, UPDATE_PARAM_TAINTS_TO_ADD),
            (&pending.remove_taints, UPDATE_PARAM_TAINTS_TO_REMOVE),
        ] {
            if !taints.is_empty() {
                params.push(UpdateParam {
                    r#type: Some(param.to_string()),
                    value: Some(taints_param(taints)),
                });
            }
        }
        if let Some(scaling) = pending.scaling_config.as_ref() {
            for (size, param) in [
                (scaling.min_size, UPDATE_PARAM_MIN_SIZE),
                (scaling.max_size, UPDATE_PARAM_MAX_SIZE),
                (scaling.desired_size, UPDATE_PARAM_DESIRED_SIZE),
            ] {
                if let Some(size) = size {
                    params.push(UpdateParam {
                        r#type: Some(param.to_string()),
                        value: Some(size.to_string()),
                    });
                }
            }
        }
        if let Some(update_config) = pending.update_config.as_ref() {
            if let Some(max_unavailable) = update_config.max_unavailable {
                params.push(UpdateParam {
                    r#type: Some(UPDATE_PARAM_MAX_UNAVAILABLE.to_string()),
                    value: Some(max_unavailable.to_string()),
                });
            }
            if let Some(percentage) = update_config.max_unavailable_percentage {
                params.push(UpdateParam {
                    r#type: Some(UPDATE_PARAM_MAX_UNAVAILABLE_PERCENTAGE.to_string()),
                    value: Some(percentage.to_string()),
                });
            }
            if let Some(strategy) = update_config.update_strategy.as_deref() {
                params.push(UpdateParam {
                    r#type: Some(UPDATE_PARAM_UPDATE_STRATEGY.to_string()),
                    value: Some(strategy.to_string()),
                });
            }
        }
        if let Some(repair) = pending.node_repair_config.as_ref() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_NODE_REPAIR_CONFIG.to_string()),
                value: Some(repair.enabled.unwrap_or(false).to_string()),
            });
        }
        if let Some(warm_pool) = pending.warm_pool_config.as_ref() {
            if let Some(enabled) = warm_pool.enabled {
                params.push(UpdateParam {
                    r#type: Some(UPDATE_PARAM_WARM_POOL_ENABLED.to_string()),
                    value: Some(enabled.to_string()),
                });
            }
            if let Some(min_size) = warm_pool.min_size {
                params.push(UpdateParam {
                    r#type: Some(UPDATE_PARAM_WARM_POOL_MIN_SIZE.to_string()),
                    value: Some(min_size.to_string()),
                });
            }
            if let Some(max_prepared) = warm_pool.max_group_prepared_capacity {
                params.push(UpdateParam {
                    r#type: Some(
                        UPDATE_PARAM_WARM_POOL_MAX_GROUP_PREPARED_CAPACITY.to_string(),
                    ),
                    value: Some(max_prepared.to_string()),
                });
            }
            if let Some(pool_state) = warm_pool.pool_state.as_deref() {
                params.push(UpdateParam {
                    r#type: Some(UPDATE_PARAM_WARM_POOL_STATE.to_string()),
                    value: Some(pool_state.to_string()),
                });
            }
            if let Some(reuse) = warm_pool.reuse_on_scale_in {
                params.push(UpdateParam {
                    r#type: Some(UPDATE_PARAM_WARM_POOL_REUSE_ON_SCALE_IN.to_string()),
                    value: Some(reuse.to_string()),
                });
            }
        }

        let completes_at = now + UPDATE_COMPLETION_DELAY;
        let update_id = state::new_update_id(guard.next_update_sequence());
        let update = UpdateRecord {
            id: update_id.clone(),
            // A node group configuration change is reported as the generic
            // `ConfigUpdate` of the `UpdateType` enum, which is the member for a change
            // to a resource's configuration rather than to its version.
            r#type: UPDATE_TYPE_CONFIG.to_string(),
            status: UPDATE_IN_PROGRESS.to_string(),
            created_at: now,
            params,
            client_request_token: replayed_token.map(str::to_owned),
            succeeds_at: Some(completes_at),
            // The change this update carries is the NODE GROUP's, not the cluster's: it
            // is parked on the node group record below and applied there when the clock
            // completes this update.
            pending_config: None,
            // "This field is present only if any cancellation is attempted for the
            // update": none has been, so `CancelUpdate` is the only writer of either.
            cancellation: None,
            cancellation_request_token: None,
            scope,
        };
        let answer = update.to_update();

        let server_error = |message: &str| {
            UpdateNodegroupConfigError::from(UpdateNodegroupConfigErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    nodegroup_name: Some(input.nodegroup_name.clone()),
                    message: Some(message.to_string()),
                    ..Default::default()
                },
            ))
        };

        // The update joins its cluster's history, which is where `DescribeUpdate` and
        // `ListUpdates` report it from.
        let cluster = guard.clusters.get_mut(&cluster_key).ok_or_else(|| {
            server_error("the cluster record disappeared while updating its node group")
        })?;
        cluster.record_update(update);

        // The node group is under the update until the clock completes it, and what it
        // reports changes only then — the update is asynchronous ("your node group
        // continues to function during the update"), so none of the requested values is
        // visible on the node group the instant it is requested.
        let record = guard
            .nodegroups
            .get_mut(&nodegroup_key)
            .ok_or_else(|| server_error("the node group record disappeared while updating it"))?;
        record.status = STATUS_UPDATING.to_string();
        record.active_at = Some(completes_at);
        record.pending_config = Some(PendingNodegroupConfig {
            update_id,
            ..pending
        });

        Ok(UpdateNodegroupConfigOutput {
            update: Some(answer),
        })
    }
}

/// The `params` value a taint change reports: one `key=value:effect` entry per
/// taint, in the order the request named them.
///
/// `UpdateParam$value` is a single string, so a list of taints has to be rendered
/// into one — and `key=value:effect` is the form `kubectl taint` takes and the
/// one a reader of the update can act on.
fn taints_param(taints: &[Taint]) -> String {
    taints
        .iter()
        .map(|taint| {
            format!(
                "{}={}:{}",
                taint.key.clone().unwrap_or_default(),
                taint.value.clone().unwrap_or_default(),
                taint.effect.clone().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}
