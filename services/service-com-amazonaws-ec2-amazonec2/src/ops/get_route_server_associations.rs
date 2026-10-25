// SPDX-License-Identifier: CC0-1.0

//! @op GetRouteServerAssociations
//!
//! Reports the associations one route server holds — the connections
//! `AssociateRouteServer` established between that route server and the account's VPCs —
//! in `VpcId` order.
//!
//! A READ: the clock-owed `associating` -> `associated` move is DERIVED from each
//! association's due instant by [`crate::state::RouteServerAssociationRecord::view_at`],
//! so getting a route server's associations changes nothing a later caller sees. A route
//! server id the account holds no route server for is refused rather than answered with an
//! empty set, so "no associations" and "no such route server" are distinguishable.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store
//! is reached through [`crate::state::require_service_state`], the way every handler beside
//! this one reaches it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetRouteServerAssociations`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Gets information about the associations for the specified route server. A route server
//! association is the connection established between a route server and a VPC. For more
//! information see Dynamic routing in your VPC with VPC Route Server in the Amazon VPC User
//! Guide.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `RouteServerId` (RouteServerId) **required** — The ID of the route server for which to
//!   get association information.
//!
//! ## Output
//!
//! * `RouteServerAssociations` (RouteServerAssociationsList) — Information about the
//!   associations for the specified route server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetRouteServerAssociationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetRouteServerAssociationsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#GetRouteServerAssociations")]
#[derive(Debug)]
pub enum GetRouteServerAssociationsErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_route_server_associations::GetRouteServerAssociationsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetRouteServerAssociationsError = crate::scaffolded::Scaffolded<GetRouteServerAssociationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetRouteServerAssociationsErrorModeled> for GetRouteServerAssociationsError {
    fn from(modeled: GetRouteServerAssociationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetRouteServerAssociations` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetRouteServerAssociations<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetRouteServerAssociations<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetRouteServerAssociations<Eng>
{
    type Input = GetRouteServerAssociationsInput;
    type Output = GetRouteServerAssociationsOutput;
    type Error = GetRouteServerAssociationsError;

    const NAME: &'static str = "GetRouteServerAssociations";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ROUTE_SERVER_ASSOCIATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ROUTE_SERVER_ASSOCIATIONS_OUTPUT_SCHEMA;
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

        // "The ID of the route server for which to get association information"
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
        // association set: a caller has to be able to tell "this route server holds no
        // association" from "there is no such route server". Resolved through the crate's
        // one helper for that lookup, so a malformed id and an unknown one are told apart
        // here exactly as `DescribeRouteServers` and `CreateRouteServerEndpoint` tell them
        // apart.
        state::require_known_route_server(&store, &input.route_server_id)?;

        // DryRun sits AFTER the named route server has been resolved, the order every other
        // route server operation of this crate answers a dry run in: the flag asks whether
        // the request WOULD have worked, so one that would have been refused is refused with
        // that reason instead.
        if input.dry_run == Some(true) {
            return Err(state::dry_run_operation());
        }

        // Reporting only: the `associating` -> `associated` move the clock already owed each
        // association is OBSERVED here, never written — the state is derived from the stored
        // due instant — so this read changes nothing a later caller sees.
        let now = ::std::time::Instant::now();
        Ok(GetRouteServerAssociationsOutput {
            route_server_associations: Some(
                store.route_server_associations_at(&input.route_server_id, now),
            ),
        })
    }
}
