// SPDX-License-Identifier: CC0-1.0

//! @op BatchGetApplications
//! Answers the stored `ApplicationInfo` of each application the request names, in
//! the order it named them.
//!
//! This is the ONLY file to touch for this operation.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `BatchGetApplications::new(engine)` and drives it through the centralized request walk:
//! deserialize -> validate -> `handle` -> serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request, so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::with_state`], which seeds the built-ins and applies every
//! clock-owed transition before the body it is given reads the store.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only:
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! ---
//!
//! # Modeled contract for `BatchGetApplications`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json")
//! ```
//!
//! Gets information about one or more applications. The maximum number of applications that
//! can be returned is 100.
//!
//! ## Input
//!
//! * `applicationNames` (ApplicationsList) **required** - A list of application names
//!   separated by spaces. The maximum number of application names you can specify is 100.
//!
//! ## Output
//!
//! * `applicationsInfo` (ApplicationsInfoList) - Information about the applications.
//!
//! ## Errors
//!
//! * `ApplicationDoesNotExistException` (HTTP 400) - The application does not exist with the
//!   user or Amazon Web Services account.
//! * `ApplicationNameRequiredException` (HTTP 400) - The minimum number of required
//!   application names was not specified.
//! * `BatchLimitExceededException` (HTTP 400) - The maximum number of names or IDs allowed
//!   for this request (100) was exceeded.
//! * `InvalidApplicationNameException` (HTTP 400) - The application name was specified in an
//!   invalid format.

#[allow(unused_imports)]
use super::types::*;

/// How many application names one `BatchGetApplications` request may carry.
///
/// AWS documents the bound in the operation's own text ("The maximum number of
/// applications that can be returned is 100", "The maximum number of application
/// names you can specify is 100") and models `BatchLimitExceededException` as the
/// refusal past it, so the value is NOT chosen here.
const MAX_BATCH_NAMES: usize = 100;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `BatchGetApplicationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here, hence the `BatchGetApplicationsError` alias below.
#[smithy_macros::operation_error("com.amazonaws.codedeploy#BatchGetApplications")]
#[derive(Debug)]
pub enum BatchGetApplicationsErrorModeled {
    ApplicationDoesNotExistException(ApplicationDoesNotExistException),
    ApplicationNameRequiredException(ApplicationNameRequiredException),
    BatchLimitExceededException(BatchLimitExceededException),
    InvalidApplicationNameException(InvalidApplicationNameException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::batch_get_applications::BatchGetApplicationsError`). The wrapper also
/// carries the `ValidationException` a refusal this operation models no code for is
/// answered with.
pub type BatchGetApplicationsError = crate::scaffolded::Scaffolded<BatchGetApplicationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<BatchGetApplicationsErrorModeled> for BatchGetApplicationsError {
    fn from(modeled: BatchGetApplicationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `BatchGetApplications` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct BatchGetApplications<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> BatchGetApplications<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE: the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for BatchGetApplications<Eng>
{
    type Input = BatchGetApplicationsInput;
    type Output = BatchGetApplicationsOutput;
    type Error = BatchGetApplicationsError;

    const NAME: &'static str = "BatchGetApplications";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_GET_APPLICATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_GET_APPLICATIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        // `applicationNames` is @required and each MEMBER carries the modelled length
        // and pattern, so the boundary has already refused an absent list and any name
        // that is empty or malformed (`service.rs` words those as this member's
        // `ApplicationNameRequiredException` / `InvalidApplicationNameException`).
        // `ApplicationsList` itself is unconstrained, so the two bounds on the BATCH,
        // at least one name and at most `MAX_BATCH_NAMES`, are this handler's to
        // enforce.
        let names = input.application_names;
        if names.is_empty() {
            return Err(
                BatchGetApplicationsErrorModeled::ApplicationNameRequiredException(
                    ApplicationNameRequiredException {
                        message: Some(
                            "At least one application name must be specified".to_string(),
                        ),
                    },
                )
                .into(),
            );
        }
        if names.len() > MAX_BATCH_NAMES {
            return Err(BatchGetApplicationsErrorModeled::BatchLimitExceededException(
                BatchLimitExceededException {
                    message: Some(format!(
                        "The maximum number of names or IDs allowed for this request \
                         ({MAX_BATCH_NAMES}) was exceeded: {} were specified",
                        names.len()
                    )),
                },
            )
            .into());
        }

        // One `ApplicationInfo` per requested name, read from the store in the order
        // the request named them, so a caller can line the answer up with its input.
        // A name that is not stored fails the whole batch: `applicationsInfo` carries
        // no per-name error channel, so a short list would silently drop the refusal.
        let found = crate::state::with_state(&*self.engine, |state| {
            names
                .iter()
                .map(|name| {
                    state
                        .applications
                        .get(name)
                        .map(crate::state::application_info)
                        .ok_or_else(|| name.clone())
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .ok_or_else(|| {
            crate::scaffolded::Scaffolded::validation(crate::state::STATE_UNAVAILABLE)
        })?;

        let applications_info = found.map_err(|missing| {
            BatchGetApplicationsErrorModeled::ApplicationDoesNotExistException(
                ApplicationDoesNotExistException {
                    message: Some(format!("No application found for name {missing}")),
                },
            )
        })?;

        Ok(BatchGetApplicationsOutput {
            applications_info: Some(applications_info),
        })
    }
}
