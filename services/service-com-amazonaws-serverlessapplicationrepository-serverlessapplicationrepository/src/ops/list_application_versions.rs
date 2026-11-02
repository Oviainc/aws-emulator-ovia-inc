// SPDX-License-Identifier: CC0-1.0

//! @op ListApplicationVersions
//! Walks the versions one application holds in [`crate::state`], one page at a time:
//! `maxItems` sizes the page and `nextToken` resumes it, so consecutive pages visit
//! every stored version exactly once and the final page carries no token. The walk is
//! ordered by semantic-version precedence
//! ([`crate::state::Application::versions_by_precedence`]), the same ordering that picks
//! the application's latest version.
//!
//! A pure read: it answers from the store and writes nothing.
//!
//! This is the ONLY file to touch for this operation.
//!
//! The engine is held on `self` (constructor-injected); reach the store through
//! [`crate::state::service_state`]. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListApplicationVersions`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json")
//! ```
//!
//! Lists versions for the specified application.
//!
//! ## Input
//!
//! * `ApplicationId` (__string) **required** — The Amazon Resource Name (ARN) of the
//!   application.
//! * `MaxItems` (MaxItems, range 1..100) — The total number of items to return.
//! * `NextToken` (__string) — A token to specify where to start paginating.
//!
//! ## Output
//!
//! * `NextToken` (__string) — The token to request the next page of results.
//! * `Versions` (__listOfVersionSummary) — An array of version summaries for the application.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — One of the parameters in the request is invalid.
//! * `ForbiddenException` (HTTP 403) — The client is not authenticated.
//! * `InternalServerErrorException` (HTTP 500) — The AWS Serverless Application Repository
//!   service encountered an internal error.
//! * `NotFoundException` (HTTP 404) — The resource (for example, an access policy statement)
//!   specified in the request doesn't exist.
//! * `TooManyRequestsException` (HTTP 429) — The client is sending more than the allowed
//!   number of requests per unit of time.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListApplicationVersionsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListApplicationVersionsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.serverlessapplicationrepository#ListApplicationVersions")]
#[derive(Debug)]
pub enum ListApplicationVersionsErrorModeled {
    BadRequestException(BadRequestException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_application_versions::ListApplicationVersionsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListApplicationVersionsError = crate::scaffolded::Scaffolded<ListApplicationVersionsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListApplicationVersionsErrorModeled> for ListApplicationVersionsError {
    fn from(modeled: ListApplicationVersionsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListApplicationVersions` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListApplicationVersions<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListApplicationVersions<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListApplicationVersions<Eng>
{
    type Input = ListApplicationVersionsInput;
    type Output = ListApplicationVersionsOutput;
    type Error = ListApplicationVersionsError;

    const NAME: &'static str = "ListApplicationVersions";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_APPLICATION_VERSIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_APPLICATION_VERSIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListApplicationVersionsErrorModeled::InternalServerErrorException(
                crate::state::internal_server_error("the service state is unavailable"),
            )
            .into());
        };

        let store = state.lock();
        let Some(application) = store.applications.get(&input.application_id) else {
            return Err(ListApplicationVersionsErrorModeled::NotFoundException(
                crate::state::not_found(
                    crate::state::ErrorCode::ApplicationNotFound,
                    crate::state::application_not_found_message(&input.application_id),
                ),
            )
            .into());
        };

        // The collection is walked in ascending semantic-version precedence, the order a
        // version list is reported in, which `crate::state` derives from the stored
        // versions themselves — so the walk's last entry is the version the application
        // reports as its latest, and `1.10.0` follows `1.9.0` rather than preceding it as
        // its text would.
        let page = crate::state::page_of(
            application.versions_by_precedence().into_iter(),
            input.max_items,
            input.next_token.as_deref(),
        )
        .map_err(|error| {
            ListApplicationVersionsError::from(
                ListApplicationVersionsErrorModeled::BadRequestException(
                    crate::state::bad_request(error.message()),
                ),
            )
        })?;

        let versions: Vec<VersionSummary> = page
            .keys
            .iter()
            .filter_map(|semantic_version| application.versions.get(semantic_version))
            .map(|version| version.summary(&input.application_id))
            .collect();

        Ok(ListApplicationVersionsOutput {
            next_token: page.next_token,
            versions: Some(versions),
        })
    }
}
