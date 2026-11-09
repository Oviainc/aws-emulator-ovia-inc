// SPDX-License-Identifier: CC0-1.0

//! @op ListApplications
//! Walks the applications in [`crate::state`], one page at a time: `maxItems` sizes the
//! page and `nextToken` resumes it, so consecutive pages visit every stored application
//! exactly once and the final page carries no token. A created application is present
//! and a deleted one is absent, because the page is read off the store.
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
//! # Modeled contract for `ListApplications`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json")
//! ```
//!
//! Lists applications owned by the requester.
//!
//! ## Input
//!
//! * `MaxItems` (MaxItems, range 1..100) — The total number of items to return.
//! * `NextToken` (__string) — A token to specify where to start paginating.
//!
//! ## Output
//!
//! * `Applications` (__listOfApplicationSummary) — An array of application summaries.
//! * `NextToken` (__string) — The token to request the next page of results.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — One of the parameters in the request is invalid.
//! * `ForbiddenException` (HTTP 403) — The client is not authenticated.
//! * `InternalServerErrorException` (HTTP 500) — The AWS Serverless Application Repository
//!   service encountered an internal error.
//! * `NotFoundException` (HTTP 404) — The resource (for example, an access policy statement)
//!   specified in the request doesn't exist.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListApplicationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListApplicationsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.serverlessapplicationrepository#ListApplications")]
#[derive(Debug)]
pub enum ListApplicationsErrorModeled {
    BadRequestException(BadRequestException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_applications::ListApplicationsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListApplicationsError = crate::scaffolded::Scaffolded<ListApplicationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListApplicationsErrorModeled> for ListApplicationsError {
    fn from(modeled: ListApplicationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListApplications` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListApplications<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListApplications<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListApplications<Eng>
{
    type Input = ListApplicationsInput;
    type Output = ListApplicationsOutput;
    type Error = ListApplicationsError;

    const NAME: &'static str = "ListApplications";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_APPLICATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_APPLICATIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // AWS decides a signed `serverlessrepo` call against the CALLER's identity-based
        // policies BEFORE the operation reads anything. This listing addresses no single
        // application — `serverlessrepo:ListApplications` is an action AWS defines over no
        // resource type — so the decision is made about
        // [`crate::state::authorization::ANY_RESOURCE`].
        crate::state::authorization::authorize(
            &*self.engine,
            ctx,
            Self::NAME,
            crate::state::authorization::ANY_RESOURCE,
        )?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListApplicationsErrorModeled::InternalServerErrorException(
                crate::state::internal_server_error("the service state is unavailable"),
            )
            .into());
        };

        let store = state.lock();
        let page = crate::state::page_of(
            store.applications.keys().cloned(),
            input.max_items,
            input.next_token.as_deref(),
        )
        .map_err(|error| {
            ListApplicationsError::from(ListApplicationsErrorModeled::BadRequestException(
                crate::state::bad_request(error.message()),
            ))
        })?;

        let applications: Vec<ApplicationSummary> = page
            .keys
            .iter()
            .filter_map(|application_id| store.applications.get(application_id))
            .map(crate::state::Application::summary)
            .collect();

        Ok(ListApplicationsOutput {
            applications: Some(applications),
            next_token: page.next_token,
        })
    }
}
