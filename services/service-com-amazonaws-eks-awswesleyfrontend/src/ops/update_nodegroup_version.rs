// SPDX-License-Identifier: CC0-1.0

//! @op UpdateNodegroupVersion
//! IMPLEMENTED. Starts the asynchronous version update of one managed node group and
//! answers with the `Update` that tracks it: "updates the Kubernetes version or AMI
//! version of an Amazon EKS managed node group". The new version is NOT on the node
//! group the instant the call returns — it waits on the record as a
//! [`crate::state::PendingNodegroupConfig`] and is written onto it when the clock
//! completes the update, which is also when the node group leaves `UPDATING` for
//! `ACTIVE`. `DescribeNodegroup` reads that one record; `DescribeUpdate` and
//! `ListUpdates` read the update this mints.
//!
//! The rules it enforces, each from the model's own documentation:
//!
//! * both `clusterName` and `nodegroupName` are `@httpLabel`s addressing ONE
//!   resource, so a miss of either half is the `ResourceNotFoundException`
//!   `DescribeNodegroup` gives for the same pair;
//! * "If no version is specified, then the node group will be updated to match the
//!   cluster's current Kubernetes version", so an absent `version` is the cluster's;
//! * "You cannot roll back a node group to an earlier Kubernetes version": a version
//!   below the one the node group runs is `InvalidParameterException`, and so is one
//!   above the cluster's own, which a node group never exceeds;
//! * "You can only update a node group using a launch template if the node group was
//!   originally deployed with a launch template... you must specify the same launch
//!   template ID or name that was used to create the node group";
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
//! # Modeled contract for `UpdateNodegroupVersion`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Updates the Kubernetes version or AMI version of an Amazon EKS managed node group. You
//! can update a node group using a launch template only if the node group was originally
//! deployed with a launch template. Additionally, the launch template ID or name must match
//! what was used when the node group was created. You can update the launch template
//! version with necessary changes. If you need to update a custom AMI in a node group that
//! was deployed with a launch template, then update your custom AMI, specify the new ID in
//! a new version of the launch template, and then update the node group to the new version
//! of the launch template. If you update without a launch template, then you can update to
//! the latest available AMI version of a node group's current Kubernetes version by not
//! specifying a Kubernetes version in the request. You can update to the latest AMI version
//! of your cluster's current Kubernetes version by specifying your cluster's Kubernetes
//! version in the request. For information about Linux versions, see Amazon EKS optimized
//! Amazon Linux AMI versions in the Amazon EKS User Guide. For information about Windows
//! versions, see Amazon EKS optimized Windows AMI versions in the Amazon EKS User Guide.
//! You cannot roll back a node group to an earlier Kubernetes version or AMI version. When
//! a node in a managed node group is terminated due to a scaling action or update, every
//! Pod on that node is drained first. Amazon EKS attempts to drain the nodes gracefully and
//! will fail if it is unable to do so. You can force the update if Amazon EKS is unable to
//! drain the nodes as a result of a Pod disruption budget issue.
//!
//! ## Input
//!
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `force` (Boolean, default false) **required** — Force the update if any Pod on the
//!   existing node group can't be drained due to a Pod disruption budget issue. If an update
//!   fails because all Pods can't be drained, you can force the update after it fails to
//!   terminate the old node whether or not any Pod is running on the node.
//! * `launchTemplate` (LaunchTemplateSpecification) — An object representing a node group's
//!   launch template specification. You can only update a node group using a launch template
//!   if the node group was originally deployed with a launch template. When updating, you
//!   must specify the same launch template ID or name that was used to create the node group.
//! * `nodegroupName` (String) **required** — The name of the managed node group to update.
//! * `releaseVersion` (String) — The AMI version of the Amazon EKS optimized AMI to use for
//!   the update. By default, the latest available AMI version for the node group's Kubernetes
//!   version is used. For information about Linux versions, see Amazon EKS optimized Amazon
//!   Linux AMI versions in the Amazon EKS User Guide. Amazon EKS managed node groups support
//!   the November 2022 and later releases of the Windows AMIs. For information about Windows
//!   versions, see Amazon EKS optimized Windows AMI versions in the Amazon EKS User Guide. If
//!   you specify launchTemplate, and your launch template uses a custom AMI, then don't
//!   specify releaseVersion, or the node group update will fail. For more information about
//!   using launch templates with …
//! * `version` (String) — The Kubernetes version to update to. If no version is specified,
//!   then the node group will be updated to match the cluster's current Kubernetes version,
//!   and the latest available AMI for that version will be used. You can also specify the
//!   Kubernetes version of the cluster to update the node group to the latest AMI version of
//!   the cluster's Kubernetes version. If you specify launchTemplate, and your launch
//!   template uses a custom AMI, then don't specify version, or the node group update will
//!   fail. For more information about using launch templates with Amazon EKS, see Customizing
//!   managed nodes with launch templates in the Amazon EKS User Guide.
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
    self, PendingNodegroupConfig, UpdateRecord, UpdateScope, DEFAULT_KUBERNETES_VERSION,
    STATUS_ACTIVE, STATUS_UPDATING, UPDATE_COMPLETION_DELAY, UPDATE_IN_PROGRESS,
    UPDATE_PARAM_LAUNCH_TEMPLATE_NAME, UPDATE_PARAM_LAUNCH_TEMPLATE_VERSION,
    UPDATE_PARAM_RELEASE_VERSION, UPDATE_PARAM_VERSION, UPDATE_TYPE_VERSION,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateNodegroupVersionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateNodegroupVersionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#UpdateNodegroupVersion")]
#[derive(Debug)]
pub enum UpdateNodegroupVersionErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_nodegroup_version::UpdateNodegroupVersionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateNodegroupVersionError = crate::scaffolded::Scaffolded<UpdateNodegroupVersionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateNodegroupVersionErrorModeled> for UpdateNodegroupVersionError {
    fn from(modeled: UpdateNodegroupVersionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateNodegroupVersion` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateNodegroupVersion<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateNodegroupVersion<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateNodegroupVersion<Eng>
{
    type Input = UpdateNodegroupVersionInput;
    type Output = UpdateNodegroupVersionOutput;
    type Error = UpdateNodegroupVersionError;

    const NAME: &'static str = "UpdateNodegroupVersion";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_NODEGROUP_VERSION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_NODEGROUP_VERSION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();

        let store = state::service_state(&*self.engine).ok_or_else(|| {
            UpdateNodegroupVersionError::from(UpdateNodegroupVersionErrorModeled::ServerException(
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
        // exactly as `DescribeNodegroup` answers it ([`crate::state::resource_not_found`]).
        let not_found = || {
            UpdateNodegroupVersionError::from(
                UpdateNodegroupVersionErrorModeled::ResourceNotFoundException(
                    state::resource_not_found(),
                ),
            )
        };
        let invalid_parameter = |message: String| {
            UpdateNodegroupVersionError::from(
                UpdateNodegroupVersionErrorModeled::InvalidParameterException(
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
            UpdateNodegroupVersionError::from(
                UpdateNodegroupVersionErrorModeled::InvalidRequestException(
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
        // cluster only within the region the request is signed for.
        let cluster_key = (region.clone(), input.cluster_name.clone());
        let Some(cluster) = guard.clusters.get(&cluster_key) else {
            return Err(not_found());
        };
        let cluster_status = cluster.status.clone();
        // Every Amazon EKS cluster reports a Kubernetes version; the service's default
        // stands in for a record that somehow carries none, as `CreateNodegroup`'s own
        // reading does.
        let cluster_version = cluster
            .version
            .clone()
            .unwrap_or_else(|| DEFAULT_KUBERNETES_VERSION.to_string());
        let nodegroup_key = (
            region.clone(),
            input.cluster_name.clone(),
            input.nodegroup_name.clone(),
        );
        let Some(nodegroup) = guard.nodegroups.get(&nodegroup_key) else {
            return Err(not_found());
        };
        let nodegroup_status = nodegroup.status.clone();
        let nodegroup_version = nodegroup.version.clone();
        let deployed_launch_template = nodegroup.launch_template.clone();

        // An update of a node group is an update OF THAT NODE GROUP, the same scope
        // `UpdateNodegroupConfig` records, so the two cannot run at once on one node
        // group.
        let scope = UpdateScope::Nodegroup(input.nodegroup_name.clone());

        // A retry. `clientRequestToken` is `@idempotencyToken`: a retry of the call
        // that started an update of this node group is answered with THAT update
        // rather than starting a second one. A blank token identifies nothing.
        let replayed_token = input
            .client_request_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());
        if let Some(update) = replayed_token
            .and_then(|token| cluster.update_for_token(token))
            .filter(|update| update.scope == scope)
        {
            return Ok(UpdateNodegroupVersionOutput {
                update: Some(update.to_update()),
            });
        }

        if let Some(in_flight) = cluster.in_flight_update_in_scope(&scope) {
            return Err(
                UpdateNodegroupVersionErrorModeled::ResourceInUseException(
                    state::update_in_progress(
                        UPDATE_TYPE_VERSION,
                        &input.cluster_name,
                        in_flight,
                    ),
                )
                .into(),
            );
        }

        // "The request is invalid given the state of the cluster": nodes are replaced
        // through a running control plane, and a node group can only take an update
        // once it has SETTLED.
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

        // THE BODY'S OWN MEMBERS.
        //
        // "You can only update a node group using a launch template if the node group
        // was originally deployed with a launch template. Additionally, the launch
        // template ID or name must match what was used when the node group was
        // created."
        if let Some(requested) = input.launch_template.as_ref() {
            let Some(deployed) = deployed_launch_template.as_ref() else {
                return Err(invalid_parameter(format!(
                    "Nodegroup {} was not deployed with a launch template, so it cannot be \
                     updated with one.",
                    input.nodegroup_name
                )));
            };
            let names_same_template = match (requested.id.as_deref(), requested.name.as_deref()) {
                (Some(id), _) if !id.trim().is_empty() => deployed.id.as_deref() == Some(id),
                (_, Some(name)) if !name.trim().is_empty() => {
                    deployed.name.as_deref() == Some(name)
                }
                // A specification naming neither identifies no launch template at all.
                _ => false,
            };
            if !names_same_template {
                return Err(invalid_parameter(
                    "launchTemplate must name the same launch template id or name the node \
                     group was created with."
                        .to_string(),
                ));
            }
        }

        // "If no version is specified, then the node group will be updated to match the
        // cluster's current Kubernetes version."
        let target_version = input
            .version
            .clone()
            .map(|version| version.trim().to_string())
            .filter(|version| !version.is_empty())
            .unwrap_or_else(|| cluster_version.clone());
        // "You cannot roll back a node group to an earlier Kubernetes version", and a
        // node group never runs a version ahead of its cluster ("you can only create a
        // node group for your cluster that is equal to the current Kubernetes version
        // for the cluster", `CreateNodegroupRequest$version`). Both ends are compared
        // on the minor version the token names, which is what an EKS Kubernetes version
        // is ([`crate::state::kubernetes_minor_version`]); a token that is not one at
        // all is a bad parameter.
        let Some(target_minor) = state::kubernetes_minor_version(&target_version) else {
            return Err(invalid_parameter(format!(
                "version {} is not a Kubernetes version of the form <major>.<minor>",
                state::elided_value(&target_version, state::MAX_ECHOED_VALUE)
            )));
        };
        if state::kubernetes_minor_version(&nodegroup_version)
            .is_some_and(|current| target_minor < current)
        {
            return Err(invalid_parameter(format!(
                "version {target_version} is earlier than the {nodegroup_version} nodegroup {} \
                 runs; a node group cannot be rolled back to an earlier Kubernetes version.",
                input.nodegroup_name
            )));
        }
        if state::kubernetes_minor_version(&cluster_version)
            .is_some_and(|cluster_minor| target_minor > cluster_minor)
        {
            return Err(invalid_parameter(format!(
                "version {target_version} is ahead of the cluster's {cluster_version}; a node \
                 group cannot run a Kubernetes version its cluster does not."
            )));
        }

        // "A key-value map that contains the parameters associated with the update":
        // the version this update moves the node group to, plus every other member the
        // request asked for.
        let release_version = input
            .release_version
            .clone()
            .map(|version| version.trim().to_string())
            .filter(|version| !version.is_empty());
        let mut params = vec![UpdateParam {
            r#type: Some(UPDATE_PARAM_VERSION.to_string()),
            value: Some(target_version.clone()),
        }];
        if let Some(release_version) = release_version.as_deref() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_RELEASE_VERSION.to_string()),
                value: Some(release_version.to_string()),
            });
        }
        if let Some(template) = input.launch_template.as_ref() {
            if let Some(name) = template.name.as_deref().or(template.id.as_deref()) {
                params.push(UpdateParam {
                    r#type: Some(UPDATE_PARAM_LAUNCH_TEMPLATE_NAME.to_string()),
                    value: Some(name.to_string()),
                });
            }
            if let Some(version) = template.version.as_deref() {
                params.push(UpdateParam {
                    r#type: Some(UPDATE_PARAM_LAUNCH_TEMPLATE_VERSION.to_string()),
                    value: Some(version.to_string()),
                });
            }
        }

        let completes_at = now + UPDATE_COMPLETION_DELAY;
        let update_id = state::new_update_id(guard.next_update_sequence());
        let update = UpdateRecord {
            id: update_id.clone(),
            r#type: UPDATE_TYPE_VERSION.to_string(),
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
            UpdateNodegroupVersionError::from(UpdateNodegroupVersionErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    nodegroup_name: Some(input.nodegroup_name.clone()),
                    message: Some(message.to_string()),
                    ..Default::default()
                },
            ))
        };

        let cluster = guard.clusters.get_mut(&cluster_key).ok_or_else(|| {
            server_error("the cluster record disappeared while updating its node group")
        })?;
        cluster.record_update(update);

        // The node group is under the update until the clock completes it: the nodes
        // are replaced one batch at a time, so neither the Kubernetes version nor the
        // AMI version it reports changes the instant the update is requested.
        let record = guard
            .nodegroups
            .get_mut(&nodegroup_key)
            .ok_or_else(|| server_error("the node group record disappeared while updating it"))?;
        record.status = STATUS_UPDATING.to_string();
        record.active_at = Some(completes_at);
        record.pending_config = Some(PendingNodegroupConfig {
            update_id,
            version: Some(target_version),
            release_version,
            ..PendingNodegroupConfig::default()
        });

        Ok(UpdateNodegroupVersionOutput {
            update: Some(answer),
        })
    }
}
