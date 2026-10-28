// SPDX-License-Identifier: CC0-1.0

//! The ONE page-size rule every paginated operation of this service applies,
//! driven end to end through each of those operations.
//!
//! The rule: a request's `MaxResults` is the page size verbatim up to the upper
//! bound of the range the service publishes, which is also the size a request
//! naming no `MaxResults` is served at, and a `MaxResults` below one asks for no
//! items at all and is refused with the modelled 400 naming the member and the
//! value that broke the floor. It is the SERVICE's rule, not one listing's, so
//! the table below spans the OPERATION: every operation that cuts a page runs
//! every row.
//!
//! `tests/all/list_page_size.rs` pins what a page CARRIES over a table of sizes
//! against filled collections; this suite pins that each paginated operation
//! reaches the same rule — the floor, its wording and the ceiling — whichever
//! operation a client asks.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    input_device_transfer_type, MAX_LIST_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::multiplex_lifecycle::{
    create_body, settings, TRANSPORT_STREAM_BITRATE, TRANSPORT_STREAM_ID,
};
use crate::wire::{call, error_code, error_message, medialive};

/// The only `ClusterType` the model declares
/// (`smithy_query(query="shape", shape="ClusterType")`: ON_PREMISES).
const ON_PREMISES: &str = "ON_PREMISES";

/// The resource a row's listing hangs off, when its URI carries one: the owner
/// this suite creates before driving that operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// A collection of the account itself, whose URI carries no owner id.
    Account,
    Channel,
    Cluster,
    Multiplex,
}

/// How a row builds its request URI from its owner's id (the empty string for
/// [`Owner::Account`]).
type Uri = fn(&str) -> String;

/// One row of the table: a paginated operation, the collection it reports and
/// what a request to it needs besides a page size.
struct Paginated {
    /// The operation the row drives, as the model names it.
    op: &'static str,
    /// The resource its URI hangs off.
    owner: Owner,
    /// Its URI, from `smithy_query(query="operation", operation="<Operation>")`.
    uri: Uri,
    /// The query parameters a request to it carries beside `maxResults` —
    /// a REQUIRED member bound to the query string, where it has one.
    extra: String,
    /// The member it reports its page in (the `jsonName` of the operation's
    /// output list member).
    member: &'static str,
}

/// Every paginated operation of this service, with the collection each one
/// reports: `smithy_query(query="operation", operation="<Operation>")` for the
/// method, the URI and the output member of each.
fn paginated_operations() -> Vec<Paginated> {
    vec![
        Paginated {
            op: "ListChannels",
            owner: Owner::Account,
            uri: |_| "/prod/channels".to_string(),
            extra: String::new(),
            member: "channels",
        },
        Paginated {
            op: "DescribeSchedule",
            owner: Owner::Channel,
            uri: |channel| format!("/prod/channels/{channel}/schedule"),
            extra: String::new(),
            member: "scheduleActions",
        },
        Paginated {
            op: "ListAlerts",
            owner: Owner::Channel,
            uri: |channel| format!("/prod/channels/{channel}/alerts"),
            extra: String::new(),
            member: "alerts",
        },
        Paginated {
            op: "ListInputs",
            owner: Owner::Account,
            uri: |_| "/prod/inputs".to_string(),
            extra: String::new(),
            member: "inputs",
        },
        Paginated {
            op: "ListInputSecurityGroups",
            owner: Owner::Account,
            uri: |_| "/prod/inputSecurityGroups".to_string(),
            extra: String::new(),
            member: "inputSecurityGroups",
        },
        Paginated {
            op: "ListInputDevices",
            owner: Owner::Account,
            uri: |_| "/prod/inputDevices".to_string(),
            extra: String::new(),
            member: "inputDevices",
        },
        Paginated {
            op: "ListInputDeviceTransfers",
            owner: Owner::Account,
            uri: |_| "/prod/inputDeviceTransfers".to_string(),
            // `TransferType` is REQUIRED and bound to the query string, so a
            // request this operation can serve at all names a direction — one
            // of the model's own (`InputDeviceTransferType`: OUTGOING |
            // INCOMING).
            extra: format!("transferType={}", input_device_transfer_type::OUTGOING),
            member: "inputDeviceTransfers",
        },
        Paginated {
            op: "ListMultiplexes",
            owner: Owner::Account,
            uri: |_| "/prod/multiplexes".to_string(),
            extra: String::new(),
            member: "multiplexes",
        },
        Paginated {
            op: "ListMultiplexPrograms",
            owner: Owner::Multiplex,
            uri: |multiplex| format!("/prod/multiplexes/{multiplex}/programs"),
            extra: String::new(),
            member: "multiplexPrograms",
        },
        Paginated {
            op: "ListMultiplexAlerts",
            owner: Owner::Multiplex,
            uri: |multiplex| format!("/prod/multiplexes/{multiplex}/alerts"),
            extra: String::new(),
            member: "alerts",
        },
        Paginated {
            op: "ListClusters",
            owner: Owner::Account,
            uri: |_| "/prod/clusters".to_string(),
            extra: String::new(),
            member: "clusters",
        },
        Paginated {
            op: "ListClusterAlerts",
            owner: Owner::Cluster,
            uri: |cluster| format!("/prod/clusters/{cluster}/alerts"),
            extra: String::new(),
            member: "alerts",
        },
        Paginated {
            op: "ListNodes",
            owner: Owner::Cluster,
            uri: |cluster| format!("/prod/clusters/{cluster}/nodes"),
            extra: String::new(),
            member: "nodes",
        },
        Paginated {
            op: "ListChannelPlacementGroups",
            owner: Owner::Cluster,
            uri: |cluster| format!("/prod/clusters/{cluster}/channelplacementgroups"),
            extra: String::new(),
            member: "channelPlacementGroups",
        },
        Paginated {
            op: "ListNetworks",
            owner: Owner::Account,
            uri: |_| "/prod/networks".to_string(),
            extra: String::new(),
            member: "networks",
        },
        Paginated {
            op: "ListSdiSources",
            owner: Owner::Account,
            uri: |_| "/prod/sdiSources".to_string(),
            extra: String::new(),
            member: "sdiSources",
        },
        Paginated {
            op: "ListOfferings",
            owner: Owner::Account,
            uri: |_| OFFERINGS.to_string(),
            extra: String::new(),
            member: "offerings",
        },
        Paginated {
            op: "ListReservations",
            owner: Owner::Account,
            uri: |_| "/prod/reservations".to_string(),
            extra: String::new(),
            member: "reservations",
        },
        Paginated {
            op: "ListSignalMaps",
            owner: Owner::Account,
            uri: |_| "/prod/signal-maps".to_string(),
            extra: String::new(),
            member: "signalMaps",
        },
        Paginated {
            op: "ListCloudWatchAlarmTemplates",
            owner: Owner::Account,
            uri: |_| "/prod/cloudwatch-alarm-templates".to_string(),
            extra: String::new(),
            member: "cloudWatchAlarmTemplates",
        },
        Paginated {
            op: "ListCloudWatchAlarmTemplateGroups",
            owner: Owner::Account,
            uri: |_| "/prod/cloudwatch-alarm-template-groups".to_string(),
            extra: String::new(),
            member: "cloudWatchAlarmTemplateGroups",
        },
        Paginated {
            op: "ListEventBridgeRuleTemplates",
            owner: Owner::Account,
            uri: |_| "/prod/eventbridge-rule-templates".to_string(),
            extra: String::new(),
            member: "eventBridgeRuleTemplates",
        },
        Paginated {
            op: "ListEventBridgeRuleTemplateGroups",
            owner: Owner::Account,
            uri: |_| "/prod/eventbridge-rule-template-groups".to_string(),
            extra: String::new(),
            member: "eventBridgeRuleTemplateGroups",
        },
    ]
}

/// The offering catalog, which this service serves without a create
/// (`smithy_query(query="operation", operation="ListOfferings")`: GET
/// /prod/offerings).
const OFFERINGS: &str = "/prod/offerings";

/// The owner a row's listing hangs off, created on `router`, answering its
/// minted id (the empty string for a collection of the account itself).
async fn owner_id(router: &MultiServiceRouter, case: &str, owner: Owner) -> String {
    let (uri, body, carrier) = match owner {
        Owner::Account => return String::new(),
        Owner::Channel => (
            "/prod/channels",
            json!({
                "name": "page-size-rule",
                "encoderSettings": crate::support::channel_encoder_settings(),
            }),
            "channel",
        ),
        Owner::Cluster => (
            "/prod/clusters",
            json!({ "name": "page-size-rule", "clusterType": ON_PREMISES }),
            "",
        ),
        Owner::Multiplex => (
            "/prod/multiplexes",
            create_body(
                "page-size-rule",
                "page-size-rule-token",
                settings(TRANSPORT_STREAM_BITRATE.0, TRANSPORT_STREAM_ID.0),
            ),
            "multiplex",
        ),
    };
    let (status, response) = call(router, "POST", uri, Some(body)).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: expected the {owner:?} this listing hangs off to be created, got \
         {status}: {response}"
    );
    let resource = if carrier.is_empty() {
        response.clone()
    } else {
        response[carrier].clone()
    };
    resource["id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the created {owner:?} to carry an id, got {response}")
        })
        .to_string()
}

/// The page a listing reported in `member`, as a list whatever its length: an
/// absent member and an empty list both mean "no entries" to a client, and which
/// of the two the restJson1 codec writes is the serializer's choice.
fn reported(case: &str, body: &Value, member: &str) -> Vec<Value> {
    match body.get(member) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(other) => panic!("case {case:?}: expected a {member} list, got {other}"),
    }
}

/// The URI one row drives, with `maxResults` set to `asked_for` when the row
/// asks for a size at all.
fn uri_of(row: &Paginated, owner: &str, asked_for: Option<i64>) -> String {
    let mut query: Vec<String> = Vec::new();
    if !row.extra.is_empty() {
        query.push(row.extra.clone());
    }
    if let Some(size) = asked_for {
        query.push(format!("maxResults={size}"));
    }
    let path = (row.uri)(owner);
    if query.is_empty() {
        path
    } else {
        format!("{path}?{}", query.join("&"))
    }
}

/// Rule: the page size is the SERVICE's, so every paginated operation reads its
/// `MaxResults` by the same rule — a size below one asks for no items and is
/// refused with the modelled 400 naming the member and that value, any other
/// size is the page size verbatim up to the published ceiling, and a request
/// naming no size is served at that ceiling.
///
/// The rows span the OPERATION — all 23 paginated operations of this service,
/// across collections whose stores, owners, identifiers and summaries are
/// unrelated — and, within each, the size asked for: below the floor (zero, one
/// below it and a largely negative one), at the floor, at the published ceiling,
/// past it, and no size at all.
#[tokio::test]
async fn one_page_size_rule_for_every_paginated_operation() {
    let ceiling = i64::try_from(MAX_LIST_PAGE_SIZE).expect("the page ceiling fits an i64");

    for row in paginated_operations() {
        // A router of its own per row, so each operation is driven against a
        // store only this row filled.
        let router = medialive();
        let owner = owner_id(&router, row.op, row.owner).await;

        // Below the floor: refused, with the one wording the shared rule states.
        for asked_for in [0, -1, -4321] {
            let case = format!("{} maxResults={asked_for}", row.op);
            let uri = uri_of(&row, &owner, Some(asked_for));
            let (status, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected GET {uri} to refuse a page size below the floor, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected the modelled client error, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("maxResults must be at least 1, got {asked_for}"),
                "case {case:?}: expected the shared page-size refusal wording, got {:?}",
                error_message(&body)
            );
            assert!(
                reported(&case, &body, row.member).is_empty(),
                "case {case:?}: expected a refusal to report no {} entries, got {body}",
                row.member
            );
        }

        // At the floor, at the ceiling, past it, and no size at all: served, and
        // the page is never longer than the size the rule cuts it at.
        for asked_for in [Some(1), Some(ceiling), Some(ceiling + 500), None] {
            let case = format!("{} maxResults={asked_for:?}", row.op);
            let uri = uri_of(&row, &owner, asked_for);
            let (status, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GET {uri} to be served, got {status}: {body}"
            );
            let cut_at = asked_for.unwrap_or(ceiling).min(ceiling);
            let page = reported(&case, &body, row.member);
            assert!(
                i64::try_from(page.len()).expect("a page length fits an i64") <= cut_at,
                "case {case:?}: expected at most {cut_at} {} entries in a page, got {}",
                row.member,
                page.len()
            );
            // A token is handed out only when an entry was left over after the
            // page, which it cannot be unless the page was cut at that size.
            if body.get("nextToken").and_then(Value::as_str).is_some() {
                assert_eq!(
                    i64::try_from(page.len()).expect("a page length fits an i64"),
                    cut_at,
                    "case {case:?}: expected a nextToken only beside a page cut at {cut_at}, got \
                     {} entries",
                    page.len()
                );
            }
        }
    }
}

/// Rule: the same shared page size, observed against a collection that is not
/// empty — the offering catalog this service serves without a create. A size
/// within the published range cuts the page exactly there and leaves a token for
/// what is left over; the published ceiling, every size past it and a request
/// naming no size at all are one and the same page, because the rule clamps to
/// that ceiling and serves an absent member at it.
#[tokio::test]
async fn the_shared_page_size_cuts_a_non_empty_collection_where_it_was_asked_to() {
    let router = medialive();
    let ceiling = i64::try_from(MAX_LIST_PAGE_SIZE).expect("the page ceiling fits an i64");

    /// One page of the catalog: its offerings and the token beside them, with
    /// the call asserted to have been served.
    async fn catalog_page(
        router: &MultiServiceRouter,
        case: &str,
        query: &str,
    ) -> (Vec<Value>, Option<String>) {
        let uri = format!("{OFFERINGS}{query}");
        let (status, body) = call(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GET {uri} to be served, got {status}: {body}"
        );
        let token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        (reported(case, &body, "offerings"), token)
    }

    // A size the catalog has more offerings than: the page is cut exactly
    // there, with a token for the offerings left over.
    for asked_for in [1, 2, 3, 17] {
        let case = format!("maxResults={asked_for}");
        let (page, token) = catalog_page(&router, &case, &format!("?maxResults={asked_for}")).await;
        assert_eq!(
            i64::try_from(page.len()).expect("a page length fits an i64"),
            asked_for,
            "case {case:?}: expected a page of {asked_for} offerings, got {}",
            page.len()
        );
        assert!(
            token.is_some(),
            "case {case:?}: expected a nextToken with offerings left over, got {token:?}"
        );
    }

    // The ceiling is where the rule clamps, and the size an absent member is
    // served at, so these three requests are one and the same page.
    let (at_ceiling, ceiling_token) =
        catalog_page(&router, "at the ceiling", &format!("?maxResults={ceiling}")).await;
    assert_eq!(
        i64::try_from(at_ceiling.len()).expect("a page length fits an i64"),
        ceiling,
        "case \"at the ceiling\": expected the catalog to fill a page of {ceiling}, got {}",
        at_ceiling.len()
    );
    for asked_for in [Some(ceiling + 1), Some(ceiling * 7), None] {
        let case = format!("maxResults={asked_for:?}");
        let query = asked_for.map_or_else(String::new, |size| format!("?maxResults={size}"));
        let (page, token) = catalog_page(&router, &case, &query).await;
        assert_eq!(
            page, at_ceiling,
            "case {case:?}: expected the same page the ceiling is served at"
        );
        assert_eq!(
            token, ceiling_token,
            "case {case:?}: expected the same continuation the ceiling is served with"
        );
    }
}
