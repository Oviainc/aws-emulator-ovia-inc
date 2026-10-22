// SPDX-License-Identifier: CC0-1.0

//! @op DescribeEnvironments
//!
//! Reports the stored `Environment` record of every requested id, in request
//! order. A reporting operation: the only write it makes is materialising the
//! `creating` -> `ready` transition the clock has already made due (see
//! [`crate::state::Environment::settle`]). An id the store does not hold is a
//! `NotFoundException`.
//!
//! ---
//!
//! # Modeled contract for `DescribeEnvironments`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloud9-awscloud9workspacemanagementservice/smithy-model/com-amazonaws-cloud9-awscloud9workspacemanagementservice.json")
//! ```
//!
//! Gets information about Cloud9 development environments. Cloud9 is no longer available to
//! new customers. Existing customers of Cloud9 can continue to use the service as normal.
//! Learn more"
//!
//! ## Input
//!
//! * `environmentIds` (BoundedEnvironmentIdList, len 1..25) **required** — The IDs of
//!   individual environments to get information about.
//!
//! ## Output
//!
//! * `environments` (EnvironmentList) — Information about the environments that are returned.
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

use crate::state::Environment as EnvironmentRecord;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeEnvironmentsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloud9-awscloud9workspacemanagementservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeEnvironmentsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloud9#DescribeEnvironments")]
#[derive(Debug)]
pub enum DescribeEnvironmentsErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    LimitExceededException(LimitExceededException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_environments::DescribeEnvironmentsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeEnvironmentsError = crate::scaffolded::Scaffolded<DescribeEnvironmentsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeEnvironmentsErrorModeled> for DescribeEnvironmentsError {
    fn from(modeled: DescribeEnvironmentsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// `NotFoundException` — "The target resource cannot be found." `code` keeps
/// the model's declared default and `className` is left unset: the model gives
/// neither a value nor a rule for them.
fn not_found(environment_id: &str) -> DescribeEnvironmentsError {
    DescribeEnvironmentsErrorModeled::NotFoundException(NotFoundException {
        message: Some(format!("environment {environment_id} does not exist")),
        ..Default::default()
    })
    .into()
}

/// Project a stored environment onto the modeled `Environment` structure.
///
/// Every member comes from the record, so what a caller reads back is what some
/// earlier `CreateEnvironmentEC2`/`UpdateEnvironment` put there.
fn wire_environment(record: &EnvironmentRecord) -> Environment {
    Environment {
        id: Some(record.id.clone()),
        name: Some(record.name.clone()),
        description: record.description.clone(),
        r#type: record.env_type.as_wire().to_string(),
        connection_type: Some(record.connection_type.as_wire().to_string()),
        arn: record.arn.clone(),
        owner_arn: record.owner_arn.clone(),
        lifecycle: Some(EnvironmentLifecycle {
            status: Some(record.lifecycle.status.as_wire().to_string()),
            reason: record.lifecycle.reason.clone(),
            failure_resource: record.lifecycle.failure_resource.clone(),
        }),
        managed_credentials_status: record
            .managed_credentials_status
            .map(|status| status.as_wire().to_string()),
    }
}

/// The `DescribeEnvironments` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeEnvironments<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeEnvironments<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeEnvironments<Eng>
{
    type Input = DescribeEnvironmentsInput;
    type Output = DescribeEnvironmentsOutput;
    type Error = DescribeEnvironmentsError;

    const NAME: &'static str = "DescribeEnvironments";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ENVIRONMENTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ENVIRONMENTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeEnvironmentsErrorModeled::InternalServerErrorException(
                InternalServerErrorException {
                    message: Some("the Cloud9 environment store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        let mut store = state.lock();
        let now = SystemTime::now();

        let mut environments = Vec::with_capacity(input.environment_ids.len());
        for environment_id in &input.environment_ids {
            let record = store
                .environments
                .get_mut(environment_id)
                .ok_or_else(|| not_found(environment_id))?;
            // Only the environments this call resolves are settled, so the work
            // done under the state lock is bounded by the request's own modeled
            // `@length(max: 25)` rather than by the size of the store.
            record.settle(now);
            environments.push(wire_environment(record));
        }

        Ok(DescribeEnvironmentsOutput {
            environments: Some(environments),
        })
    }
}
