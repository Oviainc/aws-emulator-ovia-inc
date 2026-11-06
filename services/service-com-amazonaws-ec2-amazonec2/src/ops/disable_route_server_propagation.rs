// SPDX-License-Identifier: CC0-1.0

//! @op DisableRouteServerPropagation
//!
//! Takes back the propagation one route server holds into one route table: the pair
//! `EnableRouteServerPropagation` recorded stops propagating, so
//! `GetRouteServerPropagations` reports it `deleting` while the disable is being carried out
//! and reports it no longer once the clock has carried it out.
//!
//! A WRITE. A pair that does not propagate has no disable to perform, so the request is
//! refused rather than answered with a propagation that was never there, and the pair is
//! resolved through the one rule this operation shares with the enable
//! ([`crate::state::require_route_server_propagation_pair`]).
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
//! # Modeled contract for `DisableRouteServerPropagation`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Disables route propagation from a route server to a specified route table. When enabled,
//! route server propagation installs the routes in the FIB on the route table you've
//! specified. Route server supports IPv4 and IPv6 route propagation. Amazon VPC Route
//! Server simplifies routing for traffic between workloads that are deployed within a VPC
//! and its internet gateways. With this feature, VPC Route Server dynamically updates VPC
//! and internet gateway route tables with your preferred IPv4 or IPv6 routes to achieve
//! routing fault tolerance for those workloads. This enables you to automatically reroute
//! traffic within a VPC, which increases the manageability of VPC routing and
//! interoperability with third-party workloads. Route server supports the follow route
//! table types: VPC route tables not associated with subnets Subnet route tables Internet
//! gateway route tables Route server does not support route tables associated with virtual
//! private gateways. To propagate routes into a transit gateway route table, use Transit
//! Gateway Connect. For more information see Dynamic routing in your VPC with VPC Route
//! Server in the Amazon VPC User Guide.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `RouteServerId` (RouteServerId) **required** — The ID of the route server for which to
//!   disable propagation.
//! * `RouteTableId` (RouteTableId) **required** — The ID of the route table for which to
//!   disable route server propagation.
//!
//! ## Output
//!
//! * `RouteServerPropagation` (RouteServerPropagation) — Information about the disabled route
//!   server propagation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DisableRouteServerPropagationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DisableRouteServerPropagationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DisableRouteServerPropagation")]
#[derive(Debug)]
pub enum DisableRouteServerPropagationErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::disable_route_server_propagation::DisableRouteServerPropagationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DisableRouteServerPropagationError = crate::scaffolded::Scaffolded<DisableRouteServerPropagationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DisableRouteServerPropagationErrorModeled> for DisableRouteServerPropagationError {
    fn from(modeled: DisableRouteServerPropagationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DisableRouteServerPropagation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DisableRouteServerPropagation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DisableRouteServerPropagation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DisableRouteServerPropagation<Eng>
{
    type Input = DisableRouteServerPropagationInput;
    type Output = DisableRouteServerPropagationOutput;
    type Error = DisableRouteServerPropagationError;

    const NAME: &'static str = "DisableRouteServerPropagation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DISABLE_ROUTE_SERVER_PROPAGATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DISABLE_ROUTE_SERVER_PROPAGATION_OUTPUT_SCHEMA;
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
        // `EnableRouteServerPropagation` share: both parameters present, both ids known,
        // and the route server associated with the VPC the named route table routes for.
        state::require_route_server_propagation_pair(
            &store,
            &input.route_server_id,
            &input.route_table_id,
        )?;

        let now = ::std::time::Instant::now();

        // A pair that does not propagate has no disable to perform: the request is refused
        // rather than answered with a propagation nothing recorded. A pair whose disable is
        // already being carried out reports `deleting`, which is a propagation this call
        // leaves exactly as it is rather than restarting its clock.
        if store
            .route_server_propagation_at(&input.route_server_id, &input.route_table_id, now)
            .is_none()
        {
            return Err(state::incorrect_state(format!(
                "The {resource} ID '{route_server_id}' does not propagate routes to \
                 routeTable ID '{route_table_id}'",
                resource = state::ROUTE_SERVER_TAGGABLE_RESOURCE,
                route_server_id = input.route_server_id,
                route_table_id = input.route_table_id,
            )));
        }

        // DryRun sits AFTER every id has been resolved and every refusal asked, and BEFORE
        // the write, the order every other route server operation of this crate answers a
        // dry run in.
        if input.dry_run == Some(true) {
            return Err(state::dry_run_operation());
        }

        // The pair moves into `deleting` now, with the clock owing it the end of the
        // disable one transition span later: the span is never zero, so a caller reading
        // straight after this call still sees the propagation being taken down rather than
        // a pair that silently vanished.
        let Some(propagation) = store.begin_route_server_propagation_removal(
            &input.route_server_id,
            &input.route_table_id,
            now,
        ) else {
            // The propagation was resolved a moment ago, so this cannot happen — and if it
            // ever did, the request must not answer 200 with nothing disabled.
            return Err(state::internal_error(
                "the route server propagation checked a moment ago could not be disabled",
            ));
        };
        Ok(DisableRouteServerPropagationOutput {
            route_server_propagation: Some(propagation),
        })
    }
}
