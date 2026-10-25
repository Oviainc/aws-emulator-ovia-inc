// SPDX-License-Identifier: CC0-1.0

//! @op UpdateClusterVersion
//!
//! IMPLEMENTED. `UpdateClusterVersion` starts ONE asynchronous update that moves an
//! existing cluster's Kubernetes version: it mints a [`crate::state::UpdateRecord`] on
//! that cluster's record in the shared engine's store and answers with the `Update` it
//! just wrote. `DescribeUpdate` reads back the same record, and `DescribeCluster` reports
//! the cluster as `UPDATING` — still at its OLD version — until the clock completes it.
//!
//! "Cluster updates are asynchronous... During an update, the cluster status moves to
//! UPDATING... When the update is complete... the cluster status moves to Active", so the
//! requested version is held on the update as a [`crate::state::PendingClusterConfig`]
//! and written onto the cluster only when the clock completes it
//! ([`crate::state::UPDATE_COMPLETION_DELAY`] after the call) — the same seam
//! `UpdateClusterConfig` and `AssociateEncryptionConfig` use. Nothing sleeps: the due
//! instant is stored, and whichever read runs after it materialises both the update's
//! status and the version it puts on the cluster.
//!
//! A cluster moves ONE Kubernetes minor version per update, and the DIRECTION it moves in
//! is what the update's `type` reports: forward is a `VersionUpdate`, backward is a
//! `VersionRollback` — the one type `CancelUpdate` can still stop while its nodes roll
//! back. The version a request asks for must be a released row of the Kubernetes version
//! calendar this service publishes (`DescribeClusterVersions`), so the two cannot
//! disagree about which versions Amazon EKS offers.
//!
//! `force` overrides the one upgrade readiness check this service can run: "if your
//! cluster has managed node groups attached to it, all of your node groups' Kubernetes
//! versions must match the cluster's Kubernetes version in order to update the cluster to
//! a new Kubernetes version", which is reported as the modelled `InvalidStateException`.
//! No other readiness issue is invented: a cluster insight is a resource this service
//! does not hold yet, so there is none to block on.
//!
//! `rollbackConfig.timeoutMinutes` is "the length of time in minutes to wait before
//! cancelling the update", a minimum-bound wait of at least 120 minutes — far longer than
//! the span an update here takes to complete — so the published range is enforced on the
//! request and the deadline it sets never comes due.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateClusterVersion`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Updates an Amazon EKS cluster to the specified Kubernetes version. Your cluster
//! continues to function during the update. The response output includes an update ID that
//! you can use to track the status of your cluster update with the DescribeUpdate API
//! operation. Cluster updates are asynchronous, and they should finish within a few
//! minutes. During an update, the cluster status moves to UPDATING (this status transition
//! is eventually consistent). When the update is complete (either Failed or Successful),
//! the cluster status moves to Active. If your cluster has managed node groups attached to
//! it, all of your node groups' Kubernetes versions must match the cluster's Kubernetes
//! version in order to update the cluster to a new Kubernetes version.
//!
//! ## Input
//!
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `force` (Boolean, default false) **required** — Set this value to true to override
//!   upgrade-blocking or rollback-blocking readiness checks when updating a cluster.
//! * `name` (String) **required** — The name of the Amazon EKS cluster to update.
//! * `rollbackConfig` (RollbackConfig) — The rollback configuration for the cluster version
//!   rollback.
//! * `version` (String) **required** — The desired Kubernetes version following a successful
//!   update.
//!
//! ## Output
//!
//! * `update` (Update) — The full description of the specified update
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
//! * `InvalidStateException` (HTTP 400) — Amazon EKS detected upgrade readiness issues. Call
//!   the ListInsights API to view detected upgrade blocking issues. Pass the force flag when
//!   updating to override upgrade readiness errors.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ThrottlingException` (HTTP 429) — The request or operation couldn't be performed
//!   because a service is throttling requests.

use std::time::SystemTime;

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, kubernetes_minor_version, PendingClusterConfig, UpdateRecord,
    MAX_ROLLBACK_TIMEOUT_MINUTES, MIN_ROLLBACK_TIMEOUT_MINUTES, PLATFORM_VERSION, STATUS_ACTIVE,
    STATUS_UPDATING, UPDATE_COMPLETION_DELAY, UPDATE_IN_PROGRESS, UPDATE_PARAM_PLATFORM_VERSION,
    UPDATE_PARAM_VERSION, UPDATE_TYPE_VERSION, UPDATE_TYPE_VERSION_ROLLBACK,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateClusterVersionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateClusterVersionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#UpdateClusterVersion")]
#[derive(Debug)]
pub enum UpdateClusterVersionErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    InvalidStateException(InvalidStateException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_cluster_version::UpdateClusterVersionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateClusterVersionError = crate::scaffolded::Scaffolded<UpdateClusterVersionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateClusterVersionErrorModeled> for UpdateClusterVersionError {
    fn from(modeled: UpdateClusterVersionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// A 400 `InvalidParameterException` naming the cluster and the constraint the request
/// broke: "The specified parameter is invalid. Review the available parameters for the
/// API request."
fn invalid_parameter(cluster_name: &str, message: String) -> UpdateClusterVersionError {
    UpdateClusterVersionErrorModeled::InvalidParameterException(InvalidParameterException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(message),
        ..Default::default()
    })
    .into()
}

/// A 400 `InvalidRequestException`: "The request is invalid given the state of the
/// cluster."
fn invalid_request(cluster_name: &str, message: String) -> UpdateClusterVersionError {
    UpdateClusterVersionErrorModeled::InvalidRequestException(InvalidRequestException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(message),
        ..Default::default()
    })
    .into()
}

/// A 400 `InvalidStateException`: "Amazon EKS detected upgrade readiness issues... Pass
/// the force flag when updating to override upgrade readiness errors."
fn invalid_state(cluster_name: &str, message: String) -> UpdateClusterVersionError {
    UpdateClusterVersionErrorModeled::InvalidStateException(InvalidStateException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(message),
    })
    .into()
}

/// Whether `update_type` is one THIS operation mints, which is what makes a replayed
/// `clientRequestToken` a retry of this call rather than a token reused from another.
fn is_version_update_type(update_type: &str) -> bool {
    update_type == UPDATE_TYPE_VERSION || update_type == UPDATE_TYPE_VERSION_ROLLBACK
}

/// The `UpdateClusterVersion` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateClusterVersion<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateClusterVersion<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateClusterVersion<Eng>
{
    type Input = UpdateClusterVersionInput;
    type Output = UpdateClusterVersionOutput;
    type Error = UpdateClusterVersionError;

    const NAME: &'static str = "UpdateClusterVersion";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_CLUSTER_VERSION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_CLUSTER_VERSION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let cluster_name = input.name.clone();
        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();

        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateClusterVersionError::from(UpdateClusterVersionErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // A cluster name identifies a cluster only within one region, so the record this
        // updates is the one in the region the request is signed for. `name` is the
        // `@httpLabel` that names the resource being acted on, so a call against a cluster
        // this region does not hold is a 404 whatever the body asks for.
        let key = (region.clone(), cluster_name.clone());
        let Some(record) = guard.clusters.get(&key) else {
            return Err(
                UpdateClusterVersionErrorModeled::ResourceNotFoundException(
                    crate::state::cluster_not_found(&cluster_name),
                )
                .into(),
            );
        };

        // `clientRequestToken` is `@idempotencyToken`: an SDK generates it once per call
        // and RESENDS the same value when it retries, so a retry of the call that started
        // a version update is answered with THAT update rather than starting a second
        // one. A blank token identifies nothing, so it never matches, and only an update
        // THIS operation mints can be a retry of this call.
        let replayed_token = input
            .client_request_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());
        let replayed_update = replayed_token
            .and_then(|token| record.update_for_token(token))
            .filter(|update| is_version_update_type(&update.r#type));
        if let Some(update) = replayed_update {
            return Ok(UpdateClusterVersionOutput {
                update: Some(update.to_update()),
            });
        }

        // One update at a time per cluster: while another is running the cluster is in
        // use, which is exactly what `ResourceInUseException` reports.
        if let Some(in_flight) = record.in_flight_update() {
            return Err(
                UpdateClusterVersionErrorModeled::ResourceInUseException(ResourceInUseException {
                    cluster_name: Some(cluster_name.clone()),
                    message: Some(format!(
                        "Cluster {cluster_name} already has update {} in progress",
                        in_flight.id
                    )),
                    ..Default::default()
                })
                .into(),
            );
        }

        // A Kubernetes version belongs to an Amazon EKS control plane, and a cluster
        // `RegisterCluster` connected has none — it reports a `connectorConfig` and
        // nothing else — so this operation does not apply to it at all. That is a
        // PERMANENT fact about the cluster, which is why it is `ClientException` and not
        // `InvalidRequestException` ("the request is invalid given the STATE of the
        // cluster"): a client reads the state refusal as transient and waits for a status
        // that will never come.
        if record.is_connected() {
            return Err(
                UpdateClusterVersionErrorModeled::ClientException(
                    crate::state::connected_cluster_refusal(&cluster_name),
                )
                .into(),
            );
        }
        let Some(current_version) = record.version.clone() else {
            return Err(invalid_request(
                &cluster_name,
                format!("Cluster {cluster_name} reports no Kubernetes version to update"),
            ));
        };

        // An update can only be started on a cluster that is running: one still being
        // created, or being deleted, or that failed, cannot take one.
        if record.status != STATUS_ACTIVE {
            return Err(invalid_request(
                &cluster_name,
                format!(
                    "Cluster {cluster_name} is in {} status; it must be {STATUS_ACTIVE} to update \
                     its Kubernetes version",
                    record.status
                ),
            ));
        }

        // "The length of time in minutes to wait before cancelling the update... This
        // value can be between 120 (2 hours) and 10080 (7 days)"
        // (`RollbackConfig$timeoutMinutes`), a range the model declares no constraint for.
        if let Some(timeout_minutes) = input
            .rollback_config
            .as_ref()
            .and_then(|config| config.timeout_minutes)
        {
            if !(MIN_ROLLBACK_TIMEOUT_MINUTES..=MAX_ROLLBACK_TIMEOUT_MINUTES)
                .contains(&timeout_minutes)
            {
                return Err(invalid_parameter(
                    &cluster_name,
                    format!(
                        "rollbackConfig.timeoutMinutes must be between \
                         {MIN_ROLLBACK_TIMEOUT_MINUTES} and {MAX_ROLLBACK_TIMEOUT_MINUTES}, got \
                         {timeout_minutes}"
                    ),
                ));
            }
        }

        // "The desired Kubernetes version following a successful update": a version
        // Amazon EKS offers, which is a released row of the calendar this service
        // publishes through `DescribeClusterVersions` — so the catalog a caller reads to
        // pick a version and the versions an update accepts cannot disagree.
        if !guard
            .released_cluster_versions(now)
            .any(|row| row.cluster_version == input.version)
        {
            let offered: Vec<&str> = guard
                .released_cluster_versions(now)
                .map(|row| row.cluster_version.as_str())
                .collect();
            return Err(invalid_parameter(
                &cluster_name,
                format!(
                    "Unsupported Kubernetes version {}. Amazon EKS offers {}.",
                    input.version,
                    offered.join(", ")
                ),
            ));
        }

        let Some(target) = kubernetes_minor_version(&input.version) else {
            return Err(invalid_parameter(
                &cluster_name,
                format!(
                    "version {} is not a Kubernetes minor version",
                    input.version
                ),
            ));
        };
        let Some(current) = kubernetes_minor_version(&current_version) else {
            return Err(invalid_request(
                &cluster_name,
                format!(
                    "Cluster {cluster_name} reports Kubernetes version {current_version}, which \
                     is not a minor version an update can move from"
                ),
            ));
        };
        if target == current {
            return Err(invalid_request(
                &cluster_name,
                format!("Cluster {cluster_name} already runs Kubernetes version {current_version}"),
            ));
        }
        // A cluster moves ONE minor version per update — forward as an upgrade, backward
        // as the rollback of one — so the direction decides the `UpdateType` and a request
        // that skips versions names no update this cluster can make.
        if target.0 != current.0 || target.1.abs_diff(current.1) != 1 {
            return Err(invalid_parameter(
                &cluster_name,
                format!(
                    "Unsupported Kubernetes minor version update from {current_version} to {}: a \
                     cluster moves one minor version per update",
                    input.version
                ),
            ));
        }
        let update_type = if target > current {
            UPDATE_TYPE_VERSION
        } else {
            UPDATE_TYPE_VERSION_ROLLBACK
        };

        // "If your cluster has managed node groups attached to it, all of your node
        // groups' Kubernetes versions must match the cluster's Kubernetes version in order
        // to update the cluster to a new Kubernetes version" — the one upgrade readiness
        // issue this service holds the state to detect, and `force` is documented as what
        // "override[s] upgrade-blocking or rollback-blocking readiness checks".
        if !input.force {
            let lagging: Vec<String> = guard
                .nodegroups_of(&region, &cluster_name)
                .filter(|nodegroup| nodegroup.version != current_version)
                .map(|nodegroup| format!("{} ({})", nodegroup.nodegroup_name, nodegroup.version))
                .collect();
            if !lagging.is_empty() {
                return Err(invalid_state(
                    &cluster_name,
                    format!(
                        "Cluster {cluster_name} has managed node groups whose Kubernetes version \
                         does not match the cluster's {current_version}: {}. Set force to true to \
                         override upgrade readiness errors.",
                        lagging.join(", ")
                    ),
                ));
            }
        }

        let sequence = guard.next_update_sequence();
        let update = UpdateRecord {
            id: state::new_update_id(sequence),
            r#type: update_type.to_string(),
            status: UPDATE_IN_PROGRESS.to_string(),
            created_at: now,
            // "A key-value map that contains the parameters associated with the update":
            // the Kubernetes version the cluster moves to, and the platform version it
            // runs at that version — the one revision this service publishes for every
            // Kubernetes version, so the update and the catalog cannot disagree.
            params: vec![
                UpdateParam {
                    r#type: Some(UPDATE_PARAM_VERSION.to_string()),
                    value: Some(input.version.clone()),
                },
                UpdateParam {
                    r#type: Some(UPDATE_PARAM_PLATFORM_VERSION.to_string()),
                    value: Some(PLATFORM_VERSION.to_string()),
                },
            ],
            client_request_token: replayed_token.map(str::to_owned),
            succeeds_at: Some(now + UPDATE_COMPLETION_DELAY),
            pending_config: Some(PendingClusterConfig {
                version: Some(input.version.clone()),
                ..Default::default()
            }),
            // "This field is present only if any cancellation is attempted for the
            // update": none has been, so `CancelUpdate` is the only writer of either.
            cancellation: None,
            cancellation_request_token: None,
        };
        let answer = update.to_update();

        let record = guard.clusters.get_mut(&key).ok_or_else(|| {
            UpdateClusterVersionError::from(UpdateClusterVersionErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(cluster_name.clone()),
                    message: Some("the cluster record disappeared while updating it".to_string()),
                    ..Default::default()
                },
            ))
        })?;
        // The cluster is under the update until the clock completes it, and the version it
        // reports changes only then — the update is asynchronous, so the new version is
        // not visible on the cluster the instant it is requested. The update joins the
        // cluster's history through [`crate::state::ClusterRecord::record_update`], which
        // keeps that history bounded.
        record.status = STATUS_UPDATING.to_string();
        record.record_update(update);

        ::tracing::info!(
            operation = "UpdateClusterVersion",
            cluster = %cluster_name,
            update_type,
            from_version = %current_version,
            to_version = %input.version,
            "started a cluster Kubernetes version update"
        );

        Ok(UpdateClusterVersionOutput {
            update: Some(answer),
        })
    }
}
