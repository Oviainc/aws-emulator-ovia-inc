// SPDX-License-Identifier: CC0-1.0

//! @op UntagResource
//!
//! REMOVES the named keys from the tag set the resource holds in
//! [`crate::state`], so the map `ListTagsForResource` reports afterwards is what
//! is left. It is the counterpart `TagResource`'s own modeled documentation
//! names as the way back under the per-resource tag limit
//! ([`crate::state::MAX_TAGS_PER_RESOURCE`]), which is why it is implemented
//! beside it.
//!
//! A key the resource does not carry is a no-op, not a failure: the request's
//! stated outcome — the resource no longer carries that key — already holds.
//! The only taggable Verified Permissions resource is a policy store, so an ARN
//! no live store carries is `ResourceNotFoundException`.
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Removes one or more tags from the specified Amazon Verified Permissions resource. In
//! Verified Permissions, policy stores can be tagged.
//!
//! ## Input
//!
//! * `resourceArn` (AmazonResourceName, len 1..2048) **required** — The ARN of the resource
//!   from which you are removing tags.
//! * `tagKeys` (TagKeyList, len 1..200) **required** — The list of tag keys to remove from
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

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UntagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UntagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.verifiedpermissions#UntagResource")]
#[derive(Debug)]
pub enum UntagResourceErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
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
        let not_found = || {
            UntagResourceErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: format!(
                    "No policy store exists with the input ARN: {}",
                    input.resource_arn
                ),
                resource_id: crate::state::resource_id_from_arn(&input.resource_arn),
                resource_type: "POLICY_STORE".to_string(),
            })
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UntagResourceErrorModeled::InternalServerException(InternalServerException {
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

        for key in &input.tag_keys {
            store.tags.remove(key);
        }

        crate::ops::audit(ctx, "UntagResource", &store_id, "untagged");
        Ok(UntagResourceOutput {})
    }
}
