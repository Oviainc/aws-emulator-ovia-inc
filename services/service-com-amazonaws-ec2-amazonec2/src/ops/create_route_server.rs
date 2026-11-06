// SPDX-License-Identifier: CC0-1.0

//! @op CreateRouteServer
//!
//! Mints one route server from the request and stores it, so every later reader of the
//! store — `DescribeRouteServers` — reports the same route server this call answered with.
//!
//! What the service owns is derived here rather than taken from the request: the
//! `RouteServerId`, the SNS topic ARN a notifying route server publishes to, and the two
//! states, which [`crate::state::RouteServerRecord::view_at`] derives from the instants the
//! clock owes them. What the request owns — the ASN, the persistence choice and its
//! duration, the notification flag, the tags — round-trips unchanged.
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
//! # Modeled contract for `CreateRouteServer`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Creates a new route server to manage dynamic routing in a VPC. Amazon VPC Route Server
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
//! * `AmazonSideAsn` (Long) **required** — The private Autonomous System Number (ASN) for the
//!   Amazon side of the BGP session. Valid values are from 1 to 4294967295. We recommend
//!   using a private ASN in the 64512–65534 (16-bit ASN) or 4200000000–4294967294 (32-bit
//!   ASN) range.
//! * `ClientToken` (String, @idempotencyToken) — Unique, case-sensitive identifier to ensure
//!   idempotency of the request.
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `PersistRoutes` (RouteServerPersistRoutesAction, one of: disable | enable | reset) —
//!   Indicates whether routes should be persisted after all BGP sessions are terminated.
//! * `PersistRoutesDuration` (BoxedLong) — The number of minutes a route server will wait
//!   after BGP is re-established to unpersist the routes in the FIB and RIB. Value must be in
//!   the range of 1-5. Required if PersistRoutes is enabled. If you set the duration to 1
//!   minute, then when your network appliance re-establishes BGP with route server, it has 1
//!   minute to relearn it's adjacent network and advertise those routes to route server
//!   before route server resumes normal functionality. In most cases, 1 minute is probably
//!   sufficient. If, however, you have concerns that your BGP network may not be capable of
//!   fully re-establishing and re-learning everything in 1 minute, you can increase the
//!   duration up to 5 minutes.
//! * `SnsNotificationsEnabled` (Boolean) — Indicates whether SNS notifications should be
//!   enabled for route server events. Enabling SNS notifications persists BGP status changes
//!   to an SNS topic provisioned by Amazon Web Services.
//! * `TagSpecifications` (TagSpecificationList) — The tags to apply to the route server
//!   during creation.
//!
//! ## Output
//!
//! * `RouteServer` (RouteServer) — Information about the created route server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateRouteServerError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateRouteServerError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#CreateRouteServer")]
#[derive(Debug)]
pub enum CreateRouteServerErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_route_server::CreateRouteServerError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateRouteServerError = crate::scaffolded::Scaffolded<CreateRouteServerErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateRouteServerErrorModeled> for CreateRouteServerError {
    fn from(modeled: CreateRouteServerErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateRouteServer` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateRouteServer<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateRouteServer<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateRouteServer<Eng>
{
    type Input = CreateRouteServerInput;
    type Output = CreateRouteServerOutput;
    type Error = CreateRouteServerError;

    const NAME: &'static str = "CreateRouteServer";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ROUTE_SERVER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ROUTE_SERVER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::scaffolded::Scaffolded;
        use crate::state::{
            idempotent_parameter_mismatch, persist_routes_duration_invalid, request_region,
            require_service_state, route_server_limit_exceeded, route_server_payload_bytes,
            route_server_sns_topic_arn, same_tags, tags_from_specifications, RouteServerRecord,
            AMAZON_SIDE_ASN_RANGE, MAX_CLIENT_TOKEN_CHARS, MAX_STORED_BYTES,
            MAX_STORED_ROUTE_SERVERS, PERSIST_ROUTES_DURATION_RANGE, PERSIST_ROUTES_ENABLE,
            PERSIST_ROUTES_STATE_ENABLING, ROUTE_SERVER_ID_PREFIX,
            ROUTE_SERVER_TAGGABLE_RESOURCE, TRANSITION_DELAY,
        };

        let region = request_region(ctx.parts());

        // "Valid values are from 1 to 4294967295"
        // (`CreateRouteServerRequest$AmazonSideAsn`). The member is a plain `Long` with no
        // modelled `@range`, so this is the only place the published bounds are enforced.
        let (asn_min, asn_max) = AMAZON_SIDE_ASN_RANGE;
        let amazon_side_asn = input.amazon_side_asn;
        if amazon_side_asn < asn_min || amazon_side_asn > asn_max {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "Invalid value '{amazon_side_asn}' for amazonSideAsn. \
                 It must be from {asn_min} to {asn_max}"
            )));
        }

        // "Indicates whether routes should be persisted after all BGP sessions are
        // terminated" (`CreateRouteServerRequest$PersistRoutes`). The modelled enum has
        // already refused any spelling outside `enable | disable | reset`, so only
        // `enable` asks for persistence here: a route server being created holds no
        // persisted routes, so neither `disable` nor `reset` leaves it with any.
        let persist_routes_enabled = input.persist_routes.as_deref() == Some(PERSIST_ROUTES_ENABLE);

        // "Value must be in the range of 1-5. Required if PersistRoutes is enabled"
        // (`CreateRouteServerRequest$PersistRoutesDuration`), and on the reported member:
        // "Only valid if persistRoutesState is 'enabled'"
        // (`RouteServer$PersistRoutesDuration`) — so a duration without that choice is
        // members that cannot hold together, and the choice without a duration is a
        // missing parameter.
        let (duration_min, duration_max) = PERSIST_ROUTES_DURATION_RANGE;
        let persist_routes_duration = match (input.persist_routes_duration, persist_routes_enabled)
        {
            // Refused with the service's own sentence for a persist-routes time it will not
            // act on, which names the parameter rather than quoting the value sent
            // ([`crate::state::persist_routes_duration_invalid`]).
            (Some(duration), _) if duration < duration_min || duration > duration_max => {
                return Err(persist_routes_duration_invalid());
            }
            // A duration given with anything other than `enable` (so `disable`, `reset`, or
            // no choice at all) is refused. The documented sentences tie the duration to
            // `enable` alone and say nothing about `reset`, so extending the refusal to
            // `reset` is THIS emulator's reading of "only valid if persistRoutesState is
            // 'enabled'" rather than text AWS publishes for `reset`.
            (Some(_), false) => {
                return Err(Scaffolded::invalid_parameter_combination(format!(
                    "persistRoutesDuration may be specified only when persistRoutes is \
                     '{PERSIST_ROUTES_ENABLE}'"
                )));
            }
            (Some(duration), true) => Some(duration),
            (None, true) => {
                return Err(Scaffolded::missing_parameter(format!(
                    "persistRoutesDuration is required when persistRoutes is \
                     '{PERSIST_ROUTES_ENABLE}'"
                )));
            }
            (None, false) => None,
        };

        // "Unique, case-sensitive identifier to ensure idempotency of the request"
        // (`CreateRouteServerRequest$ClientToken`): the token map keeps whatever it is
        // given for the process's lifetime, so its length is bounded the way every other
        // create in this crate bounds it.
        if input
            .client_token
            .as_ref()
            .is_some_and(|token| token.chars().count() > MAX_CLIENT_TOKEN_CHARS)
        {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "clientToken accepts at most {MAX_CLIENT_TOKEN_CHARS} characters"
            )));
        }

        // "The tags to apply to the route server during creation"
        // (`CreateRouteServerRequest$TagSpecifications`): the route server is the only
        // resource this call creates, so a specification naming any other resource type is
        // refused rather than accepted and discarded — by the one helper every create in
        // this crate bounds its tag payload with.
        let tags = tags_from_specifications(
            input.tag_specifications.as_deref(),
            ROUTE_SERVER_TAGGABLE_RESOURCE,
            Self::NAME,
        )?;

        // "Indicates whether SNS notifications should be enabled for route server events"
        // (`CreateRouteServerRequest$SnsNotificationsEnabled`): absent is off, which is the
        // state of a route server nobody asked to notify.
        let sns_notifications_enabled = input.sns_notifications_enabled.unwrap_or(false);

        // The caller-chosen bytes this create would retain for the process's lifetime: the
        // tags, and the client token the token map keeps. A COUNT ceiling admits megabytes
        // on its own, because each record holds strings the caller chose the length of, so
        // both are asked below.
        let payload_bytes = route_server_payload_bytes(&tags, input.client_token.as_deref());

        let state = require_service_state(&*self.engine)?;
        let mut store = state.lock();
        let now = ::std::time::Instant::now();

        // Idempotency: one token names the one route server it minted, so a replay reports
        // that route server and creates nothing. Read here, answered below the DryRun
        // check, because a replay stores nothing and so passes the ceilings a fresh create
        // has to clear.
        let replayed = input
            .client_token
            .as_deref()
            .and_then(|token| store.route_server_client_tokens.get(token).cloned());

        // A token is only idempotent for the REQUEST it was first given with: replayed
        // with different parameters it names a resource the caller did not ask for, which
        // EC2 refuses rather than answers. Checked before the DryRun flag, so a dry run of
        // such a request is refused with this reason too.
        if let Some(route_server_id) = replayed.as_deref() {
            let record = store.route_servers.get(route_server_id).ok_or_else(|| {
                Scaffolded::internal_error(format!(
                    "a client token names route server '{route_server_id}', \
                     which is not stored"
                ))
            })?;
            let same_request = record.view.amazon_side_asn == Some(amazon_side_asn)
                && record.persist_routes_enabled == persist_routes_enabled
                && record.view.persist_routes_duration == persist_routes_duration
                && record.view.sns_notifications_enabled == Some(sns_notifications_enabled)
                && same_tags(record.view.tags.as_deref().unwrap_or_default(), &tags);
            if !same_request {
                // The crate's ONE helper for this refusal, which carries EC2's own
                // documented wording for the code rather than echoing what the caller
                // sent (`crate::state::idempotent_parameter_mismatch`).
                return Err(idempotent_parameter_mismatch());
            }
        }

        if replayed.is_none() && store.route_servers.len() >= MAX_STORED_ROUTE_SERVERS {
            return Err(route_server_limit_exceeded(format!(
                "This emulator holds {MAX_STORED_ROUTE_SERVERS} route servers, \
                 which is as many as it stores"
            )));
        }
        if replayed.is_none() && store.would_exceed_stored_bytes(payload_bytes) {
            return Err(route_server_limit_exceeded(format!(
                "This emulator holds {} of at most {MAX_STORED_BYTES} bytes of \
                 caller-supplied data, so it cannot store {payload_bytes} more",
                store.stored_bytes()
            )));
        }

        // DryRun sits AFTER every parameter has been validated and BEFORE the first write:
        // the flag asks whether the request WOULD have worked, so one that would have been
        // refused is refused with that reason instead, and one that would have succeeded
        // leaves state untouched.
        if input.dry_run == Some(true) {
            return Err(Scaffolded::dry_run_operation());
        }

        if let Some(route_server_id) = replayed {
            return store
                .route_servers
                .get(&route_server_id)
                .map(|record| CreateRouteServerOutput {
                    route_server: Some(record.view_at(now)),
                })
                .ok_or_else(|| {
                    Scaffolded::internal_error(format!(
                        "a client token names route server '{route_server_id}', \
                         which is not stored"
                    ))
                });
        }

        let route_server_id = store.next_id(ROUTE_SERVER_ID_PREFIX);
        let record = RouteServerRecord {
            view: RouteServer {
                route_server_id: Some(route_server_id.clone()),
                amazon_side_asn: Some(amazon_side_asn),
                persist_routes_duration,
                sns_notifications_enabled: Some(sns_notifications_enabled),
                // "The ARN of the SNS topic where notifications are published"
                // (`RouteServer$SnsTopicArn`): the topic is provisioned for a route server
                // whose notifications are enabled, so a route server without them names
                // no topic.
                sns_topic_arn: sns_notifications_enabled
                    .then(|| route_server_sns_topic_arn(&region, &route_server_id)),
                tags: Some(tags),
                // Both derived from the due instants below on every read.
                state: None,
                persist_routes_state: None,
            },
            // A route server AWS hands back is `pending` and reaches `available` without
            // anybody asking: the same clock-owed span a `pending` VPC's `available` move
            // takes in this crate.
            available_at: now + TRANSITION_DELAY,
            persist_routes_enabled,
            // Persistence that was asked for is `enabling` until the clock settles it; a
            // route server that asked for none is `disabled` from this instant, having
            // nothing to turn off.
            persist_routes_settled_at: if persist_routes_enabled {
                now + TRANSITION_DELAY
            } else {
                now
            },
            persist_routes_transition: persist_routes_enabled
                .then_some(PERSIST_ROUTES_STATE_ENABLING),
            // Nothing has asked this route server to change yet, so its `modifying` span is
            // already behind every read.
            modify_settled_at: now,
            // Nothing has asked it to delete either, so it owes no delete move.
            deleted_at: None,
        };
        if let Some(token) = input.client_token.clone() {
            store
                .route_server_client_tokens
                .insert(token, route_server_id.clone());
        }
        // The bytes this record and its token retain are charged against the store's budget
        // at the insert, so no path can store them uncharged; a removal path must give them
        // back with `release_stored_bytes`.
        store.add_stored_bytes(payload_bytes);
        store.route_servers.insert(route_server_id.clone(), record);

        // Read the response back OUT of the store, so the caller sees what a later
        // `DescribeRouteServers` will see rather than the request object.
        store
            .route_servers
            .get(&route_server_id)
            .map(|record| CreateRouteServerOutput {
                route_server: Some(record.view_at(now)),
            })
            .ok_or_else(|| {
                Scaffolded::internal_error(format!(
                    "the newly created route server '{route_server_id}' could not be read back"
                ))
            })
    }
}

// What a replayed `ClientToken` compares its tags on lives in `crate::state::same_tags`,
// beside the other comparisons every create of this crate shares, so one rule about what
// "the same request" means serves all of them.
