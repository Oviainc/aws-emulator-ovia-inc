// SPDX-License-Identifier: CC0-1.0

//! @op ListCustomModelDeployments
//!
//! Reports this account's custom model deployments in the region the request addressed,
//! out of [`crate::state`]: the deployments `ops/create_custom_model_deployment.rs` minted,
//! each as the summary that agrees with what `ops/get_custom_model_deployment.rs` reports
//! for that one deployment ([`crate::state::custom_model_deployment_summary`]).
//!
//! The rules it follows, each out of the model:
//!
//! * ORDER. `sortBy` admits only `CreationTime`, and `sortOrder` "Default is Descending",
//!   so the listing is in creation order, newest first unless the request asks for
//!   `Ascending`. The cursor's trailing half breaks a tie by ARN, so the order is total.
//! * FILTERS. Every filter the request carries narrows the listing: `createdAfter` /
//!   `createdBefore` (strict, as the members word them), `statusEquals`, `modelArnEquals`
//!   and `nameContains`.
//! * PAGES. `maxResults` bounds one page and `nextToken` resumes the next, as the cursor
//!   of the first row the page did not carry — the one
//!   [`crate::state::listing_cursor`] every creation-time listing of this service pages
//!   with — so the pages partition the listing with neither a gap nor a repeat. A token
//!   that cursor does not render ([`crate::state::cursor_belongs`]) is refused 400
//!   `ValidationException`.
//!
//! A REPORTING operation (`@readonly`): the one write it makes is
//! [`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::settle_custom_model_deployments`],
//! which materialises the status transitions the clock had already made due at the same
//! instant even if nobody had called — so the listing reports, and filters on, the status a
//! read of each deployment would report.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListCustomModelDeployments`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Lists custom model deployments in your account. You can filter the results by creation
//! time, name, status, and associated model. Use this operation to manage and monitor your
//! custom model deployments. We recommend using pagination to ensure that the operation
//! returns quickly and successfully. The following actions are related to the
//! ListCustomModelDeployments operation: CreateCustomModelDeployment
//! GetCustomModelDeployment DeleteCustomModelDeployment
//!
//! ## Input
//!
//! * `createdAfter` (Timestamp) — Filters deployments created after the specified date and
//!   time.
//! * `createdBefore` (Timestamp) — Filters deployments created before the specified date and
//!   time.
//! * `maxResults` (MaxResults, range 1..1000) — The maximum number of results to return in a
//!   single call.
//! * `modelArnEquals` (CustomModelArn, len 20..1011, pattern
//!   ^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model/(imported|[a-z0-9-]{1,63}[.]{1}[a-z0-9
//!   …) — Filters deployments by the Amazon Resource Name (ARN) of the associated custom
//!   model.
//! * `nameContains` (ModelDeploymentName, len 1..63, pattern ^([0-9a-zA-Z][_-]?){1,63}$) —
//!   Filters deployments whose names contain the specified string.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — The token for the next set
//!   of results. Use this token to retrieve additional results when the response is
//!   truncated.
//! * `sortBy` (SortModelsBy, one of: CreationTime) — The field to sort the results by. The
//!   only supported value is CreationTime.
//! * `sortOrder` (SortOrder, one of: Ascending | Descending) — The sort order for the
//!   results. Valid values are Ascending and Descending. Default is Descending.
//! * `statusEquals` (CustomModelDeploymentStatus, one of: Active | Creating | Failed) —
//!   Filters deployments by status. Valid values are CREATING, ACTIVE, and FAILED.
//!
//! ## Output
//!
//! * `modelDeploymentSummaries` (CustomModelDeploymentSummaryList) — A list of custom model
//!   deployment summaries.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — The token for the next set
//!   of results. This value is null when there are no more results to return.
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
// `ListCustomModelDeploymentsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListCustomModelDeploymentsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ListCustomModelDeployments")]
#[derive(Debug)]
pub enum ListCustomModelDeploymentsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_custom_model_deployments::ListCustomModelDeploymentsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListCustomModelDeploymentsError = crate::scaffolded::Scaffolded<ListCustomModelDeploymentsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListCustomModelDeploymentsErrorModeled> for ListCustomModelDeploymentsError {
    fn from(modeled: ListCustomModelDeploymentsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListCustomModelDeployments` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListCustomModelDeployments<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListCustomModelDeployments<Eng> {
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

/// `SortOrder` — oldest first. The other value, and the one an absent `sortOrder` takes,
/// is `Descending`: "Default is Descending"
/// (`smithy_query(query="operation",operation="ListCustomModelDeployments")`).
const SORT_ASCENDING: &str = "Ascending";

/// Whether every filter `input` carries holds for `record`.
///
/// The time bounds are STRICT, as the members word them: "created after the specified date
/// and time" and "created before the specified date and time".
fn matches(
    input: &ListCustomModelDeploymentsInput,
    record: &crate::state::CustomModelDeploymentRecord,
) -> bool {
    let created = crate::state::epoch_seconds(record.created_at);
    if let Some(after) = input.created_after.as_ref() {
        if created <= after.as_secs_f64() {
            return false;
        }
    }
    if let Some(before) = input.created_before.as_ref() {
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
    for ListCustomModelDeployments<Eng>
{
    type Input = ListCustomModelDeploymentsInput;
    type Output = ListCustomModelDeploymentsOutput;
    type Error = ListCustomModelDeploymentsError;

    const NAME: &'static str = "ListCustomModelDeployments";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_CUSTOM_MODEL_DEPLOYMENTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_CUSTOM_MODEL_DEPLOYMENTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ListCustomModelDeploymentsErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "custom model deployment state is not available on this engine"
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

        // The status each deployment holds NOW is what a listing reports and what
        // `statusEquals` filters on, so the transitions the clock has already made due are
        // materialised before either happens.
        store.settle_custom_model_deployments(now);

        // `sortOrder` defaults to `Descending` for this listing, so ASCENDING is the order
        // a request has to ask for.
        let descending = input
            .sort_order
            .as_deref()
            .is_none_or(|order| order != SORT_ASCENDING);
        let mut rows: Vec<(String, CustomModelDeploymentSummary)> = store
            .custom_model_deployments
            .values()
            // The ARN carries the region, so a deployment of another region is another
            // account-region's resource and is not listed here.
            .filter(|record| record.region == region)
            .filter(|record| matches(&input, record))
            .map(|record| {
                (
                    crate::state::listing_cursor(record.created_at, &record.arn),
                    crate::state::custom_model_deployment_summary(record),
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
                    ListCustomModelDeploymentsErrorModeled::ValidationException(
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

        Ok(ListCustomModelDeploymentsOutput {
            model_deployment_summaries: Some(
                rows.into_iter().map(|(_, summary)| summary).collect(),
            ),
            next_token,
        })
    }
}
