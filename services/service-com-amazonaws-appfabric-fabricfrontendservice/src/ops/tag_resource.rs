// SPDX-License-Identifier: CC0-1.0

//! @op TagResource
//!
//! Assigns tags to the resource `resourceArn` names and PERSISTS them on that
//! resource's own record, so `ListTagsForResource` (and `UntagResource`) answer
//! from the same tag set this call wrote.
//!
//! A tag list is a map of key-value pairs: a key the resource already holds
//! takes the new value rather than appearing twice (see
//! [`crate::state::merge_tags`]), and an ARN no stored resource carries is the
//! modelled `ResourceNotFoundException`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`] and read and write it
//! through the guard it hands back. `ctx` carries request metadata only.
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appfabric-fabricfrontendservice/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json")
//! ```
//!
//! Assigns one or more tags (key-value pairs) to the specified resource.
//!
//! ## Input
//!
//! * `resourceArn` (Arn, len 1..1011, pattern ^arn:.+$) **required** — The Amazon Resource
//!   Name (ARN) of the resource that you want to tag.
//! * `tags` (TagList, len 0..50) **required** — A map of the key-value pairs of the tag or
//!   tags to assign to the resource.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You are not authorized to perform this operation.
//! * `InternalServerException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception, or failure with an internal server.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource does not exist.
//! * `ThrottlingException` (HTTP 429) — The request rate exceeds the limit.
//! * `ValidationException` (HTTP 400) — The request has invalid or missing parameters.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appfabric#TagResource")]
#[derive(Debug)]
pub enum TagResourceErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
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
                    message: "the AppFabric emulator could not reach its state store".to_string(),
                    retry_after_seconds: None,
                },
            )
            .into());
        };

        let mut store = state.lock();
        // The ARN is the only identifier this operation takes, so a resource is
        // taggable exactly when its own record is in the store.
        let Some(target) = store.locate_tagged(&input.resource_arn) else {
            return Err(TagResourceErrorModeled::ResourceNotFoundException(
                crate::state::tagged_resource_not_found(&input.resource_arn),
            )
            .into());
        };
        let Some(held) = store.tags_of_mut(&target) else {
            return Err(TagResourceErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the located resource's tags could not be read back".to_string(),
                    retry_after_seconds: None,
                },
            )
            .into());
        };

        // Merged first, so a merge past the modelled bound is refused with the
        // resource's tags left exactly as they were.
        let merged = crate::state::merge_tags(held, &input.tags);
        if merged.len() > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(TagResourceErrorModeled::ValidationException(
                crate::state::field_validation_failed(
                    "tags",
                    &format!(
                        "A resource holds at most {} tags; this request would leave it with {}.",
                        crate::state::MAX_TAGS_PER_RESOURCE,
                        merged.len()
                    ),
                ),
            )
            .into());
        }
        *held = merged;

        crate::state::audit_resource_change(
            ctx.parts(),
            Self::NAME,
            target.resource_type(),
            target.id(),
            "tagged",
        );
        Ok(TagResourceOutput {})
    }
}

