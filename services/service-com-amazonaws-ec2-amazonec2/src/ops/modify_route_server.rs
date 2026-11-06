// SPDX-License-Identifier: CC0-1.0

//! @op ModifyRouteServer
//!
//! Applies a configuration change to one stored route server, so every later reader of the
//! store — `DescribeRouteServers` — reports the configuration this call answered with.
//!
//! What the request owns — the persistence action, its duration, the notification flag —
//! is written onto the stored record; what the service owns is derived: the SNS topic ARN a
//! notifying route server publishes to, and the two states, which
//! [`crate::state::RouteServerRecord::view_at`] derives from the instants the clock owes
//! them. A change is applied over a span, so the route server is `modifying` and its
//! persistence `enabling`, `disabling` or `resetting` until the clock settles both.
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
//! # Modeled contract for `ModifyRouteServer`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Modifies the configuration of an existing route server. Amazon VPC Route Server
//! simplifies routing for traffic between workloads that are deployed within a VPC and its
//! internet gateways. With this feature, VPC Route Server dynamically updates VPC and
//! internet gateway route tables with your preferred IPv4 or IPv6 routes to achieve routing
//! fault tolerance for those workloads. This enables you to automatically reroute traffic
//! within a VPC, which increases the manageability of VPC routing and interoperability with
//! third-party workloads. Route server supports the follow route table types: VPC route
//! tables not associated with subnets Subnet route tables Internet gateway route tables
//! Route server does not support route tables associated with virtual private gateways. To
//! propagate routes into a transit gateway route table, use Transit Gateway Connect. For
//! more information see Dynamic routing in your VPC with VPC Route Server in the Amazon VPC
//! User Guide.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `PersistRoutes` (RouteServerPersistRoutesAction, one of: disable | enable | reset) —
//!   Specifies whether to persist routes after all BGP sessions are terminated. enable:
//!   Routes will be persisted in FIB and RIB after all BGP sessions are terminated. disable:
//!   Routes will not be persisted in FIB and RIB after all BGP sessions are terminated.
//!   reset: If a route server has persisted routes due to all BGP sessions having ended,
//!   reset will withdraw all routes and reset route server to an empty FIB and RIB.
//! * `PersistRoutesDuration` (BoxedLong) — The number of minutes a route server will wait
//!   after BGP is re-established to unpersist the routes in the FIB and RIB. Value must be in
//!   the range of 1-5. Required if PersistRoutes is enabled. If you set the duration to 1
//!   minute, then when your network appliance re-establishes BGP with route server, it has 1
//!   minute to relearn it's adjacent network and advertise those routes to route server
//!   before route server resumes normal functionality. In most cases, 1 minute is probably
//!   sufficient. If, however, you have concerns that your BGP network may not be capable of
//!   fully re-establishing and re-learning everything in 1 minute, you can increase the
//!   duration up to 5 minutes.
//! * `RouteServerId` (RouteServerId) **required** — The ID of the route server to modify.
//! * `SnsNotificationsEnabled` (Boolean) — Specifies whether to enable SNS notifications for
//!   route server events. Enabling SNS notifications persists BGP status changes to an SNS
//!   topic provisioned by Amazon Web Services.
//!
//! ## Output
//!
//! * `RouteServer` (RouteServer) — Information about the modified route server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ModifyRouteServerError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ModifyRouteServerError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#ModifyRouteServer")]
#[derive(Debug)]
pub enum ModifyRouteServerErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::modify_route_server::ModifyRouteServerError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ModifyRouteServerError = crate::scaffolded::Scaffolded<ModifyRouteServerErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ModifyRouteServerErrorModeled> for ModifyRouteServerError {
    fn from(modeled: ModifyRouteServerErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ModifyRouteServer` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ModifyRouteServer<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ModifyRouteServer<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ModifyRouteServer<Eng>
{
    type Input = ModifyRouteServerInput;
    type Output = ModifyRouteServerOutput;
    type Error = ModifyRouteServerError;

    const NAME: &'static str = "ModifyRouteServer";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::MODIFY_ROUTE_SERVER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::MODIFY_ROUTE_SERVER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::scaffolded::Scaffolded;
        use crate::state::{
            missing_parameter, persist_routes_duration_invalid, request_region,
            require_live_route_server, require_service_state, route_server_sns_topic_arn,
            PERSIST_ROUTES_DISABLE, PERSIST_ROUTES_DURATION_RANGE, PERSIST_ROUTES_ENABLE,
            PERSIST_ROUTES_RESET, PERSIST_ROUTES_STATE_DISABLING, PERSIST_ROUTES_STATE_ENABLING,
            PERSIST_ROUTES_STATE_RESETTING, TRANSITION_DELAY,
        };

        // The Region the request was signed for, which the SNS topic a notifying route
        // server publishes to is named in — the same fact the create reads it from.
        let region = request_region(ctx.parts());

        // "The ID of the route server to modify" (`$RouteServerId`): `@required` but
        // modelled `clientOptional`, so a request that left it out reaches this handler as
        // an empty string rather than being refused by the boundary validator. A parameter
        // the request did not carry is EC2's `MissingParameter`.
        if input.route_server_id.is_empty() {
            return Err(missing_parameter(
                "The request must contain the parameter routeServerId",
            ));
        }

        // This operation "modifies the configuration of an existing route server"
        // (`ModifyRouteServer`), and the configuration is these three members: a request
        // naming none of them asks for no change at all, so there is nothing to apply.
        if input.persist_routes.is_none()
            && input.persist_routes_duration.is_none()
            && input.sns_notifications_enabled.is_none()
        {
            return Err(missing_parameter(
                "The request must contain the parameter persistRoutes, \
                 persistRoutesDuration or snsNotificationsEnabled",
            ));
        }

        // "Value must be in the range of 1-5" (`$PersistRoutesDuration`). The member is a
        // plain `BoxedLong` with no modelled `@range`, so the published bounds are enforced
        // here, with the service's own sentence for a persist-routes time it will not act on
        // ([`crate::state::persist_routes_duration_invalid`]).
        let (duration_min, duration_max) = PERSIST_ROUTES_DURATION_RANGE;
        if input
            .persist_routes_duration
            .is_some_and(|duration| duration < duration_min || duration > duration_max)
        {
            return Err(persist_routes_duration_invalid());
        }

        let state = require_service_state(&*self.engine)?;
        let mut store = state.lock();

        // An id that names no route server of the account is an error, not a route server
        // created out of thin air, and a route server a delete has taken down has no
        // configuration left to change. The crate's one resolver for an operation ACTING on a
        // route server tells a malformed id from an unknown one and refuses one in a delete
        // state, and it is asked BEFORE the dry run: each is a reason the request would not
        // have succeeded.
        require_live_route_server(&store, &input.route_server_id)?;
        let record = store
            .route_servers
            .get_mut(&input.route_server_id)
            .ok_or_else(|| {
                Scaffolded::internal_error(format!(
                    "the route server '{route_server_id}' could not be read after it was \
                     resolved",
                    route_server_id = input.route_server_id,
                ))
            })?;
        let stored_persist_enabled = record.persist_routes_enabled;
        let stored_duration = record.view.persist_routes_duration;

        // What the persistence action asks of the route server's configuration. The modelled
        // enum has already refused any spelling outside `enable | disable | reset`, and
        // `reset` "will withdraw all routes and reset route server to an empty FIB and RIB"
        // (`$PersistRoutes`) — an action on the routes already persisted, which leaves
        // whether this route server persists them at all as it is stored.
        let action = input.persist_routes.as_deref();
        let persist_routes_enabled = match action {
            Some(PERSIST_ROUTES_ENABLE) => true,
            Some(PERSIST_ROUTES_DISABLE) => false,
            _ => stored_persist_enabled,
        };

        // "Required if PersistRoutes is enabled" and, on the reported member, "Only valid if
        // persistRoutesState is 'enabled'" (`RouteServer$PersistRoutesDuration`): a duration
        // is part of a persisting configuration, so it cannot be given for one that does not
        // persist, and turning persistence on without one leaves the configuration
        // incomplete — unless the route server already carries a duration, which is the
        // value "required" asks for and is kept.
        let persist_routes_duration = match (input.persist_routes_duration, persist_routes_enabled)
        {
            (Some(_), false) => return Err(persist_routes_duration_invalid()),
            (Some(duration), true) => Some(duration),
            (None, true) => match stored_duration {
                Some(duration) => Some(duration),
                None => {
                    return Err(missing_parameter(format!(
                        "persistRoutesDuration is required when persistRoutes is \
                         '{PERSIST_ROUTES_ENABLE}'"
                    )));
                }
            },
            // A route server that persists nothing has no duration to wait out, which is
            // what the reported member's "only valid if" sentence leaves it with.
            (None, false) => None,
        };

        // "Specifies whether to enable SNS notifications for route server events"
        // (`$SnsNotificationsEnabled`): a flag the request did not carry asks for no change,
        // so the stored choice stands.
        let sns_notifications_enabled = input
            .sns_notifications_enabled
            .or(record.view.sns_notifications_enabled);

        // Which transient `RouteServerPersistRoutesState` the clock has to settle, and so
        // whether the persistence move has a span at all: an action that was asked for is
        // within one, and a request that named none leaves the persistence state as stored.
        let persist_routes_transition = match action {
            Some(PERSIST_ROUTES_ENABLE) => Some(PERSIST_ROUTES_STATE_ENABLING),
            Some(PERSIST_ROUTES_DISABLE) => Some(PERSIST_ROUTES_STATE_DISABLING),
            Some(PERSIST_ROUTES_RESET) => Some(PERSIST_ROUTES_STATE_RESETTING),
            _ => None,
        };

        // DryRun sits AFTER every parameter has been validated and the route server
        // resolved, and BEFORE the first write: the flag asks whether the request WOULD have
        // worked, so one that would have been refused is refused with that reason instead,
        // and one that would have succeeded leaves state untouched.
        if input.dry_run == Some(true) {
            return Err(Scaffolded::dry_run_operation());
        }

        // The modification itself, applied to the route server resolved above under this
        // same lock, so nothing can have come between the two.
        let now = ::std::time::Instant::now();
        record.persist_routes_enabled = persist_routes_enabled;
        record.view.persist_routes_duration = persist_routes_duration;
        record.view.sns_notifications_enabled = sns_notifications_enabled;
        // "Enabling SNS notifications persists BGP status changes to an SNS topic provisioned
        // by Amazon Web Services" (`$SnsNotificationsEnabled`): the topic belongs to a route
        // server whose notifications are on, so turning them off leaves it naming none and
        // turning them on names the one this service provisions for that route server.
        record.view.sns_topic_arn = sns_notifications_enabled
            .unwrap_or(false)
            .then(|| route_server_sns_topic_arn(&region, &input.route_server_id));
        record.persist_routes_transition = persist_routes_transition;
        // A persistence move the request asked for is within its transient state until the
        // clock settles it, the same span every other move this store owes takes; one the
        // request did not ask for is already settled, so the stored instant stands.
        if persist_routes_transition.is_some() {
            record.persist_routes_settled_at = now + TRANSITION_DELAY;
        }
        // The route server itself is `modifying` for that same span, so a caller reading
        // straight after this call still sees the change being applied.
        record.modify_settled_at = now + TRANSITION_DELAY;

        // Read the answer back OUT of the store, so the caller sees what a later
        // `DescribeRouteServers` will see rather than the request object.
        Ok(ModifyRouteServerOutput {
            route_server: Some(record.view_at(now)),
        })
    }
}
