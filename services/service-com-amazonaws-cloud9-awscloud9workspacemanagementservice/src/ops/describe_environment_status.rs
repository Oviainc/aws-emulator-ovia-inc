// SPDX-License-Identifier: CC0-1.0

//! @op DescribeEnvironmentStatus
//!
//! Reports the stored status and status message of one environment. A reporting
//! operation: the only write it makes is materialising the `creating` ->
//! `ready` transition the clock has already made due (see
//! [`crate::state::Environment::settle`]). An id the store does not hold —
//! including one that was deleted — is a `NotFoundException`.
//!
//! ---
//!
//! # Modeled contract for `DescribeEnvironmentStatus`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloud9-awscloud9workspacemanagementservice/smithy-model/com-amazonaws-cloud9-awscloud9workspacemanagementservice.json")
//! ```
//!
//! Gets status information for an Cloud9 development environment. Cloud9 is no longer
//! available to new customers. Existing customers of Cloud9 can continue to use the service
//! as normal. Learn more"
//!
//! ## Input
//!
//! * `environmentId` (EnvironmentId, pattern ^[a-zA-Z0-9]{8,32}$) **required** — The ID of
//!   the environment to get status information about.
//!
//! ## Output
//!
//! * `message` (String) **required** — Any informational message about the status of the
//!   environment.
//! * `status` (EnvironmentStatus, one of: connecting | creating | deleting | error | ready |
//!   stopped | stopping) **required** — The status of the environment. Available values
//!   include: connecting: The environment is connecting. creating: The environment is being
//!   created. deleting: The environment is being deleted. error: The environment is in an
//!   error state. ready: The environment is ready. stopped: The environment is stopped.
//!   stopping: The environment is stopping.
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

use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeEnvironmentStatusError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloud9-awscloud9workspacemanagementservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeEnvironmentStatusError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloud9#DescribeEnvironmentStatus")]
#[derive(Debug)]
pub enum DescribeEnvironmentStatusErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    LimitExceededException(LimitExceededException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_environment_status::DescribeEnvironmentStatusError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeEnvironmentStatusError = crate::scaffolded::Scaffolded<DescribeEnvironmentStatusErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeEnvironmentStatusErrorModeled> for DescribeEnvironmentStatusError {
    fn from(modeled: DescribeEnvironmentStatusErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// `NotFoundException` — "The target resource cannot be found." `code` keeps
/// the model's declared default and `className` is left unset: the model gives
/// neither a value nor a rule for them.
fn not_found(environment_id: &str) -> DescribeEnvironmentStatusError {
    DescribeEnvironmentStatusErrorModeled::NotFoundException(NotFoundException {
        message: Some(format!("environment {environment_id} does not exist")),
        ..Default::default()
    })
    .into()
}

/// The `DescribeEnvironmentStatus` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeEnvironmentStatus<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeEnvironmentStatus<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeEnvironmentStatus<Eng>
{
    type Input = DescribeEnvironmentStatusInput;
    type Output = DescribeEnvironmentStatusOutput;
    type Error = DescribeEnvironmentStatusError;

    const NAME: &'static str = "DescribeEnvironmentStatus";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ENVIRONMENT_STATUS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ENVIRONMENT_STATUS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeEnvironmentStatusErrorModeled::InternalServerErrorException(
                InternalServerErrorException {
                    message: Some("the Cloud9 environment store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        let mut store = state.lock();
        let now = SystemTime::now();
        let record = store
            .environments
            .get_mut(&input.environment_id)
            .ok_or_else(|| not_found(&input.environment_id))?;
        record.settle(now);

        Ok(DescribeEnvironmentStatusOutput {
            status: record.status.as_wire().to_string(),
            message: record.status_message.clone(),
        })
    }
}
