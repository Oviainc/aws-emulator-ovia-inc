// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what this service answers a request that binds no value for a
//! `@required` `@httpQuery` member of the operation the model binds to its method and path.
//!
//! The framework's REST router rejects a candidate route whose `@required` query bindings
//! are not on the request, which is the rule that tells S3-class operations apart on ONE
//! method + URI template. This service overloads no route, so for such a request the rule
//! only misfires: nothing would match, and the request would be answered "unknown
//! operation" (HTTP 501) instead of reaching the one operation the model binds to it. The
//! rule pinned here is that method + path alone decides WHICH operation answers, so every
//! such request is answered by that operation:
//!
//! 1. a single-valued member that is absent is the operation's own refusal for an absent
//!    `@required` member — the service's `ValidationException` (HTTP 400), naming that
//!    member — whichever route carries it;
//! 2. a required LIST member that is absent is the EMPTY list, because a query-bound list
//!    spells "no elements" as no values at all, so the call is SERVED;
//! 3. a method + path the model binds no route for still answers the routing failure, so
//!    resolving the routes above takes nothing away from the model's routing table.
//!
//! The table in rule 1 spans every route of this service that binds such a member, the two
//! methods and the three URI templates involved, and — for each — a request with no query
//! string at all beside one that binds the route's OTHER query members but not the absent
//! one, since what matters is the member's presence and not whether a query string was
//! sent.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch` with a signed
//! restJson1 request), and each served call is read back through an implemented reader so
//! the answer is never the only evidence.

use serde_json::json;
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    call, create, emulator, experiment_arn, message, percent_encoded, put_metrics_destination,
    Reply, REGION,
};

/// The status and the wire code a request's own values are refused with on an app-monitor
/// call: the `ValidationException` this service's model declares with `@httpError(400)`.
/// Source: `smithy_query(query="operation", operation="DeleteRumMetricsDestination")`.
const BAD_REQUEST: u16 = 400;
const VALIDATION_CODE: &str = "ValidationException";

/// The status a request that names no operation at all is answered with: the REST
/// protocols' routing failure, `501 NotImplemented`.
const NOT_IMPLEMENTED: u16 = 501;

/// The whole message a refusal of ONE absent member carries: AWS's envelope over the
/// clause the framework states a required member's absence in, with the member at the
/// dotted lowerCamel path a violation reports it at.
fn absent_member_message(member: &str) -> String {
    format!(
        "1 validation error detected: Value null at '{member}' failed to satisfy constraint: \
         Member must not be null"
    )
}

/// What a request is expected to be answered with.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// Refused on the absent single-valued member, at the dotted path a violation reports
    /// it at.
    Refused(&'static str),
    /// Served: the status the operation answers for the call.
    Served(u16),
}

/// One request, with `{monitor}` standing for the app monitor each case creates, `{arn}`
/// for that monitor's percent-encoded ARN and `{experiment}` for a percent-encoded
/// Evidently experiment ARN.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    method: &'static str,
    path_and_query: &'static str,
    expected: Expected,
}

/// Every route of this service that binds a `@required` `@httpQuery` member, with a request
/// that binds no value for one of those members.
///
/// Source: `smithy_query(query="operation", operation="UntagResource,
/// BatchDeleteRumMetricDefinitions,BatchGetRumMetricDefinitions,DeleteRumMetricsDestination")`
/// — `UntagResource$TagKeys` (`tagKeys`, a `TagKeyList`),
/// `BatchDeleteRumMetricDefinitions$Destination` (`destination`) and
/// `$MetricDefinitionIds` (`metricDefinitionIds`, a list),
/// `BatchGetRumMetricDefinitions$Destination` and
/// `DeleteRumMetricsDestination$Destination`. No other operation of this service binds a
/// `@required` member to the query string.
const ROWS: &[Row] = &[
    // `Destination` is a single `MetricDestination` value, so its absence is a refusal on
    // every one of the three routes that bind it — with no query string at all, and with
    // the route's other query members bound but not that one.
    Row {
        case: "BatchDeleteRumMetricDefinitions with no query string",
        method: "DELETE",
        path_and_query: "/rummetrics/{monitor}/metrics",
        expected: Expected::Refused("destination"),
    },
    Row {
        case: "BatchDeleteRumMetricDefinitions binding only the ids",
        method: "DELETE",
        path_and_query: "/rummetrics/{monitor}/metrics?metricDefinitionIds=aaaaaaaaaaaaaaaaaaaa",
        expected: Expected::Refused("destination"),
    },
    Row {
        case: "BatchGetRumMetricDefinitions with no query string",
        method: "GET",
        path_and_query: "/rummetrics/{monitor}/metrics",
        expected: Expected::Refused("destination"),
    },
    Row {
        case: "BatchGetRumMetricDefinitions binding only the page size",
        method: "GET",
        path_and_query: "/rummetrics/{monitor}/metrics?maxResults=10",
        expected: Expected::Refused("destination"),
    },
    Row {
        case: "DeleteRumMetricsDestination with no query string",
        method: "DELETE",
        path_and_query: "/rummetrics/{monitor}/metricsdestination",
        expected: Expected::Refused("destination"),
    },
    Row {
        case: "DeleteRumMetricsDestination binding only the destination ARN",
        method: "DELETE",
        path_and_query: "/rummetrics/{monitor}/metricsdestination?destinationArn={experiment}",
        expected: Expected::Refused("destination"),
    },
    // A required LIST member that is absent is the empty list the wire spells that way, so
    // these are served: `UntagResource` removes no key (204, the status its empty output
    // answers) and `BatchDeleteRumMetricDefinitions` deletes no definition (200).
    Row {
        case: "UntagResource binding no key",
        method: "DELETE",
        path_and_query: "/tags/{arn}",
        expected: Expected::Served(204),
    },
    Row {
        case: "BatchDeleteRumMetricDefinitions binding no id",
        method: "DELETE",
        path_and_query: "/rummetrics/{monitor}/metrics?destination=CloudWatch",
        expected: Expected::Served(200),
    },
];

/// A method + path this service's model binds NO route for, each one a near miss of a row
/// above: a segment short, a segment long, and a method the model does not bind on that
/// template.
const UNBOUND: &[(&str, &str, &str)] = &[
    ("a segment short of the tag route", "DELETE", "/tags"),
    (
        "a segment past the tag route",
        "DELETE",
        "/tags/{arn}/extra",
    ),
    (
        "a segment short of the metrics route",
        "DELETE",
        "/rummetrics/{monitor}",
    ),
    (
        "a segment past the metrics route",
        "DELETE",
        "/rummetrics/{monitor}/metrics/extra",
    ),
    (
        "a segment past the metrics-destination route",
        "DELETE",
        "/rummetrics/{monitor}/metricsdestination/extra",
    ),
    (
        "a sub-resource the model does not publish",
        "DELETE",
        "/rummetrics/{monitor}/nosuchsubresource",
    ),
    (
        "a method the model does not bind on the metrics template",
        "PUT",
        "/rummetrics/{monitor}/metrics",
    ),
];

/// The ARN of the app monitor named `name`, as `crate::state::app_monitor_arn` derives it
/// — the ARN the tagging operations address a monitor by.
fn arn_of(name: &str) -> String {
    engine::emulator_arn("rum", REGION, &format!("appmonitor/{name}"))
}

/// One case's monitor: a created app monitor with a CloudWatch extended-metrics
/// destination, which is what a metrics call addresses, and its ARN.
async fn monitor(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    create(
        router,
        case,
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;
    put_metrics_destination(router, case, name, "CloudWatch", None).await;
    arn_of(name)
}

/// A row's path with the placeholders resolved.
fn resolved(template: &str, name: &str, arn: &str) -> String {
    template
        .replace("{monitor}", name)
        .replace("{arn}", &percent_encoded(arn))
        .replace(
            "{experiment}",
            &percent_encoded(&experiment_arn("qbrm-experiment")),
        )
}

/// Assert that `reply` is the 400 the operation refuses an absent `member` with.
fn assert_refused(case: &str, reply: &Reply, member: &str) {
    assert_eq!(
        reply.status,
        BAD_REQUEST,
        "case {case:?}: expected HTTP {BAD_REQUEST} for the absent member {member:?}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code(case),
        VALIDATION_CODE,
        "case {case:?}: expected {VALIDATION_CODE}, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json(case);
    let stated = message(&body).unwrap_or_default();
    let expected = absent_member_message(member);
    assert_eq!(
        stated, expected,
        "case {case:?}: expected the message {expected:?}, got {stated:?}"
    );
}

#[tokio::test]
async fn an_absent_required_query_member_is_answered_by_the_operation_the_model_binds() {
    let (router, _engine) = emulator();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("qbrm-{index:02}");
        let arn = monitor(&router, case, &name).await;
        let path_and_query = resolved(row.path_and_query, &name, &arn);
        let reply = call(&router, row.method, &path_and_query, None).await;
        match row.expected {
            Expected::Refused(member) => assert_refused(case, &reply, member),
            Expected::Served(status) => assert_eq!(
                reply.status,
                status,
                "case {case:?}: expected HTTP {status}, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            ),
        }
        assert_ne!(
            reply.status, NOT_IMPLEMENTED,
            "case {case:?}: {} {path_and_query} must reach the operation the model binds \
             to it, not the routing failure",
            row.method
        );
    }
}

#[tokio::test]
async fn a_method_and_path_the_model_binds_no_route_for_keeps_the_routing_failure() {
    let (router, _engine) = emulator();
    let name = "qbrm-unbound";
    let arn = monitor(&router, "the near misses", name).await;
    for (case, method, template) in UNBOUND {
        let path_and_query = resolved(template, name, &arn);
        let reply = call(&router, method, &path_and_query, None).await;
        assert_eq!(
            reply.status,
            NOT_IMPLEMENTED,
            "case {case:?}: {method} {path_and_query} names no modelled route, so it must \
             answer HTTP {NOT_IMPLEMENTED}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}

#[tokio::test]
async fn a_complete_request_on_each_of_those_routes_still_reaches_its_operation() {
    // The routes resolved by method and path are the SAME routes the generated table
    // resolves when the required member is bound, so a complete request still reaches its
    // own operation and its answer is read back through an implemented reader.
    let (router, _engine) = emulator();
    let case = "the complete requests";
    let name = "qbrm-complete";
    monitor(&router, case, name).await;

    let page = call(
        &router,
        "GET",
        &format!("/rummetrics/{name}/metrics?destination=CloudWatch"),
        None,
    )
    .await;
    assert_eq!(
        page.status,
        200,
        "case {case:?}: expected BatchGetRumMetricDefinitions to answer 200, got {} {:?}",
        page.status,
        String::from_utf8_lossy(&page.body)
    );

    let removed = call(
        &router,
        "DELETE",
        &format!("/rummetrics/{name}/metricsdestination?destination=CloudWatch"),
        None,
    )
    .await;
    assert_eq!(
        removed.status,
        200,
        "case {case:?}: expected DeleteRumMetricsDestination to answer 200, got {} {:?}",
        removed.status,
        String::from_utf8_lossy(&removed.body)
    );

    let listed = call(
        &router,
        "GET",
        &format!("/rummetrics/{name}/metricsdestination"),
        None,
    )
    .await;
    assert_eq!(
        listed.status,
        200,
        "case {case:?}: expected ListRumMetricsDestinations to answer 200, got {} {:?}",
        listed.status,
        String::from_utf8_lossy(&listed.body)
    );
    let reported = listed.json(case)["Destinations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        reported.is_empty(),
        "case {case:?}: the removed destination must no longer be reported, got {reported:?}"
    );
}
