// SPDX-License-Identifier: CC0-1.0

//! @op StopFlow
//! Deactivates a stored flow: disarms the trigger of a flow that runs ITSELF and answers
//! with the status that leaves the flow in, which `DescribeFlow` then reports. The stored
//! DEFINITION is untouched; only the flow's status moves.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `StopFlow`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json")
//! ```
//!
//! Deactivates the existing flow. For on-demand flows, this operation returns an
//! unsupportedOperationException error message. For schedule and event-triggered flows,
//! this operation deactivates the flow.
//!
//! ## Input
//!
//! * `flowName` (FlowName, len 0..256, pattern ^[a-zA-Z0-9][\w!@#.-]+$) **required** — The
//!   specified name of the flow. Spaces are not allowed. Use underscores (_) or hyphens (-)
//!   only.
//!
//! ## Output
//!
//! * `flowArn` (FlowArn, len 0..512, pattern ^arn:aws:appflow:.*:[0-9]+:) — The flow's Amazon
//!   Resource Name (ARN).
//! * `flowStatus` (FlowStatus, one of: Active | Deleted | Deprecated | Draft | Errored |
//!   Suspended) — Indicates the current status of the flow.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — There was a conflict when processing the request (for
//!   example, a flow with the given name already exists within the account. Check for
//!   conflicting resource names and try again.
//! * `InternalServerException` (HTTP 500) — An internal service error occurred during the
//!   processing of your request. Try again later.
//! * `ResourceNotFoundException` (HTTP 404) — The resource specified in the request (such as
//!   the source or destination connector profile) is not found.
//! * `UnsupportedOperationException` (HTTP 400) — The requested operation is not supported
//!   for the current flow.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StopFlowError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StopFlowError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appflow#StopFlow")]
#[derive(Debug)]
pub enum StopFlowErrorModeled {
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    UnsupportedOperationException(UnsupportedOperationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::stop_flow::StopFlowError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StopFlowError = crate::scaffolded::Scaffolded<StopFlowErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StopFlowErrorModeled> for StopFlowError {
    fn from(modeled: StopFlowErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StopFlow` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StopFlow<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StopFlow<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StopFlow<Eng>
{
    type Input = StopFlowInput;
    type Output = StopFlowOutput;
    type Error = StopFlowError;

    const NAME: &'static str = "StopFlow";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::STOP_FLOW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::STOP_FLOW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StopFlowErrorModeled::InternalServerException(InternalServerException {
                message: Some("the AppFlow emulator holds no state on this engine".to_string()),
            })
        })?;

        let mut store = state.lock();
        let flow = store.flows.get_mut(&input.flow_name).ok_or_else(|| {
            StopFlowErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::no_such_flow(&input.flow_name)),
            })
        })?;

        // Deactivating a flow disarms the trigger that runs it. A flow with no armed
        // trigger left to disarm — one that runs only on demand, which has none at all,
        // and one whose trigger is already disarmed — is refused as the state conflict it
        // is, and its status is left exactly where it was.
        if !flow.deactivate_trigger() {
            return Err(StopFlowErrorModeled::ConflictException(ConflictException {
                message: Some(crate::state::flow_already_deactivated(&flow.flow_name)),
            })
            .into());
        }

        Ok(StopFlowOutput {
            flow_arn: Some(flow.flow_arn.clone()),
            flow_status: Some(flow.flow_status.as_str().to_string()),
        })
    }
}
