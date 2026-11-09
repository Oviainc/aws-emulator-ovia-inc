// SPDX-License-Identifier: CC0-1.0

//! @op DeleteFlow
//! Removes a flow from this service's store, so every later read of it answers
//! `ResourceNotFoundException` and `ListFlows` stops listing it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteFlow`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json")
//! ```
//!
//! Enables your application to delete an existing flow. Before deleting the flow, Amazon
//! AppFlow validates the request by checking the flow configuration and status. You can
//! delete flows one at a time.
//!
//! ## Input
//!
//! * `flowName` (FlowName, len 0..256, pattern ^[a-zA-Z0-9][\w!@#.-]+$) **required** — The
//!   specified name of the flow. Spaces are not allowed. Use underscores (_) or hyphens (-)
//!   only.
//! * `forceDelete` (Boolean, default false) **required** — Indicates whether Amazon AppFlow
//!   should delete the flow, even if it is currently in use.
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

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteFlowError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteFlowError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appflow#DeleteFlow")]
#[derive(Debug)]
pub enum DeleteFlowErrorModeled {
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_flow::DeleteFlowError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteFlowError = crate::scaffolded::Scaffolded<DeleteFlowErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteFlowErrorModeled> for DeleteFlowError {
    fn from(modeled: DeleteFlowErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteFlow` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteFlow<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteFlow<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteFlow<Eng>
{
    type Input = DeleteFlowInput;
    type Output = DeleteFlowOutput;
    type Error = DeleteFlowError;

    const NAME: &'static str = "DeleteFlow";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_FLOW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_FLOW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteFlowErrorModeled::InternalServerException(InternalServerException {
                message: Some("the AppFlow emulator holds no state on this engine".to_string()),
            })
        })?;

        let mut store = state.lock();
        let flow = store.flows.get_mut(&input.flow_name).ok_or_else(|| {
            DeleteFlowErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::no_such_flow(&input.flow_name)),
            })
        })?;
        flow.settle_due_executions(::std::time::SystemTime::now());

        // `forceDelete` is the caller's explicit opt-in to destroying a flow that is IN
        // USE — "whether Amazon AppFlow should delete the flow, even if it is currently in
        // use" (`smithy_query(query="doc", shape="DeleteFlowRequest",
        // member="forceDelete")`). A flow is in use while its trigger is armed, which is
        // the `Active` status, and while a run of it has not finished; without the opt-in
        // such a flow is left exactly as it was, and the caller is told the two ways
        // forward.
        if !input.force_delete
            && (flow.flow_status == crate::state::FlowStatus::Active
                || flow.has_running_execution())
        {
            return Err(DeleteFlowErrorModeled::ConflictException(ConflictException {
                message: Some(crate::state::flow_is_active(&flow.flow_name)),
            })
            .into());
        }

        store.flows.remove(&input.flow_name);
        Ok(DeleteFlowOutput {})
    }
}
