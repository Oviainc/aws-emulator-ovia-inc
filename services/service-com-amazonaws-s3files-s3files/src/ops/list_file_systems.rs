// SPDX-License-Identifier: CC0-1.0

//! @op ListFileSystems
//!
//! Reports every persisted file system, optionally only those on one `bucket`, in one
//! stable creation order, cut into pages of at most `maxResults` with an opaque
//! `nextToken` that resumes exactly where the previous page ended. The only state this
//! reporting operation writes is the `creating` -> `available` transition the clock
//! already made due.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListFileSystems`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json")
//! ```
//!
//! Returns a list of all S3 File Systems owned by the account with optional filtering by
//! bucket.
//!
//! ## Input
//!
//! * `bucket` (BucketArn, pattern ^(arn:aws[a-zA-Z0-9-]*:s3:::.+)$) — Optional filter to list
//!   only file systems associated with the specified S3 bucket Amazon Resource Name (ARN). If
//!   provided, only file systems that provide access to this bucket will be returned in the
//!   response.
//! * `maxResults` (Integer, default 100) **required** — The maximum number of file systems to
//!   return in a single response. If not specified, up to 100 file systems are returned.
//! * `nextToken` (String) — A pagination token returned from a previous call to continue
//!   listing file systems.
//!
//! ## Output
//!
//! * `fileSystems` (FileSystems) **required** — An array of file system descriptions.
//! * `nextToken` (String) — A pagination token to use in a subsequent request if more results
//!   are available.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ValidationException` (HTTP 400) — The input parameters are not valid. Check the
//!   parameter values and try again.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListFileSystemsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-s3files-s3files.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListFileSystemsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.s3files#ListFileSystems")]
#[derive(Debug)]
pub enum ListFileSystemsErrorModeled {
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_file_systems::ListFileSystemsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListFileSystemsError = crate::scaffolded::Scaffolded<ListFileSystemsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListFileSystemsErrorModeled> for ListFileSystemsError {
    fn from(modeled: ListFileSystemsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListFileSystems` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListFileSystems<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListFileSystems<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListFileSystems<Eng>
{
    type Input = ListFileSystemsInput;
    type Output = ListFileSystemsOutput;
    type Error = ListFileSystemsError;

    const NAME: &'static str = "ListFileSystems";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_FILE_SYSTEMS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_FILE_SYSTEMS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListFileSystemsErrorModeled::InternalServerException(crate::state::store_unavailable())
        })?;
        let mut store = state.lock();
        // The only write a reporting operation makes: the transition the clock already
        // owed at this instant, whether or not anybody called.
        store.settle(now);

        let after = crate::state::decode_cursor(input.next_token.as_deref()).map_err(|_| {
            ListFileSystemsErrorModeled::ValidationException(crate::state::invalid(
                "nextToken is not a token this service issued",
            ))
        })?;

        let ordered: Vec<(u64, ListFileSystemsDescription)> = store
            .file_systems_in_order(input.bucket.as_deref())
            .into_iter()
            .map(|file_system| {
                (
                    file_system.sequence,
                    ListFileSystemsDescription {
                        creation_time: ::aws_smithy_types::DateTime::from(
                            file_system.creation_time,
                        ),
                        file_system_arn: file_system.file_system_arn.clone(),
                        file_system_id: file_system.file_system_id.clone(),
                        name: file_system.name.clone(),
                        bucket: file_system.bucket.clone(),
                        status: file_system.status.clone(),
                        status_message: file_system.status_message.clone(),
                        role_arn: file_system.role_arn.clone(),
                        owner_id: file_system.owner_id.clone(),
                    },
                )
            })
            .collect();

        let page = crate::state::page_of(
            ordered,
            after,
            input.max_results,
            crate::state::MAX_RESULTS_LIMIT,
        );
        Ok(ListFileSystemsOutput {
            next_token: page.next_token,
            file_systems: page.items,
        })
    }
}
