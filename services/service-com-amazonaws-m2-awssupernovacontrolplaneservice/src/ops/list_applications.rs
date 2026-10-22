// SPDX-License-Identifier: CC0-1.0

//! @op ListApplications
//!
//! Reports one `ApplicationSummary` per persisted application, newest last, honouring the
//! `names` and `environmentId` filters and the `@paginated` contract: at most
//! `maxResults` items, a `nextToken` only when more remain, and a resume that neither
//! repeats nor skips an item.
//!
//! This is a REPORTING operation (`@readonly` in the model): it writes nothing, and the
//! status it reports is computed from each record's stored settle instant.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-m2-awssupernovacontrolplaneservice/smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json")
//! ```
//!
//! Lists the applications associated with a specific Amazon Web Services account. You can
//! provide the unique identifier of a specific runtime environment in a query parameter to
//! see all applications associated with that environment.
//!
//! ## Input
//!
//! * `environmentId` (Identifier, pattern ^\S{1,80}$) — The unique identifier of the runtime
//!   environment where the applications are deployed.
//! * `maxResults` (MaxResults, range 1..2000) — The maximum number of applications to return.
//! * `names` (EntityNameList, len 1..10) — The names of the applications.
//! * `nextToken` (NextToken, pattern ^\S{1,2000}$) — A pagination token to control the number
//!   of applications displayed in the list.
//!
//! ## Output
//!
//! * `applications` (ApplicationSummaryList) **required** — Returns a list of summary details
//!   for all the applications in a runtime environment.
//! * `nextToken` (NextToken, pattern ^\S{1,2000}$) — A pagination token that's returned when
//!   the response doesn't contain all applications.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The account or role doesn't have the right
//!   permissions to make the request.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during the
//!   processing of the request.
//! * `ThrottlingException` (HTTP 429) — The number of requests made exceeds the limit.
//! * `ValidationException` (HTTP 400) — One or more parameters provided in the request is not
//!   valid.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{decode_page_token, encode_page_token, page_key, Application};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListApplicationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListApplicationsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.m2#ListApplications")]
#[derive(Debug)]
pub enum ListApplicationsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
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

/// The summary this operation reports for one persisted application.
fn summary_of(application: &Application, now: SystemTime) -> ApplicationSummary {
    ApplicationSummary {
        name: application.name.clone(),
        description: application.description.clone(),
        application_id: application.application_id.clone(),
        application_arn: application.application_arn.clone(),
        application_version: application.latest_version,
        status: application.status(now).to_string(),
        engine_type: application.engine_type.clone(),
        creation_time: ::aws_smithy_types::DateTime::from(application.creation_time),
        environment_id: application.environment_id.clone(),
        last_start_time: application
            .last_start_time
            .map(::aws_smithy_types::DateTime::from),
        version_status: application
            .latest()
            .map(|latest| latest.status(now).to_string()),
        // An application reports a deployment status only once it has been deployed to a
        // runtime environment, which this slice of the service does not record.
        deployment_status: None,
        role_arn: application.role_arn.clone(),
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
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListApplicationsErrorModeled::InternalServerException(InternalServerException {
                message: "the m2 application store is unavailable".to_string(),
                retry_after_seconds: 0,
            })
        })?;

        // A token this service did not issue is a bad request, not an empty page.
        let resume = match input.next_token.as_deref() {
            Some(token) => Some(decode_page_token(token).ok_or_else(|| {
                ListApplicationsErrorModeled::ValidationException(ValidationException {
                    message: "nextToken is not a pagination token this service issued"
                        .to_string(),
                    reason: Some("fieldValidationFailed".to_string()),
                    field_list: Some(vec![ValidationExceptionField {
                        name: "nextToken".to_string(),
                        message: "not a pagination token issued by ListApplications".to_string(),
                    }]),
                })
            })?),
            None => None,
        };

        let now = SystemTime::now();
        let store = state.lock();

        let mut matching: Vec<&Application> = store
            .applications
            .values()
            .filter(|application| {
                input
                    .names
                    .as_ref()
                    .is_none_or(|names| names.contains(&application.name))
            })
            .filter(|application| {
                input.environment_id.as_ref().is_none_or(|environment| {
                    application.environment_id.as_deref() == Some(environment.as_str())
                })
            })
            .collect();
        // One total, stable order, so a page boundary means the same thing on every call.
        matching.sort_by_key(|application| {
            page_key(application.creation_time, &application.application_id)
        });

        // Resume AFTER the item the token names, by key rather than by offset.
        let start = match resume {
            Some(key) => matching.partition_point(|application| {
                page_key(application.creation_time, &application.application_id) <= key
            }),
            None => 0,
        };
        let remaining = &matching[start.min(matching.len())..];

        // A listing is never unbounded: without `maxResults` the page is clamped to the
        // service's own default and a `nextToken` carries the rest.
        let page_size = input.max_results.map_or(
            crate::state::DEFAULT_LIST_PAGE_SIZE,
            |limit| usize::try_from(limit).unwrap_or(crate::state::DEFAULT_LIST_PAGE_SIZE),
        );
        let truncated = remaining.len() > page_size;
        let page = &remaining[..page_size.min(remaining.len())];

        let applications = page
            .iter()
            .map(|application| summary_of(application, now))
            .collect();
        // A token ONLY when items remain beyond this page.
        let next_token = truncated
            .then(|| {
                page.last().map(|last| {
                    encode_page_token(last.creation_time, &last.application_id)
                })
            })
            .flatten();

        Ok(ListApplicationsOutput {
            applications,
            next_token,
        })
    }
}
