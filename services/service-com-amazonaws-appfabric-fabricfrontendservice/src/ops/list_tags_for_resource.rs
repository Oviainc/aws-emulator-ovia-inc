// SPDX-License-Identifier: CC0-1.0

//! @op ListTagsForResource
//!
//! REPORTS the tags the resource `resourceArn` names holds, read straight out of
//! that resource's own record — `@readonly`, so it writes nothing: a tag set
//! changes only through `TagResource` / `UntagResource`.
//!
//! An ARN no stored resource carries is the modelled
//! `ResourceNotFoundException`, which is what distinguishes a resource with no
//! tags (an empty list) from one that does not exist.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`] and read it through
//! the guard it hands back. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `ListTagsForResource`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appfabric-fabricfrontendservice/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json")
//! ```
//!
//! Returns a list of tags for a resource.
//!
//! ## Input
//!
//! * `resourceArn` (Arn, len 1..1011, pattern ^arn:.+$) **required** — The Amazon Resource
//!   Name (ARN) of the resource for which you want to retrieve tags.
//!
//! ## Output
//!
//! * `tags` (TagList, len 0..50) — A map of the key-value pairs for the tag or tags assigned
//!   to the specified resource.
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
// `ListTagsForResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTagsForResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appfabric#ListTagsForResource")]
#[derive(Debug)]
pub enum ListTagsForResourceErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
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
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListTagsForResourceErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the AppFabric emulator could not reach its state store".to_string(),
                    retry_after_seconds: None,
                },
            )
            .into());
        };

        let store = state.lock();
        let Some(target) = store.locate_tagged(&input.resource_arn) else {
            return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
                crate::state::tagged_resource_not_found(&input.resource_arn),
            )
            .into());
        };
        let Some(held) = store.tags_of(&target) else {
            return Err(ListTagsForResourceErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the located resource's tags could not be read back".to_string(),
                    retry_after_seconds: None,
                },
            )
            .into());
        };

        // Always a list, empty for a resource that holds no tags: the member is
        // how a caller reads the tag set, and omitting it would read as unknown.
        Ok(ListTagsForResourceOutput {
            tags: Some(held.clone()),
        })
    }
}
