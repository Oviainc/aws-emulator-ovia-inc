// SPDX-License-Identifier: CC0-1.0

//! @op ListMarketplaceModelEndpoints
//!
//! Lists the Bedrock Marketplace model endpoints of the region the request addresses, out
//! of what `ops/create_marketplace_model_endpoint.rs` persisted in [`crate::state`]: one
//! `MarketplaceModelEndpointSummary` per stored endpoint, in ARN order, paginated. A read
//! — apart from the `endpointStatus` transition the CLOCK has already made due, which it
//! materialises the way `ops/get_marketplace_model_endpoint.rs` does, so a listing and a
//! read never disagree about an endpoint's status.
//!
//! * An endpoint's ARN carries the region its create addressed
//!   ([`crate::state::marketplace_model_endpoint_arn`]), so the ARN prefix of this region
//!   is what scopes the listing.
//! * `modelSourceEquals` keeps only the endpoints deployed from that model source, as the
//!   member documents ("only endpoints for the given model source identifier are
//!   returned").
//! * The cursor IS the ARN the next page starts on, so the pages partition the listing with
//!   neither a gap nor a repeat; `maxResults` is modelled `range 1..1000`, so 1000 is both
//!   the largest page and the page an absent `maxResults` takes.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListMarketplaceModelEndpoints`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Lists the endpoints for models from Amazon Bedrock Marketplace in your Amazon Web
//! Services account.
//!
//! ## Input
//!
//! * `maxResults` (MaxResults, range 1..1000) — The maximum number of results to return in a
//!   single call. If more results are available, the operation returns a NextToken value.
//! * `modelSourceEquals` (ModelSourceIdentifier, len ..2048, pattern
//!   arn:aws:sagemaker:.*:hub-content/SageMakerPublicHub/Model/.*) — If specified, only
//!   endpoints for the given model source identifier are returned.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — The token for the next set
//!   of results. You receive this token from a previous ListMarketplaceModelEndpoints call.
//!
//! ## Output
//!
//! * `marketplaceModelEndpoints` (MarketplaceModelEndpointSummaries, len ..1000) — An array
//!   of endpoint summaries.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — The token for the next set
//!   of results. Use this token to get the next set of results.
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
// `ListMarketplaceModelEndpointsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListMarketplaceModelEndpointsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ListMarketplaceModelEndpoints")]
#[derive(Debug)]
pub enum ListMarketplaceModelEndpointsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_marketplace_model_endpoints::ListMarketplaceModelEndpointsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListMarketplaceModelEndpointsError = crate::scaffolded::Scaffolded<ListMarketplaceModelEndpointsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListMarketplaceModelEndpointsErrorModeled> for ListMarketplaceModelEndpointsError {
    fn from(modeled: ListMarketplaceModelEndpointsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListMarketplaceModelEndpoints` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListMarketplaceModelEndpoints<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListMarketplaceModelEndpoints<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The largest page `ListMarketplaceModelEndpoints` answers with, and the page size an
/// absent `maxResults` takes: `maxResults` is modelled `range 1..1000`, so 1000 is the
/// whole span it admits, and `MarketplaceModelEndpointSummaries` is `@length(max: 1000)`,
/// so no larger page could be reported anyway.
const MAX_PAGE_SIZE: usize = 1000;

/// One listed endpoint as the `MarketplaceModelEndpointSummary` the response carries: the
/// summary's members are the `MarketplaceModelEndpoint` members minus the configuration and
/// the endpoint-level status, so they come from the same stored record.
fn summary(
    endpoint: &crate::state::MarketplaceModelEndpointRecord,
) -> MarketplaceModelEndpointSummary {
    MarketplaceModelEndpointSummary {
        endpoint_arn: endpoint.arn.clone(),
        model_source_identifier: endpoint.model_source_identifier.clone(),
        status: Some(endpoint.status.to_string()),
        status_message: None,
        created_at: crate::state::date_time(endpoint.created_at),
        updated_at: crate::state::date_time(endpoint.updated_at),
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListMarketplaceModelEndpoints<Eng>
{
    type Input = ListMarketplaceModelEndpointsInput;
    type Output = ListMarketplaceModelEndpointsOutput;
    type Error = ListMarketplaceModelEndpointsError;

    const NAME: &'static str = "ListMarketplaceModelEndpoints";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_MARKETPLACE_MODEL_ENDPOINTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_MARKETPLACE_MODEL_ENDPOINTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ListMarketplaceModelEndpointsErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "marketplace model endpoint state is not available on this engine"
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

        // The transition the clock has already made due, materialised for every endpoint
        // before any of them is reported, so a summary carries the same status a read of
        // that endpoint would.
        for endpoint in store.marketplace_model_endpoints.values_mut() {
            endpoint.settle(now);
        }

        // The endpoints of THIS region, of the model source the request asked for, in ARN
        // order — which is the cursor order too.
        let region_prefix = crate::state::marketplace_model_endpoint_arn(&region, "");
        let mut rows: Vec<&crate::state::MarketplaceModelEndpointRecord> = store
            .marketplace_model_endpoints
            .values()
            .filter(|endpoint| {
                endpoint.arn.starts_with(&region_prefix)
                    && input
                        .model_source_equals
                        .as_ref()
                        .is_none_or(|source| &endpoint.model_source_identifier == source)
            })
            .collect();
        rows.sort_unstable_by(|left, right| left.arn.cmp(&right.arn));

        // The cursor IS the ARN the next page starts on, so the pages partition the
        // listing with neither a gap nor a repeat even when an endpoint has been deleted
        // between two pages.
        if let Some(token) = input.next_token.as_deref() {
            if !token.contains(":endpoint/") {
                return Err(
                    ListMarketplaceModelEndpointsErrorModeled::ValidationException(
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
            rows.retain(|endpoint| endpoint.arn.as_str() >= token);
        }

        let page_size = input
            .max_results
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(MAX_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let next_token = rows.get(page_size).map(|endpoint| endpoint.arn.clone());
        rows.truncate(page_size);

        Ok(ListMarketplaceModelEndpointsOutput {
            // `MarketplaceModelEndpointSummaries` admits zero entries, so an account with
            // no endpoint in this region reports the empty list rather than omitting the
            // member.
            marketplace_model_endpoints: Some(rows.into_iter().map(summary).collect()),
            next_token,
        })
    }
}
