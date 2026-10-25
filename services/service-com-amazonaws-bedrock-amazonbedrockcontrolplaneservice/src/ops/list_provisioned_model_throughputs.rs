// SPDX-License-Identifier: CC0-1.0

//! @op ListProvisionedModelThroughputs
//!
//! Reports the Provisioned Throughputs of the signing region out of [`crate::state`]:
//! filtered, sorted, and paged.
//!
//! The rules, each read off the operation's own members
//! (`smithy_query(query="operation",operation="ListProvisionedModelThroughputs")`) rather
//! than off one request:
//!
//! * every filter the request carries must hold for a Provisioned Throughput to be listed
//!   — `statusEquals` on its status, `modelArnEquals` on the model it is associated with,
//!   `nameContains` on a substring of its name, and `creationTimeAfter` /
//!   `creationTimeBefore` on the instant it was created;
//! * `sortBy` admits only `CreationTime`, so the listing is ordered by that instant, in
//!   `sortOrder` (`Ascending` by default), with the ARN breaking a tie so the order is
//!   total and a page boundary is unambiguous;
//! * `maxResults` bounds the page and a further page is reported by `nextToken`, which is
//!   the cursor of the row the next page starts on — so the pages partition the listing
//!   with neither a gap nor a repeat even when a row is deleted in between.
//!
//! `@readonly`, and the one write it makes is the status transition the CLOCK already made
//! due for each record
//! ([`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::settle_provisioned_model_throughputs`]):
//! a listing reports — and `statusEquals` filters on — the status each Provisioned
//! Throughput holds now, which is the status a read of that one record would report at the
//! same instant whether or not anybody listed.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListProvisionedModelThroughputs`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Lists the Provisioned Throughputs in the account. For more information, see Provisioned
//! Throughput in the Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `creationTimeAfter` (Timestamp) — A filter that returns Provisioned Throughputs created
//!   after the specified time.
//! * `creationTimeBefore` (Timestamp) — A filter that returns Provisioned Throughputs created
//!   before the specified time.
//! * `maxResults` (MaxResults, range 1..1000) — THe maximum number of results to return in
//!   the response. If there are more results than the number you specified, the response
//!   returns a nextToken value. To see the next batch of results, send the nextToken value in
//!   another list request.
//! * `modelArnEquals` (ModelArn, len 20..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) — A filter that returns Provisioned Throughputs whose model Amazon Resource Name
//!   (ARN) is equal to the value that you specify.
//! * `nameContains` (ProvisionedModelName, len 1..63, pattern ^([0-9a-zA-Z][_-]?)+$) — A
//!   filter that returns Provisioned Throughputs if their name contains the expression that
//!   you specify.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — If there are more results
//!   than the number you specified in the maxResults field, the response returns a nextToken
//!   value. To see the next batch of results, specify the nextToken value in this field.
//! * `sortBy` (SortByProvisionedModels, one of: CreationTime) — The field by which to sort
//!   the returned list of Provisioned Throughputs.
//! * `sortOrder` (SortOrder, one of: Ascending | Descending) — The sort order of the results.
//! * `statusEquals` (ProvisionedModelStatus, one of: Creating | Failed | InService |
//!   Updating) — A filter that returns Provisioned Throughputs if their statuses matches the
//!   value that you specify.
//!
//! ## Output
//!
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — If there are more results
//!   than the number you specified in the maxResults field, this value is returned. To see
//!   the next batch of results, include this value in the nextToken field in another list
//!   request.
//! * `provisionedModelSummaries` (ProvisionedModelSummaries) — A list of summaries, one for
//!   each Provisioned Throughput in the response.
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
// `ListProvisionedModelThroughputsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListProvisionedModelThroughputsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ListProvisionedModelThroughputs")]
#[derive(Debug)]
pub enum ListProvisionedModelThroughputsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_provisioned_model_throughputs::ListProvisionedModelThroughputsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListProvisionedModelThroughputsError = crate::scaffolded::Scaffolded<ListProvisionedModelThroughputsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListProvisionedModelThroughputsErrorModeled> for ListProvisionedModelThroughputsError {
    fn from(modeled: ListProvisionedModelThroughputsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListProvisionedModelThroughputs` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListProvisionedModelThroughputs<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListProvisionedModelThroughputs<Eng> {
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
/// The time bounds are STRICT, as the members word them: "created after the specified
/// time" and "created before the specified time".
fn matches(
    input: &ListProvisionedModelThroughputsInput,
    record: &crate::state::ProvisionedModelThroughputRecord,
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
        .status_equals
        .as_deref()
        .is_some_and(|status| status != record.status)
    {
        return false;
    }
    if input
        .model_arn_equals
        .as_deref()
        .is_some_and(|model_arn| model_arn != record.model_arn)
    {
        return false;
    }
    if input
        .name_contains
        .as_deref()
        .is_some_and(|fragment| !record.name.contains(fragment))
    {
        return false;
    }
    true
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListProvisionedModelThroughputs<Eng>
{
    type Input = ListProvisionedModelThroughputsInput;
    type Output = ListProvisionedModelThroughputsOutput;
    type Error = ListProvisionedModelThroughputsError;

    const NAME: &'static str = "ListProvisionedModelThroughputs";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_PROVISIONED_MODEL_THROUGHPUTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_PROVISIONED_MODEL_THROUGHPUTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ListProvisionedModelThroughputsErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "provisioned throughput state is not available on this engine"
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

        // The status each Provisioned Throughput holds NOW is what a listing reports and
        // what `statusEquals` filters on, so the transitions the clock has already made
        // due are materialised before either happens.
        store.settle_provisioned_model_throughputs(now);

        let descending = input
            .sort_order
            .as_deref()
            .is_some_and(|order| order != SORT_ASCENDING);
        let mut rows: Vec<(String, ProvisionedModelSummary)> = store
            .provisioned_model_throughputs
            .values()
            // The ARN carries the region, so a Provisioned Throughput of another region is
            // another account-region's resource and is not listed here.
            .filter(|record| record.region == region)
            .filter(|record| matches(&input, record))
            .map(|record| {
                (
                    crate::state::listing_cursor(record.created_at, &record.arn),
                    crate::state::provisioned_model_summary(record),
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
                    ListProvisionedModelThroughputsErrorModeled::ValidationException(
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

        Ok(ListProvisionedModelThroughputsOutput {
            provisioned_model_summaries: Some(
                rows.into_iter().map(|(_, summary)| summary).collect(),
            ),
            next_token,
        })
    }
}
