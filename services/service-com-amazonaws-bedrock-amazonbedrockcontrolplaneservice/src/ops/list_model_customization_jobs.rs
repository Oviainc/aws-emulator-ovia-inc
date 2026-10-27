// SPDX-License-Identifier: CC0-1.0

//! @op ListModelCustomizationJobs
//!
//! Reports the model customization jobs of the signing region out of [`crate::state`]:
//! filtered, sorted, and paged — the jobs `ops/create_model_customization_job.rs` admitted
//! and no others, so a create that was refused is reported by nothing here.
//!
//! The rules, each read off the operation's own members
//! (`smithy_query(query="operation",operation="ListModelCustomizationJobs")`) rather than
//! off one request:
//!
//! * every filter the request carries must hold for a job to be listed — `nameContains` on
//!   a substring of its name, `statusEquals` on the status it holds now, and
//!   `creationTimeAfter` / `creationTimeBefore` on the instant it was submitted;
//! * `sortBy` admits only `CreationTime`, so the listing is ordered by that instant, in
//!   `sortOrder` (`Ascending` by default), with the ARN breaking a tie so the order is
//!   total and a page boundary is unambiguous;
//! * `maxResults` bounds the page and a further page is reported by `nextToken`, which is
//!   the cursor of the row the next page starts on — so the pages partition the listing
//!   with neither a gap nor a repeat.
//!
//! `@readonly`, and the one write it makes is the status transition the CLOCK already made
//! due for each job
//! ([`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::settle_model_customization_jobs`]):
//! a listing reports — and `statusEquals` filters on — the status each job holds now, which
//! is the status `ops/get_model_customization_job.rs` would report for that one job at the
//! same instant whether or not anybody listed.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListModelCustomizationJobs`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Returns a list of model customization jobs that you have submitted. You can filter the
//! jobs to return based on one or more criteria. For more information, see Custom models in
//! the Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `creationTimeAfter` (Timestamp) — Return customization jobs created after the specified
//!   time.
//! * `creationTimeBefore` (Timestamp) — Return customization jobs created before the
//!   specified time.
//! * `maxResults` (MaxResults, range 1..1000) — The maximum number of results to return in
//!   the response. If the total number of results is greater than this value, use the token
//!   returned in the response in the nextToken field when making another request to return
//!   the next batch of results.
//! * `nameContains` (JobName, len 1..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9\+\-\.])*$) —
//!   Return customization jobs only if the job name contains these characters.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — If the total number of
//!   results is greater than the maxResults value provided in the request, enter the token
//!   returned in the nextToken field in the response in this field to return the next batch
//!   of results.
//! * `sortBy` (SortJobsBy, one of: CreationTime) — The field to sort by in the returned list
//!   of jobs.
//! * `sortOrder` (SortOrder, one of: Ascending | Descending) — The sort order of the results.
//! * `statusEquals` (FineTuningJobStatus, one of: Completed | Failed | InProgress | Stopped |
//!   Stopping) — Return customization jobs with the specified status.
//!
//! ## Output
//!
//! * `modelCustomizationJobSummaries` (ModelCustomizationJobSummaries) — Job summaries.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — If the total number of
//!   results is greater than the maxResults value provided in the request, use this token
//!   when making another request in the nextToken field to return the next batch of results.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListModelCustomizationJobsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListModelCustomizationJobsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ListModelCustomizationJobs")]
#[derive(Debug)]
pub enum ListModelCustomizationJobsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_model_customization_jobs::ListModelCustomizationJobsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListModelCustomizationJobsError = crate::scaffolded::Scaffolded<ListModelCustomizationJobsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListModelCustomizationJobsErrorModeled> for ListModelCustomizationJobsError {
    fn from(modeled: ListModelCustomizationJobsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListModelCustomizationJobs` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListModelCustomizationJobs<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListModelCustomizationJobs<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The largest page this listing answers with, and the page size an absent `maxResults`
/// takes: `maxResults` is modelled `range 1..1000`, so 1000 is the whole span it admits.
const MAX_PAGE_SIZE: usize = 1000;

/// `SortOrder` — oldest first, which is the order an absent `sortOrder` takes.
const SORT_ASCENDING: &str = "Ascending";

/// Whether every filter `input` carries holds for `record`.
///
/// The time bounds are STRICT, as the members word them: "created after the specified time"
/// and "created before the specified time".
fn matches(
    input: &ListModelCustomizationJobsInput,
    record: &crate::state::ModelCustomizationJobRecord,
) -> bool {
    let created = crate::state::epoch_seconds(record.created_at);
    if let Some(after) = input.creation_time_after.as_ref() {
        if created <= after.as_secs_f64() {
            return false;
        }
    }
    if let Some(before) = input.creation_time_before.as_ref() {
        if created >= before.as_secs_f64() {
            return false;
        }
    }
    if input
        .name_contains
        .as_deref()
        .is_some_and(|fragment| !record.name.contains(fragment))
    {
        return false;
    }
    if input
        .status_equals
        .as_deref()
        .is_some_and(|status| status != record.status)
    {
        return false;
    }
    true
}

/// One row of the listing: the stored job as the summary shape reports it.
///
/// Every member comes off the stored record, so a row says exactly what
/// `ops/get_model_customization_job.rs` says about the same job — including the two it
/// reports for the model the job is customizing, which are fixed from the create onwards.
fn summary(
    record: &crate::state::ModelCustomizationJobRecord,
) -> ModelCustomizationJobSummary {
    ModelCustomizationJobSummary {
        job_arn: record.arn.clone(),
        base_model_arn: record.base_model_arn.clone(),
        job_name: record.name.clone(),
        status: record.status.to_string(),
        // "Details about the status of the data processing sub-task of the job": this
        // emulator runs no sub-task, so there is no sub-task status to report — the same
        // answer the read of one job gives.
        status_details: None,
        last_modified_time: Some(crate::state::date_time(record.last_modified_at)),
        creation_time: crate::state::date_time(record.created_at),
        // "Time that the customization job ended": absent while it is still running or
        // winding down.
        end_time: record.ended_at.map(crate::state::date_time),
        custom_model_arn: Some(record.output_model_arn.clone()),
        custom_model_name: Some(record.output_model_name.clone()),
        customization_type: Some(record.customization_type.to_string()),
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListModelCustomizationJobs<Eng>
{
    type Input = ListModelCustomizationJobsInput;
    type Output = ListModelCustomizationJobsOutput;
    type Error = ListModelCustomizationJobsError;

    const NAME: &'static str = "ListModelCustomizationJobs";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_MODEL_CUSTOMIZATION_JOBS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_MODEL_CUSTOMIZATION_JOBS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ListModelCustomizationJobsErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "model customization job state is not available on this engine"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // The status each job holds NOW is what a listing reports and what `statusEquals`
        // filters on, so the transitions the clock has already made due are materialised
        // before either happens.
        store.settle_model_customization_jobs(now);

        let descending = input
            .sort_order
            .as_deref()
            .is_some_and(|order| order != SORT_ASCENDING);
        let mut rows: Vec<(String, ModelCustomizationJobSummary)> = store
            .model_customization_jobs
            .values()
            // The ARN carries the region, so a job of another region is another
            // account-region's resource and is not listed here.
            .filter(|record| record.region == region)
            .filter(|record| matches(&input, record))
            .map(|record| {
                (
                    crate::state::listing_cursor(record.created_at, &record.arn),
                    summary(record),
                )
            })
            .collect();
        // `sortBy` admits only `CreationTime`, which the cursor's leading half IS; its
        // trailing half breaks a tie by ARN, so the order is total either way round.
        rows.sort_by(|(left, _), (right, _)| left.cmp(right));
        if descending {
            rows.reverse();
        }

        if let Some(token) = input.next_token.as_deref() {
            if !crate::state::cursor_belongs(token) {
                return Err(
                    ListModelCustomizationJobsErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(
                                "nextToken is not a pagination token this service issued"
                                    .to_string(),
                            ),
                        },
                    )
                    .into(),
                );
            }
            rows.retain(|(cursor, _)| {
                if descending {
                    cursor.as_str() <= token
                } else {
                    cursor.as_str() >= token
                }
            });
        }

        let page_size = input
            .max_results
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(MAX_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        // The row the next page starts on IS the next page's cursor, so the pages
        // partition the listing with neither a gap nor a repeat.
        let next_token = rows.get(page_size).map(|(cursor, _)| cursor.clone());
        rows.truncate(page_size);

        Ok(ListModelCustomizationJobsOutput {
            model_customization_job_summaries: Some(
                rows.into_iter().map(|(_, summary)| summary).collect(),
            ),
            next_token,
        })
    }
}
