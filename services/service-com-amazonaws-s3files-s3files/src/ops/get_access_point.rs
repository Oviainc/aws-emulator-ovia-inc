// SPDX-License-Identifier: CC0-1.0

//! @op GetAccessPoint
//!
//! Reports the persisted access point `accessPointId` names, accepting either the bare
//! `fsap-` id or the access point's ARN. The only state this reporting operation writes
//! is the `creating` -> `available` transition the clock already made due.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetAccessPoint`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json")
//! ```
//!
//! Returns resource information for an S3 File System Access Point.
//!
//! ## Input
//!
//! * `accessPointId` (AccessPointId, len ..256, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}/access-point/fsap-[0-9a-f]{17,40}|fsap-[0-9a-f]{17,4
//!   …) **required** — The ID or Amazon Resource Name (ARN) of the access point to retrieve
//!   information for.
//!
//! ## Output
//!
//! * `accessPointArn` (AccessPointArn, len ..256, pattern
//!   ^arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}/access-point/fsap-[0-9a-f]{17,40}$)
//!   **required** — The ARN of the access point.
//! * `accessPointId` (AccessPointId, len ..256, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}/access-point/fsap-[0-9a-f]{17,40}|fsap-[0-9a-f]{17,4
//!   …) **required** — The ID of the access point.
//! * `clientToken` (ClientToken, len 1..64, pattern ^(.+)$) **required** — The client token
//!   used for idempotency when the access point was created.
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
// `GetAccessPointError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-s3files-s3files.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAccessPointError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.s3files#GetAccessPoint")]
#[derive(Debug)]
pub enum GetAccessPointErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_access_point::GetAccessPointError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetAccessPointError = crate::scaffolded::Scaffolded<GetAccessPointErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetAccessPointErrorModeled> for GetAccessPointError {
    fn from(modeled: GetAccessPointErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAccessPoint` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetAccessPoint<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAccessPoint<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAccessPoint<Eng>
{
    type Input = GetAccessPointInput;
    type Output = GetAccessPointOutput;
    type Error = GetAccessPointError;

    const NAME: &'static str = "GetAccessPoint";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ACCESS_POINT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ACCESS_POINT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetAccessPointErrorModeled::InternalServerException(crate::state::store_unavailable())
        })?;
        let mut store = state.lock();
        // The only write a reporting operation makes: the transition the clock already
        // owed at this instant, whether or not anybody called.
        store.settle(now);

        let access_point_id = crate::state::access_point_id_of(&input.access_point_id)
            .ok_or_else(|| {
                GetAccessPointErrorModeled::ValidationException(crate::state::invalid(format!(
                    "accessPointId must be an access point id (fsap-<hex>) or its ARN, got {:?}",
                    input.access_point_id
                )))
            })?
            .to_string();

        let stored = store.access_points.get(&access_point_id).ok_or_else(|| {
            GetAccessPointErrorModeled::ResourceNotFoundException(crate::state::not_found(
                crate::state::ACCESS_POINT_NOT_FOUND_MESSAGE,
            ))
        })?;

        Ok(GetAccessPointOutput {
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
