// SPDX-License-Identifier: CC0-1.0

//! @op DeleteCapability
//! IMPLEMENTED. Removes one capability of one cluster from the store and answers with
//! the record it removed, reporting `DELETING`. A second `DeleteCapability`, or any later
//! `DescribeCapability`, for that name is then a `ResourceNotFoundException`, and
//! `ListCapabilities` no longer lists it — the same shape `DeleteCluster` has for a
//! cluster.
//!
//! "Amazon EKS removes the capability infrastructure but retains all resources that were
//! managed by the capability" — `deletePropagationPolicy` is modelled with the single
//! value `RETAIN`, so there is nothing for this emulator to cascade: the Kubernetes
//! resources a capability managed are not this service's state. The cluster itself is
//! untouched; an unknown cluster, or a cluster that holds no capability by that name, is a
//! `ResourceNotFoundException`.
//!
//! Deleting a capability that is still `CREATING` is NOT refused: AWS publishes no
//! status-based precondition for this call, so none is invented, and the record leaves the
//! store whatever status it had reached.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteCapability`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Deletes a managed capability from your Amazon EKS cluster. When you delete a capability,
//! Amazon EKS removes the capability infrastructure but retains all resources that were
//! managed by the capability. Before deleting a capability, you should delete all
//! Kubernetes resources that were created by the capability. After the capability is
//! deleted, these resources become difficult to manage because the controller that managed
//! them is no longer available. To delete resources before removing the capability, use
//! kubectl delete or remove them through your GitOps workflow.
//!
//! ## Input
//!
//! * `capabilityName` (String) **required** — The name of the capability to delete.
//! * `clusterName` (String) **required** — The name of the Amazon EKS cluster that contains
//!   the capability you want to delete.
//!
//! ## Output
//!
//! * `capability` (Capability) — An object containing information about the deleted
//!   capability, including its final status and configuration.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have permissions to perform the requested
//!   operation. The IAM principal making the request must have at least one IAM permissions
//!   policy attached that grants the required permissions. For more information, see Access
//!   management in the IAM User Guide.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::CAPABILITY_STATUS_DELETING;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteCapabilityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteCapabilityError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DeleteCapability")]
#[derive(Debug)]
pub enum DeleteCapabilityErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidParameterException(InvalidParameterException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_capability::DeleteCapabilityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteCapabilityError = crate::scaffolded::Scaffolded<DeleteCapabilityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteCapabilityErrorModeled> for DeleteCapabilityError {
    fn from(modeled: DeleteCapabilityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteCapability` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteCapability<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteCapability<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteCapability<Eng>
{
    type Input = DeleteCapabilityInput;
    type Output = DeleteCapabilityOutput;
    type Error = DeleteCapabilityError;

    const NAME: &'static str = "DeleteCapability";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_CAPABILITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_CAPABILITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteCapabilityError::from(DeleteCapabilityErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let not_found = |message: String| {
            DeleteCapabilityError::from(DeleteCapabilityErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };

        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // A capability is addressed through its cluster, and a cluster name identifies
        // a cluster only within one region, so this deletes the capability of the
        // cluster in the region the request is signed for.
        if !guard
            .clusters
            .contains_key(&(region.clone(), input.cluster_name.clone()))
        {
            return Err(not_found(format!(
                "No cluster found for name: {}.",
                input.cluster_name
            )));
        }

        let key = (region, input.cluster_name.clone(), input.capability_name.clone());
        let Some(mut record) = guard.capabilities.remove(&key) else {
            return Err(not_found(format!(
                "No capability found for name: {} in cluster: {}.",
                input.capability_name, input.cluster_name
            )));
        };

        // The answer reports "its final status": the capability is on its way out.
        record.status = CAPABILITY_STATUS_DELETING.to_string();
        // The capability is gone, so the `CREATING` -> `ACTIVE` move the clock owed it
        // is no longer owed to anybody.
        record.active_at = None;
        record.modified_at = now;

        Ok(DeleteCapabilityOutput {
            capability: Some(record.to_capability()),
        })
    }
}
