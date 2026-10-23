// SPDX-License-Identifier: CC0-1.0

//! @op GetPlan
//!
//! A pure read: projects the stored plan record back as the modelled `Plan`, including
//! the server-owned `owner`, `version` and `updatedAt`. An ARN no plan is stored under
//! answers the modelled `ResourceNotFoundException` (HTTP 404).
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach
//! the store through [`crate::state::service_state`] and read it through the guard it
//! hands back. `ctx` ([`smithy_server_runtime::OperationContext`]) carries request
//! metadata only; the engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetPlan`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Retrieves detailed information about a Region switch plan. You must specify the ARN of
//! the plan.
//!
//! ## Input
//!
//! * `arn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The Amazon Resource Name (ARN) of the plan.
//!
//! ## Output
//!
//! * `plan` (Plan) — The detailed information about the requested Region switch plan.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found. HTTP
//!   Status Code: 404

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetPlanError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetPlanError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// `InternalServerException` — a shape the SERVICE models, which this operation does not
// list — is widened in through `extra_errors`, so an unreachable shared store has a
// modelled server failure to answer with.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#GetPlan",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum GetPlanErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_plan::GetPlanError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetPlanError = crate::scaffolded::Scaffolded<GetPlanErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetPlanErrorModeled> for GetPlanError {
    fn from(modeled: GetPlanErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetPlan` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetPlan<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetPlan<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetPlan<Eng>
{
    type Input = GetPlanInput;
    type Output = GetPlanOutput;
    type Error = GetPlanError;

    const NAME: &'static str = "GetPlan";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PLAN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PLAN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetPlanErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Region switch plan store is unavailable".to_string(),
                },
            )
            .into());
        };

        // `@readonly`: the only write is `settle`, which materialises the state changes the
        // clock already made due — owed at the same instant whether or not anybody called.
        // Every handler settles, so no two readers disagree about the current state.
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        store.settle(now);
        let Some(record) = store.plans.get(&input.arn) else {
            return Err(GetPlanErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::plan_not_found_message(&input.arn),
                },
            )
            .into());
        };
        Ok(GetPlanOutput {
            plan: Some(record.as_plan()),
        })
    }
}
