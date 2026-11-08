// SPDX-License-Identifier: CC0-1.0

//! @op TagResource
//!
//! Associates the request's tags with the resource its `resourceARN` names, writing them
//! onto the record that already holds that resource in [`crate::state`] — a guardrail, one
//! revision of an Automated Reasoning policy, a prompt router, a model import job, the
//! model such a job imports, an evaluation job or a Provisioned Throughput — so the tags a
//! create carried and the tags this call adds are ONE list, which
//! `ops/list_tags_for_resource.rs` reports back.
//!
//! Tags are keyed by `key`: a key the resource already carries takes the new value and a
//! key it does not carry is added, so repeating a call leaves the resource exactly as the
//! first one left it and a resource never holds one key twice.
//!
//! An ARN naming no resource this account holds is `ResourceNotFoundException`, worded for
//! the resource TYPE the ARN spells
//! ([`crate::state::tagging_resource_not_found_message`]), and a
//! request that would leave the resource carrying more than the 50 tags
//! `TooManyTagsException` documents is refused in full, before any pair is applied, so a
//! refused call changes nothing.
//!
//! The one resolution of a tagging ARN lives in `crate::state` (`tagged_resource`), shared
//! with `ops/list_tags_for_resource.rs` and `ops/untag_resource.rs`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Associate tags with a resource. For more information, see Tagging resources in the
//! Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `resourceARN` (TaggableResourcesArn, len 20..1011, pattern
//!   (^[a-zA-Z0-9][a-zA-Z0-9\-]*$)|(^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model/(imported)/[a-z0-9]{12}$
//!   …) **required** — The Amazon Resource Name (ARN) of the resource to tag.
//! * `tags` (TagList, len 0..200) **required** — Tags to associate with the resource.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `TooManyTagsException` (HTTP 400) — The request contains more tags than can be
//!   associated with a resource (50 tags per resource). The maximum number of tags includes
//!   both existing tags and those included in your current request.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#TagResource")]
#[derive(Debug)]
pub enum TagResourceErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    TooManyTagsException(TooManyTagsException),
    ValidationException(ValidationException),
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
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(TagResourceErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("tagging state is not available on this engine".to_string()),
                },
            )
            .into());
        };
        // A bare resource name names a resource of THIS region, and an ARN spelling the
        // store keys a resource under another is read back in the region the ARN carries,
        // so resolving the ARN needs the region the request addressed.
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();

        let mut store = state.lock();
        let Some(held) = store.resource_tags_mut(&input.resource_arn, &region, now) else {
            return Err(TagResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::tagging_resource_not_found_message(
                        &input.resource_arn,
                    )),
                },
            )
            .into());
        };

        // The bound `TooManyTagsException` documents counts "both existing tags and those
        // included in your current request", and only the keys the resource does not
        // already carry grow it. The whole request is refused before a single pair is
        // applied, so a refused call leaves the resource's tags exactly as they were.
        let merged = crate::state::tag_count_after(held, &input.tags);
        if merged > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(TagResourceErrorModeled::TooManyTagsException(
                TooManyTagsException {
                    message: Some(crate::state::too_many_tags_message()),
                    resource_name: Some(input.resource_arn.clone()),
                },
            )
            .into());
        }

        crate::state::apply_tags(held, input.tags);
        Ok(TagResourceOutput {})
    }
}
