// SPDX-License-Identifier: CC0-1.0

//! @op GetDeploymentStatus
//!
//! Reports the stored deployment: the `DeploymentType` it was created with, the
//! `UpdatedAt` of its last status change, and its `DeploymentStatus`. A reporting
//! operation — the only write it makes is materialising a status transition the
//! clock had already made due (`Building` -> `InProgress` -> `Success`), which was
//! owed at that instant whether or not anybody polled.
//!
//! What `Success` here does and does not claim: a Greengrass V1 deployment record is
//! control-plane state, and that part is real — the group version the deployment
//! names is a version this service actually stored, and the record is read back by
//! id. No core device exists to deliver anything to, so `Success` reports that the
//! deployment was accepted and recorded, not that software reached a device. The
//! modelled `Failure` path (`DeploymentStatus` `Failure` with `ErrorMessage` /
//! `ErrorDetails`) has no producer yet for the same reason: nothing in this slice can
//! observe a delivery failing. `ResetDeployments` and the bulk-deployment operations,
//! which are where the remaining deployment behaviour lives, are still stubs with
//! their own gap issues.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `GetDeploymentStatus`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-greengrass-greengrass/smithy-model/com-amazonaws-greengrass-greengrass.json")
//! ```
//!
//! Returns the status of a deployment.
//!
//! ## Input
//!
//! * `DeploymentId` (__string) **required** — The ID of the deployment.
//! * `GroupId` (__string) **required** — The ID of the Greengrass group.
//!
//! ## Output
//!
//! * `DeploymentStatus` (__string) — The status of the deployment: ''InProgress'',
//!   ''Building'', ''Success'', or ''Failure''.
//! * `DeploymentType` (DeploymentType, one of: ForceResetDeployment | NewDeployment |
//!   Redeployment | ResetDeployment) — The type of the deployment.
//! * `ErrorDetails` (ErrorDetails) — Error details
//! * `ErrorMessage` (__string) — Error message
//! * `UpdatedAt` (__string) — The time, in milliseconds since the epoch, when the deployment
//!   status was updated.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — General error information.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetDeploymentStatusError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-greengrass-greengrass.json");

// The operation's MODELED errors. `InternalServerErrorException` is the service's own
// modelled server error (HTTP 500), widened in through `extra_errors` so the one
// unreachable-store path has a modelled answer.
#[smithy_macros::operation_error(
    "com.amazonaws.greengrass#GetDeploymentStatus",
    extra_errors = ["InternalServerErrorException"]
)]
#[derive(Debug)]
pub enum GetDeploymentStatusErrorModeled {
    BadRequestException(BadRequestException),
    InternalServerErrorException(InternalServerErrorException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_deployment_status::GetDeploymentStatusError`).
pub type GetDeploymentStatusError = crate::scaffolded::Scaffolded<GetDeploymentStatusErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetDeploymentStatusErrorModeled> for GetDeploymentStatusError {
    fn from(modeled: GetDeploymentStatusErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetDeploymentStatus` operation.
pub struct GetDeploymentStatus<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetDeploymentStatus<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetDeploymentStatus<Eng>
{
    type Input = GetDeploymentStatusInput;
    type Output = GetDeploymentStatusOutput;
    type Error = GetDeploymentStatusError;

    const NAME: &'static str = "GetDeploymentStatus";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_DEPLOYMENT_STATUS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_DEPLOYMENT_STATUS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetDeploymentStatusErrorModeled::InternalServerErrorException(
                crate::state::store_unreachable(),
            )
        })?;

        let mut store = state.lock();
        let group = store.groups.get_mut(&input.group_id).ok_or_else(|| {
            GetDeploymentStatusErrorModeled::BadRequestException(crate::state::bad_request(
                format!("That Group Definition does not exist: {}", input.group_id),
            ))
        })?;
        let deployment = group.deployment_mut(&input.deployment_id).ok_or_else(|| {
            GetDeploymentStatusErrorModeled::BadRequestException(crate::state::bad_request(
                format!(
                    "Deployment {} of group {} does not exist",
                    input.deployment_id, input.group_id
                ),
            ))
        })?;

        // Apply whatever the clock already owed this deployment, and stamp
        // `UpdatedAt` with the moment the status actually changed.
        if deployment.settle(std::time::Instant::now()) {
            deployment.updated_at = crate::state::now_timestamp();
        }

        Ok(GetDeploymentStatusOutput {
            deployment_status: Some(deployment.status.clone()),
            deployment_type: Some(deployment.deployment_type.clone()),
            error_details: (!deployment.error_details.is_empty())
                .then(|| deployment.error_details.clone()),
            error_message: deployment.error_message.clone(),
            updated_at: Some(deployment.updated_at.clone()),
        })
    }
}
