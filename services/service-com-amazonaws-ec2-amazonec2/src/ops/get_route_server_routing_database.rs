// SPDX-License-Identifier: CC0-1.0

//! @op GetRouteServerRoutingDatabase
//! IMPLEMENTED: reports one route server's Routing Information Base — the routes its BGP
//! peers have advertised to it — narrowed by the request's filters and paged with the token
//! this service mints, beside whether that route server persists what its database holds.
//!
//! This is the ONLY file to touch for this operation.
//!
//! ## What a route server's database holds
//!
//! A database entry is a prefix one of the route server's peers ADVERTISED over an
//! established BGP session — which is why every `RouteServerRoute` names the peer and the
//! endpoint the prefix arrived on. A peer whose session is not up has advertised nothing and
//! so contributes no entry, and nothing in this emulator speaks BGP to a peer address: every
//! stored peer reports its session `down`
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::insert_route_server_peer`]). A route
//! server's database is therefore what its established sessions carried and nothing else —
//! minting a prefix for a session that carried none would be routing data nothing learned.
//!
//! A READ: whether routes are persisted is DERIVED from the stored route server's
//! persistence configuration and the instant the clock owes that move
//! ([`crate::state::RouteServerRecord::persist_routes_state_at`]), so reading the database
//! changes nothing a later caller sees. A route server id the account holds no route server
//! for is refused rather than answered with an empty database, so "no routes" and "no such
//! route server" stay distinguishable.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so engine
//! access is guaranteed by the time any request runs. The store is reached through
//! [`crate::state::require_service_state`], the way every handler beside this one reaches
//! it.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetRouteServerRoutingDatabase`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Gets the routing database for the specified route server. The Routing Information Base
//! (RIB) serves as a database that stores all the routing information and network topology
//! data collected by a router or routing system, such as routes learned from BGP peers. The
//! RIB is constantly updated as new routing information is received or existing routes
//! change. This ensures that the route server always has the most current view of the
//! network topology and can make optimal routing decisions. Amazon VPC Route Server
//! simplifies routing for traffic between workloads that are deployed within a VPC and its
//! internet gateways. With this feature, VPC Route Server dynamically updates VPC and
//! internet gateway route tables with your preferred IPv4 or IPv6 routes to achieve routing
//! fault tolerance for those workloads. This enables you to automatically reroute traffic
//! within a VPC, which increases the manageability of VPC routing and interoperability with
//! third-party workloads. Route server supports the follow route table types: VPC route
//! tables not associated with subnets Subnet route tables Internet gateway route tables
//! Route server does not support route tables associated with virtual private gateways. To
//! propagate routes into a transit gateway route table, use Transit Gateway Connect.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `Filters` (FilterList) — Filters to apply to the routing database query.
//! * `MaxResults` (RouteServerMaxResults, range 5..1000) — The maximum number of routing
//!   database entries to return in a single response.
//! * `NextToken` (String) — The token for the next page of results.
//! * `RouteServerId` (RouteServerId) **required** — The ID of the route server for which to
//!   get the routing database.
//!
//! ## Output
//!
//! * `AreRoutesPersisted` (Boolean) — Indicates whether routes are being persisted in the
//!   routing database.
//! * `NextToken` (String) — The token to use to retrieve the next page of results. This value
//!   is null when there are no more results to return.
//! * `Routes` (RouteServerRouteList) — The collection of routes in the route server's routing
//!   database.

#[allow(unused_imports)]
use super::types::*;

use std::borrow::Cow;

use crate::state::{
    CompiledFilter, MAX_FILTER_MATCH_STEPS, MAX_FILTER_VALUE_BYTES, MAX_FILTER_VALUES, MAX_FILTERS,
    MatchBudget,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetRouteServerRoutingDatabaseError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetRouteServerRoutingDatabaseError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#GetRouteServerRoutingDatabase")]
#[derive(Debug)]
pub enum GetRouteServerRoutingDatabaseErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_route_server_routing_database::GetRouteServerRoutingDatabaseError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetRouteServerRoutingDatabaseError = crate::scaffolded::Scaffolded<GetRouteServerRoutingDatabaseErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetRouteServerRoutingDatabaseErrorModeled> for GetRouteServerRoutingDatabaseError {
    fn from(modeled: GetRouteServerRoutingDatabaseErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetRouteServerRoutingDatabase` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetRouteServerRoutingDatabase<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetRouteServerRoutingDatabase<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetRouteServerRoutingDatabase<Eng>
{
    type Input = GetRouteServerRoutingDatabaseInput;
    type Output = GetRouteServerRoutingDatabaseOutput;
    type Error = GetRouteServerRoutingDatabaseError;

    const NAME: &'static str = "GetRouteServerRoutingDatabase";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ROUTE_SERVER_ROUTING_DATABASE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ROUTE_SERVER_ROUTING_DATABASE_OUTPUT_SCHEMA;
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

        // "The ID of the route server for which to get the routing database"
        // (`RouteServerId`) is `@required` but modelled `clientOptional`, so a request that
        // left it out reaches this handler as an empty string rather than being refused by
        // the boundary validator. A parameter the request did not carry is EC2's
        // `MissingParameter`, not an id to look up.
        if input.route_server_id.is_empty() {
            return Err(state::missing_parameter(
                "The request must contain the parameter routeServerId",
            ));
        }

        // Matching cost is filters x values x value length x database entries, and the model
        // bounds none of them (`FilterList` and `Filter$Values` carry no `@length`), so
        // every request-controlled factor is capped HERE, before any work proportional to it
        // is done and before the store is locked — the same ceilings every other filtered
        // report of this crate enforces, from `crate::state`.
        let filters: &[Filter] = input.filters.as_deref().unwrap_or_default();
        if filters.len() > MAX_FILTERS {
            return Err(state::invalid_parameter_value(format!(
                "The request carries {} filters, above the maximum of {MAX_FILTERS}",
                filters.len()
            )));
        }
        let mut compiled: Vec<CompiledFilter> = Vec::with_capacity(filters.len());
        for filter in filters {
            let name = filter.name.as_deref().unwrap_or_default();
            if !is_documented_filter(name) {
                return Err(state::invalid_parameter_value(format!(
                    "The filter '{name}' is invalid"
                )));
            }
            let values: &[String] = filter.values.as_deref().unwrap_or_default();
            if values.len() > MAX_FILTER_VALUES {
                return Err(state::invalid_parameter_value(format!(
                    "The filter '{name}' carries {} values, above the maximum of \
                     {MAX_FILTER_VALUES}",
                    values.len()
                )));
            }
            // A capped NUMBER of comparisons is not capped WORK: the matcher's cost is the
            // product of the pattern's length and the value's, so a value's size is bounded
            // here too.
            if let Some(oversize) = values
                .iter()
                .find(|value| value.len() > MAX_FILTER_VALUE_BYTES)
            {
                return Err(state::invalid_parameter_value(format!(
                    "A value of the filter '{name}' carries {} bytes, above the maximum of \
                     {MAX_FILTER_VALUE_BYTES}",
                    oversize.len()
                )));
            }
            compiled.push(CompiledFilter::new(name.to_string(), values.to_vec()));
        }

        // "The token for the next page of results" (`NextToken`) — a token this service did
        // not mint is refused rather than read as a position, and it is refused before the
        // store is read, so whether a bogus token is caught never depends on what the store
        // holds. The cursor carries the ROUTE SERVER it was minted for, so a token presented
        // back against another route server's database is refused rather than resuming
        // inside a database it never reported.
        let start_at = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                state::decode_scoped_page_token(token, &input.route_server_id)
                    .ok_or_else(|| state::invalid_pagination_token_for(token))?,
            ),
        };

        // "The maximum number of routing database entries to return in a single response"
        // (`MaxResults`). A request that names none gets the largest page the model
        // publishes, so the `NextToken` path below behaves the same whether or not a page
        // size was asked for. The model's `range 5..1000` has already refused anything
        // outside it; the floor of [`MIN_MAX_RESULTS`] is applied anyway, because a page size
        // of zero would mint a token for a page that carried nothing and leave a paging
        // client in a loop that never advances.
        let limit = input
            .max_results
            .and_then(|max_results| usize::try_from(max_results).ok())
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .max(MIN_MAX_RESULTS);

        let service_state = state::require_service_state(&*self.engine)?;
        // Nothing is written and no clock-owed move is materialized: what the clock owes a
        // route server and its peers is a STATE move this report derives on read, and it
        // leaves the database those sessions carried exactly as it was.
        let store = service_state.lock();

        // An id that names no route server of the account is an ERROR, not an empty
        // database: a caller has to be able to tell "this route server has learned no route"
        // from "there is no such route server". Resolved through the crate's one helper for
        // that lookup, so a malformed id and an unknown one are told apart here exactly as
        // every other route server operation tells them apart.
        state::require_known_route_server(&store, &input.route_server_id)?;

        // DryRun sits AFTER the named route server has been resolved, the order every other
        // route server operation of this crate answers a dry run in: the flag asks whether
        // the request WOULD have worked, so one that would have been refused is refused with
        // that reason instead.
        if input.dry_run == Some(true) {
            return Err(state::dry_run_operation());
        }

        let now = ::std::time::Instant::now();
        // Resolved above, so the route server is in the store; read through the same id
        // rather than carried out of that check, so the lookup and the report cannot
        // disagree about which record they mean.
        let record = store
            .route_servers
            .get(&input.route_server_id)
            .ok_or_else(|| state::invalid_route_server_id_not_found(&input.route_server_id))?;

        // "Indicates whether routes are being persisted in the routing database"
        // (`AreRoutesPersisted`): the stored route server's persistence configuration as of
        // now — one whose persistence is still `enabling`, or is being turned off, is not
        // persisting, so only the settled `enabled` state reports `true`.
        let are_routes_persisted =
            record.persist_routes_state_at(now) == state::PERSIST_ROUTES_STATE_ENABLED;

        // Every entry of this route server's database the request selects, each beside the
        // position it holds in the order the report pages over.
        //
        // Every comparison is charged to ONE budget, so the product of entries, filters,
        // values and value length is bounded however the caller combines them — which is
        // also what bounds how long this holds the store's lock.
        let mut budget = MatchBudget::new(MAX_FILTER_MATCH_STEPS);
        let mut selected: Vec<(String, RouteServerRoute)> =
            learned_routes(&store, &input.route_server_id)
                .into_iter()
                .filter(|route| {
                    state::matches_every_filter(&compiled, &mut budget, &|name| {
                        filter_values(route, name)
                    })
                })
                .map(|route| (entry_position(&route), route))
                .collect();
        // A spent budget means the answer is INCOMPLETE, which is not something to report as
        // a result: the request asked for more filtering than one call will do, and is told
        // so.
        if budget.exhausted() {
            return Err(crate::scaffolded::Scaffolded::request_limit_exceeded(
                format!(
                    "The request's filters would take more than {MAX_FILTER_MATCH_STEPS} \
                     comparisons to evaluate; narrow them or page with MaxResults"
                ),
            ));
        }
        // The paging order the cursor below names a position in: total, so the cursor always
        // advances.
        selected.sort_by(|(left, _), (right, _)| left.cmp(right));

        // "This value is null when there are no more results to return" (`NextToken`): a
        // token only when an entry follows this page, scoped to the route server this report
        // is of — the crate's one reading of that rule
        // (`crate::state::scoped_page_from_cursor`), so this report cannot page differently
        // from the others.
        let (page, next_token) = state::scoped_page_from_cursor(
            &input.route_server_id,
            &selected,
            start_at.as_deref(),
            Some(limit),
            state::PageTokenRule::WhenAnotherItemFollows,
            |(position, _)| position.as_str(),
        );

        Ok(GetRouteServerRoutingDatabaseOutput {
            are_routes_persisted: Some(are_routes_persisted),
            routes: Some(page.iter().map(|(_, route)| route.clone()).collect()),
            next_token,
        })
    }
}

/// The page size a request that names no `MaxResults` gets: the largest the model publishes
/// (`RouteServerMaxResults`, `range 5..1000`), which is also the most EC2 returns in one
/// page.
const DEFAULT_MAX_RESULTS: usize = 1000;

/// The smallest page this operation will serve: the minimum of that same published range
/// (`RouteServerMaxResults`, `range 5..1000`). The boundary validator refuses anything below
/// it, so this is the floor that keeps a page of zero — which would mint a token for a page
/// carrying nothing — unreachable however the request is read.
const MIN_MAX_RESULTS: usize = 5;

/// The BGP session state a peer that has established its session reports: the `up` member of
/// `RouteServerBgpState` (`smithy_query(query="shape",shape="RouteServerBgpState")`), whose
/// other member is [`crate::state::ROUTE_SERVER_SESSION_DOWN`].
const SESSION_UP: &str = "up";

/// Every route the route server `route_server_id` has LEARNED, as its routing database holds
/// them.
///
/// "The Routing Information Base (RIB) serves as a database that stores all the routing
/// information and network topology data collected by a router or routing system, such as
/// routes learned from BGP peers" (`GetRouteServerRoutingDatabase`): an entry is a prefix one
/// of this route server's peers advertised over an ESTABLISHED BGP session, which is why
/// `RouteServerRoute` names the peer and the endpoint it arrived on. A peer whose session is
/// not [`SESSION_UP`] has advertised nothing and contributes no entry — and nothing in this
/// emulator speaks BGP to a peer address, so every stored peer's session is reported
/// [`crate::state::ROUTE_SERVER_SESSION_DOWN`]
/// ([`crate::state::ComAmazonawsEc2Amazonec2State::insert_route_server_peer`]) and a route
/// server's database holds what its established sessions carried and nothing else. A prefix
/// minted for a session that carried none would be routing data this emulator never learned.
fn learned_routes(
    store: &crate::state::ComAmazonawsEc2Amazonec2State,
    route_server_id: &str,
) -> Vec<RouteServerRoute> {
    store
        .route_server_peers
        .values()
        .filter(|peer| peer.route_server_id.as_deref() == Some(route_server_id))
        .filter(|peer| {
            peer.bgp_status
                .as_ref()
                .and_then(|status| status.status.as_deref())
                == Some(SESSION_UP)
        })
        // The advertisements an established session carried are that peer's entries in the
        // database, and a session that carried none contributes nothing. No session in this
        // emulator is established, so nothing reaches this point.
        .flat_map(|_established| ::std::iter::empty::<RouteServerRoute>())
        .collect()
}

/// The position one database entry holds in the order this report pages over.
///
/// The prefix, then the peer that advertised it and the endpoint it arrived on: a route
/// server learns the same prefix from as many peers as advertise it — `RouteServerRoute`
/// names the peer and the endpoint for exactly that reason — so the prefix alone is not a
/// position. Joined by [`crate::state::CURSOR_SCOPE_SEPARATOR`], the character this crate
/// already joins the halves of a cursor with because no id of this service carries it.
fn entry_position(route: &RouteServerRoute) -> String {
    let separator = crate::state::CURSOR_SCOPE_SEPARATOR;
    format!(
        "{prefix}{separator}{peer}{separator}{endpoint}",
        prefix = route.prefix.as_deref().unwrap_or_default(),
        peer = route.route_server_peer_id.as_deref().unwrap_or_default(),
        endpoint = route.route_server_endpoint_id.as_deref().unwrap_or_default(),
    )
}

/// The filter names this operation answers.
///
/// `GetRouteServerRoutingDatabaseRequest$Filters` publishes no catalog at all — "Filters to
/// apply to the routing database query"
/// (`smithy_query(query="doc",shape="GetRouteServerRoutingDatabaseRequest",member="Filters")`)
/// — so the names are EC2's own convention for a resource that publishes none: each member of
/// the reported shape (`RouteServerRoute`) under its kebab-cased name, with a member of a
/// nested structure reached through the dotted path the nesting gives it. A database entry is
/// not a taggable resource, so there is no `tag:` pair here. Every entry must be answered by
/// [`filter_values`].
const ROUTE_FILTERS: [&str; 10] = [
    "as-paths",
    "med",
    "next-hop-ip",
    "prefix",
    "route-installation-details.route-installation-status",
    "route-installation-details.route-installation-status-reason",
    "route-installation-details.route-table-id",
    "route-server-endpoint-id",
    "route-server-peer-id",
    "route-status",
];

/// Is `name` a filter this operation answers? "Filter names are case-sensitive"
/// (`Filter$Name`), so the comparison is exact.
fn is_documented_filter(name: &str) -> bool {
    ROUTE_FILTERS.contains(&name)
}

/// The values the EC2 filter `name` compares against one database entry.
///
/// A filter naming an attribute the entry does not carry compares against NO values, so it
/// selects nothing — the same answer AWS gives for, say, an installation status on a route
/// that was never offered to a route table.
fn filter_values<'a>(route: &'a RouteServerRoute, name: &str) -> Vec<Cow<'a, str>> {
    let text = |value: Option<&'a str>| -> Vec<Cow<'a, str>> {
        value.map(Cow::Borrowed).into_iter().collect()
    };
    let installation_details = || route.route_installation_details.iter().flatten();
    match name {
        // A list member is compared against every value it holds: "if you specify multiple
        // values for a filter, the values are joined with an OR" (`Filter$Values`), and one
        // of the entry's own values matching is a match.
        "as-paths" => route
            .as_paths
            .iter()
            .flatten()
            .map(|as_path| Cow::Borrowed(as_path.as_str()))
            .collect(),
        // A numeric attribute is filtered on the value a client sends for it, which is how
        // the wire renders it: the metric in decimal.
        "med" => route
            .med
            .map(|med| Cow::Owned(med.to_string()))
            .into_iter()
            .collect(),
        "next-hop-ip" => text(route.next_hop_ip.as_deref()),
        "prefix" => text(route.prefix.as_deref()),
        "route-installation-details.route-installation-status" => installation_details()
            .filter_map(|detail| {
                detail
                    .route_installation_status
                    .as_deref()
                    .map(Cow::Borrowed)
            })
            .collect(),
        "route-installation-details.route-installation-status-reason" => installation_details()
            .filter_map(|detail| {
                detail
                    .route_installation_status_reason
                    .as_deref()
                    .map(Cow::Borrowed)
            })
            .collect(),
        "route-installation-details.route-table-id" => installation_details()
            .filter_map(|detail| detail.route_table_id.as_deref().map(Cow::Borrowed))
            .collect(),
        "route-server-endpoint-id" => text(route.route_server_endpoint_id.as_deref()),
        "route-server-peer-id" => text(route.route_server_peer_id.as_deref()),
        "route-status" => text(route.route_status.as_deref()),
        // Unreachable: `is_documented_filter` has already refused every other name. An
        // undefined name compares against nothing, so it could never widen a result even if
        // that check were bypassed.
        _ => Vec::new(),
    }
}
