// SPDX-License-Identifier: CC0-1.0

//! @op CancelUpdate
//! IMPLEMENTED. Cancels one of the [`crate::state::UpdateRecord`]s stored on the
//! cluster the update belongs to ([`crate::state::ClusterRecord::updates`]), resolving
//! that cluster first.
//!
//! The order of the checks is the order of the contract:
//!
//! 1. the cluster named by the request must exist IN THE REGION the request is signed
//!    for, or the answer is 404 `ResourceNotFoundException` — the same cluster
//!    resolution, with the same message construction, that `DescribeCluster` and
//!    `DescribeUpdate` perform;
//! 2. that cluster must hold the named update id, or the answer is 404
//!    `ResourceNotFoundException` naming the id, as `DescribeUpdate` answers;
//! 3. the update must be cancellable — "Cancellation is only performed if the update
//!    can be cancelled": in flight, of the one cancellable type, on an EKS Auto Mode
//!    cluster, and still in its node rollback phase — or the answer is 400
//!    `InvalidRequestException`, which is exactly "the request is invalid given the
//!    state of the cluster".
//!
//! A cancellation that is performed is PERSISTED on the update (status `Cancelled`,
//! plus the `cancellation` the model says is "present only if any cancellation is
//! attempted"), and the cluster it took is handed back from `UPDATING` to `ACTIVE`, so
//! a later `DescribeUpdate`/`DescribeCluster` sees both; a retry carrying the same
//! `clientRequestToken` — the member the model marks `@idempotencyToken` — is answered
//! with that stored cancellation rather than being refused as a second attempt.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CancelUpdate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Cancels an in-progress update to an Amazon EKS cluster on a best-effort basis.
//! Cancellation is only performed if the update can be cancelled. Currently, this is
//! supported for VersionRollback update types on EKS Auto Mode clusters when nodes are
//! rolling back. A successful cancellation stops the node rollback. After cancellation,
//! nodes converge to the current cluster version honoring configured disruption controls.
//! If the control plane rollback has already begun, the cancellation request fails.
//!
//! ## Input
//!
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `name` (String) **required** — The name of the Amazon EKS cluster associated with the
//!   update.
//! * `updateId` (String) **required** — The ID of the update to cancel.
//!
//! ## Output
//!
//! * `update` (Update) — The full description of the specified update.
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

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    CANCELLATION_STATUS_SUCCESSFUL, STATUS_ACTIVE, STATUS_UPDATING, UPDATE_CANCELLED,
    UPDATE_TYPE_VERSION_ROLLBACK,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CancelUpdateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CancelUpdateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#CancelUpdate")]
#[derive(Debug)]
pub enum CancelUpdateErrorModeled {
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
/// (`crate::ops::cancel_update::CancelUpdateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CancelUpdateError = crate::scaffolded::Scaffolded<CancelUpdateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CancelUpdateErrorModeled> for CancelUpdateError {
    fn from(modeled: CancelUpdateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CancelUpdate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CancelUpdate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CancelUpdate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CancelUpdate<Eng>
{
    type Input = CancelUpdateInput;
    type Output = CancelUpdateOutput;
    type Error = CancelUpdateError;

    const NAME: &'static str = "CancelUpdate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CANCEL_UPDATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CANCEL_UPDATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let now = SystemTime::now();
        let region = crate::state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CancelUpdateError::from(CancelUpdateErrorModeled::ServerException(ServerException {
                cluster_name: Some(input.name.clone()),
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        let invalid_request = |message: String| {
            CancelUpdateError::from(CancelUpdateErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };
        let not_found = |message: String| {
            CancelUpdateError::from(CancelUpdateErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };

        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // The cluster is resolved FIRST: an update id only means anything relative to
        // the cluster that owns it (the updates are stored ON the cluster record), so a
        // request naming a cluster this region does not hold is a not-found about the
        // CLUSTER, whatever the update id looks like.
        let Some(cluster) = guard.clusters.get_mut(&(region, input.name.clone())) else {
            return Err(not_found(format!(
                "No cluster found for name: {}.",
                input.name
            )));
        };
        // "Currently, this is supported for VersionRollback update types on EKS Auto
        // Mode clusters": a cluster whose compute capability EKS does not manage rolls
        // its own nodes back, so there is no node rollback for this call to stop.
        let auto_mode = cluster.is_auto_mode();
        let cluster_status = cluster.status.clone();

        let Some(update) = cluster.updates.get_mut(&input.update_id) else {
            return Err(not_found(format!(
                "No update found for ID: {}.",
                input.update_id
            )));
        };

        // `clientRequestToken` is the request's idempotency token, so a retry of the
        // cancel request that already ran is answered with what it did, not refused
        // for having done it. A blank token identifies nothing, so it never matches.
        let replayed_token = input
            .client_request_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());
        if update.cancellation.is_some()
            && replayed_token.is_some()
            && update.cancellation_request_token.as_deref() == replayed_token
        {
            return Ok(CancelUpdateOutput {
                update: Some(update.to_update()),
            });
        }

        if !update.is_in_progress() {
            return Err(invalid_request(format!(
                "Update {} is in status {} and is no longer in progress, so it cannot be \
                 cancelled.",
                update.id, update.status
            )));
        }
        if update.r#type != UPDATE_TYPE_VERSION_ROLLBACK {
            return Err(invalid_request(format!(
                "Update {} has type {}. Cancellation is supported only for \
                 {UPDATE_TYPE_VERSION_ROLLBACK} updates.",
                update.id, update.r#type
            )));
        }
        if !auto_mode {
            return Err(invalid_request(format!(
                "Cluster {} is not an EKS Auto Mode cluster, so update {} cannot be \
                 cancelled.",
                input.name, update.id
            )));
        }
        // Past the node rollback phase the control plane rollback has begun, and "if the
        // control plane rollback has already begun, the cancellation request fails".
        if update
            .control_plane_phase_at()
            .is_none_or(|begins_at| now >= begins_at)
        {
            return Err(invalid_request(format!(
                "The control plane rollback of update {} has already begun, so it can no \
                 longer be cancelled.",
                update.id
            )));
        }

        // A successful cancellation stops the node rollback: the update reaches the
        // terminal `Cancelled` status, so neither the `Successful` move the clock owed it
        // nor the change it carried is owed to anybody any more, and the cancellation
        // itself is recorded the way the model describes it.
        update.status = UPDATE_CANCELLED.to_string();
        update.succeeds_at = None;
        update.pending_encryption_config = None;
        update.cancellation = Some(Cancellation {
            status: Some(CANCELLATION_STATUS_SUCCESSFUL.to_string()),
            reason: Some(
                "Cancelled on request. The node rollback is stopped; nodes converge to the \
                 current cluster version honoring configured disruption controls."
                    .to_string(),
            ),
        });
        update.cancellation_request_token = replayed_token.map(str::to_owned);
        let answer = update.to_update();

        // The cluster was under this update until now; with the update terminal it is
        // back to the status the update was started from, exactly as completing one
        // hands it back — unless another update of the same cluster is still running,
        // in which case the cluster is still updating.
        if cluster_status == STATUS_UPDATING && cluster.in_flight_update().is_none() {
            cluster.status = STATUS_ACTIVE.to_string();
        }

        ::tracing::info!(
            operation = "CancelUpdate",
            cluster = %input.name,
            update_id = %input.update_id,
            outcome = "cancelled",
            "cancelled an in-flight cluster update"
        );

        Ok(CancelUpdateOutput {
            update: Some(answer),
        })
    }
}
