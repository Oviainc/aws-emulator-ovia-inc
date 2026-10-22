// SPDX-License-Identifier: CC0-1.0

//! @op DescribeCapability
//! IMPLEMENTED. Reports the [`crate::state::CapabilityRecord`] `CreateCapability` wrote
//! for one capability of one cluster, materialising only the status move the clock
//! already owes it. An unknown cluster, or a cluster that holds no capability by that
//! name, is a `ResourceNotFoundException`; a capability is looked up in the region the
//! request is signed for, because the cluster that holds it is.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeCapability`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Returns detailed information about a specific managed capability in your Amazon EKS
//! cluster, including its current status, configuration, health information, and any issues
//! that may be affecting its operation.
//!
//! ## Input
//!
//! * `capabilityName` (String) **required** — The name of the capability to describe.
//! * `clusterName` (String) **required** — The name of the Amazon EKS cluster that contains
//!   the capability you want to describe.
//!
//! ## Output
//!
//! * `capability` (Capability) — An object containing detailed information about the
//!   capability, including its name, ARN, type, status, version, configuration, health
//!   status, and timestamps for when it was created and last modified.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have permissions to perform the requested
//!   operation. The IAM principal making the request must have at least one IAM permissions
//!   policy attached that grants the required permissions. For more information, see Access
//!   management in the IAM User Guide.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeCapabilityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeCapabilityError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DescribeCapability")]
#[derive(Debug)]
pub enum DescribeCapabilityErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidParameterException(InvalidParameterException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_capability::DescribeCapabilityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeCapabilityError = crate::scaffolded::Scaffolded<DescribeCapabilityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeCapabilityErrorModeled> for DescribeCapabilityError {
    fn from(modeled: DescribeCapabilityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeCapability` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeCapability<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeCapability<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeCapability<Eng>
{
    type Input = DescribeCapabilityInput;
    type Output = DescribeCapabilityOutput;
    type Error = DescribeCapabilityError;

    const NAME: &'static str = "DescribeCapability";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CAPABILITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CAPABILITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeCapabilityError::from(DescribeCapabilityErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let not_found = |message: String| {
            DescribeCapabilityError::from(
                DescribeCapabilityErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };

        let mut guard = store.lock();
        guard.materialize_due_transitions(SystemTime::now());

        // A cluster name identifies a cluster only within one region, and a capability
        // only within its cluster, so the record this reads is the one in the region the
        // request is signed for.
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
        match guard.capabilities.get(&key) {
            Some(record) => Ok(DescribeCapabilityOutput {
                capability: Some(record.to_capability()),
            }),
            None => Err(not_found(format!(
                "No capability found for name: {} in cluster: {}.",
                input.capability_name, input.cluster_name
            ))),
        }
    }
}
