// SPDX-License-Identifier: CC0-1.0

//! @op CancelFlowExecutions
//! Cancels the unfinished runs of a stored flow: each run the request can cancel moves
//! to `CancelStarted` and the clock then owes it `Canceled`, while every run the
//! request named and this flow could not cancel comes back in `invalidExecutions`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CancelFlowExecutions`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json")
//! ```
//!
//! Cancels active runs for a flow. You can cancel all of the active runs for a flow, or you
//! can cancel specific runs by providing their IDs. You can cancel a flow run only when the
//! run is in progress. You can't cancel a run that has already completed or failed. You
//! also can't cancel a run that's scheduled to occur but hasn't started yet. To prevent a
//! scheduled run, you can deactivate the flow with the StopFlow action. You cannot resume a
//! run after you cancel it. When you send your request, the status for each run becomes
//! CancelStarted. When the cancellation completes, the status becomes Canceled. When you
//! cancel a run, you still incur charges for any data that the run already processed before
//! the cancellation. If the run had already written some data to the flow destination, then
//! that data remains in the destination. If you configured the flow to use a batch API
//! (such as the Salesforce Bulk API 2.0), then the run will finish reading or writing its
//! entire batch of data after the cancellation. For these operations, the data processing
//! charges for Amazon AppFlow apply. For the pricing information, see Amazon AppFlow
//! pricing.
//!
//! ## Input
//!
//! * `executionIds` (ExecutionIds, len 0..100) — The ID of each active run to cancel. These
//!   runs must belong to the flow you specify in your request. If you omit this parameter,
//!   your request ends all active runs that belong to the flow.
//! * `flowName` (FlowName, len 0..256, pattern ^[a-zA-Z0-9][\w!@#.-]+$) **required** — The
//!   name of a flow with active runs that you want to cancel.
//!
//! ## Output
//!
//! * `invalidExecutions` (ExecutionIds, len 0..100) — The IDs of runs that Amazon AppFlow
//!   couldn't cancel. These runs might be ineligible for canceling because they haven't
//!   started yet or have already completed.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — AppFlow/Requester has invalid or missing
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal service error occurred during the
//!   processing of your request. Try again later.
//! * `ResourceNotFoundException` (HTTP 404) — The resource specified in the request (such as
//!   the source or destination connector profile) is not found.
//! * `ThrottlingException` (HTTP 429) — API calls have exceeded the maximum allowed API
//!   request rate per account and per Region.
//! * `ValidationException` (HTTP 400) — The request has invalid or missing parameters.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CancelFlowExecutionsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CancelFlowExecutionsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appflow#CancelFlowExecutions")]
#[derive(Debug)]
pub enum CancelFlowExecutionsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::cancel_flow_executions::CancelFlowExecutionsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CancelFlowExecutionsError = crate::scaffolded::Scaffolded<CancelFlowExecutionsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CancelFlowExecutionsErrorModeled> for CancelFlowExecutionsError {
    fn from(modeled: CancelFlowExecutionsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CancelFlowExecutions` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CancelFlowExecutions<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CancelFlowExecutions<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CancelFlowExecutions<Eng>
{
    type Input = CancelFlowExecutionsInput;
    type Output = CancelFlowExecutionsOutput;
    type Error = CancelFlowExecutionsError;

    const NAME: &'static str = "CancelFlowExecutions";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CANCEL_FLOW_EXECUTIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CANCEL_FLOW_EXECUTIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CancelFlowExecutionsErrorModeled::InternalServerException(InternalServerException {
                message: Some("the AppFlow emulator holds no state on this engine".to_string()),
            })
        })?;

        let now = ::std::time::SystemTime::now();

        let mut store = state.lock();
        let flow = store.flows.get_mut(&input.flow_name).ok_or_else(|| {
            CancelFlowExecutionsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(format!(
                    "No flow named {} exists in this account",
                    input.flow_name
                )),
            })
        })?;

        // Which runs are cancellable is judged against the statuses the clock has
        // already reached, so a run it finished before this call is not cancelled now.
        flow.settle_due_executions(now);

        let invalid_executions: Vec<String> = match input.execution_ids {
            // The runs a request names must belong to the flow it names, and a run that
            // has already finished cannot be cancelled: each such id is reported back
            // rather than failing the whole request.
            Some(execution_ids) => execution_ids
                .into_iter()
                .filter(|execution_id| !flow.begin_cancelling_execution(execution_id, now))
                .collect(),
            // A request that names no run ends every active run of the flow, so there is
            // no named run it could have failed to cancel.
            None => {
                flow.begin_cancelling_all_executions(now);
                Vec::new()
            }
        };

        Ok(CancelFlowExecutionsOutput {
            invalid_executions: Some(invalid_executions),
        })
    }
}
