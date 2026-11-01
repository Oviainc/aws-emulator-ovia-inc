// SPDX-License-Identifier: CC0-1.0

//! @op TagResource
//!
//! Associates the request's tags with the Amazon Translate resource the ARN
//! names, merging them into the tags that resource already carries: a key it
//! already holds takes the request's value, and every other key is added.
//!
//! The tag set one resource may hold is bounded, and the bound is a rule of this
//! operation rather than of the request's shape — the model admits a far longer
//! `Tags` list than a resource may end up holding — so an over-limit call is
//! refused with the modelled error and leaves the stored tags exactly as they
//! were.
//!
//! Shared types live in `super::types`; the stored shapes and the tag limit live
//! in [`crate::state`].
//!
//! ---
//!
//! # Modeled contract for `TagResource`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json")
//! ```
//!
//! Associates a specific tag with a resource. A tag is a key-value pair that adds as a
//! metadata to a resource. For more information, see Tagging your resources.
//!
//! ## Input
//!
//! * `ResourceArn` (ResourceArn, len 1..512) **required** — The Amazon Resource Name (ARN) of
//!   the given Amazon Translate resource to which you want to associate the tags.
//! * `Tags` (TagList, len 0..200) **required** — Tags being associated with a specific Amazon
//!   Translate resource. There can be a maximum of 50 tags (both existing and pending)
//!   associated with a specific resource.
//!
//! ## Errors
//!
//! * `ConcurrentModificationException` (HTTP 409) — Another modification is being made. That
//!   modification must complete before you can make your change.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `InvalidParameterValueException` (HTTP 400) — The value of the parameter is not valid.
//!   Review the value of the parameter you are using to correct it, and then retry your
//!   operation.
//! * `ResourceNotFoundException` (HTTP 404) — The resource you are looking for has not been
//!   found. Review the resource you're looking for and see if a different resource will
//!   accomplish your needs before retrying the revised request.
//! * `TooManyTagsException` (HTTP 400) — You have added too many tags to this resource. The
//!   maximum is 50 tags.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{resource_not_found, service_state, too_many_tags, MAX_TAGS_PER_RESOURCE};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.translate#TagResource")]
#[derive(Debug)]
pub enum TagResourceErrorModeled {
    ConcurrentModificationException(ConcurrentModificationException),
    InternalServerException(InternalServerException),
    InvalidParameterValueException(InvalidParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
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
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = service_state(&*self.engine).ok_or_else(|| {
            TagResourceErrorModeled::InternalServerException(InternalServerException {
                message: Some("the terminology store is unavailable".to_string()),
            })
        })?;

        let mut store = state.lock();
        // A tagging call addresses the resource by ARN, so the resource it tags
        // is the stored one carrying that ARN.
        let name = store
            .terminology_name_for_arn(&input.resource_arn)
            .ok_or_else(|| {
                TagResourceErrorModeled::ResourceNotFoundException(resource_not_found())
            })?;
        let record = store.terminologies.get_mut(&name).ok_or_else(|| {
            TagResourceErrorModeled::InternalServerException(InternalServerException {
                message: Some("the tagged terminology could not be read back".to_string()),
            })
        })?;

        // "There can be a maximum of 50 tags (both existing and pending)
        // associated with a specific resource", so the limit applies to the set
        // the resource would be left holding: the tags it already carries, with
        // a key the request restates taking the request's value.
        let mut merged = record.tags.clone();
        for tag in &input.tags {
            match merged.iter_mut().find(|held| held.key == tag.key) {
                Some(held) => held.value = tag.value.clone(),
                None => merged.push(tag.clone()),
            }
        }
        if merged.len() > MAX_TAGS_PER_RESOURCE {
            // Refused before anything is written, so none of the request's tags
            // is applied and the resource keeps the tags it had.
            return Err(TagResourceErrorModeled::TooManyTagsException(too_many_tags(
                &input.resource_arn,
            ))
            .into());
        }

        record.tags = merged;
        Ok(TagResourceOutput {})
    }
}
