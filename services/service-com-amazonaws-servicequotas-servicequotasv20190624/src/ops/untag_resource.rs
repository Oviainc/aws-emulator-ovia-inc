// SPDX-License-Identifier: CC0-1.0

//! @op UntagResource
//! Removes tags from an applied quota, named by its ARN.
//!
//! Removal is by `TagKey` and is idempotent: a key the quota does not carry is not an
//! error, and every other key it carries survives — so calling this twice with the
//! same `TagKeys` leaves the same set as calling it once.
//!
//! The resource is named only by `ResourceARN`, checked against the ARN this service
//! reports for an applied quota by
//! [`crate::state::ComAmazonawsServicequotasServicequotasv20190624State::resolve_quota_arn`]:
//! an ARN that is not an applied-quota ARN is `IllegalArgumentException`, and one
//! that names no quota of this account is `NoSuchResourceException`.
//!
//! ---
//!
//! # Modeled contract for `UntagResource`
//!
//! For the FULL text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Removes tags from the specified applied quota. You can specify one or more tags to
//! remove.
//!
//! ## Input
//!
//! * `ResourceARN` (AmazonResourceName, len 1..1011, pattern
//!   ^arn:aws(-[\w]+)*:*:.+:[0-9]{12}:.+$) **required** — The Amazon Resource Name (ARN) for
//!   the applied quota that you want to untag. You can get this information by using the
//!   Service Quotas console, or by listing the quotas using the list-service-quotas CLI
//!   command or the ListServiceQuotas Amazon Web Services API operation.
//! * `TagKeys` (InputTagKeys) **required** — The keys of the tags that you want to remove
//!   from the resource.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permission to perform
//!   this action.
//! * `IllegalArgumentException` (HTTP 400) — Invalid input was provided.
//! * `NoSuchResourceException` (HTTP 404) — The specified resource does not exist.
//! * `ServiceException` (HTTP 500) — Something went wrong.
//! * `TooManyRequestsException` (HTTP 429) — Due to throttling, the request was denied. Slow
//!   down the rate of request calls, or request an increase for this quota.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UntagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UntagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#UntagResource")]
#[derive(Debug)]
pub enum UntagResourceErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::untag_resource::UntagResourceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UntagResourceError = crate::scaffolded::Scaffolded<UntagResourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UntagResourceErrorModeled> for UntagResourceError {
    fn from(modeled: UntagResourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UntagResource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UntagResource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UntagResource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UntagResource<Eng>
{
    type Input = UntagResourceInput;
    type Output = UntagResourceOutput;
    type Error = UntagResourceError;

    const NAME: &'static str = "UntagResource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UNTAG_RESOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UNTAG_RESOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The resource is named by its ARN, which carries its own region (and, for a
        // global quota, deliberately carries none), so this operation reads nothing
        // from the caller's endpoint.
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UntagResourceErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;
        let mut quotas_state = state.lock();

        let canonical_arn = quotas_state
            .resolve_quota_arn(&input.resource_arn)
            .map_err(|error| match error {
                crate::state::ResourceArnError::Malformed => {
                    UntagResourceErrorModeled::IllegalArgumentException(IllegalArgumentException {
                        message: Some(crate::state::malformed_resource_arn_message(
                            &input.resource_arn,
                        )),
                    })
                }
                crate::state::ResourceArnError::NoSuchQuota => {
                    UntagResourceErrorModeled::NoSuchResourceException(NoSuchResourceException {
                        message: Some(crate::state::no_such_resource_arn_message(
                            &input.resource_arn,
                        )),
                    })
                }
            })?;

        quotas_state.remove_resource_tags(&canonical_arn, &input.tag_keys);

        Ok(UntagResourceOutput {})
    }
}
