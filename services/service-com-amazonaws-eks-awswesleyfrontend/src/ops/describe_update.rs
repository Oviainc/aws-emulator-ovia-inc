// SPDX-License-Identifier: CC0-1.0

//! @op DescribeUpdate
//!
//! IMPLEMENTED. `DescribeUpdate` reports one [`crate::state::UpdateRecord`] held on its
//! cluster's record in the shared engine's store — the record the call that started the
//! update wrote (`AssociateEncryptionConfig` today) — so a caller polls the update it
//! was handed until the clock moves it to `Successful`.
//!
//! It is a REPORTING operation: the only write it makes is materialising the status
//! moves the clock already owed, which were due whether or not anybody called.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeUpdate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Describes an update to an Amazon EKS resource. When the status of the update is
//! Successful, the update is complete. If an update fails, the status is Failed, and an
//! error detail explains the reason for the failure.
//!
//! ## Input
//!
//! * `addonName` (String) — The name of the add-on. The name must match one of the names
//!   returned by ListAddons . This parameter is required if the update is an add-on update.
//! * `capabilityName` (String) — The name of the capability for which you want to describe
//!   updates.
//! * `name` (String) **required** — The name of the Amazon EKS cluster associated with the
//!   update.
//! * `nodegroupName` (String) — The name of the Amazon EKS node group associated with the
//!   update. This parameter is required if the update is a node group update.
//! * `updateId` (String) **required** — The ID of the update to describe.
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
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

use std::time::SystemTime;

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeUpdateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeUpdateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DescribeUpdate")]
#[derive(Debug)]
pub enum DescribeUpdateErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_update::DescribeUpdateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeUpdateError = crate::scaffolded::Scaffolded<DescribeUpdateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeUpdateErrorModeled> for DescribeUpdateError {
    fn from(modeled: DescribeUpdateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeUpdate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeUpdate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeUpdate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeUpdate<Eng>
{
    type Input = DescribeUpdateInput;
    type Output = DescribeUpdateOutput;
    type Error = DescribeUpdateError;

    const NAME: &'static str = "DescribeUpdate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_UPDATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_UPDATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeUpdateError::from(DescribeUpdateErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let mut guard = store.lock();
        guard.materialize_due_transitions(SystemTime::now());

        // A cluster name identifies a cluster only within one region, so the updates this
        // reads are the ones of the cluster in the region the request is signed for.
        let Some(record) = guard.clusters.get(&(region, input.name.clone())) else {
            return Err(DescribeUpdateErrorModeled::ResourceNotFoundException(
                crate::state::cluster_not_found(&input.name),
            )
            .into());
        };

        // Each of these selects the updates of a resource INSIDE the cluster: "this
        // parameter is required if the update is a node group update", and the same for an
        // add-on or a capability. This service holds no nodegroup, add-on or capability at
        // all (each create is still 501 with its own gap issue), so an update scoped to
        // one cannot be found.
        for (member, value) in [
            ("nodegroupName", input.nodegroup_name.as_deref()),
            ("addonName", input.addon_name.as_deref()),
            ("capabilityName", input.capability_name.as_deref()),
        ] {
            if let Some(value) = value {
                // The UPDATE is the resource reported missing, so the message names it
                // and the member that scoped the lookup; the exception's identity
                // members stay unset — EKS does not echo the request's
                // `nodegroupName`/`addonName` into a not-found payload (see
                // [`crate::state::child_resource_not_found`]).
                return Err(DescribeUpdateErrorModeled::ResourceNotFoundException(
                    crate::state::child_resource_not_found(format!(
                        "No update found for ID: {} with {member}: {value}.",
                        input.update_id
                    )),
                )
                .into());
            }
        }

        match record.updates.get(&input.update_id) {
            Some(update) => Ok(DescribeUpdateOutput {
                update: Some(update.to_update()),
            }),
            None => Err(DescribeUpdateErrorModeled::ResourceNotFoundException(
                crate::state::child_resource_not_found(format!(
                    "No update found for ID: {}.",
                    input.update_id
                )),
            )
            .into()),
        }
    }
}
