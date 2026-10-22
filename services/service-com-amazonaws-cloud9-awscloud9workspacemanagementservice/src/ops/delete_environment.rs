// SPDX-License-Identifier: CC0-1.0

//! @op DeleteEnvironment
//!
//! Removes the environment — with its memberships and tags, which the record
//! owns — from the store and answers with an empty body. Every later
//! `DescribeEnvironments`, `DescribeEnvironmentStatus` or `UpdateEnvironment`
//! on that id answers `NotFoundException`, and `ListEnvironments` no longer
//! lists it.
//!
//! ---
//!
//! # Modeled contract for `DeleteEnvironment`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloud9-awscloud9workspacemanagementservice/smithy-model/com-amazonaws-cloud9-awscloud9workspacemanagementservice.json")
//! ```
//!
//! Deletes an Cloud9 development environment. If an Amazon EC2 instance is connected to the
//! environment, also terminates the instance. Cloud9 is no longer available to new
//! customers. Existing customers of Cloud9 can continue to use the service as normal. Learn
//! more"
//!
//! ## Input
//!
//! * `environmentId` (EnvironmentId, pattern ^[a-zA-Z0-9]{8,32}$) **required** — The ID of
//!   the environment to delete.
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

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteEnvironmentError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloud9-awscloud9workspacemanagementservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteEnvironmentError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloud9#DeleteEnvironment")]
#[derive(Debug)]
pub enum DeleteEnvironmentErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    LimitExceededException(LimitExceededException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_environment::DeleteEnvironmentError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteEnvironmentError = crate::scaffolded::Scaffolded<DeleteEnvironmentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteEnvironmentErrorModeled> for DeleteEnvironmentError {
    fn from(modeled: DeleteEnvironmentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// `NotFoundException` — "The target resource cannot be found." `code` keeps
/// the model's declared default and `className` is left unset: the model gives
/// neither a value nor a rule for them.
fn not_found(environment_id: &str) -> DeleteEnvironmentError {
    DeleteEnvironmentErrorModeled::NotFoundException(NotFoundException {
        message: Some(format!("environment {environment_id} does not exist")),
        ..Default::default()
    })
    .into()
}

/// The `DeleteEnvironment` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteEnvironment<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteEnvironment<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteEnvironment<Eng>
{
    type Input = DeleteEnvironmentInput;
    type Output = DeleteEnvironmentOutput;
    type Error = DeleteEnvironmentError;

    const NAME: &'static str = "DeleteEnvironment";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_ENVIRONMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_ENVIRONMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteEnvironmentErrorModeled::InternalServerErrorException(
                InternalServerErrorException {
                    message: Some("the Cloud9 environment store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        let mut store = state.lock();
        // Removing the record removes the memberships and tags it owns with it.
        if store.environments.remove(&input.environment_id).is_none() {
            return Err(not_found(&input.environment_id));
        }
        // Drop the idempotency entries that named it, so a repeated
        // `CreateEnvironmentEC2` with the same token creates a new environment
        // rather than reporting an id that no longer resolves.
        store
            .idempotency
            .retain(|_, created| created != &input.environment_id);

        Ok(DeleteEnvironmentOutput {})
    }
}
