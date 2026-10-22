// SPDX-License-Identifier: CC0-1.0

//! @op ListAccessPoints
//!
//! Reports the access points persisted under ONE file system — the parent the request
//! names, by bare id or by ARN — in creation order, cut into pages of at most
//! `maxResults` with the opaque resume cursor [`crate::state::encode_cursor`] mints.
//! Nothing is canned: every member of every description is read from the stored
//! [`crate::state::AccessPoint`], so a listed access point is exactly what
//! `GetAccessPoint` reads back.
//!
//! A reporting operation, so the only write it makes is
//! [`crate::state::ComAmazonawsS3filesS3filesState::settle`]: the `creating` ->
//! `available` transition the clock already owed at this instant.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListAccessPoints`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json")
//! ```
//!
//! Returns resource information for all S3 File System Access Points associated with the
//! specified S3 File System.
//!
//! ## Input
//!
//! * `fileSystemId` (FileSystemId, len ..128, pattern
//!   ^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}|fs-[0-9a-f]{17,40})$)
//!   **required** — The ID or Amazon Resource Name (ARN) of the S3 File System to list access
//!   points for.
//! * `maxResults` (Integer, default 100) **required** — The maximum number of access points
//!   to return in a single response.
//! * `nextToken` (String) — A pagination token returned from a previous call to continue
//!   listing access points.
//!
//! ## Output
//!
//! * `accessPoints` (AccessPoints) **required** — An array of access point descriptions.
//! * `nextToken` (String) — A pagination token to use in a subsequent request if more results
//!   are available.
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
// `ListAccessPointsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-s3files-s3files.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAccessPointsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.s3files#ListAccessPoints")]
#[derive(Debug)]
pub enum ListAccessPointsErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_access_points::ListAccessPointsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAccessPointsError = crate::scaffolded::Scaffolded<ListAccessPointsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAccessPointsErrorModeled> for ListAccessPointsError {
    fn from(modeled: ListAccessPointsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAccessPoints` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAccessPoints<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAccessPoints<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAccessPoints<Eng>
{
    type Input = ListAccessPointsInput;
    type Output = ListAccessPointsOutput;
    type Error = ListAccessPointsError;

    const NAME: &'static str = "ListAccessPoints";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ACCESS_POINTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ACCESS_POINTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListAccessPointsErrorModeled::InternalServerException(crate::state::store_unavailable())
        })?;
        let mut store = state.lock();
        // The only write a reporting operation makes: the transition the clock already
        // owed at this instant, whether or not anybody called.
        store.settle(now);

        // `fileSystemId` accepts the bare id or the ARN; both name the same record.
        let file_system_id = crate::state::file_system_id_of(&input.file_system_id)
            .ok_or_else(|| {
                ListAccessPointsErrorModeled::ValidationException(crate::state::invalid(format!(
                    "fileSystemId must be a file system id (fs-<hex>) or its ARN, got {:?}",
                    input.file_system_id
                )))
            })?
            .to_string();
        // The listing is scoped to the file system the request names, so a parent this
        // service does not hold is the modelled 404 rather than an empty page.
        if !store.file_systems.contains_key(&file_system_id) {
            return Err(ListAccessPointsErrorModeled::ResourceNotFoundException(
                crate::state::not_found(format!("no file system {file_system_id}")),
            )
            .into());
        }

        let after = crate::state::decode_cursor(input.next_token.as_deref()).map_err(|_| {
            ListAccessPointsErrorModeled::ValidationException(crate::state::invalid(
                "nextToken is not a token this service issued",
            ))
        })?;

        let ordered: Vec<(u64, ListAccessPointsDescription)> = store
            .access_points_in_order(&file_system_id)
            .into_iter()
            .map(|access_point| {
                (
                    access_point.sequence,
                    ListAccessPointsDescription {
                        access_point_arn: access_point.access_point_arn.clone(),
                        access_point_id: access_point.access_point_id.clone(),
                        file_system_id: access_point.file_system_id.clone(),
                        status: access_point.status.clone(),
                        owner_id: access_point.owner_id.clone(),
                        posix_user: access_point.posix_user.clone(),
                        root_directory: access_point.root_directory.clone(),
                        name: access_point.name.clone(),
                    },
                )
            })
            .collect();

        // `ListAccessPointsRequest$maxResults` is `@range(1, 1000)` — a different
        // ceiling from the other listings' — so this operation passes its own.
        let page = crate::state::page_of(
            ordered,
            after,
            input.max_results,
            crate::state::MAX_RESULTS_LIMIT_ACCESS_POINTS,
        );
        Ok(ListAccessPointsOutput {
            next_token: page.next_token,
            access_points: page.items,
        })
    }
}
