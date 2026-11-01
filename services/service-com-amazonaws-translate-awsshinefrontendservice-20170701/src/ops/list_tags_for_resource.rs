// SPDX-License-Identifier: CC0-1.0

//! @op ListTagsForResource
//!
//! Reports the tags the Amazon Translate resource the ARN names carries, as
//! `ImportTerminology` and `TagResource` left them. An ARN this account holds no
//! resource for is a not-found refusal, never an empty tag list.
//!
//! Shared types live in `super::types`; the stored shapes live in
//! [`crate::state`].
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json")
//! ```
//!
//! Lists all tags associated with a given Amazon Translate resource. For more information,
//! see Tagging your resources.
//!
//! ## Input
//!
//! * `ResourceArn` (ResourceArn, len 1..512) **required** — The Amazon Resource Name (ARN) of
//!   the given Amazon Translate resource you are querying.
//!
//! ## Output
//!
//! * `Tags` (TagList, len 0..200) — Tags associated with the Amazon Translate resource being
//!   queried. A tag is a key-value pair that adds as a metadata to a resource used by Amazon
//!   Translate. For example, a tag with "Sales" as the key might be added to a resource to
//!   indicate its use by the sales department.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `InvalidParameterValueException` (HTTP 400) — The value of the parameter is not valid.
//!   Review the value of the parameter you are using to correct it, and then retry your
//!   operation.
//! * `ResourceNotFoundException` (HTTP 404) — The resource you are looking for has not been
//!   found. Review the resource you're looking for and see if a different resource will
//!   accomplish your needs before retrying the revised request.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{resource_not_found, service_state};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListTagsForResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTagsForResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.translate#ListTagsForResource")]
#[derive(Debug)]
pub enum ListTagsForResourceErrorModeled {
    InternalServerException(InternalServerException),
    InvalidParameterValueException(InvalidParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
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
        let state = service_state(&*self.engine).ok_or_else(|| {
            ListTagsForResourceErrorModeled::InternalServerException(InternalServerException {
                message: Some("the terminology store is unavailable".to_string()),
            })
        })?;

        let store = state.lock();
        let record = store
            .terminology_for_arn(&input.resource_arn)
            .ok_or_else(|| {
                ListTagsForResourceErrorModeled::ResourceNotFoundException(resource_not_found())
            })?;

        // Reported from the stored record, so a resource carrying no tags
        // answers with an empty list rather than with no member at all.
        Ok(ListTagsForResourceOutput {
            tags: Some(record.tags.clone()),
        })
    }
}
