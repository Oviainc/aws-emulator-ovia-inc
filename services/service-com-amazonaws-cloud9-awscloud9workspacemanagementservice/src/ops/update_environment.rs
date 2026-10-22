// SPDX-License-Identifier: CC0-1.0

//! @op UpdateEnvironment
//!
//! Mutates the stored environment record in place: a member the request carries
//! replaces the stored value, a member it omits leaves the stored value
//! untouched. Output is empty, so the change is only observable through a later
//! `DescribeEnvironments`. An id the store does not hold is a
//! `NotFoundException`.
//!
//! ---
//!
//! # Modeled contract for `UpdateEnvironment`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloud9-awscloud9workspacemanagementservice/smithy-model/com-amazonaws-cloud9-awscloud9workspacemanagementservice.json")
//! ```
//!
//! Changes the settings of an existing Cloud9 development environment. Cloud9 is no longer
//! available to new customers. Existing customers of Cloud9 can continue to use the service
//! as normal. Learn more"
//!
//! ## Input
//!
//! * `description` (EnvironmentDescription, len 0..200) — Any new or replacement description
//!   for the environment.
//! * `environmentId` (EnvironmentId, pattern ^[a-zA-Z0-9]{8,32}$) **required** — The ID of
//!   the environment to change settings.
//! * `managedCredentialsAction` (ManagedCredentialsAction, one of: DISABLE | ENABLE) — Allows
//!   the environment owner to turn on or turn off the Amazon Web Services managed temporary
//!   credentials for an Cloud9 environment by using one of the following values: ENABLE
//!   DISABLE Only the environment owner can change the status of managed temporary
//!   credentials. An AccessDeniedException is thrown if an attempt to turn on or turn off
//!   managed temporary credentials is made by an account that's not the environment owner.
//! * `name` (EnvironmentName, len 1..60) — A replacement name for the environment.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The target request is invalid.
//! * `ConflictException` (HTTP 400) — A conflict occurred.
//! * `ForbiddenException` (HTTP 400) — An access permissions issue occurred.
//! * `InternalServerErrorException` (HTTP 500) — An internal server error occurred.
//! * `LimitExceededException` (HTTP 400) — A service limit was exceeded.
//! * `NotFoundException` (HTTP 400) — The target resource cannot be found.
//! * `TooManyRequestsException` (HTTP 400) — Too many service requests were made over the
//!   given time period.

#[allow(unused_imports)]
use super::types::*;

use crate::state::ManagedCredentialsAction;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateEnvironmentError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloud9-awscloud9workspacemanagementservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateEnvironmentError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloud9#UpdateEnvironment")]
#[derive(Debug)]
pub enum UpdateEnvironmentErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    LimitExceededException(LimitExceededException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_environment::UpdateEnvironmentError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateEnvironmentError = crate::scaffolded::Scaffolded<UpdateEnvironmentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateEnvironmentErrorModeled> for UpdateEnvironmentError {
    fn from(modeled: UpdateEnvironmentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// `BadRequestException` — "The target request is invalid." `code` keeps the
/// model's declared default and `className` is left unset: the model gives
/// neither a value nor a rule for them.
fn bad_request(message: String) -> UpdateEnvironmentError {
    UpdateEnvironmentErrorModeled::BadRequestException(BadRequestException {
        message: Some(message),
        ..Default::default()
    })
    .into()
}

/// `NotFoundException` — "The target resource cannot be found."
fn not_found(environment_id: &str) -> UpdateEnvironmentError {
    UpdateEnvironmentErrorModeled::NotFoundException(NotFoundException {
        message: Some(format!("environment {environment_id} does not exist")),
        ..Default::default()
    })
    .into()
}

/// The `UpdateEnvironment` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateEnvironment<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateEnvironment<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateEnvironment<Eng>
{
    type Input = UpdateEnvironmentInput;
    type Output = UpdateEnvironmentOutput;
    type Error = UpdateEnvironmentError;

    const NAME: &'static str = "UpdateEnvironment";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_ENVIRONMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_ENVIRONMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateEnvironmentErrorModeled::InternalServerErrorException(
                InternalServerErrorException {
                    message: Some("the Cloud9 environment store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        // A value outside the enum is refused at the boundary, so reaching here
        // with an unknown one would mean the model changed under us — refuse it
        // as the invalid request it is rather than silently ignoring it.
        let credentials_action = match input.managed_credentials_action.as_deref() {
            None => None,
            Some(value) => Some(ManagedCredentialsAction::from_wire(value).ok_or_else(|| {
                bad_request(format!(
                    "managedCredentialsAction '{value}' is not a supported action"
                ))
            })?),
        };

        let mut store = state.lock();
        let record = store
            .environments
            .get_mut(&input.environment_id)
            .ok_or_else(|| not_found(&input.environment_id))?;

        // Present members replace; absent members leave the stored value alone.
        if let Some(name) = input.name {
            record.name = name;
        }
        if let Some(description) = input.description {
            record.description = Some(description);
        }
        if let Some(action) = credentials_action {
            record.managed_credentials_status = Some(action.resulting_status());
        }

        Ok(UpdateEnvironmentOutput {})
    }
}
