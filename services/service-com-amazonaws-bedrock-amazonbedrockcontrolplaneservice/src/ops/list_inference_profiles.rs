// SPDX-License-Identifier: CC0-1.0

//! @op ListInferenceProfiles
//!
//! Lists the inference profiles of the region the request addresses: the `SYSTEM_DEFINED`
//! ones Amazon Bedrock publishes there ([`crate::state::system_defined_inference_profiles`])
//! and the `APPLICATION` ones `ops/create_inference_profile.rs` persisted in
//! [`crate::state`], as one `InferenceProfileSummary` each — the stored ones carrying
//! exactly what `ops/get_inference_profile.rs` reports for that same record. The listing is
//! ordered by creation instant and paginated with the cursor every creation-time listing of
//! this service pages with ([`crate::state::listing_cursor`]); a published profile was
//! created by no call, so it is ordered ahead of them, by the name Amazon Bedrock publishes
//! it under.
//!
//! `typeEquals` "filters for inference profiles that match the type you specify", so a
//! request that names none asks for every profile of the region, of either type. A profile
//! a caller created is `APPLICATION` and a published one is `SYSTEM_DEFINED`, so each value
//! the filter admits selects one of those two sets.
//!
//! A pure READ: it writes nothing, because a created profile is `ACTIVE` from the call that
//! minted it and is owed no later transition, and a published one is not the store's at all.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListInferenceProfiles`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Returns a list of inference profiles that you can use. For more information, see
//! Increase throughput and resilience with cross-region inference in Amazon Bedrock. in the
//! Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `maxResults` (MaxResults, range 1..1000) — The maximum number of results to return in
//!   the response. If the total number of results is greater than this value, use the token
//!   returned in the response in the nextToken field when making another request to return
//!   the next batch of results.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — If the total number of
//!   results is greater than the maxResults value provided in the request, enter the token
//!   returned in the nextToken field in the response in this field to return the next batch
//!   of results.
//! * `typeEquals` (InferenceProfileType, one of: APPLICATION | SYSTEM_DEFINED) — Filters for
//!   inference profiles that match the type you specify. SYSTEM_DEFINED – The inference
//!   profile is defined by Amazon Bedrock. You can route inference requests across regions
//!   with these inference profiles. APPLICATION – The inference profile was created by a
//!   user. This type of inference profile can track metrics and costs when invoking the model
//!   in it. The inference profile may route requests to one or multiple regions.
//!
//! ## Output
//!
//! * `inferenceProfileSummaries` (InferenceProfileSummaries) — A list of information about
//!   each inference profile that you can use.
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
// `ListInferenceProfilesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListInferenceProfilesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ListInferenceProfiles")]
#[derive(Debug)]
pub enum ListInferenceProfilesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_inference_profiles::ListInferenceProfilesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListInferenceProfilesError = crate::scaffolded::Scaffolded<ListInferenceProfilesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListInferenceProfilesErrorModeled> for ListInferenceProfilesError {
    fn from(modeled: ListInferenceProfilesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListInferenceProfiles` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListInferenceProfiles<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListInferenceProfiles<Eng> {
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

// A listed profile is reported through the ONE mapping in
// `crate::state::inference_profile_summary`, which `GetInferenceProfile` reports a profile
// through too, so a listing and a read never disagree about a member.

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListInferenceProfiles<Eng>
{
    type Input = ListInferenceProfilesInput;
    type Output = ListInferenceProfilesOutput;
    type Error = ListInferenceProfilesError;

    const NAME: &'static str = "ListInferenceProfiles";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_INFERENCE_PROFILES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_INFERENCE_PROFILES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListInferenceProfilesErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "inference profile state is not available on this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let store = state.lock();

        // Every profile this listing can report, each with the cursor it pages at.
        //
        // The system-defined profiles Amazon Bedrock publishes in this region come first —
        // they were not created by any call, so their cursor's instant half is zero and
        // they are ordered among themselves by the NAME Amazon Bedrock publishes them
        // under, which is the order it reports them in — and the account's own profiles
        // follow in the order they were created.
        let wanted_type = input.type_equals.as_deref();
        let system_defined = crate::state::INFERENCE_PROFILE_TYPE_SYSTEM_DEFINED;
        let mut rows: Vec<(String, InferenceProfileSummary)> = Vec::new();
        if wanted_type.is_none_or(|wanted| wanted == system_defined) {
            rows.extend(
                crate::state::system_defined_inference_profiles(&region)
                    .into_iter()
                    .enumerate()
                    .map(|(position, published)| {
                        (
                            crate::state::listing_cursor(
                                ::std::time::SystemTime::UNIX_EPOCH,
                                &crate::state::published_row_cursor_key(position),
                            ),
                            published,
                        )
                    }),
            );
        }
        // A profile's ARN carries the region its create addressed, so a profile of another
        // region is another account-region's resource and is not listed here.
        rows.extend(
            store
                .inference_profiles
                .values()
                .filter(|profile| profile.region == region)
                .filter(|profile| wanted_type.is_none_or(|wanted| wanted == profile.profile_type))
                .map(|profile| {
                    (
                        crate::state::listing_cursor(profile.created_at, &profile.arn),
                        crate::state::inference_profile_summary(profile),
                    )
                }),
        );
        // The cursor's leading half is the creation instant and its trailing half breaks a
        // tie by ARN, so byte order over cursors is a TOTAL order on the listing.
        rows.sort_by(|(left, _), (right, _)| left.cmp(right));

        if let Some(token) = input.next_token.as_deref() {
            if !crate::state::cursor_belongs(token) {
                return Err(ListInferenceProfilesErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(
                            "nextToken is not a pagination token this service issued".to_string(),
                        ),
                    },
                )
                .into());
            }
            rows.retain(|(cursor, _)| cursor.as_str() >= token);
        }

        let page_size = input
            .max_results
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(MAX_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        // The row the next page starts on IS the next page's cursor, so the pages partition
        // the listing with neither a gap nor a repeat even when a profile has been deleted
        // between two pages.
        let next_token = rows.get(page_size).map(|(cursor, _)| cursor.clone());
        rows.truncate(page_size);

        Ok(ListInferenceProfilesOutput {
            // `InferenceProfileSummaries` admits zero entries, so a region holding no
            // profile of this type reports the empty list rather than omitting the member.
            inference_profile_summaries: Some(
                rows.into_iter().map(|(_, summary)| summary).collect(),
            ),
            next_token,
        })
    }
}
