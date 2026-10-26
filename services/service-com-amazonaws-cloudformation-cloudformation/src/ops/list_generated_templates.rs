// SPDX-License-Identifier: CC0-1.0

//! @op ListGeneratedTemplates
//!
//! Lists the generated templates of the Region the request was signed for, in creation
//! order, as the summary of each record [`crate::state::generated_template`] holds: a
//! template `CreateGeneratedTemplate` wrote is listed here carrying the same status,
//! timestamps and resource count `DescribeGeneratedTemplate` reports for it.
//!
//! A reporting operation: the only write is settling the generation clock, a transition
//! that was already due, so listing a template never advances its generation.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `ListGeneratedTemplates::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListGeneratedTemplates`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Lists your generated templates in this Region.
//!
//! ## Input
//!
//! * `MaxResults` (MaxResults, range 1..100) — If the number of available results exceeds
//!   this maximum, the response includes a NextToken value that you can use for the NextToken
//!   parameter to get the next set of results. By default the ListGeneratedTemplates API
//!   action will return at most 50 results in each response. The maximum value is 100.
//! * `NextToken` (NextToken, len 1..1024) — The token for the next set of items to return.
//!   (You received this token from a previous call.)
//!
//! ## Output
//!
//! * `NextToken` (NextToken, len 1..1024) — If the request doesn't return all the remaining
//!   results, NextToken is set to a token. To retrieve the next set of results, call
//!   ListGeneratedTemplates again and use that value for the NextToken parameter. If the
//!   request returns all results, NextToken is set to an empty string.
//! * `Summaries` (TemplateSummaries) — A list of summaries of the generated templates.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListGeneratedTemplatesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListGeneratedTemplatesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#ListGeneratedTemplates")]
#[derive(Debug)]
pub enum ListGeneratedTemplatesErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_generated_templates::ListGeneratedTemplatesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListGeneratedTemplatesError = crate::scaffolded::Scaffolded<ListGeneratedTemplatesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListGeneratedTemplatesErrorModeled> for ListGeneratedTemplatesError {
    fn from(modeled: ListGeneratedTemplatesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListGeneratedTemplates` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListGeneratedTemplates<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListGeneratedTemplates<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListGeneratedTemplates<Eng>
{
    type Input = ListGeneratedTemplatesInput;
    type Output = ListGeneratedTemplatesOutput;
    type Error = ListGeneratedTemplatesError;

    const NAME: &'static str = "ListGeneratedTemplates";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_GENERATED_TEMPLATES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_GENERATED_TEMPLATES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no generated-template store to
            // list, so this answers 501 rather than an empty, truthful-looking list.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "ListGeneratedTemplates",
            ));
        };
        let region = crate::state::request_region(ctx.parts());

        let mut store = state.lock();
        // Reporting operation: the only write is materialising a transition the clock had
        // already made due — each generation's own conclusion, which was owed at the same
        // instant whether or not anybody listed it.
        store.settle(::std::time::SystemTime::now());

        // "Lists your generated templates in this Region": every record this Region holds,
        // in creation order, so a page is a stable window over them.
        let summaries: Vec<TemplateSummary> =
            store.generated_templates_in(&region).map(summary).collect();

        // `@paginated(inputToken: NextToken, outputToken: NextToken, items: Summaries,
        // pageSize: MaxResults)`: one response carries at most the `MaxResults` the request
        // asks for, and the token resumes just past that page. A request that names none
        // gets the documented default,
        // [`crate::state::generated_template::DEFAULT_LISTED_TEMPLATES`].
        let max_results = input
            .max_results
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(crate::state::generated_template::DEFAULT_LISTED_TEMPLATES);
        let (summaries, next_token) = crate::state::paginate_at_most(
            crate::state::PageOf::TemplateSummaries,
            summaries,
            input.next_token.as_deref(),
            Some(max_results),
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;

        Ok(ListGeneratedTemplatesOutput {
            // Always present, as an empty element for a Region holding no generated
            // template: a client reading the list sees the same shape either way.
            summaries: Some(summaries),
            next_token,
        })
    }
}

/// One stored generated template as the `TemplateSummary` this list reports: the record
/// `DescribeGeneratedTemplate` reads, narrowed to the members of the summary.
fn summary(
    template: &crate::state::generated_template::StoredGeneratedTemplate,
) -> TemplateSummary {
    TemplateSummary {
        generated_template_id: Some(template.generated_template_id.clone()),
        generated_template_name: Some(template.generated_template_name.clone()),
        status: Some(template.status.clone()),
        status_reason: template.status_reason.clone(),
        creation_time: Some(crate::state::timestamp(template.creation_time)),
        last_updated_time: Some(crate::state::timestamp(template.last_updated_time)),
        // "The number of resources in the generated template. This is a total of resources
        // in pending, in-progress, completed, and failed states" — every resource the
        // record covers, whatever state its generation is in.
        number_of_resources: Some(i32::try_from(template.resources.len()).unwrap_or(i32::MAX)),
    }
}
