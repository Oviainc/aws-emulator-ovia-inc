// SPDX-License-Identifier: CC0-1.0

//! @op CreateFileSystem
//!
//! Mints a file system, persists it in [`crate::state`] and answers from the stored
//! record, so what this returns is exactly what `GetFileSystem` and `ListFileSystems`
//! read back.
//!
//! `clientToken` is an `@idempotencyToken`: a second call carrying the same token
//! returns the file system the first one created rather than minting another. A new
//! file system is reported `creating` and the clock owes it `available`
//! ([`crate::state::PROVISIONING_SPAN`]).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateFileSystem`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json")
//! ```
//!
//! Creates an S3 File System resource scoped to a bucket or prefix within a bucket,
//! enabling file system access to S3 data. To create a file system, you need an S3 bucket
//! and an IAM role that grants the service permission to access the bucket.
//!
//! ## Input
//!
//! * `acceptBucketWarning` (Boolean) — Set to true to acknowledge and accept any warnings
//!   about the bucket configuration. If not specified, the operation may fail if there are
//!   bucket configuration warnings.
//! * `bucket` (BucketArn, pattern ^(arn:aws[a-zA-Z0-9-]*:s3:::.+)$) **required** — The Amazon
//!   Resource Name (ARN) of the S3 bucket that will be accessible through the file system.
//!   The bucket must exist and be in the same Amazon Web Services Region as the file system.
//! * `clientToken` (CreationToken, len 1..64, pattern ^(.+)$, @idempotencyToken) — A unique,
//!   case-sensitive identifier that you provide to ensure idempotent creation. Up to 64 ASCII
//!   characters are allowed. If you don't specify a client token, the Amazon Web Services SDK
//!   automatically generates one.
//! * `kmsKeyId` (KmsKeyId, len ..2048, pattern
//!   ^([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|mrk-[0-9a-f]{32}|alias/[a-zA-Z0-9/_-]+|(arn:aws[-a-z]*:km
//!   …) — The ARN, key ID, or alias of the KMS key to use for encryption. If not specified,
//!   the service uses a service-owned key for encryption. You can specify a KMS key using the
//!   following formats: key ID, ARN, key alias, or key alias ARN. If you use KmsKeyId, the
//!   file system will be encrypted.
//! * `prefix` (String) — An optional prefix within the S3 bucket to scope the file system
//!   access. If specified, the file system provides access only to objects with keys that
//!   begin with this prefix. If not specified, the file system provides access to the entire
//!   bucket.
//! * `roleArn` (RoleArn, len ..2048, pattern
//!   ^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$) **required** — The ARN
//!   of the IAM role that grants the S3 Files service permission to read and write data
//!   between the file system and the S3 bucket. This role must have the necessary permissions
//!   to access the specified bucket and prefix.
//! * `tags` (TagList, len 1..50) — An array of key-value pairs to apply as tags to the file
//!   system resource. Each tag is a user-defined key-value pair. You can use tags to
//!   categorize and manage your file systems. Each key must be unique for the resource.
//!
//! ## Output
//!
//! * `bucket` (BucketArn, pattern ^(arn:aws[a-zA-Z0-9-]*:s3:::.+)$) — The Amazon Resource
//!   Name (ARN) of the S3 bucket associated with the file system.
//! * `clientToken` (ClientToken, len 1..64, pattern ^(.+)$) — The client token used for
//!   idempotency.
//! * `creationTime` (Timestamp) — The time when the file system was created, in seconds since
//!   1970-01-01T00:00:00Z (Unix epoch time).
//! * `fileSystemArn` (FileSystemArn, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40})$) — The ARN for the
//!   S3 file system, in the format
//!   arn:aws:s3files:region:account-id:file-system/file-system-id.
//! * `fileSystemId` (FileSystemId, len ..128, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}|fs-[0-9a-f]{17,40})$)
//!   — The ID of the file system, assigned by S3 Files. This ID is used to reference the file
//!   system in subsequent API calls.
//! * `kmsKeyId` (KmsKeyId, len ..2048, pattern
//!   ^([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|mrk-[0-9a-f]{32}|alias/[a-zA-Z0-9/_-]+|(arn:aws[-a-z]*:km
//!   …) — The ARN or alias of the KMS key used for encryption.
//! * `name` (TagValue, len ..256, pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$) — The name of the
//!   file system, derived from the Name tag if present.
//! * `ownerId` (AwsAccountId, len ..12, pattern ^(\d{12})|(\d{4}-{4}-\d{4})$) — The Amazon
//!   Web Services account ID of the file system owner.
//! * `prefix` (String, default "") — The prefix within the S3 bucket that scopes the file
//!   system access.
//! * `roleArn` (RoleArn, len ..2048, pattern
//!   ^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$) — The ARN of the IAM
//!   role used for S3 access.
//! * `status` (LifeCycleState, one of: available | creating | deleted | deleting | error |
//!   misconfigured | updating) — The lifecycle state of the file system. Valid values are:
//!   AVAILABLE (the file system is available for use), CREATING (the file system is being
//!   created), DELETING (the file system is being deleted), DELETED (the file system has been
//!   deleted), ERROR (the file system is in an error state), MISCONFIGURED (the file system
//!   has a misconfiguration), or UPDATING (the file system is being updated).
//! * `statusMessage` (StatusMessage) — Additional information about the file system status.
//!   This field provides more details when the status is ERROR, or during state transitions.
//! * `tags` (TagList, len 1..50) — The tags associated with the file system.
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
//! * `ServiceQuotaExceededException` (HTTP 402) — The request would exceed a service quota.
//!   Review your service quotas and either delete resources or request a quota increase.
//! * `ValidationException` (HTTP 400) — The input parameters are not valid. Check the
//!   parameter values and try again.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateFileSystemError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-s3files-s3files.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateFileSystemError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.s3files#CreateFileSystem")]
#[derive(Debug)]
pub enum CreateFileSystemErrorModeled {
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_file_system::CreateFileSystemError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateFileSystemError = crate::scaffolded::Scaffolded<CreateFileSystemErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateFileSystemErrorModeled> for CreateFileSystemError {
    fn from(modeled: CreateFileSystemErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateFileSystem` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateFileSystem<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateFileSystem<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateFileSystem<Eng>
{
    type Input = CreateFileSystemInput;
    type Output = CreateFileSystemOutput;
    type Error = CreateFileSystemError;

    const NAME: &'static str = "CreateFileSystem";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_FILE_SYSTEM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_FILE_SYSTEM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx);

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateFileSystemErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
        })?;
        let mut store = state.lock();
        store.settle(now);

        // Caller-driven growth needs a ceiling, and this operation models the quota
        // error for refusing past one. See `state::MAX_FILE_SYSTEMS`.
        if store.file_systems.len() >= crate::state::MAX_FILE_SYSTEMS {
            return Err(CreateFileSystemErrorModeled::ServiceQuotaExceededException(
                crate::state::quota_exceeded(format!(
                    "the number of file systems would exceed the limit of {}",
                    crate::state::MAX_FILE_SYSTEMS
                )),
            )
            .into());
        }

        // `clientToken` is an @idempotencyToken: a replay returns the file system the
        // first request minted instead of creating a second one. A caller that sent
        // none gets one minted, so the creation is still recorded under a token.
        let client_token = match input.client_token {
            Some(token) => token,
            None => store.mint_client_token(),
        };
        let replayed = store
            .replayed_id(Self::NAME, Some(client_token.as_str()))
            .filter(|id| store.file_systems.contains_key(id));

        let file_system_id = match replayed {
            Some(file_system_id) => file_system_id,
            None => {
                let file_system_id = store.mint_file_system_id();
                let sequence = store.next_sequence();
                let tags = input.tags.unwrap_or_default();
                let record = crate::state::FileSystem {
                    file_system_arn: crate::state::file_system_arn(&region, &file_system_id),
                    file_system_id: file_system_id.clone(),
                    bucket: input.bucket,
                    // `prefix` carries @default(""): a caller that named none scopes
                    // the file system to the whole bucket.
                    prefix: input.prefix.unwrap_or_default(),
                    kms_key_id: input.kms_key_id,
                    role_arn: input.role_arn,
                    owner_id: ::engine::emulator_account_id().to_string(),
                    // A new file system is not usable yet: the clock owes it
                    // `available`.
                    status: crate::state::STATUS_CREATING.to_string(),
                    status_message: None,
                    creation_time: now,
                    available_due_at: now + crate::state::PROVISIONING_SPAN,
                    client_token: Some(client_token.clone()),
                    name: crate::state::name_from_tags(&tags),
                    tags,
                    // A new file system carries no resource policy until
                    // `PutFileSystemPolicy` attaches one.
                    policy: None,
                    // Nor any synchronization configuration until
                    // `PutSynchronizationConfiguration` creates one.
                    synchronization: None,
                    sequence,
                };
                store.file_systems.insert(file_system_id.clone(), record);
                store.remember_client_token(Self::NAME, &client_token, &file_system_id);
                file_system_id
            }
        };

        // Answer from the stored record, so the response cannot drift from what a later
        // GetFileSystem / ListFileSystems reads.
        let stored = store.file_systems.get(&file_system_id).ok_or_else(|| {
            CreateFileSystemErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
        })?;
        Ok(CreateFileSystemOutput {
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
