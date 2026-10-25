// SPDX-License-Identifier: CC0-1.0

//! @op ListCustomModels
//!
//! Reports the custom models of the signing region out of [`crate::state`]: filtered,
//! sorted, and paged.
//!
//! The rules, each read off the operation's own members
//! (`smithy_query(query="operation",operation="ListCustomModels")`) rather than off one
//! request:
//!
//! * every filter the request carries must hold for a custom model to be listed —
//!   `nameContains` on a substring of its name, `baseModelArnEquals` on the model it was
//!   customized from, `foundationModelArnEquals` on the foundation model it descends from,
//!   `modelStatus` on its status, `isOwned` on whether this account owns it, and
//!   `creationTimeAfter` / `creationTimeBefore` on the instant it was created;
//! * `sortBy` admits only `CreationTime`, so the listing is ordered by that instant, in
//!   `sortOrder` (`Ascending` by default), with the ARN breaking a tie so the order is
//!   total and a page boundary is unambiguous;
//! * `maxResults` bounds the page and a further page is reported by `nextToken`, which is
//!   the cursor of the row the next page starts on — so the pages partition the listing
//!   with neither a gap nor a repeat even when a row is deleted in between.
//!
//! `@readonly`, and the one write it makes is the status transition the CLOCK already made
//! due for each record
//! ([`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::settle_custom_models`]):
//! a listing reports — and `modelStatus` filters on — the status each custom model holds
//! now, which is the status a read of that one record would report at the same instant
//! whether or not anybody listed.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListCustomModels`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Returns a list of the custom models that you have created with the
//! CreateModelCustomizationJob operation. For more information, see Custom models in the
//! Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `baseModelArnEquals` (ModelArn, len 20..1011, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/((imported)|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}))((
//!   …) — Return custom models only if the base model Amazon Resource Name (ARN) matches this
//!   parameter.
//! * `creationTimeAfter` (Timestamp) — Return custom models created after the specified time.
//! * `creationTimeBefore` (Timestamp) — Return custom models created before the specified
//!   time.
//! * `foundationModelArnEquals` (FoundationModelArn, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}::foundation-model/[a-z0-9-]{1,63}[.]{1}([a-z0-9-]{1,63}[.]){0,2}[a-z0-9-]{1,63
//!   …) — Return custom models only if the foundation model Amazon Resource Name (ARN)
//!   matches this parameter.
//! * `isOwned` (Boolean) — Return custom models depending on if the current account owns them
//!   (true) or if they were shared with the current account (false).
//! * `maxResults` (MaxResults, range 1..1000) — The maximum number of results to return in
//!   the response. If the total number of results is greater than this value, use the token
//!   returned in the response in the nextToken field when making another request to return
//!   the next batch of results.
//! * `modelStatus` (ModelStatus, one of: Active | Creating | Failed) — The status of them
//!   model to filter results by. Possible values include: Creating - Include only models that
//!   are currently being created and validated. Active - Include only models that have been
//!   successfully created and are ready for use. Failed - Include only models where the
//!   creation process failed. If you don't specify a status, the API returns models in all
//!   states.
//! * `nameContains` (CustomModelName, len 1..63, pattern ^([0-9a-zA-Z][_-]?){1,63}$) — Return
//!   custom models only if the job name contains these characters.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — If the total number of
//!   results is greater than the maxResults value provided in the request, enter the token
//!   returned in the nextToken field in the response in this field to return the next batch
//!   of results.
//! * `sortBy` (SortModelsBy, one of: CreationTime) — The field to sort by in the returned
//!   list of models.
//! * `sortOrder` (SortOrder, one of: Ascending | Descending) — The sort order of the results.
//!
//! ## Output
//!
//! * `modelSummaries` (CustomModelSummaryList) — Model summaries.
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
// `ListCustomModelsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListCustomModelsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ListCustomModels")]
#[derive(Debug)]
pub enum ListCustomModelsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_custom_models::ListCustomModelsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListCustomModelsError = crate::scaffolded::Scaffolded<ListCustomModelsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListCustomModelsErrorModeled> for ListCustomModelsError {
    fn from(modeled: ListCustomModelsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListCustomModels` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListCustomModels<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListCustomModels<Eng> {
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
fn matches(input: &ListCustomModelsInput, record: &crate::state::CustomModelRecord) -> bool {
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
        .base_model_arn_equals
        .as_deref()
        .is_some_and(|base| base != record.base_model_arn)
    {
        return false;
    }
    // The foundation model a custom model descends from, which is NOT its base model: a
    // model customized from another custom model has both, and a model whose artifacts
    // were brought in has no foundation model at all, so this filter never admits one.
    if input.foundation_model_arn_equals.as_deref().is_some_and(|wanted| {
        record
            .foundation_model_arn
            .as_deref()
            .is_none_or(|owned| owned != wanted)
    }) {
        return false;
    }
    if input
        .model_status
        .as_deref()
        .is_some_and(|status| status != record.status)
    {
        return false;
    }
    // "Return custom models depending on if the current account owns them (true) or if
    // they were shared with the current account (false)": every model this store holds was
    // created by the one account the emulator presents, so it owns all of them and a
    // request for the shared ones matches none.
    if input.is_owned == Some(false) {
        return false;
    }
    true
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListCustomModels<Eng>
{
    type Input = ListCustomModelsInput;
    type Output = ListCustomModelsOutput;
    type Error = ListCustomModelsError;

    const NAME: &'static str = "ListCustomModels";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_CUSTOM_MODELS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_CUSTOM_MODELS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListCustomModelsErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("custom model state is not available on this engine".to_string()),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // The status each custom model holds NOW is what a listing reports and what
        // `modelStatus` filters on, so the transitions the clock has already made due are
        // materialised before either happens.
        store.settle_custom_models(now);

        let descending = input
            .sort_order
            .as_deref()
            .is_some_and(|order| order != SORT_ASCENDING);
        let mut rows: Vec<(String, CustomModelSummary)> = store
            .custom_models
            .values()
            // The ARN carries the region, so a custom model of another region is another
            // account-region's resource and is not listed here.
            .filter(|record| record.region == region)
            .filter(|record| matches(&input, record))
            .map(|record| {
                (
                    crate::state::listing_cursor(record.created_at, &record.arn),
                    crate::state::custom_model_summary(record),
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
                return Err(ListCustomModelsErrorModeled::ValidationException(
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
        // The row the next page starts on IS the next page's cursor, so the pages
        // partition the listing with neither a gap nor a repeat.
        let next_token = rows.get(page_size).map(|(cursor, _)| cursor.clone());
        rows.truncate(page_size);

        Ok(ListCustomModelsOutput {
            model_summaries: Some(rows.into_iter().map(|(_, summary)| summary).collect()),
            next_token,
        })
    }
}
