// SPDX-License-Identifier: CC0-1.0

//! @op GetWorkflowExecutionHistory
//!
//! A pure read: it replays the persisted event log of the ONE stored run addressed by
//! `(domain, workflowId, runId)`, honouring `maximumPageSize` and `reverseOrder` and
//! round-tripping `nextPageToken`, and writes nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetWorkflowExecutionHistory`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-swf-simpleworkflowservice/smithy-model/com-amazonaws-swf-simpleworkflowservice.json")
//! ```
//!
//! Returns the history of the specified workflow execution. The results may be split into
//! multiple pages. To retrieve subsequent pages, make the call again using the
//! nextPageToken returned by the initial call. This operation is eventually consistent. The
//! results are best effort and may not exactly reflect recent updates and changes. Access
//! Control You can use IAM policies to control this action's access to Amazon SWF resources
//! as follows: Use a Resource element with the domain name to limit the action to only
//! specified domains. Use an Action element to allow or deny permission to call this
//! action. You cannot use an IAM policy to constrain this action's parameters. If the
//! caller doesn't have sufficient permissions to invoke the action, or the parameter values
//! fall outside the specified constraints, the action fails. The associated event
//! attribute's cause parameter is set to OPERATION_NOT_PERMITTED. For details and example
//! IAM policies, see Using IAM to Manage Access to Amazon SWF Workflows in the Amazon SWF
//! Developer Guide.
//!
//! ## Input
//!
//! * `domain` (DomainName, len 1..256) **required** — The name of the domain containing the
//!   workflow execution.
//! * `execution` (WorkflowExecution) **required** — Specifies the workflow execution for
//!   which to return the history.
//! * `maximumPageSize` (PageSize, range 0..1000, default 0) **required** — The maximum number
//!   of results that are returned per call. Use nextPageToken to obtain further pages of
//!   results.
//! * `nextPageToken` (PageToken, len 0..2048) — If NextPageToken is returned there are more
//!   results available. The value of NextPageToken is a unique pagination token for each
//!   page. Make the call again using the returned token to retrieve the next page. Keep all
//!   other arguments unchanged. Each pagination token expires after 24 hours. Using an
//!   expired pagination token will return a 400 error: "Specified token has exceeded its
//!   maximum lifetime". The configured maximumPageSize determines how many results can be
//!   returned in a single call.
//! * `reverseOrder` (ReverseOrder, default false) **required** — When set to true, returns
//!   the events in reverse order. By default the results are returned in ascending order of
//!   the eventTimeStamp of the events.
//!
//! ## Output
//!
//! * `events` (HistoryEventList) **required** — The list of history events.
//! * `nextPageToken` (PageToken, len 0..2048) — If a NextPageToken was returned by a previous
//!   call, there are more results available. To retrieve the next page of results, make the
//!   call again using the returned token in nextPageToken. Keep all other arguments
//!   unchanged. The configured maximumPageSize determines how many results can be returned in
//!   a single call.
//!
//! ## Errors
//!
//! * `OperationNotPermittedFault` (HTTP 400) — Returned when the caller doesn't have
//!   sufficient permissions to invoke the action.
//! * `UnknownResourceFault` (HTTP 400) — Returned when the named resource cannot be found
//!   with in the scope of this operation (region or domain). This could happen if the named
//!   resource was never created or is no longer available for this operation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetWorkflowExecutionHistoryError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-swf-simpleworkflowservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetWorkflowExecutionHistoryError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.swf#GetWorkflowExecutionHistory")]
#[derive(Debug)]
pub enum GetWorkflowExecutionHistoryErrorModeled {
    OperationNotPermittedFault(OperationNotPermittedFault),
    UnknownResourceFault(UnknownResourceFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_workflow_execution_history::GetWorkflowExecutionHistoryError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetWorkflowExecutionHistoryError = crate::scaffolded::Scaffolded<GetWorkflowExecutionHistoryErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetWorkflowExecutionHistoryErrorModeled> for GetWorkflowExecutionHistoryError {
    fn from(modeled: GetWorkflowExecutionHistoryErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetWorkflowExecutionHistory` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetWorkflowExecutionHistory<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetWorkflowExecutionHistory<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetWorkflowExecutionHistory<Eng>
{
    type Input = GetWorkflowExecutionHistoryInput;
    type Output = GetWorkflowExecutionHistoryOutput;
    type Error = GetWorkflowExecutionHistoryError;

    const NAME: &'static str = "GetWorkflowExecutionHistory";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_WORKFLOW_EXECUTION_HISTORY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_WORKFLOW_EXECUTION_HISTORY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetWorkflowExecutionHistoryErrorModeled::OperationNotPermittedFault(
                OperationNotPermittedFault {
                    message: Some("the SWF domain store is unavailable".to_string()),
                },
            )
        })?;

        let guard = state.lock();
        // An `UnknownResourceFault` message names the resource the lookup failed on and
        // nothing else: the domain when no domain is stored under that name, otherwise
        // the requested execution, in the
        // `WorkflowExecution=[workflowId=.., runId=..]` form every execution-scoped
        // operation of this service uses. The domain the lookup was scoped to is never
        // appended to that clause.
        let unknown_execution = || {
            GetWorkflowExecutionHistoryErrorModeled::UnknownResourceFault(UnknownResourceFault {
                message: Some(crate::state::unknown_execution_message(
                    &input.execution.workflow_id,
                    &input.execution.run_id,
                )),
            })
        };
        let domain = guard.domains.get(&input.domain).ok_or_else(|| {
            GetWorkflowExecutionHistoryErrorModeled::UnknownResourceFault(UnknownResourceFault {
                message: Some(format!("Unknown domain: {}", input.domain)),
            })
        })?;
        let execution = domain
            .executions
            .get(&(
                input.execution.workflow_id.clone(),
                input.execution.run_id.clone(),
            ))
            .ok_or_else(unknown_execution)?;

        // `maximumPageSize` defaults to 0 ("not specified"), which means the whole
        // history up to the modelled page maximum. The modelled `PageSize` range
        // (0..1000) is enforced by the boundary validator before this handler runs, so
        // a value above the maximum never reaches here; the clamp is the internal
        // safety bound that keeps one response bounded regardless.
        let page_size = match usize::try_from(input.maximum_page_size) {
            Ok(size) if size > 0 => size.min(crate::state::MAX_HISTORY_PAGE_SIZE),
            _ => crate::state::MAX_HISTORY_PAGE_SIZE,
        };

        let resume_after = match crate::state::non_empty(&input.next_page_token) {
            Some(token) => Some(
                crate::state::decode_history_page_token(&token, input.reverse_order)
                    .map_err(crate::scaffolded::Scaffolded::validation)?,
            ),
            None => None,
        };

        // `reverseOrder` reverses the stored (oldest-first) log, so paging walks the
        // same events in the opposite direction — never a different set.
        let ordered: Vec<&crate::state::HistoryEventRecord> = if input.reverse_order {
            execution.history.iter().rev().collect()
        } else {
            execution.history.iter().collect()
        };
        let total = ordered.len();
        // The token names the event the previous page ended on, so the next page
        // starts at the one after it in THIS ordering. The position is searched for
        // rather than computed from the id, so the cursor stays correct for any log
        // the store can hold — including one whose ids are not a dense 1..n range.
        let start = match resume_after {
            Some(event_id) => ordered
                .iter()
                .position(|record| record.event_id == event_id)
                .map(|position| position + 1)
                .ok_or_else(|| {
                    crate::scaffolded::Scaffolded::validation(format!(
                        "nextPageToken resumes after event {event_id}, which this execution's \
                         history does not hold"
                    ))
                })?,
            None => 0,
        };
        let end = (start + page_size).min(total);
        let page = &ordered[start..end];
        let events: Vec<HistoryEvent> = page.iter().map(|record| record.event()).collect();

        Ok(GetWorkflowExecutionHistoryOutput {
            events,
            // The token names the last event of THIS page, so `last()` is what the
            // cursor is built from — no index arithmetic to underflow.
            next_page_token: page.last().filter(|_| end < total).map(|last| {
                crate::state::encode_history_page_token(input.reverse_order, last.event_id)
            }),
        })
    }
}
