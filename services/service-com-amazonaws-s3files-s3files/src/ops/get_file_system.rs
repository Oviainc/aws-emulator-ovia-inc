// SPDX-License-Identifier: CC0-1.0

//! @op GetFileSystem
//!
//! Reports the persisted file system `fileSystemId` names, accepting either the bare
//! `fs-` id or the file system's ARN. The only state this reporting operation writes is
//! the `creating` -> `available` transition the clock already made due.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetFileSystem`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json")
//! ```
//!
//! Returns resource information for the specified S3 File System including status,
//! configuration, and metadata.
//!
//! ## Input
//!
//! * `fileSystemId` (FileSystemId, len ..128, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}|fs-[0-9a-f]{17,40})$)
//!   **required** — The ID or Amazon Resource Name (ARN) of the S3 File System to retrieve
//!   information for.
//!
//! ## Output
//!
//! * `bucket` (BucketArn, pattern ^(arn:aws[a-zA-Z0-9-]*:s3:::.+)$) — The Amazon Resource
//!   Name (ARN) of the S3 bucket.
//! * `clientToken` (ClientToken, len 1..64, pattern ^(.+)$) — The client token used for
//!   idempotency when the file system was created.
//! * `creationTime` (Timestamp) — The time when the file system was created.
//! * `fileSystemArn` (FileSystemArn, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40})$) — The Amazon
//!   Resource Name (ARN) of the file system.
//! * `fileSystemId` (FileSystemId, len ..128, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}|fs-[0-9a-f]{17,40})$)
//!   — The ID of the file system.
//! * `kmsKeyId` (KmsKeyId, len ..2048, pattern
//!   ^([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|mrk-[0-9a-f]{32}|alias/[a-zA-Z0-9/_-]+|(arn:aws[-a-z]*:km
//!   …) — The Amazon Resource Name (ARN) of the Amazon Web Services KMS key used for
//!   encryption.
//! * `name` (TagValue, len ..256, pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$) — The name of the
//!   file system.
//! * `ownerId` (AwsAccountId, len ..12, pattern ^(\d{12})|(\d{4}-{4}-\d{4})$) — The Amazon
//!   Web Services account ID of the file system owner.
//! * `prefix` (String, default "") — The prefix in the S3 bucket that the file system
//!   provides access to.
//! * `roleArn` (RoleArn, len ..2048, pattern
//!   ^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$) — The Amazon Resource
//!   Name (ARN) of the IAM role used for S3 access.
//! * `status` (LifeCycleState, one of: available | creating | deleted | deleting | error |
//!   misconfigured | updating) — The current status of the file system.
//! * `statusMessage` (StatusMessage) — Additional information about the file system status.
//! * `tags` (TagList, len 1..50) — The tags associated with the file system.
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
// `GetFileSystemError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-s3files-s3files.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetFileSystemError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.s3files#GetFileSystem")]
#[derive(Debug)]
pub enum GetFileSystemErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_file_system::GetFileSystemError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetFileSystemError = crate::scaffolded::Scaffolded<GetFileSystemErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetFileSystemErrorModeled> for GetFileSystemError {
    fn from(modeled: GetFileSystemErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetFileSystem` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetFileSystem<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetFileSystem<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetFileSystem<Eng>
{
    type Input = GetFileSystemInput;
    type Output = GetFileSystemOutput;
    type Error = GetFileSystemError;

    const NAME: &'static str = "GetFileSystem";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_FILE_SYSTEM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_FILE_SYSTEM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetFileSystemErrorModeled::InternalServerException(crate::state::store_unavailable())
        })?;
        let mut store = state.lock();
        // The only write a reporting operation makes: the transition the clock already
        // owed at this instant, whether or not anybody called.
        store.settle(now);

        let file_system_id = crate::state::file_system_id_of(&input.file_system_id)
            .ok_or_else(|| {
                GetFileSystemErrorModeled::ValidationException(crate::state::invalid(format!(
                    "fileSystemId must be a file system id (fs-<hex>) or its ARN, got {:?}",
                    input.file_system_id
                )))
            })?
            .to_string();

        let stored = store.file_systems.get(&file_system_id).ok_or_else(|| {
            GetFileSystemErrorModeled::ResourceNotFoundException(crate::state::not_found(format!(
                "no file system {file_system_id}"
            )))
        })?;

        Ok(GetFileSystemOutput {
            creation_time: Some(::aws_smithy_types::DateTime::from(stored.creation_time)),
            file_system_arn: Some(stored.file_system_arn.clone()),
            file_system_id: Some(stored.file_system_id.clone()),
            bucket: Some(stored.bucket.clone()),
            prefix: Some(stored.prefix.clone()),
            client_token: stored.client_token.clone(),
            kms_key_id: stored.kms_key_id.clone(),
            status: Some(stored.status.clone()),
            status_message: stored.status_message.clone(),
            role_arn: Some(stored.role_arn.clone()),
            owner_id: Some(stored.owner_id.clone()),
            tags: (!stored.tags.is_empty()).then(|| stored.tags.clone()),
            name: stored.name.clone(),
        })
    }
}
