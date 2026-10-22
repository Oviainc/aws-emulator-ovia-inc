// SPDX-License-Identifier: CC0-1.0

//! @op ListPlans
//!
//! A pure read: projects every stored plan as an `AbbreviatedPlan`, in ARN order, and
//! pages with an opaque `nextToken` naming the ARN the next page resumes from. A created
//! plan appears here and a deleted one is gone, because the page comes from the store.
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
//! # Modeled contract for `ListPlans`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Lists all Region switch plans in your Amazon Web Services account.
//!
//! ## Input
//!
//! * `maxResults` (MaxResults, range 1..100) — The number of objects that you want to return
//!   with this call.
//! * `nextToken` (NextToken, len 1..2048) — Specifies that you want to receive the next page
//!   of results. Valid only if you received a nextToken response in the previous request. If
//!   you did, it indicates that more output is available. Set this parameter to the value
//!   provided by the previous call's nextToken response to request the next page of results.
//!
//! ## Output
//!
//! * `nextToken` (NextToken, len 1..2048) — Specifies that you want to receive the next page
//!   of results. Valid only if you received a nextToken response in the previous request. If
//!   you did, it indicates that more output is available. Set this parameter to the value
//!   provided by the previous call's nextToken response to request the next page of results.
//! * `plans` (PlanList) — The plans that were requested.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListPlansError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListPlansError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// The operation's MODELED errors. `ListPlans` declares none, so
// `InternalServerException` — a shape the SERVICE models — is widened in through
// `extra_errors`, giving this handler a modelled server failure to answer with if the
// shared store is unreachable.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#ListPlans",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum ListPlansErrorModeled {
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_plans::ListPlansError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListPlansError = crate::scaffolded::Scaffolded<ListPlansErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListPlansErrorModeled> for ListPlansError {
    fn from(modeled: ListPlansErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListPlans` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListPlans<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListPlans<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListPlans<Eng>
{
    type Input = ListPlansInput;
    type Output = ListPlansOutput;
    type Error = ListPlansError;

    const NAME: &'static str = "ListPlans";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_PLANS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_PLANS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListPlansErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Region switch plan store is unavailable".to_string(),
                },
            )
            .into());
        };

        // `maxResults` is constrained `@range(1..100)`; an absent one asks for as many as
        // that range allows.
        let page_size = crate::state::page_size(input.max_results);

        // The token is the ARN the next page resumes from, so paging stays correct even
        // when a plan is created or deleted between pages.
        let from = match input.next_token {
            Some(token) => ::std::ops::Bound::Included(token),
            None => ::std::ops::Bound::Unbounded,
        };

        // `@readonly`: the only write is `settle`, which materialises the state changes the
        // clock already made due — owed at the same instant whether or not anybody called.
        // Every handler settles, so no two readers disagree about the current state.
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        store.settle(now);
        let mut plans = Vec::with_capacity(page_size);
        let mut next_token = None;
        for (arn, record) in store.plans.range((from, ::std::ops::Bound::Unbounded)) {
            if plans.len() == page_size {
                next_token = Some(arn.clone());
                break;
            }
            plans.push(record.as_abbreviated_plan());
        }

        Ok(ListPlansOutput {
            plans: Some(plans),
            next_token,
        })
    }
}
