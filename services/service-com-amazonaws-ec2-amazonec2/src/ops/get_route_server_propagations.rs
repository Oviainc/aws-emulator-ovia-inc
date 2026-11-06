// SPDX-License-Identifier: CC0-1.0

//! @op GetRouteServerPropagations
//!
//! Reports which route tables one route server may install its learned routes into — the
//! propagations `EnableRouteServerPropagation` defined — in `RouteTableId` order, narrowed
//! to one route table when the request names one.
//!
//! A READ: the clock-owed `pending` -> `available` move, and the end of a `deleting` a
//! disable started, are DERIVED from each propagation's due instants by
//! [`crate::state::RouteServerPropagationRecord::state_at`], so getting a route server's
//! propagations changes nothing a later caller sees. An id that resolves to nothing — a
//! route server the account does not hold, or a route table it does not hold — is refused
//! rather than answered with an empty set, so "no propagation" and "no such resource" stay
//! distinguishable.
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
//! # Modeled contract for `GetRouteServerPropagations`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Gets information about the route propagations for the specified route server. When
//! enabled, route server propagation installs the routes in the FIB on the route table
//! you've specified. Route server supports IPv4 and IPv6 route propagation. Amazon VPC
//! Route Server simplifies routing for traffic between workloads that are deployed within a
//! VPC and its internet gateways. With this feature, VPC Route Server dynamically updates
//! VPC and internet gateway route tables with your preferred IPv4 or IPv6 routes to achieve
//! routing fault tolerance for those workloads. This enables you to automatically reroute
//! traffic within a VPC, which increases the manageability of VPC routing and
//! interoperability with third-party workloads. Route server supports the follow route
//! table types: VPC route tables not associated with subnets Subnet route tables Internet
//! gateway route tables Route server does not support route tables associated with virtual
//! private gateways. To propagate routes into a transit gateway route table, use Transit
//! Gateway Connect.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `RouteServerId` (RouteServerId) **required** — The ID of the route server for which to
//!   get propagation information.
//! * `RouteTableId` (RouteTableId) — The ID of the route table for which to get propagation
//!   information.
//!
//! ## Output
//!
//! * `RouteServerPropagations` (RouteServerPropagationsList) — Information about the route
//!   propagations for the specified route server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetRouteServerPropagationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetRouteServerPropagationsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#GetRouteServerPropagations")]
#[derive(Debug)]
pub enum GetRouteServerPropagationsErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_route_server_propagations::GetRouteServerPropagationsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetRouteServerPropagationsError = crate::scaffolded::Scaffolded<GetRouteServerPropagationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetRouteServerPropagationsErrorModeled> for GetRouteServerPropagationsError {
    fn from(modeled: GetRouteServerPropagationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetRouteServerPropagations` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetRouteServerPropagations<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetRouteServerPropagations<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetRouteServerPropagations<Eng>
{
    type Input = GetRouteServerPropagationsInput;
    type Output = GetRouteServerPropagationsOutput;
    type Error = GetRouteServerPropagationsError;

    const NAME: &'static str = "GetRouteServerPropagations";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ROUTE_SERVER_PROPAGATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ROUTE_SERVER_PROPAGATIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state;

        // `ctx` carries request metadata only; everything this operation reports comes from
        // the request and from the store the engine on `self` holds.
        let _ = ctx;

        // "The ID of the route server for which to get propagation information"
        // (`RouteServerId`) is `@required` but modelled `clientOptional`, so a request that
        // left it out reaches this handler as an empty string rather than being refused by
        // the boundary validator. A parameter the request did not carry is EC2's
        // `MissingParameter`, not an id to look up.
        if input.route_server_id.is_empty() {
            return Err(state::missing_parameter(
                "The request must contain the parameter routeServerId",
            ));
        }

        let service_state = state::require_service_state(&*self.engine)?;
        let store = service_state.lock();

        // An id that names no route server of the account is an ERROR, not an empty
        // propagation set: a caller has to be able to tell "this route server propagates
        // into no route table" from "there is no such route server". Resolved through the
        // crate's one helper for that lookup, so a malformed id and an unknown one are told
        // apart here exactly as every other route server operation tells them apart.
        state::require_known_route_server(&store, &input.route_server_id)?;

        // "The ID of the route table for which to get propagation information"
        // (`RouteTableId`) is optional: a request that names one asks about that table
        // alone, and one that names none asks about every table this route server
        // propagates into. A table the account does not hold is refused with the code every
        // operation of this service refuses an unknown route table id with, rather than
        // narrowing the answer to nothing.
        let route_table_id = input
            .route_table_id
            .as_deref()
            .filter(|route_table_id| !route_table_id.is_empty());
        if let Some(route_table_id) = route_table_id {
            if !store.route_tables.contains_key(route_table_id) {
                return Err(state::invalid_route_table_id_not_found(route_table_id));
            }
        }

        // DryRun sits AFTER every id the request named has been resolved, the order every
        // other route server operation of this crate answers a dry run in: the flag asks
        // whether the request WOULD have worked, so one that would have been refused is
        // refused with that reason instead.
        if input.dry_run == Some(true) {
            return Err(state::dry_run_operation());
        }

        // Reporting only: each propagation's `pending` -> `available` move, and the end of a
        // `deleting` a disable started, are OBSERVED here from the stored due instants and
        // never written, so this read changes nothing a later caller sees.
        let now = ::std::time::Instant::now();
        Ok(GetRouteServerPropagationsOutput {
            route_server_propagations: Some(store.route_server_propagations_at(
                &input.route_server_id,
                route_table_id,
                now,
            )),
        })
    }
}
