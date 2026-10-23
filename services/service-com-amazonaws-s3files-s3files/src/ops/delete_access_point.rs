// SPDX-License-Identifier: CC0-1.0

//! @op DeleteAccessPoint
//!
//! Removes the persisted access point `accessPointId` names, accepting either the bare
//! `fsap-` id or the access point's ARN, and answers the modelled 204 with no body. The
//! removal is the whole effect: a later `GetAccessPoint` on the same id answers the
//! modelled 404 and the parent file system no longer counts it among its children, so a
//! `DeleteFileSystem` that was refused for being in use now succeeds without
//! `forceDelete`. An id this service does not hold is the modelled 404 — the operation
//! is `@idempotent` in that it leaves the same state behind, not in that a repeat
//! reports success for a resource that is already gone.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteAccessPoint`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json")
//! ```
//!
//! Deletes an S3 File System Access Point. This operation is irreversible.
//!
//! ## Input
//!
//! * `accessPointId` (AccessPointId, len ..256, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}/access-point/fsap-[0-9a-f]{17,40}|fsap-[0-9a-f]{17,4
//!   …) **required** — The ID or Amazon Resource Name (ARN) of the access point to delete.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — The request conflicts with the current state of the
//!   resource. This can occur when trying to create a resource that already exists or delete
//!   a resource that is in use.
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
// `DeleteAccessPointError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-s3files-s3files.json");

#[derive(Debug, Clone, Default)]
pub struct DeleteAccessPointOutput;

impl DeleteAccessPointOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "DELETE",
        "/access-points/{accessPointId}",
        Some(204u16),
    ));
}

pub static DELETE_ACCESS_POINT_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DeleteAccessPointOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeleteAccessPointOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteAccessPointError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.s3files#DeleteAccessPoint")]
#[derive(Debug)]
pub enum DeleteAccessPointErrorModeled {
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_access_point::DeleteAccessPointError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteAccessPointError = crate::scaffolded::Scaffolded<DeleteAccessPointErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteAccessPointErrorModeled> for DeleteAccessPointError {
    fn from(modeled: DeleteAccessPointErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteAccessPoint` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteAccessPoint<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteAccessPoint<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteAccessPoint<Eng>
{
    type Input = DeleteAccessPointInput;
    type Output = DeleteAccessPointOutput;
    type Error = DeleteAccessPointError;

    const NAME: &'static str = "DeleteAccessPoint";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_ACCESS_POINT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DELETE_ACCESS_POINT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 204u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteAccessPointErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
        })?;
        let mut store = state.lock();
        store.settle(now);

        // `accessPointId` accepts the bare id or the ARN; both name the same record.
        // The 400 below is defensive: the modelled `AccessPointId` pattern is enforced
        // before `handle` runs, so a value neither form accepts is normally refused by
        // the framework. Kept for parity with `DeleteFileSystem`.
        let access_point_id = crate::state::access_point_id_of(&input.access_point_id)
            .ok_or_else(|| {
                DeleteAccessPointErrorModeled::ValidationException(crate::state::invalid(format!(
                    "accessPointId must be an access point id (fsap-<hex>) or its ARN, got {:?}",
                    input.access_point_id
                )))
            })?
            .to_string();

        if store.access_points.remove(&access_point_id).is_none() {
            return Err(DeleteAccessPointErrorModeled::ResourceNotFoundException(
                crate::state::not_found(format!("no access point {access_point_id}")),
            )
            .into());
        }

        // The idempotency record naming the removed access point can no longer replay
        // anything, so it goes with it.
        store.forget_client_tokens_for(&[access_point_id]);

        Ok(DeleteAccessPointOutput)
    }
}
