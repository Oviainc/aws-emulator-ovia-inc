// SPDX-License-Identifier: CC0-1.0

//! @op ListTagsForResource
//!
//! Reports the tags PERSISTED on the file system or access point the request's
//! `resourceId` names, in the order the writes left them, cut into pages of at most
//! `maxResults` with the opaque resume cursor [`crate::state::encode_cursor`] mints.
//! Nothing is canned: the tags come off the resource's own record, so what this reads
//! back is exactly what `TagResource` and `UntagResource` left there.
//!
//! `@readonly`, so the only write it makes is
//! [`crate::state::ComAmazonawsS3filesS3filesState::settle`]: the `creating` ->
//! `available` transition the clock already owed at this instant.
//!
//! `tags` is a `TagList`, which is `@length(1, 50)`, so a resource carrying no tags is
//! reported with the member ABSENT rather than as an empty list the model would reject.
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json")
//! ```
//!
//! Lists all tags for S3 Files resources.
//!
//! ## Input
//!
//! * `maxResults` (Integer) — The maximum number of tags to return in a single response.
//! * `nextToken` (String) — A pagination token returned from a previous call to continue
//!   listing tags.
//! * `resourceId` (ResourceId, len ..256, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}(/access-point/fsap-[0-9a-f]{17,40})?|fs(ap)?-[0-9a-f
//!   …) **required** — The ID or Amazon Resource Name (ARN) of the resource to list tags for.
//!
//! ## Output
//!
//! * `nextToken` (String) — A pagination token to use in a subsequent request if more results
//!   are available.
//! * `tags` (TagList, len 1..50) — An array of tags associated with the resource.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found. Verify
//!   that the resource exists and that you have permission to access it.
//! * `ValidationException` (HTTP 400) — The input parameters are not valid. Check the
//!   parameter values and try again.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListTagsForResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-s3files-s3files.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTagsForResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.s3files#ListTagsForResource")]
#[derive(Debug)]
pub enum ListTagsForResourceErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
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
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListTagsForResourceErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
        })?;
        let mut store = state.lock();
        // The only write a reporting operation makes: the transition the clock already
        // owed at this instant, whether or not anybody called.
        store.settle(now);

        // `resourceId` names either resource this service holds, by bare id or by ARN.
        let resource = crate::state::tagged_resource_of(&input.resource_id).ok_or_else(|| {
            ListTagsForResourceErrorModeled::ValidationException(crate::state::invalid(format!(
                "resourceId must be a file system id (fs-<hex>), an access point id \
                 (fsap-<hex>) or either resource's ARN, got {:?}",
                input.resource_id
            )))
        })?;

        let tags = store
            .tags_of(&resource)
            .ok_or_else(|| {
                ListTagsForResourceErrorModeled::ResourceNotFoundException(
                    crate::state::not_found(format!("no {}", resource.describe())),
                )
            })?
            .to_vec();

        let after = crate::state::decode_cursor(input.next_token.as_deref()).map_err(|_| {
            ListTagsForResourceErrorModeled::ValidationException(crate::state::invalid(
                "nextToken is not a token this service issued",
            ))
        })?;

        // A tag has no creation sequence of its own — a resource's tags are a set the
        // writes order — so the cursor names the 1-based position in that order, which
        // is what a page has to resume after.
        let ordered: Vec<(u64, Tag)> = tags
            .into_iter()
            .enumerate()
            .map(|(index, tag)| (index as u64 + 1, tag))
            .collect();
        // `ListTagsForResourceRequest$maxResults` is `@range(1, 50)`; a request that
        // named none arrives absent, which takes the default page size `page_of`
        // applies.
        let page = crate::state::page_of(
            ordered,
            after,
            input.max_results.unwrap_or_default(),
            crate::state::MAX_RESULTS_LIMIT_TAGS,
        );

        Ok(ListTagsForResourceOutput {
            // `TagList` is `@length(1, 50)`: no tags means an absent member, never an
            // empty list.
            tags: (!page.items.is_empty()).then_some(page.items),
            next_token: page.next_token,
        })
    }
}
