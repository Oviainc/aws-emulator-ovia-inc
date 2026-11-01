// SPDX-License-Identifier: CC0-1.0

//! @op ListTagsForResource
//!
//! A pure read: it reports the tags stored on the ONE domain record whose own ARN is
//! the requested `resourceArn`, and writes nothing. `RegisterDomain`, `TagResource` and
//! `UntagResource` are the writers it round-trips, and an ARN that resolves to no stored
//! domain is an `UnknownResourceFault` — the modelled fault for a resource "not found
//! with in the scope of this operation (region or domain)".
//!
//! Every element it reports carries the awsJson type discriminator naming the
//! `ResourceTag` shape beside the stored key and value.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-swf-simpleworkflowservice/smithy-model/com-amazonaws-swf-simpleworkflowservice.json")
//! ```
//!
//! List tags for a given domain.
//!
//! ## Input
//!
//! * `resourceArn` (Arn, len 1..1600) **required** — The Amazon Resource Name (ARN) for the
//!   Amazon SWF domain.
//!
//! ## Output
//!
//! * `tags` (ResourceTagList) — An array of tags associated with the domain.
//!
//! ## Errors
//!
//! * `LimitExceededFault` (HTTP 400) — Returned by any operation if a system imposed
//!   limitation has been reached. To address this fault you should either clean up unused
//!   resources or increase the limit by contacting AWS.
//! * `OperationNotPermittedFault` (HTTP 400) — Returned when the caller doesn't have
//!   sufficient permissions to invoke the action.
//! * `UnknownResourceFault` (HTTP 400) — Returned when the named resource cannot be found
//!   with in the scope of this operation (region or domain). This could happen if the named
//!   resource was never created or is no longer available for this operation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListTagsForResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-swf-simpleworkflowservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTagsForResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.swf#ListTagsForResource")]
#[derive(Debug)]
pub enum ListTagsForResourceErrorModeled {
    LimitExceededFault(LimitExceededFault),
    OperationNotPermittedFault(OperationNotPermittedFault),
    UnknownResourceFault(UnknownResourceFault),
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
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            // The model declares no server fault for this operation, and
            // `OperationNotPermittedFault` is this service's common error: the request
            // cannot be served, and saying so with a modelled code beats a 501, which
            // would claim the operation is unwritten.
            ListTagsForResourceErrorModeled::OperationNotPermittedFault(
                OperationNotPermittedFault {
                    message: Some("the SWF domain store is unavailable".to_string()),
                },
            )
        })?;

        let guard = state.lock();
        let Some(domain) = guard.domain_by_arn(&input.resource_arn) else {
            return Err(ListTagsForResourceErrorModeled::UnknownResourceFault(
                UnknownResourceFault {
                    message: Some(crate::state::unknown_domain_resource_message(
                        &input.resource_arn,
                    )),
                },
            )
            .into());
        };

        // Straight out of the stored record, in the order it was written: a domain
        // registered with no tags answers an empty list rather than an absent one, so a
        // caller never has to tell "no tags" from "not reported". Each element carries
        // the awsJson type discriminator naming its own shape, which is fixed text about
        // the shape rather than anything of the tag or the resource.
        Ok(ListTagsForResourceOutput {
            tags: Some(
                domain
                    .tags
                    .iter()
                    .map(|stored| ResourceTag {
                        __type: Some(RESOURCE_TAG_SHAPE_ID.to_string()),
                        key: stored.key.clone(),
                        value: stored.value.clone(),
                    })
                    .collect(),
            ),
        })
    }
}

/// The awsJson type discriminator every `ResourceTag` of a response carries: the id of
/// the shape the element IS, as the service reports it.
const RESOURCE_TAG_SHAPE_ID: &str = "com.amazonaws.swf.base.model#ResourceTag";
