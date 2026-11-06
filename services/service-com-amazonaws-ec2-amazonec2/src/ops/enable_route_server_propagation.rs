// SPDX-License-Identifier: CC0-1.0

//! @op EnableRouteServerPropagation
//!
//! Defines which route table one route server may install its learned routes into: the
//! (route server, route table) pair this call records, which `GetRouteServerPropagations`
//! then reports and `DisableRouteServerPropagation` takes back.
//!
//! A WRITE. "When enabled, route server propagation installs the routes in the FIB on the
//! route table you've specified", and the route server reaches a route table through the
//! association it holds with that table's VPC — so a route server that is not associated
//! with the VPC the named table routes for has no path to install anything through, and the
//! request is refused rather than recorded. A pair that already propagates has no enable to
//! perform, so a repeat is refused and NOTHING is written, which is how the first call's
//! write stays observable to a later caller. The propagation is recorded in the non-terminal
//! `pending` state with the clock owing it `available`
//! ([`crate::state::RouteServerPropagationRecord`]).
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store
//! is reached through [`crate::state::require_service_state`], the way every handler beside
//! this one reaches it.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `EnableRouteServerPropagation`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Defines which route tables the route server can update with routes. When enabled, route
//! server propagation installs the routes in the FIB on the route table you've specified.
//! Route server supports IPv4 and IPv6 route propagation. For more information see Dynamic
//! routing in your VPC with VPC Route Server in the Amazon VPC User Guide.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `RouteServerId` (RouteServerId) **required** — The ID of the route server for which to
//!   enable propagation.
//! * `RouteTableId` (RouteTableId) **required** — The ID of the route table to which route
//!   server will propagate routes.
//!
//! ## Output
//!
//! * `RouteServerPropagation` (RouteServerPropagation) — Information about the enabled route
//!   server propagation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `EnableRouteServerPropagationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `EnableRouteServerPropagationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#EnableRouteServerPropagation")]
#[derive(Debug)]
pub enum EnableRouteServerPropagationErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::enable_route_server_propagation::EnableRouteServerPropagationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type EnableRouteServerPropagationError = crate::scaffolded::Scaffolded<EnableRouteServerPropagationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<EnableRouteServerPropagationErrorModeled> for EnableRouteServerPropagationError {
    fn from(modeled: EnableRouteServerPropagationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `EnableRouteServerPropagation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct EnableRouteServerPropagation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> EnableRouteServerPropagation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for EnableRouteServerPropagation<Eng>
{
    type Input = EnableRouteServerPropagationInput;
    type Output = EnableRouteServerPropagationOutput;
    type Error = EnableRouteServerPropagationError;

    const NAME: &'static str = "EnableRouteServerPropagation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ENABLE_ROUTE_SERVER_PROPAGATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ENABLE_ROUTE_SERVER_PROPAGATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state;

        // `ctx` carries request metadata only; every fact this propagation reports comes
        // from the request and from the store the engine on `self` holds.
        let _ = ctx;

        let service_state = state::require_service_state(&*self.engine)?;
        let mut store = service_state.lock();

        // The pair the request names, resolved through the one rule this operation and
        // `DisableRouteServerPropagation` share: both parameters present, both ids known,
        // and the route server associated with the VPC the named route table routes for.
        state::require_route_server_propagation_pair(
            &store,
            &input.route_server_id,
            &input.route_table_id,
        )?;

        let now = ::std::time::Instant::now();

        // A pair that already propagates has no enable to perform: the request is refused
        // and nothing is written, so the first call's write stays observable to this one.
        // The state it is already in is read off the record, so this refusal cannot name a
        // state no reader would report.
        if let Some(existing) =
            store.route_server_propagation_at(&input.route_server_id, &input.route_table_id, now)
        {
            return Err(state::incorrect_state(format!(
                "The {resource} ID '{route_server_id}' already propagates routes to \
                 routeTable ID '{route_table_id}' in state '{state}'",
                resource = state::ROUTE_SERVER_TAGGABLE_RESOURCE,
                route_server_id = input.route_server_id,
                route_table_id = input.route_table_id,
                state = existing.state.unwrap_or_default(),
            )));
        }

        if !store.has_route_server_propagation_capacity() {
            return Err(state::route_server_limit_exceeded(format!(
                "This emulator holds {max} route server propagations, which is as many as \
                 it stores",
                max = state::MAX_ROUTE_SERVER_RECORDS,
            )));
        }

        // DryRun sits AFTER every id has been resolved and every refusal asked, and BEFORE
        // the write: the flag asks whether the request WOULD have succeeded, so one that
        // would have been refused is refused with that reason instead, and one that would
        // have succeeded leaves state untouched.
        if input.dry_run == Some(true) {
            return Err(state::dry_run_operation());
        }

        // The propagation is written in `pending` with the clock owing it `available` one
        // transition span later, so a caller reading straight after this call still sees
        // `pending`. The answer is the record as it was stored, so this operation and
        // `GetRouteServerPropagations` cannot report one propagation differently.
        let propagation = store.insert_route_server_propagation(
            &input.route_server_id,
            &input.route_table_id,
            now,
        );
        Ok(EnableRouteServerPropagationOutput {
            route_server_propagation: Some(propagation),
        })
    }
}
