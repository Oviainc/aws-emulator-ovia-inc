// SPDX-License-Identifier: CC0-1.0

//! @op DescribeRouteServers
//!
//! Reports the route servers the store holds — the ones `CreateRouteServer` minted —
//! narrowed by the ids and filters the request names, paged in `RouteServerId` order.
//!
//! A READ: the two clock-owed states (`pending` -> `available`, `enabling` -> `enabled`)
//! are DERIVED from each record's due instants by
//! [`crate::state::RouteServerRecord::view_at`], so describing a route server changes
//! nothing a later caller sees.
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
//! # Modeled contract for `DescribeRouteServers`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Describes one or more route servers. Amazon VPC Route Server simplifies routing for
//! traffic between workloads that are deployed within a VPC and its internet gateways. With
//! this feature, VPC Route Server dynamically updates VPC and internet gateway route tables
//! with your preferred IPv4 or IPv6 routes to achieve routing fault tolerance for those
//! workloads. This enables you to automatically reroute traffic within a VPC, which
//! increases the manageability of VPC routing and interoperability with third-party
//! workloads. Route server supports the follow route table types: VPC route tables not
//! associated with subnets Subnet route tables Internet gateway route tables Route server
//! does not support route tables associated with virtual private gateways. To propagate
//! routes into a transit gateway route table, use Transit Gateway Connect. For more
//! information see Dynamic routing in your VPC with VPC Route Server in the Amazon VPC User
//! Guide.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `Filters` (FilterList) — One or more filters to apply to the describe request.
//! * `MaxResults` (RouteServerMaxResults, range 5..1000) — The maximum number of results to
//!   return with a single call.
//! * `NextToken` (String) — The token for the next page of results.
//! * `RouteServerIds` (RouteServerIdsList) — The IDs of the route servers to describe.
//!
//! ## Output
//!
//! * `NextToken` (String) — The token to use to retrieve the next page of results. This value
//!   is null when there are no more results to return.
//! * `RouteServers` (RouteServersList) — Information about the described route servers.

#[allow(unused_imports)]
use super::types::*;

use crate::state::TAG_FILTER_PREFIX;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeRouteServersError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeRouteServersError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DescribeRouteServers")]
#[derive(Debug)]
pub enum DescribeRouteServersErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_route_servers::DescribeRouteServersError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeRouteServersError = crate::scaffolded::Scaffolded<DescribeRouteServersErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeRouteServersErrorModeled> for DescribeRouteServersError {
    fn from(modeled: DescribeRouteServersErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeRouteServers` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeRouteServers<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeRouteServers<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeRouteServers<Eng>
{
    type Input = DescribeRouteServersInput;
    type Output = DescribeRouteServersOutput;
    type Error = DescribeRouteServersError;

    const NAME: &'static str = "DescribeRouteServers";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ROUTE_SERVERS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ROUTE_SERVERS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::scaffolded::Scaffolded;
        use crate::state::{
            decode_page_token, encode_page_token, invalid_pagination_token, matches_every_filter,
            require_known_route_server, require_service_state, CompiledFilter, MatchBudget,
            MAX_FILTERS, MAX_FILTER_MATCH_STEPS, MAX_FILTER_VALUES, MAX_FILTER_VALUE_BYTES,
            MAX_PAGE_SIZE, MAX_RESOURCE_IDS,
        };

        // `ctx` carries request metadata only; this operation reads none of it. The store
        // is reached through the engine held on `self`.
        let _ = ctx;

        let requested_ids: &[String] = input.route_server_ids.as_deref().unwrap_or_default();
        if requested_ids.len() > MAX_RESOURCE_IDS {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "RouteServerIds carries {} ids, above the maximum of {MAX_RESOURCE_IDS}",
                requested_ids.len()
            )));
        }

        // Matching cost is filters x values x held values x stored route servers and the
        // model bounds none of them, so the request-controlled factors are capped here,
        // before any work proportional to them is done — and their PRODUCT is charged to one
        // [`MatchBudget`] below, which is the bound the per-factor caps cannot give.
        let supplied_filters = input.filters.as_ref().map_or(0, Vec::len);
        if supplied_filters > MAX_FILTERS {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "The request carries {supplied_filters} filters, above the maximum \
                 of {MAX_FILTERS}"
            )));
        }

        // A filter name this operation does not document is refused whether or not any
        // route server would have matched it, so an unsupported filter can never read as
        // "no results".
        let filters: &[Filter] = input.filters.as_deref().unwrap_or_default();
        let mut selectors: Vec<CompiledFilter> = Vec::with_capacity(filters.len());
        for filter in filters {
            let name = filter.name.as_deref().unwrap_or_default();
            let canonical = canonical_filter_name(name).ok_or_else(|| {
                Scaffolded::invalid_parameter_value(format!("The filter '{name}' is invalid"))
            })?;
            let values = filter.values.as_deref().unwrap_or_default();
            if values.len() > MAX_FILTER_VALUES {
                return Err(Scaffolded::invalid_parameter_value(format!(
                    "The filter '{name}' carries {} values, above the maximum of \
                     {MAX_FILTER_VALUES}",
                    values.len()
                )));
            }
            // A capped NUMBER of comparisons is not capped WORK: the matcher's cost is the
            // product of the pattern's length and the value's, so each value's size is
            // bounded too, before any matching runs under the lock.
            if let Some(oversize) = values
                .iter()
                .find(|value| value.len() > MAX_FILTER_VALUE_BYTES)
            {
                return Err(Scaffolded::invalid_parameter_value(format!(
                    "A value of the filter '{name}' carries {} bytes, above the maximum of \
                     {MAX_FILTER_VALUE_BYTES}",
                    oversize.len()
                )));
            }
            selectors.push(CompiledFilter::new(canonical.to_string(), values.to_vec()));
        }

        // "The token for the next page of results" — anything else is refused rather than
        // read as an id, and it is refused before any other member is looked at, so whether
        // a bogus token is caught never depends on the rest of the request.
        let start_at = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(decode_page_token(token).ok_or_else(invalid_pagination_token)?),
        };

        let state = require_service_state(&*self.engine)?;
        let store = state.lock();

        // "The IDs of the route servers to describe": an id that names no route server of
        // the account is an error, not an absent row — a malformed one before an unknown
        // one.
        for route_server_id in requested_ids {
            require_known_route_server(&store, route_server_id)?;
        }

        // DryRun sits AFTER every parameter has been validated and every named route server
        // resolved: the flag asks whether the request WOULD have succeeded, so one that
        // would have been refused is refused with that reason instead.
        if input.dry_run == Some(true) {
            return Err(Scaffolded::dry_run_operation());
        }

        // The `pending` -> `available` and `enabling` -> `enabled` moves the clock already
        // owed each route server are observed here, not written: `view_at` derives both
        // from the stored due instants, so this read changes nothing a later caller sees —
        // and the `state` filter therefore compares against the state the caller is shown.
        //
        // Every comparison is charged to ONE budget, so the PRODUCT of stored route
        // servers, filters, values, held values and value length is bounded however the
        // caller combines them — which is also what bounds how long this holds the state
        // lock.
        let now = ::std::time::Instant::now();
        let mut budget = MatchBudget::new(MAX_FILTER_MATCH_STEPS);
        // The ids the request named, as a set: the narrowing below is then one hash lookup
        // per stored route server rather than a scan of up to `MAX_RESOURCE_IDS` ids for
        // each of them.
        let narrowed_to: ::std::collections::HashSet<&str> =
            requested_ids.iter().map(String::as_str).collect();
        let mut selected: Vec<RouteServer> = Vec::new();
        for record in store.route_servers.values() {
            if budget.exhausted() {
                break;
            }
            // The id narrowing is read off the stored record, so a route server the request
            // did not name costs no projection and no comparison at all.
            if !narrowed_to.is_empty()
                && !record
                    .view
                    .route_server_id
                    .as_deref()
                    .is_some_and(|id| narrowed_to.contains(id))
            {
                continue;
            }
            let route_server = record.view_at(now);
            if matches_every_filter(&selectors, &mut budget, &|name| {
                filter_values(&route_server, name)
            }) {
                selected.push(route_server);
            }
        }
        // A spent budget means the answer is INCOMPLETE, which is not something to report
        // as a result: the request asked for more filtering than one call will do, and is
        // told so rather than given a silently truncated page.
        if budget.exhausted() {
            return Err(Scaffolded::request_limit_exceeded(format!(
                "The request's filters would take more than {MAX_FILTER_MATCH_STEPS} \
                 comparisons to evaluate; narrow them and try again"
            )));
        }

        // Page in `RouteServerId` order, which the cursor below names a position in. The
        // store is an ordered map, so its iteration order already satisfies this sort;
        // doing it here establishes the invariant the cursor depends on at the point of
        // use.
        selected.sort_by(|left, right| left.route_server_id.cmp(&right.route_server_id));

        let from = match start_at.as_deref() {
            None => 0,
            Some(cursor) => selected.partition_point(|route_server| {
                route_server.route_server_id.as_deref().unwrap_or_default() < cursor
            }),
        };
        let page = selected.get(from..).unwrap_or_default();

        // "The maximum number of results to return with a single call"
        // (`DescribeRouteServersRequest$MaxResults`), whose modelled `@range` the boundary
        // validator has already enforced. A request that names no page size gets the
        // largest page this service serves, so the `NextToken` path behaves the same
        // whether or not a page size was asked for.
        let limit = input
            .max_results
            .and_then(|max_results| usize::try_from(max_results).ok())
            .unwrap_or(MAX_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);

        // A token only when a route server follows this page: "The token to use to retrieve
        // the next page of results. This value is null when there are no more results to
        // return."
        let next_token = page
            .get(limit)
            .and_then(|route_server| route_server.route_server_id.as_deref())
            .map(encode_page_token);

        Ok(DescribeRouteServersOutput {
            route_servers: Some(page.iter().take(limit).cloned().collect()),
            next_token,
        })
    }
}

/// The filter names this operation answers.
///
/// The model documents none — `DescribeRouteServersRequest$Filters` is just "One or more
/// filters to apply to the describe request"
/// (`smithy_query(query="doc",shape="DescribeRouteServersRequest",member="Filters")`) — so
/// the names are the described resource's OWN members in EC2's hyphenated filter spelling,
/// one per member of `RouteServer`
/// (`smithy_query(query="shape",shape="RouteServer")`), beside the `tag:<key>` and
/// `tag-key` family every EC2 describe takes. Every entry must be answered by
/// [`filter_values`].
const ROUTE_SERVER_FILTERS: [&str; 8] = [
    "amazon-side-asn",
    "persist-routes-duration",
    "persist-routes-state",
    "route-server-id",
    "sns-notifications-enabled",
    "sns-topic-arn",
    "state",
    "tag-key",
];

/// The name [`filter_values`] answers for the filter the request called `name`, or `None`
/// when this operation documents no such filter.
///
/// "Filter names are case-sensitive" (`Filter$Name`), so every comparison is exact — and a
/// `tag:` prefix with no key names no tag, so it is as undocumented as any other
/// misspelling. The `tag:<key>` family keeps its whole name, the key being part of what it
/// selects on.
fn canonical_filter_name(name: &str) -> Option<&str> {
    if name
        .strip_prefix(TAG_FILTER_PREFIX)
        .is_some_and(|key| !key.is_empty())
    {
        return Some(name);
    }
    ROUTE_SERVER_FILTERS.contains(&name).then_some(name)
}

/// The values the EC2 filter `name` compares against this route server.
///
/// A filter naming a member this route server does not carry compares against NO values, so
/// it selects nothing — the answer for `sns-topic-arn` on a route server whose notifications
/// are off, and for `persist-routes-duration` on one that persists no routes. `name` is a
/// name [`canonical_filter_name`] resolved.
fn filter_values<'a>(
    route_server: &'a RouteServer,
    name: &str,
) -> Vec<::std::borrow::Cow<'a, str>> {
    use ::std::borrow::Cow;

    if let Some(key) = name.strip_prefix(TAG_FILTER_PREFIX) {
        return route_server
            .tags
            .iter()
            .flatten()
            .filter(|tag| tag.key.as_deref() == Some(key))
            .filter_map(|tag| tag.value.as_deref().map(Cow::Borrowed))
            .collect();
    }

    let text = |value: Option<&'a str>| -> Vec<Cow<'a, str>> {
        value.map(Cow::Borrowed).into_iter().collect()
    };
    let number = |value: Option<i64>| -> Vec<Cow<'a, str>> {
        value
            .map(|value| Cow::Owned(value.to_string()))
            .into_iter()
            .collect()
    };
    match name {
        "amazon-side-asn" => number(route_server.amazon_side_asn),
        "persist-routes-duration" => number(route_server.persist_routes_duration),
        "persist-routes-state" => text(route_server.persist_routes_state.as_deref()),
        "route-server-id" => text(route_server.route_server_id.as_deref()),
        // A boolean member compares against its wire rendering, `true` or `false`.
        "sns-notifications-enabled" => route_server
            .sns_notifications_enabled
            .map(|set| Cow::Borrowed(if set { "true" } else { "false" }))
            .into_iter()
            .collect(),
        "sns-topic-arn" => text(route_server.sns_topic_arn.as_deref()),
        "state" => text(route_server.state.as_deref()),
        "tag-key" => route_server
            .tags
            .iter()
            .flatten()
            .filter_map(|tag| tag.key.as_deref().map(Cow::Borrowed))
            .collect(),
        // Unreachable: `canonical_filter_name` has already refused every other name. An
        // undefined name compares against nothing, so it could never widen a result even
        // if that check were bypassed.
        _ => Vec::new(),
    }
}
