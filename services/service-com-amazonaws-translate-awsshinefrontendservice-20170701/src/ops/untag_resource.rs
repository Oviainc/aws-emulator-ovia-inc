// SPDX-License-Identifier: CC0-1.0

//! @op UntagResource
//!
//! Removes from the Amazon Translate resource the ARN names every tag whose key
//! the request lists, and leaves every other tag it carries exactly as it was.
//!
//! Removal is addressed by KEY, so a key the resource does not carry removes
//! nothing and is not a refusal: repeating the same call answers the same way
//! and leaves the same tag set, and a request listing no key removes nothing at
//! all. An ARN this account holds no resource for is a not-found refusal, so the
//! call never silently succeeds against a resource that is not there.
//!
//! Shared types live in `super::types`; the stored shapes live in
//! [`crate::state`].
//!
//! ---
//!
//! # Modeled contract for `UntagResource`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json")
//! ```
//!
//! Removes a specific tag associated with an Amazon Translate resource. For more
//! information, see Tagging your resources.
//!
//! ## Input
//!
//! * `ResourceArn` (ResourceArn, len 1..512) **required** — The Amazon Resource Name (ARN) of
//!   the given Amazon Translate resource from which you want to remove the tags.
//! * `TagKeys` (TagKeyList, len 0..200) **required** — The initial part of a key-value pair
//!   that forms a tag being removed from a given resource. Keys must be unique and cannot be
//!   duplicated for a particular resource.
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

#[allow(unused_imports)]
use super::types::*;

use crate::state::{resource_not_found, service_state};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UntagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UntagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.translate#UntagResource")]
#[derive(Debug)]
pub enum UntagResourceErrorModeled {
    ConcurrentModificationException(ConcurrentModificationException),
    InternalServerException(InternalServerException),
    InvalidParameterValueException(InvalidParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
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
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = service_state(&*self.engine).ok_or_else(|| {
            UntagResourceErrorModeled::InternalServerException(InternalServerException {
                message: Some("the terminology store is unavailable".to_string()),
            })
        })?;

        let mut store = state.lock();
        // An untagging call addresses the resource by ARN, so the resource it
        // untags is the stored one carrying that ARN.
        let name = store
            .terminology_name_for_arn(&input.resource_arn)
            .ok_or_else(|| {
                UntagResourceErrorModeled::ResourceNotFoundException(resource_not_found())
            })?;
        let record = store.terminologies.get_mut(&name).ok_or_else(|| {
            UntagResourceErrorModeled::InternalServerException(InternalServerException {
                message: Some("the untagged terminology could not be read back".to_string()),
            })
        })?;

        // A tag is identified by its key, so the resource is left holding the
        // tags whose key the request did not list — in the order it held them.
        // A listed key the resource does not carry drops nothing, which is what
        // makes the call idempotent, and a request listing no key leaves the
        // whole tag set standing.
        record.tags.retain(|held| !input.tag_keys.contains(&held.key));

        Ok(UntagResourceOutput {})
    }
}
