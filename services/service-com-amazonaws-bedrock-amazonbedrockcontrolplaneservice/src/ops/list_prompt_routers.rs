// SPDX-License-Identifier: CC0-1.0

//! @op ListPromptRouters
//!
//! Lists the prompt routers of the region the request addresses, out of what
//! `ops/create_prompt_router.rs` persisted in [`crate::state`]: one `PromptRouterSummary`
//! per stored router whose `PromptRouterType` is the one the request asked for, in ARN
//! order, paginated. A read, and only a read.
//!
//! `type` is modelled with `@default("default")`, so a request that names none asks for
//! the routers Amazon Bedrock itself publishes — this emulator publishes none, and a
//! router a caller created is `custom`, so the default listing is empty until Amazon's own
//! catalog is modelled.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListPromptRouters`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Retrieves a list of prompt routers.
//!
//! ## Input
//!
//! * `maxResults` (MaxResults, range 1..1000) — The maximum number of prompt routers to
//!   return in one page of results.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — Specify the pagination token
//!   from a previous request to retrieve the next page of results.
//! * `type` (PromptRouterType, one of: custom | default, default "default") **required** —
//!   The type of the prompt routers, such as whether it's default or custom.
//!
//! ## Output
//!
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — Specify the pagination token
//!   from a previous request to retrieve the next page of results.
//! * `promptRouterSummaries` (PromptRouterSummaries) — A list of prompt router summaries.
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
// `ListPromptRoutersError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListPromptRoutersError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ListPromptRouters")]
#[derive(Debug)]
pub enum ListPromptRoutersErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_prompt_routers::ListPromptRoutersError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListPromptRoutersError = crate::scaffolded::Scaffolded<ListPromptRoutersErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListPromptRoutersErrorModeled> for ListPromptRoutersError {
    fn from(modeled: ListPromptRoutersErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListPromptRouters` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListPromptRouters<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListPromptRouters<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The largest page `ListPromptRouters` answers with, and the page size an absent
/// `maxResults` takes: `maxResults` is modelled `range 1..1000`, so 1000 is the whole
/// span it admits.
const MAX_PAGE_SIZE: usize = 1000;

/// One listed router as the `PromptRouterSummary` the response carries.
fn summary(router: &crate::state::PromptRouter) -> PromptRouterSummary {
    PromptRouterSummary {
        prompt_router_name: router.name.clone(),
        routing_criteria: router.routing_criteria.clone(),
        description: router.description.clone(),
        created_at: Some(crate::state::date_time(router.created_at)),
        updated_at: Some(crate::state::date_time(router.updated_at)),
        prompt_router_arn: router.arn.clone(),
        models: router.models.clone(),
        fallback_model: router.fallback_model.clone(),
        status: router.status.to_string(),
        r#type: router.router_type.to_string(),
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListPromptRouters<Eng>
{
    type Input = ListPromptRoutersInput;
    type Output = ListPromptRoutersOutput;
    type Error = ListPromptRoutersError;

    const NAME: &'static str = "ListPromptRouters";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_PROMPT_ROUTERS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_PROMPT_ROUTERS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListPromptRoutersErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "prompt router state is not available on this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let store = state.lock();

        // The routers of THIS region, of the type the request asked for, in ARN order —
        // which is the cursor order too. A router's ARN carries the region the create
        // addressed, so the ARN prefix of this region is what scopes the listing.
        let region_prefix = crate::state::prompt_router_arn(&region, "");
        let mut rows: Vec<&crate::state::PromptRouter> = store
            .prompt_routers
            .values()
            .filter(|router| {
                router.arn.starts_with(&region_prefix) && router.router_type == input.r#type
            })
            .collect();
        rows.sort_unstable_by(|left, right| left.arn.cmp(&right.arn));

        // The cursor IS the ARN the next page starts on, so the pages partition the
        // listing with neither a gap nor a repeat even when a router has been deleted
        // between two pages.
        if let Some(token) = input.next_token.as_deref() {
            if !token.contains(":default-prompt-router/") {
                return Err(ListPromptRoutersErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(
                            "nextToken is not a pagination token this service issued".to_string(),
                        ),
                    },
                )
                .into());
            }
            rows.retain(|router| router.arn.as_str() >= token);
        }

        let page_size = input
            .max_results
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(MAX_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let next_token = rows.get(page_size).map(|router| router.arn.clone());
        rows.truncate(page_size);

        Ok(ListPromptRoutersOutput {
            // `PromptRouterSummaries` admits zero entries, so an account with no router
            // of this type reports the empty list rather than omitting the member.
            prompt_router_summaries: Some(rows.into_iter().map(summary).collect()),
            next_token,
        })
    }
}
