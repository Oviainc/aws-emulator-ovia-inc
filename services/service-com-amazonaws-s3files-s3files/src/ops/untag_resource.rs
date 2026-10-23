// SPDX-License-Identifier: CC0-1.0

//! @op UntagResource
//!
//! Removes the tag keys a request names from ONE persisted resource — the file system
//! or access point its `resourceId` names, by bare id or by ARN — leaving every other
//! tag it carries in place and in order. The write lands on the resource's own record,
//! so `ListTagsForResource`, `GetFileSystem` and `GetAccessPoint` all stop reporting
//! the removed keys.
//!
//! `@idempotent`: the result depends only on which keys the resource ends up without,
//! so a key it does not carry is removed by doing nothing and repeating the call
//! answers the same way.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json")
//! ```
//!
//! Removes tags from S3 Files resources.
//!
//! ## Input
//!
//! * `resourceId` (ResourceId, len ..256, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}(/access-point/fsap-[0-9a-f]{17,40})?|fs(ap)?-[0-9a-f
//!   …) **required** — The ID or Amazon Resource Name (ARN) of the resource to remove tags
//!   from.
//! * `tagKeys` (TagKeys, len 1..50) **required** — An array of tag keys to remove from the
//!   resource.
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
// `UntagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-s3files-s3files.json");

#[derive(Debug, Clone, Default)]
pub struct UntagResourceOutput;

impl UntagResourceOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "DELETE",
        "/resource-tags/{resourceId}",
        Some(200u16),
    ));
}

pub static UNTAG_RESOURCE_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &UntagResourceOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for UntagResourceOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UntagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.s3files#UntagResource")]
#[derive(Debug)]
pub enum UntagResourceErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
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
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = UNTAG_RESOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UntagResourceErrorModeled::InternalServerException(crate::state::store_unavailable())
        })?;
        let mut store = state.lock();
        store.settle(now);

        // `resourceId` names either resource this service holds, by bare id or by ARN.
        let resource = crate::state::tagged_resource_of(&input.resource_id).ok_or_else(|| {
            UntagResourceErrorModeled::ValidationException(crate::state::invalid(format!(
                "resourceId must be a file system id (fs-<hex>), an access point id \
                 (fsap-<hex>) or either resource's ARN, got {:?}",
                input.resource_id
            )))
        })?;

        let current = store
            .tags_of(&resource)
            .ok_or_else(|| {
                UntagResourceErrorModeled::ResourceNotFoundException(crate::state::not_found(
                    format!("no {}", resource.describe()),
                ))
            })?
            .to_vec();

        // Every tag whose key the request did NOT name, in the order the resource held
        // them: an untag removes keys, it does not reorder what stays. A named key the
        // resource does not carry is already removed, which is what makes the operation
        // `@idempotent`.
        let kept: Vec<Tag> = current
            .into_iter()
            .filter(|tag| !input.tag_keys.contains(&tag.key))
            .collect();

        if !store.set_tags(&resource, kept) {
            // The record was there a moment ago under the same lock, so losing it here
            // is a store fault rather than a caller error.
            return Err(UntagResourceErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
            .into());
        }

        Ok(UntagResourceOutput)
    }
}
