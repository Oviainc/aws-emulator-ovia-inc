// SPDX-License-Identifier: CC0-1.0

//! @op DeregisterCluster
//!
//! IMPLEMENTED. Removes the CONNECTED cluster its `name` labels from this service's
//! store — "deregisters a connected cluster to remove it from the Amazon EKS control
//! plane" — and answers with the record it removed, reporting `DELETING`, the way
//! `DeleteCluster` answers for an Amazon EKS cluster.
//!
//! It is the counterpart of the connected-cluster gate every operation of the
//! EKS-managed plane answers ([`crate::state::connected_cluster_refusal`]): a
//! registration is removed HERE and not by `DeleteCluster`, which deletes an Amazon
//! EKS cluster's control plane. So a cluster that was never registered — one
//! `CreateCluster` provisioned — is refused `ClientException`, the modelled class
//! for "this operation does not apply to the resource you named", and a name this
//! region does not hold is `ResourceNotFoundException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeregisterCluster`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Deregisters a connected cluster to remove it from the Amazon EKS control plane. A
//! connected cluster is a Kubernetes cluster that you've connected to your control plane
//! using the Amazon EKS Connector.
//!
//! ## Input
//!
//! * `name` (String) **required** — The name of the connected cluster to deregister.
//!
//! ## Output
//!
//! * `cluster` (Cluster) — An object representing an Amazon EKS cluster.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have permissions to perform the requested
//!   operation. The IAM principal making the request must have at least one IAM permissions
//!   policy attached that grants the required permissions. For more information, see Access
//!   management in the IAM User Guide.
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.

use std::time::SystemTime;

#[allow(unused_imports)]
use super::types::*;
use crate::state::STATUS_DELETING;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeregisterClusterError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeregisterClusterError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DeregisterCluster")]
#[derive(Debug)]
pub enum DeregisterClusterErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ClientException(ClientException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::deregister_cluster::DeregisterClusterError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeregisterClusterError = crate::scaffolded::Scaffolded<DeregisterClusterErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeregisterClusterErrorModeled> for DeregisterClusterError {
    fn from(modeled: DeregisterClusterErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeregisterCluster` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeregisterCluster<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeregisterCluster<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeregisterCluster<Eng>
{
    type Input = DeregisterClusterInput;
    type Output = DeregisterClusterOutput;
    type Error = DeregisterClusterError;

    const NAME: &'static str = "DeregisterCluster";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DEREGISTER_CLUSTER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DEREGISTER_CLUSTER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeregisterClusterError::from(DeregisterClusterErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let mut guard = store.lock();
        guard.materialize_due_transitions(SystemTime::now());

        // A cluster name identifies a cluster only within one region, so this
        // deregisters the registration in the region the request is signed for.
        let key = (region, input.name.clone());
        let Some(existing) = guard.clusters.get(&key) else {
            return Err(
                DeregisterClusterErrorModeled::ResourceNotFoundException(
                    crate::state::cluster_not_found(&input.name),
                )
                .into(),
            );
        };
        // "The name of the connected cluster to deregister": an Amazon EKS cluster is
        // not a registration, so there is nothing of it to deregister — its control
        // plane is deleted with `DeleteCluster`. That is a permanent fact about the
        // cluster rather than its state, which is what `ClientException` reports.
        if !existing.is_connected() {
            return Err(DeregisterClusterErrorModeled::ClientException(ClientException {
                cluster_name: Some(input.name.clone()),
                message: Some(format!(
                    "Cluster {} is an Amazon EKS cluster, not a connected cluster",
                    input.name
                )),
                ..Default::default()
            })
            .into());
        }

        let Some(mut record) = guard.clusters.remove(&key) else {
            return Err(
                DeregisterClusterErrorModeled::ResourceNotFoundException(
                    crate::state::cluster_not_found(&input.name),
                )
                .into(),
            );
        };
        guard.arn_index.remove(&record.arn);
        // A registration carries no resource of the Amazon EKS-managed plane — every
        // operation that would create one is refused for it — but the access entries and
        // the rest live ON the record, so removing it takes them with it. The maps keyed
        // by `(region, cluster)` are swept for the same reason `DeleteCluster` sweeps
        // them: a cluster later created under this name must inherit nothing.
        guard.remove_cluster_addons(&key.0, &key.1);
        guard.remove_capabilities_of(&key.0, &key.1);
        guard.remove_pod_identity_associations_for_cluster(&key.0, &key.1);
        guard.remove_nodegroups_for_cluster(&key.0, &key.1);
        record.status = STATUS_DELETING.to_string();
        // The registration is gone, so neither the activation the clock owed it nor its
        // lapse is owed to anybody any more.
        record.active_at = None;
        record.activation_expires_at = None;

        Ok(DeregisterClusterOutput {
            cluster: Some(record.to_cluster()),
        })
    }
}
