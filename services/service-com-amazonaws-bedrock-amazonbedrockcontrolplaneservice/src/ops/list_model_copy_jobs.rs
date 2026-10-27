// SPDX-License-Identifier: CC0-1.0

//! @op ListModelCopyJobs
//!
//! Lists the model copy jobs of the region the request addresses, out of what
//! `ops/create_model_copy_job.rs` submitted into [`crate::state`]: one
//! `ModelCopyJobSummary` per job the request's filters admit, in `creationTime` order,
//! paginated.
//!
//! Every filter the model declares is applied ([`matches`]): the two creation-time bounds,
//! `statusEquals`, `sourceAccountEquals`, `sourceModelArnEquals` and the
//! `targetModelNameContains` fragment. `sortBy` admits only `CreationTime`, which is the
//! order the listing is built in, and `sortOrder` turns it round.
//!
//! A read: the only write on this path is the one the CLOCK already made due — the
//! completions the listed jobs were owed, which is what the status each one reports and
//! what `statusEquals` filters on.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListModelCopyJobs`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Returns a list of model copy jobs that you have submitted. You can filter the jobs to
//! return based on one or more criteria. For more information, see Copy models to be used
//! in other regions in the Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `creationTimeAfter` (Timestamp) — Filters for model copy jobs created after the
//!   specified time.
//! * `creationTimeBefore` (Timestamp) — Filters for model copy jobs created before the
//!   specified time.
//! * `maxResults` (MaxResults, range 1..1000) — The maximum number of results to return in
//!   the response. If the total number of results is greater than this value, use the token
//!   returned in the response in the nextToken field when making another request to return
//!   the next batch of results.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — If the total number of
//!   results is greater than the maxResults value provided in the request, enter the token
//!   returned in the nextToken field in the response in this field to return the next batch
//!   of results.
//! * `sortBy` (SortJobsBy, one of: CreationTime) — The field to sort by in the returned list
//!   of model copy jobs.
//! * `sortOrder` (SortOrder, one of: Ascending | Descending) — Specifies whether to sort the
//!   results in ascending or descending order.
//! * `sourceAccountEquals` (AccountId, pattern ^[0-9]{12}$) — Filters for model copy jobs in
//!   which the account that the source model belongs to is equal to the value that you
//!   specify.
//! * `sourceModelArnEquals` (ModelArn, len 20..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) — Filters for model copy jobs in which the Amazon Resource Name (ARN) of the source
//!   model to is equal to the value that you specify.
//! * `statusEquals` (ModelCopyJobStatus, one of: Completed | Failed | InProgress) — Filters
//!   for model copy jobs whose status matches the value that you specify.
//! * `targetModelNameContains` (CustomModelName, len 1..63, pattern
//!   ^([0-9a-zA-Z][_-]?){1,63}$) — Filters for model copy jobs in which the name of the
//!   copied model contains the string that you specify.
//!
//! ## Output
//!
//! * `modelCopyJobSummaries` (ModelCopyJobSummaries) — A list of information about each model
//!   copy job.
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
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListModelCopyJobsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListModelCopyJobsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ListModelCopyJobs")]
#[derive(Debug)]
pub enum ListModelCopyJobsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_model_copy_jobs::ListModelCopyJobsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListModelCopyJobsError = crate::scaffolded::Scaffolded<ListModelCopyJobsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListModelCopyJobsErrorModeled> for ListModelCopyJobsError {
    fn from(modeled: ListModelCopyJobsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListModelCopyJobs` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListModelCopyJobs<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListModelCopyJobs<Eng> {
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

/// Whether every filter `input` carries holds for `job`.
///
/// The time bounds are STRICT, as the members word them: jobs "created after the specified
/// time" and "created before the specified time". `targetModelNameContains` is a substring
/// of the copied model's name, the way every `…Contains` filter of this service is.
fn matches(input: &ListModelCopyJobsInput, job: &crate::state::ModelCopyJobRecord) -> bool {
    let created = crate::state::epoch_seconds(job.created_at);
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
        .status_equals
        .as_deref()
        .is_some_and(|status| status != job.status)
    {
        return false;
    }
    if input
        .source_account_equals
        .as_deref()
        .is_some_and(|account| account != job.source_account_id)
    {
        return false;
    }
    if input
        .source_model_arn_equals
        .as_deref()
        .is_some_and(|arn| arn != job.source_model_arn)
    {
        return false;
    }
    if input
        .target_model_name_contains
        .as_deref()
        .is_some_and(|fragment| !job.target_model_name.contains(fragment))
    {
        return false;
    }
    true
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListModelCopyJobs<Eng>
{
    type Input = ListModelCopyJobsInput;
    type Output = ListModelCopyJobsOutput;
    type Error = ListModelCopyJobsError;

    const NAME: &'static str = "ListModelCopyJobs";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_MODEL_COPY_JOBS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_MODEL_COPY_JOBS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListModelCopyJobsErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "model copy job state is not available on this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // The status each job holds NOW is what a listing reports and what `statusEquals`
        // filters on, so the completions the clock has already made due are materialised
        // before either happens.
        store.settle_model_copy_jobs(now);

        let descending = input
            .sort_order
            .as_deref()
            .is_some_and(|order| order != SORT_ASCENDING);
        let mut rows: Vec<(String, ModelCopyJobSummary)> = store
            .model_copy_jobs
            .values()
            // The job's ARN carries the region the copy was submitted into, so a job of
            // another region is another account-region's resource and is not listed here.
            .filter(|job| job.region == region)
            .filter(|job| matches(&input, job))
            .map(|job| {
                (
                    crate::state::listing_cursor(job.created_at, &job.arn),
                    crate::state::model_copy_job_summary(job),
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
                return Err(ListModelCopyJobsErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(
                            "nextToken is not a pagination token this service issued".to_string(),
                        ),
                    },
                )
                .into());
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
        // The row the next page starts on IS the next page's cursor, so the pages partition
        // the listing with neither a gap nor a repeat.
        let next_token = rows.get(page_size).map(|(cursor, _)| cursor.clone());
        rows.truncate(page_size);

        Ok(ListModelCopyJobsOutput {
            // `ModelCopyJobSummaries` admits zero entries, so an account that has submitted
            // no copy this listing admits reports the empty list rather than omitting the
            // member.
            model_copy_job_summaries: Some(
                rows.into_iter().map(|(_, summary)| summary).collect(),
            ),
            next_token,
        })
    }
}
