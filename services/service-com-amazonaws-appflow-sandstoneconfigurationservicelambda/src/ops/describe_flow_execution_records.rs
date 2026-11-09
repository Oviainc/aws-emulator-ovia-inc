// SPDX-License-Identifier: CC0-1.0

//! @op DescribeFlowExecutionRecords
//! Pages the runs `StartFlow` recorded against a stored flow, most recent first.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeFlowExecutionRecords`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json")
//! ```
//!
//! Fetches the execution history of the flow.
//!
//! ## Input
//!
//! * `flowName` (FlowName, len 0..256, pattern ^[a-zA-Z0-9][\w!@#.-]+$) **required** — The
//!   specified name of the flow. Spaces are not allowed. Use underscores (_) or hyphens (-)
//!   only.
//! * `maxResults` (MaxResults, range 1..100) — Specifies the maximum number of items that
//!   should be returned in the result set. The default for maxResults is 20 (for all
//!   paginated API operations).
//! * `nextToken` (NextToken, len 0..2048, pattern ^\S+$) — The pagination token for the next
//!   page of data.
//!
//! ## Output
//!
//! * `flowExecutions` (FlowExecutionList) — Returns a list of all instances when this flow
//!   was run.
//! * `nextToken` (NextToken, len 0..2048, pattern ^\S+$) — The pagination token for the next
//!   page of data.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal service error occurred during the
//!   processing of your request. Try again later.
//! * `ResourceNotFoundException` (HTTP 404) — The resource specified in the request (such as
//!   the source or destination connector profile) is not found.
//! * `ValidationException` (HTTP 400) — The request has invalid or missing parameters.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeFlowExecutionRecordsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeFlowExecutionRecordsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appflow#DescribeFlowExecutionRecords")]
#[derive(Debug)]
pub enum DescribeFlowExecutionRecordsErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_flow_execution_records::DescribeFlowExecutionRecordsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeFlowExecutionRecordsError = crate::scaffolded::Scaffolded<DescribeFlowExecutionRecordsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeFlowExecutionRecordsErrorModeled> for DescribeFlowExecutionRecordsError {
    fn from(modeled: DescribeFlowExecutionRecordsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeFlowExecutionRecords` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeFlowExecutionRecords<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeFlowExecutionRecords<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeFlowExecutionRecords<Eng>
{
    type Input = DescribeFlowExecutionRecordsInput;
    type Output = DescribeFlowExecutionRecordsOutput;
    type Error = DescribeFlowExecutionRecordsError;

    const NAME: &'static str = "DescribeFlowExecutionRecords";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_FLOW_EXECUTION_RECORDS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_FLOW_EXECUTION_RECORDS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeFlowExecutionRecordsErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "the AppFlow emulator holds no state on this engine".to_string(),
                    ),
                },
            )
        })?;

        // The cursor names the run the next page starts at. A token this listing did
        // not mint is refused rather than silently served from the beginning.
        let start_at = match input.next_token.as_deref() {
            Some(token) => Some(
                crate::state::decode_cursor(crate::state::EXECUTION_CURSOR, token).ok_or_else(
                    || {
                        DescribeFlowExecutionRecordsErrorModeled::ValidationException(
                            ValidationException {
                                message: Some(
                                    "nextToken is not a cursor this operation issued".to_string(),
                                ),
                            },
                        )
                    },
                )?,
            ),
            None => None,
        };
        let page_size = crate::state::page_size(input.max_results);

        let mut store = state.lock();
        let flow = store.flows.get_mut(&input.flow_name).ok_or_else(|| {
            DescribeFlowExecutionRecordsErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::no_such_flow(&input.flow_name)),
                },
            )
        })?;

        // Reporting, not mutating: materialise only the run transitions the clock had
        // already made due, so each record carries the state at this instant.
        flow.settle_due_executions(::std::time::SystemTime::now());

        let ordered = flow.executions_newest_first();
        let first = match start_at.as_deref() {
            Some(start) => {
                // The run the cursor resumes at may have aged out of the flow's bounded
                // history (`MAX_EXECUTIONS_PER_FLOW`) since the cursor was issued. The
                // cursor is then stale rather than meaningless, and saying so is better
                // than silently resuming somewhere else: a caller that restarts the
                // listing gets a consistent page.
                let Some(position) = ordered
                    .iter()
                    .position(|execution| execution.execution_id == start)
                else {
                    return Err(DescribeFlowExecutionRecordsErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(
                                "nextToken names a flow run this flow no longer has".to_string(),
                            ),
                        },
                    )
                    .into());
                };
                position
            }
            None => 0,
        };

        let page_end = ordered.len().min(first + page_size);
        let flow_executions: Vec<ExecutionRecord> = ordered[first..page_end]
            .iter()
            .map(|execution| execution.to_execution_record())
            .collect();
        // A cursor is issued only when a run is actually left over, so the last page
        // ends the listing instead of handing back an empty page.
        let next_token = ordered.get(page_end).map(|execution| {
            crate::state::encode_cursor(crate::state::EXECUTION_CURSOR, &execution.execution_id)
        });

        Ok(DescribeFlowExecutionRecordsOutput {
            flow_executions: Some(flow_executions),
            next_token,
        })
    }
}
