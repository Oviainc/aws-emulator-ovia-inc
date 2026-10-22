// SPDX-License-Identifier: CC0-1.0

//! @op TagResource
//!
//! MERGES the request's tags into the tag set the named resource already holds
//! in [`crate::state`], so the map `ListTagsForResource` (and
//! `GetPolicyStore`'s `tags`) reports afterwards is the merge of every tagging
//! call the resource has seen — never one request's echo.
//!
//! Three rules, all of them from the modeled documentation quoted below:
//!
//! * a key the resource does not carry is APPENDED; a key it already carries
//!   takes the request's value, replacing the previous one;
//! * the merge is refused with `TooManyTagsException` when it would leave the
//!   resource holding more than [`crate::state::MAX_TAGS_PER_RESOURCE`] tags,
//!   and then NOTHING is persisted — the resource keeps the tags it had;
//! * the only taggable Verified Permissions resource is a policy store, so an
//!   ARN no live store carries is `ResourceNotFoundException`.
//!
//! Tagging does not move the store's `lastUpdatedDate`: a tag is metadata
//! beside the resource, not part of the configuration the update operations
//! change, and the clock field is what those report.
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Assigns one or more tags (key-value pairs) to the specified Amazon Verified Permissions
//! resource. Tags can help you organize and categorize your resources. You can also use
//! them to scope user permissions by granting a user permission to access or change only
//! resources with certain tag values. In Verified Permissions, policy stores can be tagged.
//! Tags don't have any semantic meaning to Amazon Web Services and are interpreted strictly
//! as strings of characters. You can use the TagResource action with a resource that
//! already has tags. If you specify a new tag key, this tag is appended to the list of tags
//! associated with the resource. If you specify a tag key that is already associated with
//! the resource, the new tag value that you specify replaces the previous value for that
//! tag. You can associate as many as 50 tags with a resource.
//!
//! ## Input
//!
//! * `resourceArn` (AmazonResourceName, len 1..2048) **required** — The ARN of the resource
//!   that you're adding tags to.
//! * `tags` (TagMap, len ..200) **required** — The list of key-value pairs to associate with
//!   the resource.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have sufficient access to perform this
//!   action.
//! * `InternalServerException` (HTTP 500) — The request failed because of an internal error.
//!   Try your request again later
//! * `ResourceNotFoundException` (HTTP 404) — The request failed because it references a
//!   resource that doesn't exist.
//! * `ThrottlingException` (HTTP 429) — The request failed because it exceeded a throttling
//!   quota.
//! * `TooManyTagsException` (HTTP 400) — No more tags be added because the limit (50) has
//!   been reached. To add new tags, use UntagResource to remove existing tags.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.verifiedpermissions#TagResource")]
#[derive(Debug)]
pub enum TagResourceErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
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
        let not_found = || {
            TagResourceErrorModeled::ResourceNotFoundException(crate::ops::resource_not_found(
                &crate::state::resource_id_from_arn(&input.resource_arn),
                "POLICY_STORE",
            ))
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            TagResourceErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let mut state = state.lock();
        let store_id = state
            .resolve_resource_arn(&input.resource_arn)
            .ok_or_else(not_found)?;
        let store = state
            .policy_stores
            .get_mut(&store_id)
            .ok_or_else(not_found)?;

        // The merge is computed before anything is written, so the per-resource
        // limit is checked against what the resource WOULD hold and a refusal
        // leaves the stored tag set exactly as it was.
        let mut merged = store.tags.clone();
        merged.extend(
            input
                .tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        if merged.len() > crate::state::MAX_TAGS_PER_RESOURCE {
            // The refusal carries `TooManyTagsException`'s one fixed message and
            // reports the resource at the limit in the modelled `resourceName`,
            // so nothing of the request (the count, the offending key) is
            // spliced into prose a client can only regex.
            return Err(TagResourceErrorModeled::TooManyTagsException(
                crate::ops::too_many_tags(&input.resource_arn),
            )
            .into());
        }
        store.tags = merged;

        crate::ops::audit(ctx, "TagResource", &store_id, "tagged");
        Ok(TagResourceOutput {})
    }
}
