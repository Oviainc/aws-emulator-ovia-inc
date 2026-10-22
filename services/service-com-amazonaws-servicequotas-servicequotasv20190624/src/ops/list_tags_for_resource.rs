// SPDX-License-Identifier: CC0-1.0

//! @op ListTagsForResource
//! Returns the tags assigned to an applied quota, named by its ARN.
//!
//! A reporting operation: it reads the tag set `TagResource` and `UntagResource`
//! wrote and changes nothing. `Tags` is the whole set in `TagKey` order — an empty
//! list for a quota nothing has tagged, which the model's `OutputTags` (len 0..200)
//! allows, rather than an absent member.
//!
//! The resource is named only by `ResourceARN`, checked against the ARN this service
//! reports for an applied quota by
//! [`crate::state::ComAmazonawsServicequotasServicequotasv20190624State::resolve_quota_arn`]:
//! an ARN that is not an applied-quota ARN is `IllegalArgumentException`, and one
//! that names no quota of this account is `NoSuchResourceException`.
//!
//! ---
//!
//! # Modeled contract for `ListTagsForResource`
//!
//! For the FULL text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Returns a list of the tags assigned to the specified applied quota.
//!
//! ## Input
//!
//! * `ResourceARN` (AmazonResourceName, len 1..1011, pattern
//!   ^arn:aws(-[\w]+)*:*:.+:[0-9]{12}:.+$) **required** — The Amazon Resource Name (ARN) for
//!   the applied quota for which you want to list tags. You can get this information by using
//!   the Service Quotas console, or by listing the quotas using the list-service-quotas CLI
//!   command or the ListServiceQuotas Amazon Web Services API operation.
//!
//! ## Output
//!
//! * `Tags` (OutputTags, len 0..200) — A complex data type that contains zero or more tag
//!   elements.
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
// `ListTagsForResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTagsForResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#ListTagsForResource")]
#[derive(Debug)]
pub enum ListTagsForResourceErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_tags_for_resource::ListTagsForResourceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListTagsForResourceError = crate::scaffolded::Scaffolded<ListTagsForResourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListTagsForResourceErrorModeled> for ListTagsForResourceError {
    fn from(modeled: ListTagsForResourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListTagsForResource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListTagsForResource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListTagsForResource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListTagsForResource<Eng>
{
    type Input = ListTagsForResourceInput;
    type Output = ListTagsForResourceOutput;
    type Error = ListTagsForResourceError;

    const NAME: &'static str = "ListTagsForResource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TAGS_FOR_RESOURCE_OUTPUT_SCHEMA;
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
            ListTagsForResourceErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;
        let quotas_state = state.lock();

        let canonical_arn = quotas_state
            .resolve_quota_arn(&input.resource_arn)
            .map_err(|error| match error {
                crate::state::ResourceArnError::Malformed => {
                    ListTagsForResourceErrorModeled::IllegalArgumentException(
                        IllegalArgumentException {
                            message: Some(crate::state::malformed_resource_arn_message(
                                &input.resource_arn,
                            )),
                        },
                    )
                }
                crate::state::ResourceArnError::NoSuchQuota => {
                    ListTagsForResourceErrorModeled::NoSuchResourceException(
                        NoSuchResourceException {
                            message: Some(crate::state::no_such_resource_arn_message(
                                &input.resource_arn,
                            )),
                        },
                    )
                }
            })?;

        Ok(ListTagsForResourceOutput {
            tags: Some(quotas_state.resource_tags(&canonical_arn)),
        })
    }
}
