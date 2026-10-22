// SPDX-License-Identifier: CC0-1.0

//! @op ListFlows
//! Pages the stored flows, newest state and all, as `FlowDefinition` summaries built
//! from each stored flow — including the connector types its stored source and
//! destination configurations name.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListFlows`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json")
//! ```
//!
//! Lists all of the flows associated with your account.
//!
//! ## Input
//!
//! * `maxResults` (MaxResults, range 1..100) — Specifies the maximum number of items that
//!   should be returned in the result set.
//! * `nextToken` (NextToken, len 0..2048, pattern ^\S+$) — The pagination token for next page
//!   of data.
//!
//! ## Output
//!
//! * `flows` (FlowList) — The list of flows associated with your account.
//! * `nextToken` (NextToken, len 0..2048, pattern ^\S+$) — The pagination token for next page
//!   of data.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal service error occurred during the
//!   processing of your request. Try again later.
//! * `ValidationException` (HTTP 400) — The request has invalid or missing parameters.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListFlowsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListFlowsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appflow#ListFlows")]
#[derive(Debug)]
pub enum ListFlowsErrorModeled {
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_flows::ListFlowsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListFlowsError = crate::scaffolded::Scaffolded<ListFlowsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListFlowsErrorModeled> for ListFlowsError {
    fn from(modeled: ListFlowsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListFlows` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListFlows<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListFlows<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListFlows<Eng>
{
    type Input = ListFlowsInput;
    type Output = ListFlowsOutput;
    type Error = ListFlowsError;

    const NAME: &'static str = "ListFlows";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_FLOWS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_FLOWS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListFlowsErrorModeled::InternalServerException(InternalServerException {
                message: Some("the AppFlow emulator holds no state on this engine".to_string()),
            })
        })?;

        // The cursor names the flow the next page starts at. A token this listing did
        // not mint is refused rather than silently served from the beginning.
        let start_at = match input.next_token.as_deref() {
            Some(token) => Some(
                crate::state::decode_cursor(crate::state::FLOW_CURSOR, token).ok_or_else(|| {
                    ListFlowsErrorModeled::ValidationException(ValidationException {
                        message: Some("nextToken is not a cursor this operation issued".to_string()),
                    })
                })?,
            ),
            None => None,
        };
        let page_size = crate::state::page_size(input.max_results);

        let mut store = state.lock();

        // Name the page's flows FIRST, so the work this request does is proportional to
        // the page it answers with, not to the whole store: `range` SEEKS to the cursor
        // rather than walking the keys before it. The extra key past the page is the
        // cursor.
        //
        // A cursor whose own flow has since been DELETED still resumes correctly here,
        // because the key orders the listing rather than identifying a row: `range`
        // resumes at the first remaining name at or after it. That is the rule for a
        // keyed listing, and it differs from `DescribeFlowExecutionRecords`, whose runs
        // are ordered by time rather than by their id, so a cursor naming a run the
        // bounded history no longer holds has no position to resume from and is refused.
        let page_keys: Vec<String> = match start_at.as_deref() {
            // The bound is taken by reference (`String: Borrow<str>`), so seeking to the
            // cursor allocates nothing. Spelled as a `Bound` pair because that is the
            // form that implements `RangeBounds<str>` for an unsized key.
            Some(start) => store
                .flows
                .range::<str, _>((
                    ::std::ops::Bound::Included(start),
                    ::std::ops::Bound::Unbounded,
                ))
                .map(|(flow_name, _)| flow_name.clone())
                .take(page_size + 1)
                .collect(),
            None => store
                .flows
                .keys()
                .take(page_size + 1)
                .cloned()
                .collect(),
        };
        // A cursor is issued only when a flow is actually left over, so the last page
        // ends the listing instead of handing back an empty page.
        let next_token = page_keys
            .get(page_size)
            .map(|flow_name| crate::state::encode_cursor(crate::state::FLOW_CURSOR, flow_name));
        let page_keys = &page_keys[..page_keys.len().min(page_size)];

        // Reporting, not mutating: materialise only the run transitions the clock had
        // already made due, and only for the flows this page reports.
        let now = ::std::time::SystemTime::now();
        for flow_name in page_keys {
            if let Some(flow) = store.flows.get_mut(flow_name) {
                flow.settle_due_executions(now);
            }
        }

        let flows: Vec<FlowDefinition> = page_keys
            .iter()
            .filter_map(|flow_name| store.flows.get(flow_name))
            .map(crate::state::Flow::to_flow_definition)
            .collect();

        Ok(ListFlowsOutput {
            flows: Some(flows),
            next_token,
        })
    }
}
