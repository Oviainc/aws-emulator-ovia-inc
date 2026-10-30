// SPDX-License-Identifier: CC0-1.0

//! @op DeleteFileSystem
//!
//! Removes the file system `fileSystemId` names from [`crate::state`], so the next
//! `GetFileSystem` answers 404 and `ListFileSystems` no longer reports it.
//!
//! A file system that still has children is refused with the modelled
//! `ConflictException` unless `forceDelete` is true, which removes the children with
//! it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteFileSystem`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json")
//! ```
//!
//! Deletes an S3 File System. You can optionally force deletion of a file system that has
//! pending export data.
//!
//! ## Input
//!
//! * `fileSystemId` (FileSystemId, len ..128, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}|fs-[0-9a-f]{17,40})$)
//!   **required** — The ID or Amazon Resource Name (ARN) of the S3 File System to delete.
//! * `forceDelete` (Boolean) — If true, allows deletion of a file system that contains data
//!   pending export to S3. If false (the default), the deletion will fail if there is data
//!   that has not yet been exported to the S3 bucket. Use this parameter with caution as it
//!   may result in data loss.
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
// `DeleteFileSystemError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-s3files-s3files.json");

#[derive(Debug, Clone, Default)]
pub struct DeleteFileSystemOutput;

impl DeleteFileSystemOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "DELETE",
        "/file-systems/{fileSystemId}",
        Some(204u16),
    ));
}

pub static DELETE_FILE_SYSTEM_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DeleteFileSystemOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeleteFileSystemOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteFileSystemError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.s3files#DeleteFileSystem")]
#[derive(Debug)]
pub enum DeleteFileSystemErrorModeled {
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_file_system::DeleteFileSystemError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteFileSystemError = crate::scaffolded::Scaffolded<DeleteFileSystemErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteFileSystemErrorModeled> for DeleteFileSystemError {
    fn from(modeled: DeleteFileSystemErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteFileSystem` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteFileSystem<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteFileSystem<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteFileSystem<Eng>
{
    type Input = DeleteFileSystemInput;
    type Output = DeleteFileSystemOutput;
    type Error = DeleteFileSystemError;

    const NAME: &'static str = "DeleteFileSystem";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_FILE_SYSTEM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DELETE_FILE_SYSTEM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 204u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteFileSystemErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
        })?;
        let mut store = state.lock();
        store.settle(now);

        let file_system_id = crate::state::file_system_id_of(&input.file_system_id)
            .ok_or_else(|| {
                DeleteFileSystemErrorModeled::ValidationException(crate::state::invalid(format!(
                    "fileSystemId must be a file system id (fs-<hex>) or its ARN, got {:?}",
                    input.file_system_id
                )))
            })?
            .to_string();

        if !store.file_systems.contains_key(&file_system_id) {
            return Err(DeleteFileSystemErrorModeled::ResourceNotFoundException(
                crate::state::not_found(format!("no file system {file_system_id}")),
            )
            .into());
        }

        // A file system in use is not deleted out from under its children: the request
        // is refused with the modelled conflict unless the caller asked for the
        // children to go too.
        let children = store.access_point_ids_of(&file_system_id);
        if !children.is_empty() && input.force_delete != Some(true) {
            return Err(DeleteFileSystemErrorModeled::ConflictException(
                crate::state::conflict(
                    &file_system_id,
                    crate::state::RESOURCE_TYPE_FILE_SYSTEM,
                    format!(
                        "file system {file_system_id} still has {} access point(s); \
                         delete them or retry with forceDelete=true",
                        children.len()
                    ),
                ),
            )
            .into());
        }

        for access_point_id in &children {
            store.access_points.remove(access_point_id);
        }
        store.file_systems.remove(&file_system_id);
        // The idempotency records of the removed resources can no longer replay
        // anything, so they go with them.
        let mut gone = children;
        gone.push(file_system_id);
        store.forget_client_tokens_for(&gone);

        Ok(DeleteFileSystemOutput)
    }
}
