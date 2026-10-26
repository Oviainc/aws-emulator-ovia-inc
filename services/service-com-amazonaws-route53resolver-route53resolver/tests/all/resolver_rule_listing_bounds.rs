// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that pin the bounds `ListResolverRules` answers inside, and
//! that the page it answers is computed from the stored rules rather than from
//! the request.
//!
//! The rule these span: a paged Resolver list operation answers the stored
//! collection the query selects — in creation order, cut at a `MaxResults`
//! inside the range the model declares for that member and echoed back only
//! when the request named one — and refuses a request outside a declared or
//! documented bound instead of clamping it into a page the caller did not ask
//! for.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`], the rule fixtures the ones in
//! [`super::resolver_rule_deletion`], and the page projection the one in
//! [`super::resolver_rules`].

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    MAX_FILTERS, MAX_FILTER_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, ok, string_member};
use super::resolver_rule_deletion::{forward_rule, outbound_endpoint};
use super::resolver_rules::listed_ids;

/// The published bounds of `ListResolverRules$MaxResults`, from the range the
/// model puts on the member's target
/// (`smithy_query(query="shape",shape="MaxResults")`: `range 1..100`).
const MAX_RESULTS_BOUNDS: (i64, i64) = (1, 100);

/// Create `count` FORWARD rules through one endpoint and return their `Id`s in
/// creation order.
async fn seed(router: &MultiServiceRouter, prefix: &str, count: usize) -> Vec<String> {
    let endpoint = outbound_endpoint(router, &format!("ep-{prefix}")).await;
    let mut ids = Vec::new();
    for index in 0..count {
        ids.push(
            forward_rule(
                router,
                &format!("req-{prefix}-{index}"),
                &format!("{prefix}-{index}.example.com"),
                &endpoint,
            )
            .await,
        );
    }
    ids
}

/// `MaxResults` is modelled `range 1..100`, so a request outside it is refused
/// rather than clamped into a page the caller did not ask for, and a request
/// anywhere inside it is served.
///
/// The refusal is the boundary validator's, so only its HTTP status is
/// asserted: a handler-returned error and a boundary one differ in the error
/// code they render.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    seed(&router, "bounded", 3).await;

    for (case, max_results, accepted) in [
        ("far below the published minimum", -1, false),
        ("one below the published minimum", MAX_RESULTS_BOUNDS.0 - 1, false),
        ("the published minimum", MAX_RESULTS_BOUNDS.0, true),
        ("inside the range", 7, true),
        ("the published maximum", MAX_RESULTS_BOUNDS.1, true),
        ("one above the published maximum", MAX_RESULTS_BOUNDS.1 + 1, false),
        ("far above the published maximum", 10_000, false),
    ] {
        let (status, body) = call(
            &router,
            "ListResolverRules",
            json!({ "MaxResults": max_results }),
        )
        .await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: MaxResults={max_results} must answer HTTP {expected}, got {status}: {body}"
        );
    }
}

/// A request that names no `MaxResults` is answered with the whole collection in
/// one page — "If you don't specify a value for MaxResults, Resolver returns up
/// to 100 Resolver rules" — and the response echoes `MaxResults` only when the
/// request carried one, because the member is "the value that you specified for
/// MaxResults in the request".
#[tokio::test]
async fn max_results_is_echoed_only_when_the_request_names_one() {
    let router = emulator();
    let ids = seed(&router, "echo", 4).await;

    for (case, request, echoed) in [
        ("MaxResults omitted", json!({}), None),
        (
            "MaxResults at the published maximum",
            json!({ "MaxResults": MAX_RESULTS_BOUNDS.1 }),
            Some(MAX_RESULTS_BOUNDS.1),
        ),
        (
            "MaxResults larger than the collection",
            json!({ "MaxResults": 5 }),
            Some(5),
        ),
    ] {
        let listed = ok(&router, "ListResolverRules", request).await;
        assert_eq!(
            listed_ids(&listed),
            ids,
            "case {case:?}: the page must carry every stored rule in creation order, got {listed}"
        );
        assert_eq!(
            listed.get("MaxResults").and_then(Value::as_i64),
            echoed,
            "case {case:?}: expected MaxResults {echoed:?}, got {listed}"
        );
        assert_eq!(
            listed.get("NextToken"),
            None,
            "case {case:?}: a page that ends the collection owes no cursor, got {listed}"
        );
    }
}

/// One row of the filter-bound table: the `Filters` a request carries, and
/// either the member the refusal must name or `None` when the request is inside
/// every bound and must be served.
struct FilterBoundCase {
    case: &'static str,
    filters: Value,
    refused_field: Option<&'static str>,
}

/// A `Filters` list past the bound the service enforces is refused with the
/// `InvalidParameterException` the operation models, naming the member that is
/// out of bounds; a request at the bound is served.
///
/// The model constrains neither `Filters` nor `Filter$Values`, so the bounds are
/// the service's own ([`MAX_FILTERS`], [`MAX_FILTER_VALUES`]) — a filtered scan
/// costs the stored rules multiplied by the filter terms, so the request is
/// refused before the store is read rather than turning one call into unbounded
/// work.
#[tokio::test]
async fn a_filter_list_past_the_bound_is_refused() {
    let router = emulator();
    let ids = seed(&router, "capped", 2).await;

    let at_filter_cap: Vec<Value> = (0..MAX_FILTERS)
        .map(|_| json!({ "Name": "Type", "Values": ["FORWARD"] }))
        .collect();
    let mut past_filter_cap = at_filter_cap.clone();
    past_filter_cap.push(json!({ "Name": "Type", "Values": ["FORWARD"] }));

    let values_at_cap: Vec<String> = (0..MAX_FILTER_VALUES)
        .map(|index| format!("req-capped-{index}"))
        .collect();
    let mut values_past_cap = values_at_cap.clone();
    values_past_cap.push("req-capped-one-too-many".to_string());

    let cases = vec![
        FilterBoundCase {
            case: "as many filters as the bound allows",
            filters: json!(at_filter_cap),
            refused_field: None,
        },
        FilterBoundCase {
            case: "one filter past the bound",
            filters: json!(past_filter_cap),
            refused_field: Some("Filters"),
        },
        FilterBoundCase {
            case: "as many filter values as the bound allows, in one filter",
            filters: json!([{ "Name": "CreatorRequestId", "Values": values_at_cap }]),
            refused_field: None,
        },
        FilterBoundCase {
            case: "one filter value past the bound",
            filters: json!([{ "Name": "CreatorRequestId", "Values": values_past_cap }]),
            refused_field: Some("Values"),
        },
        FilterBoundCase {
            case: "the values bound counts every filter's values together",
            filters: json!([
                { "Name": "CreatorRequestId", "Values": vec!["req-capped-0"; MAX_FILTER_VALUES] },
                { "Name": "Type", "Values": ["FORWARD"] },
            ]),
            refused_field: Some("Values"),
        },
    ];

    for FilterBoundCase {
        case,
        filters,
        refused_field,
    } in cases
    {
        let (status, body) = call(
            &router,
            "ListResolverRules",
            json!({ "Filters": filters }),
        )
        .await;
        match refused_field {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a request inside every bound must be served, got {status}: {body}"
                );
                // Every seeded rule is a FORWARD rule created under a
                // `req-capped-*` id, so each served query selects them all.
                assert_eq!(
                    listed_ids(&body),
                    ids,
                    "case {case:?}: the selected rules are wrong, got {body}"
                );
            }
            Some(field) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: a request past the bound must answer HTTP 400, got {status}: {body}"
                );
                assert!(
                    error_type(&body).contains("InvalidParameterException"),
                    "case {case:?}: expected InvalidParameterException, got {body}"
                );
                assert_eq!(
                    string_member(&body, "FieldName"),
                    field,
                    "case {case:?}: expected FieldName {field:?}, got {body}"
                );
            }
        }
    }
}

/// The page is computed from the store, so it follows every write to the rule
/// collection: a rule created later is appended, and a rule deleted out of state
/// leaves the listing while the rules around it keep their order.
#[tokio::test]
async fn the_listing_follows_the_stored_collection() {
    let router = emulator();
    let endpoint = outbound_endpoint(&router, "ep-follows").await;
    let mut expected = Vec::new();
    for index in 0..3 {
        expected.push(
            forward_rule(
                &router,
                &format!("req-follows-{index}"),
                &format!("follows-{index}.example.com"),
                &endpoint,
            )
            .await,
        );
    }

    let listed = ok(&router, "ListResolverRules", json!({})).await;
    assert_eq!(
        listed_ids(&listed),
        expected,
        "case \"the seeded collection\": every stored rule must be listed, got {listed}"
    );

    // A rule created after the first listing is appended, not reordered.
    expected.push(
        forward_rule(
            &router,
            "req-follows-appended",
            "appended.example.com",
            &endpoint,
        )
        .await,
    );
    let listed = ok(&router, "ListResolverRules", json!({})).await;
    assert_eq!(
        listed_ids(&listed),
        expected,
        "case \"a rule created later\": it must be appended in creation order, got {listed}"
    );

    // A rule taken out of state is gone from the listing, and from the page the
    // filter that used to select it answers.
    let removed = expected.remove(1);
    ok(
        &router,
        "DeleteResolverRule",
        json!({ "ResolverRuleId": removed.clone() }),
    )
    .await;
    let listed = ok(&router, "ListResolverRules", json!({})).await;
    assert_eq!(
        listed_ids(&listed),
        expected,
        "case \"a deleted rule\": it must leave the listing, got {listed}"
    );
    let filtered = ok(
        &router,
        "ListResolverRules",
        json!({ "Filters": [{ "Name": "CreatorRequestId", "Values": ["req-follows-1"] }] }),
    )
    .await;
    assert_eq!(
        listed_ids(&filtered),
        Vec::<String>::new(),
        "case \"a deleted rule, filtered for\": nothing must select it, got {filtered}"
    );
}
