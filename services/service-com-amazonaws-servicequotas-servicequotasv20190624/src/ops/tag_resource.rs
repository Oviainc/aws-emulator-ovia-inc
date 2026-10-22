// SPDX-License-Identifier: CC0-1.0

//! @op TagResource
//! Adds tags to an applied quota, named by its ARN.
//!
//! A tag set is a map keyed by `TagKey`, so this operation is an UPSERT: a key the
//! quota already carries keeps one tag and takes the new value, and the set
//! `ListTagsForResource` reports afterwards is exactly the merge of the two. The
//! resource is identified only by `ResourceARN`, checked against the ARN this service
//! REPORTS for an applied quota by
//! [`crate::state::ComAmazonawsServicequotasServicequotasv20190624State::resolve_quota_arn`]
//! — so one quota has exactly one tag set, with no second spelling of its ARN to tag
//! it under.
//!
//! Refusals, each the error the model declares for it:
//!
//! * a `ResourceARN` that is not an applied-quota ARN -> `IllegalArgumentException`;
//! * one that names no quota of this account -> `NoSuchResourceException`;
//! * a `TagKey` in the reserved `aws:` namespace -> `TagPolicyViolationException`;
//! * a request that would take the quota past
//!   [`crate::state::MAX_TAGS_PER_RESOURCE`] tags -> `TooManyTagsException`, with
//!   nothing written.
//!
//! ---
//!
//! # Modeled contract for `TagResource`
//!
//! For the FULL text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Adds tags to the specified applied quota. You can include one or more tags to add to the
//! quota.
//!
//! ## Input
//!
//! * `ResourceARN` (AmazonResourceName, len 1..1011, pattern
//!   ^arn:aws(-[\w]+)*:*:.+:[0-9]{12}:.+$) **required** — The Amazon Resource Name (ARN) for
//!   the applied quota. You can get this information by using the Service Quotas console, or
//!   by listing the quotas using the list-service-quotas CLI command or the ListServiceQuotas
//!   Amazon Web Services API operation.
//! * `Tags` (InputTags, len 1..) **required** — The tags that you want to add to the
//!   resource.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permission to perform
//!   this action.
//! * `IllegalArgumentException` (HTTP 400) — Invalid input was provided.
//! * `NoSuchResourceException` (HTTP 404) — The specified resource does not exist.
//! * `ServiceException` (HTTP 500) — Something went wrong.
//! * `TagPolicyViolationException` (HTTP 401) — The specified tag is a reserved word and
//!   cannot be used.
//! * `TooManyRequestsException` (HTTP 429) — Due to throttling, the request was denied. Slow
//!   down the rate of request calls, or request an increase for this quota.
//! * `TooManyTagsException` (HTTP 400) — You've exceeded the number of tags allowed for a
//!   resource. For more information, see Tag restrictions in the Service Quotas User Guide.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#TagResource")]
#[derive(Debug)]
pub enum TagResourceErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TagPolicyViolationException(TagPolicyViolationException),
    TooManyRequestsException(TooManyRequestsException),
    TooManyTagsException(TooManyTagsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::tag_resource::TagResourceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TagResourceError = crate::scaffolded::Scaffolded<TagResourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TagResourceErrorModeled> for TagResourceError {
    fn from(modeled: TagResourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `TagResource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TagResource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TagResource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TagResource<Eng>
{
    type Input = TagResourceInput;
    type Output = TagResourceOutput;
    type Error = TagResourceError;

    const NAME: &'static str = "TagResource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TAG_RESOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TAG_RESOURCE_OUTPUT_SCHEMA;
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
            TagResourceErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;
        let mut quotas_state = state.lock();

        let canonical_arn = quotas_state
            .resolve_quota_arn(&input.resource_arn)
            .map_err(|error| match error {
                crate::state::ResourceArnError::Malformed => {
                    TagResourceErrorModeled::IllegalArgumentException(IllegalArgumentException {
                        message: Some(crate::state::malformed_resource_arn_message(
                            &input.resource_arn,
                        )),
                    })
                }
                crate::state::ResourceArnError::NoSuchQuota => {
                    TagResourceErrorModeled::NoSuchResourceException(NoSuchResourceException {
                        message: Some(crate::state::no_such_resource_arn_message(
                            &input.resource_arn,
                        )),
                    })
                }
            })?;

        if let Some(reserved) = input
            .tags
            .iter()
            .find(|tag| crate::state::is_reserved_tag_key(&tag.key))
        {
            return Err(TagResourceErrorModeled::TagPolicyViolationException(
                TagPolicyViolationException {
                    message: Some(format!(
                        "Tag key '{}' is in the reserved '{}' namespace and cannot be used",
                        reserved.key,
                        crate::state::RESERVED_TAG_KEY_PREFIX
                    )),
                },
            )
            .into());
        }

        quotas_state
            .put_resource_tags(&canonical_arn, &input.tags)
            .map_err(|exceeded| {
                TagResourceErrorModeled::TooManyTagsException(TooManyTagsException {
                    message: Some(format!(
                        "the request would leave {} tags on ResourceARN '{}', past the {} tags a \
                         resource may carry",
                        exceeded.would_hold,
                        input.resource_arn,
                        crate::state::MAX_TAGS_PER_RESOURCE
                    )),
                })
            })?;

        Ok(TagResourceOutput {})
    }
}
