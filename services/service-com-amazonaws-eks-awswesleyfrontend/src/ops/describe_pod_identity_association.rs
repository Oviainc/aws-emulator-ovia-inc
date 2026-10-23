// SPDX-License-Identifier: CC0-1.0

//! @op DescribePodIdentityAssociation
//! IMPLEMENTED. A pure read of the
//! [`crate::state::PodIdentityAssociationRecord`] `CreatePodIdentityAssociation`
//! wrote: the `PodIdentityAssociation` this answers with carries the stored
//! `associationId`, `associationArn`, `externalId`, `createdAt` and `modifiedAt`,
//! not a recomputed copy, and this operation writes nothing of its own.
//!
//! The identifiers the request carries are checked for their documented FORM
//! before any resource is resolved — an `associationId` outside `a-` plus 17
//! lowercase alphanumerics, or a `clusterName` outside the cluster-name pattern,
//! names nothing in any account or Region, so it earns an
//! `InvalidParameterException` (400) and never a not-found. A well-formed id for
//! a cluster or an association this store does not hold is the 404.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribePodIdentityAssociation`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Returns descriptive information about an EKS Pod Identity association. This action
//! requires the ID of the association. You can get the ID from the response to the
//! CreatePodIdentityAssocation for newly created associations. Or, you can list the IDs for
//! associations with ListPodIdentityAssociations and filter the list by namespace or
//! service account.
//!
//! ## Input
//!
//! * `associationId` (String) **required** — The ID of the association that you want the
//!   description of.
//! * `clusterName` (String) **required** — The name of the cluster that the association is
//!   in.
//!
//! ## Output
//!
//! * `association` (PodIdentityAssociation) — The full description of the EKS Pod Identity
//!   association.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribePodIdentityAssociationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribePodIdentityAssociationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DescribePodIdentityAssociation")]
#[derive(Debug)]
pub enum DescribePodIdentityAssociationErrorModeled {
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_pod_identity_association::DescribePodIdentityAssociationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribePodIdentityAssociationError = crate::scaffolded::Scaffolded<DescribePodIdentityAssociationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribePodIdentityAssociationErrorModeled> for DescribePodIdentityAssociationError {
    fn from(modeled: DescribePodIdentityAssociationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribePodIdentityAssociation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribePodIdentityAssociation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribePodIdentityAssociation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribePodIdentityAssociation<Eng>
{
    type Input = DescribePodIdentityAssociationInput;
    type Output = DescribePodIdentityAssociationOutput;
    type Error = DescribePodIdentityAssociationError;

    const NAME: &'static str = "DescribePodIdentityAssociation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_POD_IDENTITY_ASSOCIATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_POD_IDENTITY_ASSOCIATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribePodIdentityAssociationError::from(
                DescribePodIdentityAssociationErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        let invalid_parameter = |message: String| {
            DescribePodIdentityAssociationError::from(
                DescribePodIdentityAssociationErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };

        // FORM FIRST: neither label carries a modelled constraint, so nothing
        // refused a malformed one before this handler ran — and a value outside
        // its documented form identifies no resource anywhere, which is an
        // invalid parameter rather than a resource that could not be found.
        if !state::is_cluster_name(&input.cluster_name) {
            return Err(invalid_parameter(state::cluster_name_refusal(
                "clusterName",
                &input.cluster_name,
            )));
        }
        if !state::is_association_id(&input.association_id) {
            return Err(invalid_parameter(state::association_id_refusal(
                "associationId",
                &input.association_id,
            )));
        }

        let region = state::request_region(ctx.parts());
        let mut guard = store.lock();
        // The one write a read may make: the cluster status moves the CLOCK
        // already owed, due whether or not anybody called. Nothing about an
        // association is touched here.
        guard.materialize_due_transitions(SystemTime::now());

        // The two halves report DIFFERENT resources, so they carry different
        // payloads: an absent cluster is reported as the cluster (named, with
        // `clusterName` set), an absent association as the association (named by its
        // id, with no identity member) — see [`crate::state::cluster_not_found`].
        let not_found = |exception: ResourceNotFoundException| {
            DescribePodIdentityAssociationError::from(
                DescribePodIdentityAssociationErrorModeled::ResourceNotFoundException(exception),
            )
        };

        // A cluster name identifies a cluster only within one Region, and an
        // association only exists inside its cluster, so the absent cluster is
        // reported before the association is looked for.
        if !guard
            .clusters
            .contains_key(&(region.clone(), input.cluster_name.clone()))
        {
            return Err(not_found(crate::state::cluster_not_found(
                &input.cluster_name,
            )));
        }

        let key = (region, input.cluster_name.clone(), input.association_id.clone());
        match guard.pod_identity_associations.get(&key) {
            Some(record) => Ok(DescribePodIdentityAssociationOutput {
                association: Some(record.to_association()),
            }),
            None => Err(not_found(crate::state::child_resource_not_found(format!(
                "No pod identity association found for ID: {}.",
                input.association_id
            )))),
        }
    }
}
