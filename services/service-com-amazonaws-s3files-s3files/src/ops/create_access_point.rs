// SPDX-License-Identifier: CC0-1.0

//! @op CreateAccessPoint
//!
//! Mints an access point under an existing file system, persists it in
//! [`crate::state`] and answers from the stored record, so what this returns is exactly
//! what `GetAccessPoint` reads back.
//!
//! `clientToken` is an `@idempotencyToken`, scoped to the parent file system: a second
//! call carrying the same token under the same parent returns the access point the
//! first one created. A new access point is reported `creating` and the clock owes it
//! `available` ([`crate::state::PROVISIONING_SPAN`]).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateAccessPoint`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json")
//! ```
//!
//! Creates an S3 File System Access Point for application-specific access with POSIX user
//! identity and root directory enforcement. Access points provide a way to manage access to
//! shared datasets in multi-tenant scenarios.
//!
//! ## Input
//!
//! * `clientToken` (ClientToken, len 1..64, pattern ^(.+)$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the operation completes no more than one time.
//!   If this token matches a previous request, Amazon Web Services ignores the request, but
//!   does not return an error.
//! * `fileSystemId` (FileSystemId, len ..128, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}|fs-[0-9a-f]{17,40})$)
//!   **required** — The ID or Amazon Resource Name (ARN) of the S3 File System.
//! * `posixUser` (PosixUser) — The POSIX identity with uid, gid, and secondary group IDs for
//!   user enforcement when accessing the file system through this access point.
//! * `rootDirectory` (RootDirectory) — The root directory path for the access point, with
//!   optional creation permissions for newly created directories.
//! * `tags` (TagList, len 1..50) — An array of key-value pairs to apply to the access point
//!   for resource tagging.
//!
//! ## Output
//!
//! * `accessPointArn` (AccessPointArn, len ..256, pattern
//!   ^arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}/access-point/fsap-[0-9a-f]{17,40}$)
//!   **required** — The Amazon Resource Name (ARN) of the access point.
//! * `accessPointId` (AccessPointId, len ..256, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}/access-point/fsap-[0-9a-f]{17,40}|fsap-[0-9a-f]{17,4
//!   …) **required** — The ID of the access point.
//! * `clientToken` (ClientToken, len 1..64, pattern ^(.+)$) **required** — The client token
//!   that was provided in the request.
//! * `fileSystemId` (FileSystemId, len ..128, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}|fs-[0-9a-f]{17,40})$)
//!   **required** — The ID of the S3 File System.
//! * `name` (TagValue, len ..256, pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$) — The name of the
//!   access point.
//! * `ownerId` (AwsAccountId, len ..12, pattern ^(\d{12})|(\d{4}-{4}-\d{4})$) **required** —
//!   The Amazon Web Services account ID of the access point owner.
//! * `posixUser` (PosixUser) — The POSIX identity configured for this access point.
//! * `rootDirectory` (RootDirectory) — The root directory configuration for this access
//!   point.
//! * `status` (LifeCycleState, one of: available | creating | deleted | deleting | error |
//!   misconfigured | updating) **required** — The current status of the access point.
//! * `tags` (TagList, len 1..50) — The tags associated with the access point.
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
// `CreateAccessPointError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-s3files-s3files.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateAccessPointError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.s3files#CreateAccessPoint")]
#[derive(Debug)]
pub enum CreateAccessPointErrorModeled {
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_access_point::CreateAccessPointError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateAccessPointError = crate::scaffolded::Scaffolded<CreateAccessPointErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateAccessPointErrorModeled> for CreateAccessPointError {
    fn from(modeled: CreateAccessPointErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateAccessPoint` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateAccessPoint<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateAccessPoint<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateAccessPoint<Eng>
{
    type Input = CreateAccessPointInput;
    type Output = CreateAccessPointOutput;
    type Error = CreateAccessPointError;

    const NAME: &'static str = "CreateAccessPoint";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ACCESS_POINT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ACCESS_POINT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateAccessPointErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
        })?;
        let mut store = state.lock();
        store.settle(now);

        // `fileSystemId` accepts the bare id or the ARN; both name the same record.
        let file_system_id = crate::state::file_system_id_of(&input.file_system_id)
            .ok_or_else(|| {
                CreateAccessPointErrorModeled::ValidationException(crate::state::invalid(format!(
                    "fileSystemId must be a file system id (fs-<hex>) or its ARN, got {:?}",
                    input.file_system_id
                )))
            })?
            .to_string();
        let file_system_arn = store
            .file_systems
            .get(&file_system_id)
            .ok_or_else(|| {
                CreateAccessPointErrorModeled::ResourceNotFoundException(crate::state::not_found(
                    crate::state::FILE_SYSTEM_NOT_FOUND_MESSAGE,
                ))
            })?
            .file_system_arn
            .clone();

        // Caller-driven growth needs a ceiling, and this operation models the quota
        // error for refusing past one. See `state::MAX_ACCESS_POINTS_PER_FILE_SYSTEM`.
        if store.access_point_ids_of(&file_system_id).len()
            >= crate::state::MAX_ACCESS_POINTS_PER_FILE_SYSTEM
        {
            return Err(CreateAccessPointErrorModeled::ServiceQuotaExceededException(
                crate::state::quota_exceeded(format!(
                    "the number of access points on {file_system_id} would exceed the limit of {}",
                    crate::state::MAX_ACCESS_POINTS_PER_FILE_SYSTEM
                )),
            )
            .into());
        }

        // The idempotency scope carries the parent, so one token cannot be shared
        // across two file systems.
        let scope = format!("{}/{file_system_id}", Self::NAME);
        let client_token = match input.client_token {
            Some(token) => token,
            None => store.mint_client_token(),
        };
        let replayed = store
            .replayed_id(&scope, Some(client_token.as_str()))
            .filter(|id| store.access_points.contains_key(id));

        let access_point_id = match replayed {
            Some(access_point_id) => access_point_id,
            None => {
                let access_point_id = store.mint_access_point_id();
                let sequence = store.next_sequence();
                let tags = input.tags.unwrap_or_default();
                let record = crate::state::AccessPoint {
                    access_point_arn: crate::state::access_point_arn(
                        &file_system_arn,
                        &access_point_id,
                    ),
                    access_point_id: access_point_id.clone(),
                    file_system_id: file_system_id.clone(),
                    client_token: client_token.clone(),
                    owner_id: ::engine::emulator_account_id().to_string(),
                    // A new access point is not usable yet: the clock owes it
                    // `available`.
                    status: crate::state::STATUS_CREATING.to_string(),
                    available_due_at: now + crate::state::PROVISIONING_SPAN,
                    posix_user: input.posix_user,
                    root_directory: input.root_directory,
                    name: crate::state::name_from_tags(&tags),
                    tags,
                    sequence,
                };
                store.access_points.insert(access_point_id.clone(), record);
                store.remember_client_token(&scope, &client_token, &access_point_id);
                access_point_id
            }
        };

        // Answer from the stored record, so the response cannot drift from what a later
        // GetAccessPoint reads.
        let stored = store.access_points.get(&access_point_id).ok_or_else(|| {
            CreateAccessPointErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
        })?;
        Ok(CreateAccessPointOutput {
            access_point_arn: stored.access_point_arn.clone(),
            access_point_id: stored.access_point_id.clone(),
            client_token: stored.client_token.clone(),
            file_system_id: stored.file_system_id.clone(),
            status: stored.status.clone(),
            owner_id: stored.owner_id.clone(),
            posix_user: stored.posix_user.clone(),
            root_directory: stored.root_directory.clone(),
            tags: (!stored.tags.is_empty()).then(|| stored.tags.clone()),
            name: stored.name.clone(),
        })
    }
}
