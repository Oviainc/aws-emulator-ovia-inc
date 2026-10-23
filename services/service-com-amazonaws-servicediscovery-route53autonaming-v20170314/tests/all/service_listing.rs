// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListServices`.
//!
//! The rule under test: a paginated listing reports the stored services in one stable
//! order, serving each exactly once across any page size, handing back a `NextToken`
//! only while records remain beyond the page it scanned, filtering the SCANNED page by
//! every filter the model documents, and refusing a filter or a token the model does
//! not allow with the modelled `InvalidInput`.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, the same path the emulator
//! serves in production, reusing the harness in `cloud_map_core.rs`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::cloud_map_core::{emulator, failure, namespace, ok, service};

/// The service ids one `ListServices` response reports, in the order served.
fn listed_ids(reply: &Value) -> Vec<String> {
    reply["Services"]
        .as_array()
        .unwrap_or_else(|| panic!("Services is a list, got {reply}"))
        .iter()
        .map(|summary| {
            summary["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("every summary names its service, got {summary}"))
                .to_string()
        })
        .collect()
}

/// Page `ListServices` to exhaustion, returning one entry per page.
async fn pages(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    filters: Option<Value>,
) -> Vec<Vec<String>> {
    let mut token: Option<String> = None;
    let mut served = Vec::new();
    loop {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(filters) = filters.clone() {
            body["Filters"] = filters;
        }
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let reply = ok(router, "ListServices", body).await;
        served.push(listed_ids(&reply));
        token = reply["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return served;
        }
        assert!(
            served.len() <= 64,
            "paging must terminate, got {} pages for MaxResults {max_results:?}",
            served.len()
        );
    }
}

/// The ARN `GetNamespace` reports for a stored namespace.
async fn namespace_arn(router: &MultiServiceRouter, namespace_id: &str) -> String {
    let read = ok(router, "GetNamespace", json!({ "Id": namespace_id })).await;
    read["Namespace"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetNamespace reports the namespace's Arn, got {read}"))
        .to_string()
}

/// A `ListServices` listing serves every stored service exactly once, in one order, for
/// ANY page size — and hands back a `NextToken` only while services remain.
#[tokio::test]
async fn paging_serves_every_service_exactly_once_in_a_stable_order() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "page-ns").await;
    let total: usize = 7;
    let mut created = BTreeSet::new();
    for index in 0..total {
        created.insert(service(&router, &namespace_id, &format!("page-svc-{index}")).await);
    }

    // The single-page listing is the order every paged listing must reproduce.
    let whole = listed_ids(&ok(&router, "ListServices", json!({})).await);
    assert_eq!(
        whole.iter().cloned().collect::<BTreeSet<_>>(),
        created,
        "one page of up to the documented 100 services holds all {total}, got {whole:?}"
    );

    // The rows differ along the page size: below, at and above the stored count, plus an
    // absent MaxResults (the documented default of up to 100).
    for max_results in [Some(1), Some(2), Some(3), Some(6), Some(7), Some(8), None] {
        let served = pages(&router, max_results, None).await;
        let flattened: Vec<String> = served.iter().flatten().cloned().collect();
        assert_eq!(
            flattened, whole,
            "case MaxResults {max_results:?}: paging must serve the same services in the same \
             order as a single page, got {flattened:?}"
        );
        assert_eq!(
            flattened.iter().cloned().collect::<BTreeSet<_>>().len(),
            flattened.len(),
            "case MaxResults {max_results:?}: no service may be served twice, got {flattened:?}"
        );

        let size = usize::try_from(max_results.unwrap_or(100)).expect("a positive page size");
        let expected_pages = total.div_ceil(size).max(1);
        assert_eq!(
            served.len(),
            expected_pages,
            "case MaxResults {max_results:?}: {total} services fill {expected_pages} pages, got \
             {} pages: {served:?}",
            served.len()
        );
        for (index, page) in served.iter().enumerate() {
            let expected = if index + 1 == served.len() {
                total - index * size
            } else {
                size
            };
            assert_eq!(
                page.len(),
                expected,
                "case MaxResults {max_results:?}: page {index} must hold {expected} services, \
                 got {page:?}"
            );
        }
    }
}

/// The `NextToken` a page hands back is the only one that resumes it: resuming twice
/// with the same token serves the same page, and listing changes nothing a later caller
/// sees.
#[tokio::test]
async fn a_page_token_resumes_the_same_place_and_a_listing_changes_nothing() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "token-ns").await;
    for index in 0..4 {
        service(&router, &namespace_id, &format!("token-svc-{index}")).await;
    }

    let first = ok(&router, "ListServices", json!({ "MaxResults": 2 })).await;
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a truncated page carries a NextToken, got {first}"))
        .to_string();

    // Reading is idempotent: the same request twice answers identically, and the resumed
    // page does not move between the two reads either.
    let repeat = ok(&router, "ListServices", json!({ "MaxResults": 2 })).await;
    assert_eq!(
        first, repeat,
        "case first page: listing twice must answer identically, got {repeat}"
    );
    let resumed = ok(
        &router,
        "ListServices",
        json!({ "MaxResults": 2, "NextToken": token }),
    )
    .await;
    let resumed_again = ok(
        &router,
        "ListServices",
        json!({ "MaxResults": 2, "NextToken": token }),
    )
    .await;
    assert_eq!(
        resumed, resumed_again,
        "case resumed page: the same token must serve the same page, got {resumed_again}"
    );
    assert_eq!(
        resumed["NextToken"],
        Value::Null,
        "case resumed page: 4 services in pages of 2 end at the second page, got {resumed}"
    );

    // The modelled NextToken length is 0..4096, so the empty token is legal and starts
    // at the beginning.
    let from_empty = ok(
        &router,
        "ListServices",
        json!({ "MaxResults": 2, "NextToken": "" }),
    )
    .await;
    assert_eq!(
        from_empty, first,
        "case empty NextToken: an empty token starts at the beginning, got {from_empty}"
    );

    // Listing is a read: the services it reported are all still there afterwards.
    let after = listed_ids(&ok(&router, "ListServices", json!({})).await);
    assert_eq!(
        after.len(),
        4,
        "a listing must not consume or hide a record, got {after:?}"
    );
}

/// Each documented filter name selects on the member `ServiceFilter$Values` names for
/// it, and several filters must all match.
#[tokio::test]
async fn every_documented_filter_selects_the_services_it_names() {
    let router = emulator();
    let (_, shop_ns) = namespace(&router, "shop.internal").await;
    let (_, warehouse_ns) = namespace(&router, "warehouse.internal").await;
    let shop_arn = namespace_arn(&router, &shop_ns).await;
    let warehouse_arn = namespace_arn(&router, &warehouse_ns).await;

    let carts = service(&router, &shop_ns, "carts").await;
    let checkout = service(&router, &shop_ns, "checkout").await;
    let picking = service(&router, &warehouse_ns, "picking").await;

    let shop: BTreeSet<String> = [carts.clone(), checkout.clone()].into_iter().collect();
    let warehouse: BTreeSet<String> = [picking.clone()].into_iter().collect();
    let everything: BTreeSet<String> = shop.union(&warehouse).cloned().collect();
    let none: BTreeSet<String> = BTreeSet::new();

    // Rows differ along the filter name, the form of the value (namespace id or ARN),
    // the value matched and the number of filters combined.
    let cases: Vec<(&str, Value, BTreeSet<String>)> = vec![
        (
            "NAMESPACE_ID EQ a namespace id",
            json!([{ "Name": "NAMESPACE_ID", "Values": [shop_ns], "Condition": "EQ" }]),
            shop.clone(),
        ),
        (
            "NAMESPACE_ID with the default condition",
            json!([{ "Name": "NAMESPACE_ID", "Values": [warehouse_ns] }]),
            warehouse.clone(),
        ),
        (
            "NAMESPACE_ID naming the namespace by ARN",
            json!([{ "Name": "NAMESPACE_ID", "Values": [shop_arn] }]),
            shop.clone(),
        ),
        (
            "NAMESPACE_ID naming a namespace nothing created",
            json!([{ "Name": "NAMESPACE_ID", "Values": ["ns-0000000000000000"] }]),
            none.clone(),
        ),
        (
            "RESOURCE_OWNER EQ SELF matches every service this account created",
            json!([{ "Name": "RESOURCE_OWNER", "Values": ["SELF"] }]),
            everything.clone(),
        ),
        (
            "RESOURCE_OWNER EQ OTHER_ACCOUNTS matches nothing shared in",
            json!([{ "Name": "RESOURCE_OWNER", "Values": ["OTHER_ACCOUNTS"] }]),
            none.clone(),
        ),
        (
            "two filters a service must both satisfy",
            json!([
                { "Name": "NAMESPACE_ID", "Values": [warehouse_arn] },
                { "Name": "RESOURCE_OWNER", "Values": ["SELF"] },
            ]),
            warehouse.clone(),
        ),
        (
            "two filters no service satisfies together",
            json!([
                { "Name": "NAMESPACE_ID", "Values": [shop_ns] },
                { "Name": "RESOURCE_OWNER", "Values": ["OTHER_ACCOUNTS"] },
            ]),
            none.clone(),
        ),
        ("no filters at all", json!([]), everything.clone()),
    ];

    for (case, filters, expected) in cases {
        let reply = ok(&router, "ListServices", json!({ "Filters": filters.clone() })).await;
        let got: BTreeSet<String> = listed_ids(&reply).into_iter().collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );

        // The same filter applied one service per page selects the same set, since
        // `MaxResults` bounds the services SCANNED, not the ones that match.
        let paged: BTreeSet<String> = pages(&router, Some(1), Some(filters))
            .await
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(
            paged, expected,
            "case {case:?} paged one at a time: expected {expected:?}, got {paged:?}"
        );
    }
}

/// A filter the documentation does not allow, and a `NextToken` this service never
/// issued, are refused with the modelled `InvalidInput` (HTTP 400) — never with an empty
/// page and never with a 501.
#[tokio::test]
async fn an_unusable_filter_or_foreign_token_is_refused_as_invalid_input() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "refusal-ns").await;
    service(&router, &namespace_id, "refusal-svc").await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "a filter name outside ServiceFilterName",
            json!({ "Filters": [{ "Name": "NAME", "Values": ["refusal-svc"] }] }),
        ),
        (
            "a filter name ListNamespaces allows but a service filter does not",
            json!({ "Filters": [{ "Name": "TYPE", "Values": ["HTTP"] }] }),
        ),
        (
            "the BEGINS_WITH condition, which a service filter does not support",
            json!({
                "Filters": [{
                    "Name": "NAMESPACE_ID",
                    "Values": [namespace_id],
                    "Condition": "BEGINS_WITH",
                }]
            }),
        ),
        (
            "the IN condition, which a service filter does not support",
            json!({
                "Filters": [{
                    "Name": "RESOURCE_OWNER",
                    "Values": ["SELF"],
                    "Condition": "IN",
                }]
            }),
        ),
        (
            "the BETWEEN condition, which a service filter does not support",
            json!({
                "Filters": [{
                    "Name": "RESOURCE_OWNER",
                    "Values": ["SELF"],
                    "Condition": "BETWEEN",
                }]
            }),
        ),
        (
            "two values where EQ allows only one",
            json!({
                "Filters": [{ "Name": "NAMESPACE_ID", "Values": [namespace_id, "ns-other"] }]
            }),
        ),
        (
            "no value at all",
            json!({ "Filters": [{ "Name": "NAMESPACE_ID", "Values": [] }] }),
        ),
        (
            "a RESOURCE_OWNER value that names no owner",
            json!({ "Filters": [{ "Name": "RESOURCE_OWNER", "Values": ["EVERYONE"] }] }),
        ),
        (
            "a NextToken this service never issued",
            json!({ "NextToken": "not-a-token" }),
        ),
        (
            "a NextToken minted for another paginated read's scope",
            json!({ "NextToken": hex::encode("ListNamespaces\u{1f}ns-1") }),
        ),
        (
            "a NextToken whose bytes carry no scope at all",
            json!({ "NextToken": hex::encode("services:srv-0") }),
        ),
        (
            "a NextToken naming a service id with no token envelope",
            json!({ "NextToken": "srv-0123456789abcdef" }),
        ),
    ];

    for (case, body) in cases {
        let (status, code) = failure(&router, "ListServices", body.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInput"),
            "case {case:?}: expected HTTP 400 InvalidInput, got {status} / {code:?} for {body}"
        );
    }
}

/// A value the MODEL itself rejects — before any handler runs — is refused with the
/// error code this service's model declares for invalid input, `InvalidInput`, and not
/// with the framework's generic `ValidationException`: Cloud Map models no such shape.
#[tokio::test]
async fn a_modelled_constraint_is_refused_as_invalid_input_too() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "boundary-ns").await;
    service(&router, &namespace_id, "boundary-svc").await;

    // Rows differ along the constraint tripped: each published bound of `MaxResults`
    // (@range(1..100)) and one past it on either side, and the modelled `NextToken`
    // length (0..4096) overrun.
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "MaxResults at the lower bound",
            json!({ "MaxResults": 1 }),
            true,
        ),
        (
            "MaxResults one below the lower bound",
            json!({ "MaxResults": 0 }),
            false,
        ),
        (
            "MaxResults at the upper bound",
            json!({ "MaxResults": 100 }),
            true,
        ),
        (
            "MaxResults one above the upper bound",
            json!({ "MaxResults": 101 }),
            false,
        ),
        (
            "a NextToken one character past the modelled length",
            json!({ "NextToken": "a".repeat(4097) }),
            false,
        ),
        (
            "a NextToken at the modelled length, so only its contents are wrong",
            json!({ "NextToken": "a".repeat(4096) }),
            false,
        ),
    ];

    for (case, body, accepted) in cases {
        if accepted {
            let reply = ok(&router, "ListServices", body.clone()).await;
            assert_eq!(
                listed_ids(&reply).len(),
                1,
                "case {case:?}: a value inside the modelled range is served, got {reply}"
            );
            continue;
        }
        let (status, code) = failure(&router, "ListServices", body.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInput"),
            "case {case:?}: expected HTTP 400 InvalidInput, got {status} / {code:?} for {body}"
        );
    }
}

/// A `ServiceSummary` is the stored record, so it agrees member for member with what
/// `GetService` reports — including the derived `InstanceCount` — and carries none of
/// the members the shape does not declare.
#[tokio::test]
async fn a_summary_agrees_with_get_service_member_for_member() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "summary-ns").await;
    let created = ok(
        &router,
        "CreateService",
        json!({
            "Name": "summary-svc",
            "NamespaceId": namespace_id,
            "Description": "the service a summary is read back from",
            "HealthCheckCustomConfig": { "FailureThreshold": 1 },
            "CreatorRequestId": "summary-token",
        }),
    )
    .await;
    let service_id = created["Service"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateService returns the service it stored, got {created}"))
        .to_string();
    ok(
        &router,
        "RegisterInstance",
        json!({
            "ServiceId": service_id,
            "InstanceId": "i-summary",
            "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.1" },
        }),
    )
    .await;

    let listed = ok(&router, "ListServices", json!({})).await;
    let summary = listed["Services"]
        .as_array()
        .unwrap_or_else(|| panic!("Services is a list, got {listed}"))
        .iter()
        .find(|summary| summary["Id"] == json!(service_id))
        .unwrap_or_else(|| panic!("the listing holds the service just created: {listed}"))
        .clone();
    let read = ok(&router, "GetService", json!({ "Id": service_id })).await;
    let service = &read["Service"];

    // Every member `ServiceSummary` declares
    // (smithy_query query="shape" shape="ServiceSummary").
    for member in [
        "Id",
        "Arn",
        "ResourceOwner",
        "Name",
        "Type",
        "Description",
        "InstanceCount",
        "DnsConfig",
        "HealthCheckConfig",
        "HealthCheckCustomConfig",
        "CreateDate",
        "CreatedByAccount",
    ] {
        assert_eq!(
            summary[member], service[member],
            "member {member:?}: a summary must report what GetService reports, got {summary} \
             against {service}"
        );
    }
    assert_eq!(
        summary["InstanceCount"],
        json!(1),
        "InstanceCount is derived from the registered instances, got {summary}"
    );
    // The members `Service` carries and `ServiceSummary` does not.
    for absent in ["NamespaceId", "CreatorRequestId"] {
        assert_eq!(
            summary[absent],
            Value::Null,
            "member {absent:?} is not a ServiceSummary member, got {summary}"
        );
    }
}
