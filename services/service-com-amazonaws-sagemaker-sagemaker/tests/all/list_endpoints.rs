// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `ListEndpoints`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.ListEndpoints`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness, and the
//! model/endpoint-config helpers these tests build on, are reused from
//! `inference_deployment_lifecycle`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: every
//! documented `SortBy`/`SortOrder` pair, every filter member, every member of the
//! modelled `EndpointStatus` catalog, and every page size from one entry to past the
//! published `MaxResults` bound.

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ops::types::ENDPOINT_STATUS_VALUES;
use service_com_amazonaws_sagemaker_sagemaker::state::MAX_PAGE_SIZE;

use crate::inference_deployment_lifecycle::{
    error_code, one_variant_config, primary_container, str_field, Emulator, ACCOUNT,
    CREATE_SPACING, PAST_IN_SERVICE_DUE,
};

/// The model and endpoint configuration every seeded endpoint deploys. One
/// configuration can back many endpoints, so the listing's rows differ only along the
/// axes under test.
const MODEL: &str = "list-endpoints-model";
/// See [`MODEL`].
const CONFIG: &str = "list-endpoints-config";

/// The endpoint names the sort and filter tables range over, created in this order.
/// Their names are deliberately NOT in creation order, so a `Name` sort and a
/// `CreationTime` sort cannot accidentally agree.
const SEEDED: [&str; 4] = ["alpha-one", "beta-two", "demo-three", "demo-four"];

/// `MaxResults`' documented default for `ListEndpoints` ("This value defaults to 10.").
const DOCUMENTED_DEFAULT_MAX_RESULTS: usize = 10;

/// The service's own server-side page ceiling, read from the service rather than
/// restated, so changing it does not fail this test for an unrelated reason.
const PAGE_CEILING: usize = MAX_PAGE_SIZE as usize;

/// Create the model and endpoint configuration every endpoint below deploys.
async fn seed_prerequisites(emulator: &Emulator) {
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": MODEL, "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;
    emulator
        .ok("CreateEndpointConfig", &one_variant_config(CONFIG, MODEL))
        .await;
}

/// Create one endpoint per name, spaced so `CreationTime` ordering is unambiguous.
async fn create_endpoints(emulator: &Emulator, names: &[String]) {
    for name in names {
        emulator
            .ok(
                "CreateEndpoint",
                &json!({ "EndpointName": name, "EndpointConfigName": CONFIG }),
            )
            .await;
        tokio::time::sleep(CREATE_SPACING).await;
    }
}

/// One endpoint's stored `CreationTime` and `LastModifiedTime`, read back through
/// `DescribeEndpoint`.
async fn stored_times(emulator: &Emulator, name: &str) -> (f64, f64) {
    let described = emulator
        .ok("DescribeEndpoint", &json!({ "EndpointName": name }))
        .await;
    let number = |member: &str| {
        described
            .get(member)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("expected a numeric {member} in {described}"))
    };
    (number("CreationTime"), number("LastModifiedTime"))
}

/// Seed [`SEEDED`] and wait past the `InService` instant the clock owes, so every
/// endpoint's status and `LastModifiedTime` have settled and the assertions below are
/// not racing the clock. Answers each endpoint's `(name, CreationTime,
/// LastModifiedTime)` in creation order.
async fn seed_settled_endpoints(emulator: &Emulator) -> Vec<(String, f64, f64)> {
    seed_prerequisites(emulator).await;
    let names: Vec<String> = SEEDED.iter().map(|name| name.to_string()).collect();
    create_endpoints(emulator, &names).await;
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

    let mut seeded = Vec::new();
    for name in &names {
        let (created, modified) = stored_times(emulator, name).await;
        seeded.push((name.clone(), created, modified));
    }
    seeded
}

/// The `EndpointName`s a `ListEndpoints` response lists, in response order.
fn listed_names(body: &Value) -> Vec<String> {
    body.get("Endpoints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an Endpoints list in {body}"))
        .iter()
        .map(|endpoint| str_field(endpoint, "EndpointName"))
        .collect()
}

/// RULE: `ListEndpoints` returns the stored endpoints filtered by `NameContains` and
/// ordered by `SortBy`/`SortOrder`, defaulting to `CreationTime` / `Descending` as both
/// members' model docs state, with the resource name as the tie-break so equal sort keys
/// keep one order whichever way the page is sorted.
#[tokio::test]
async fn list_endpoints_honours_every_documented_sort_and_filter() {
    let emulator = Emulator::new();
    seed_settled_endpoints(&emulator).await;

    // Rows differ along sort field (each member of `EndpointSortKey`), sort order (each
    // member of `OrderKey`), and the name filter's selectivity.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "documented-defaults",
            json!({}),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        (
            "name-ascending",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        (
            "name-descending",
            json!({ "SortBy": "Name", "SortOrder": "Descending" }),
            vec!["demo-three", "demo-four", "beta-two", "alpha-one"],
        ),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-three", "demo-four"],
        ),
        (
            "creation-time-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        // Every endpoint is settled, so the status key is equal across the result and the
        // `EndpointName` tie-break decides the order — the same both ways round.
        (
            "status-ascending-falls-back-to-the-name-tie-break",
            json!({ "SortBy": "Status", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        (
            "status-descending-falls-back-to-the-name-tie-break",
            json!({ "SortBy": "Status", "SortOrder": "Descending" }),
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        (
            "name-contains-two-matches",
            json!({ "NameContains": "demo", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["demo-four", "demo-three"],
        ),
        (
            "name-contains-one-match",
            json!({ "NameContains": "alpha", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["alpha-one"],
        ),
        (
            "name-contains-no-match",
            json!({ "NameContains": "nothing-here" }),
            vec![],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListEndpoints", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListEndpoints order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: a complete page must carry no NextToken, got {listed:?}"
        );
    }
}

/// RULE: every `EndpointSummary` replays the stored endpoint — its name, its minted ARN,
/// the creation and last-modified instants the service recorded and its current status —
/// and an endpoint removed from state stops being listed.
#[tokio::test]
async fn list_endpoints_summaries_replay_the_stored_endpoint() {
    let emulator = Emulator::new();
    let seeded = seed_settled_endpoints(&emulator).await;

    let listed = emulator
        .ok(
            "ListEndpoints",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    let summaries = listed
        .get("Endpoints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an Endpoints list in {listed}"));
    assert_eq!(
        summaries.len(),
        seeded.len(),
        "every stored endpoint must be listed, got {listed:?}"
    );

    for (index, (name, created_at, modified_at)) in seeded.iter().enumerate() {
        let summary = &summaries[index];
        assert_eq!(
            str_field(summary, "EndpointName"),
            *name,
            "case {name}: EndpointName in the summary, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "EndpointArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:endpoint/{name}"),
            "case {name}: EndpointArn in the summary, got {summary:?}"
        );
        assert_eq!(
            summary.get("CreationTime").and_then(Value::as_f64),
            Some(*created_at),
            "case {name}: CreationTime must be the stored one, got {summary:?}"
        );
        assert_eq!(
            summary.get("LastModifiedTime").and_then(Value::as_f64),
            Some(*modified_at),
            "case {name}: LastModifiedTime must be the stored one, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "EndpointStatus"),
            "InService",
            "case {name}: EndpointStatus must be the stored one, got {summary:?}"
        );
    }

    // A removed endpoint stops being listed, and only that one.
    emulator
        .ok("DeleteEndpoint", &json!({ "EndpointName": SEEDED[0] }))
        .await;
    let listed = emulator
        .ok(
            "ListEndpoints",
            &json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
        )
        .await;
    assert_eq!(
        listed_names(&listed),
        vec![
            "beta-two".to_string(),
            "demo-four".to_string(),
            "demo-three".to_string()
        ],
        "case delete-one: a deleted endpoint must not be listed, got {listed:?}"
    );
}

/// RULE: `StatusEquals` returns exactly the endpoints whose current status equals it, and
/// the listing reports the same status the reader does — including the `InService`
/// transition the clock already owes, which the listing materialises rather than
/// reporting a status the endpoint has already left. The catalog it ranges over is the
/// generated `ENDPOINT_STATUS_VALUES`, i.e. the modelled `EndpointStatus` members.
#[tokio::test]
async fn list_endpoints_filters_on_status_and_reports_the_status_the_clock_owes() {
    let emulator = Emulator::new();
    seed_prerequisites(&emulator).await;
    let names: Vec<String> = SEEDED.iter().map(|name| name.to_string()).collect();
    create_endpoints(&emulator, &names).await;

    let expected_before: Vec<String> = {
        let mut sorted = names.clone();
        sorted.sort();
        sorted
    };
    // Before the owed transition is due: every endpoint is still `Creating`. Only the one
    // unfiltered listing is taken here, so the whole pre-due assertion rests on a single
    // wire call rather than on a loop that would have to finish inside the transition's
    // span on a loaded host.
    let listed = emulator
        .ok(
            "ListEndpoints",
            &json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
        )
        .await;
    let statuses: Vec<String> = listed
        .get("Endpoints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an Endpoints list in {listed}"))
        .iter()
        .map(|endpoint| str_field(endpoint, "EndpointStatus"))
        .collect();
    assert_eq!(
        statuses,
        vec!["Creating".to_string(); names.len()],
        "case before-due: a freshly created endpoint is reported Creating, got {listed:?}"
    );
    assert_eq!(
        listed_names(&listed),
        expected_before,
        "case before-due: every created endpoint is listed, got {listed:?}"
    );

    // Past the owed transition: every endpoint is `InService`, and `Creating` now selects
    // none of them.
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
    for status in ENDPOINT_STATUS_VALUES {
        let listed = emulator
            .ok(
                "ListEndpoints",
                &json!({ "StatusEquals": status, "SortBy": "Name", "SortOrder": "Ascending" }),
            )
            .await;
        let expected = if *status == "InService" {
            expected_before.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            listed_names(&listed),
            expected,
            "case past-due StatusEquals={status}: selection, got {listed:?}"
        );
    }

    // The listing and the reader agree, and the status is stable across further reads.
    for name in &names {
        let described = emulator
            .ok("DescribeEndpoint", &json!({ "EndpointName": name }))
            .await;
        assert_eq!(
            str_field(&described, "EndpointStatus"),
            "InService",
            "case {name}: DescribeEndpoint must agree with the listing, got {described:?}"
        );
    }
    let listed = emulator
        .ok("ListEndpoints", &json!({ "StatusEquals": "InService" }))
        .await;
    assert_eq!(
        listed_names(&listed).len(),
        names.len(),
        "case stability: the settled status must not change on a later read, got {listed:?}"
    );
}

/// RULE: each of the four time bounds selects exactly the endpoints on its side of the
/// instant, and the bounds compose with each other and with `NameContains`. The bounds
/// are placed between neighbouring instants, which is the finest split the `awsJson1_1`
/// epoch-seconds `Timestamp` represents without loss.
#[tokio::test]
async fn list_endpoints_filters_on_the_creation_and_last_modified_time_bounds() {
    let emulator = Emulator::new();
    let seeded = seed_settled_endpoints(&emulator).await;
    // Bounds strictly between two neighbours, so each split is unambiguous.
    let created_split = (seeded[1].1 + seeded[2].1) / 2.0;
    let created_split_upper = (seeded[2].1 + seeded[3].1) / 2.0;
    let modified_split = (seeded[1].2 + seeded[2].2) / 2.0;

    // Rows differ along which bound is used, which side it selects, how many bounds
    // compose, and whether a bound sits exactly on a stored instant.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "creation-time-before-the-split",
            json!({
                "CreationTimeBefore": created_split,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "creation-time-after-the-split",
            json!({
                "CreationTimeAfter": created_split,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
            vec!["demo-three", "demo-four"],
        ),
        (
            "creation-time-both-bounds-around-one-endpoint",
            json!({
                "CreationTimeAfter": created_split,
                "CreationTimeBefore": created_split_upper,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
            vec!["demo-three"],
        ),
        (
            "creation-time-bound-just-past-the-last-instant-selects-every-endpoint",
            json!({
                "CreationTimeBefore": seeded[3].1 + 0.001,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
            vec!["alpha-one", "beta-two", "demo-three", "demo-four"],
        ),
        (
            "creation-time-bound-that-excludes-everything",
            json!({ "CreationTimeAfter": seeded[3].1 + 3600.0 }),
            vec![],
        ),
        (
            "last-modified-time-before-the-split",
            json!({
                "LastModifiedTimeBefore": modified_split,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "last-modified-time-after-the-split",
            json!({
                "LastModifiedTimeAfter": modified_split,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
            vec!["demo-three", "demo-four"],
        ),
        (
            "last-modified-time-bound-just-before-the-first-instant-selects-nothing",
            json!({
                "LastModifiedTimeBefore": seeded[0].2 - 0.001,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
            vec![],
        ),
        (
            "creation-and-last-modified-bounds-compose",
            json!({
                "CreationTimeAfter": created_split,
                "LastModifiedTimeBefore": (seeded[2].2 + seeded[3].2) / 2.0,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
            vec!["demo-three"],
        ),
        (
            "bound-combined-with-a-name-filter",
            json!({
                "CreationTimeAfter": created_split,
                "NameContains": "demo",
                "SortBy": "Name",
                "SortOrder": "Ascending",
            }),
            vec!["demo-four", "demo-three"],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListEndpoints", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListEndpoints selection, got {listed:?}"
        );
    }
}

/// RULE: paging walks the whole ordered result exactly once — each page holds at most the
/// effective page size, a `NextToken` is present only while more remain, and feeding it
/// back resumes immediately after the previous page. The effective size is `MaxResults`
/// where the caller sends one (capped by the service's own ceiling) and this operation's
/// documented default of 10 where it does not.
#[tokio::test]
async fn list_endpoints_paging_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    seed_prerequisites(&emulator).await;
    // More endpoints than the documented default page, so a defaulted page and an
    // explicitly sized one are distinguishable.
    let names: Vec<String> = (0..12).map(|index| format!("ep-{index:02}")).collect();
    create_endpoints(&emulator, &names).await;
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

    // Rows: every page size from one entry up to past the end of the result set, plus
    // both published bounds of `MaxResults` (`range 1..100`, from
    // smithy_query(query="operation",operation="ListEndpoints")).
    for page_size in [1_usize, 2, 5, 11, 12, 13, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({
                "MaxResults": page_size,
                "SortBy": "Name",
                "SortOrder": "Ascending",
            });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListEndpoints", &body).await;
            let page = listed_names(&listed);
            let cap = page_size.min(PAGE_CEILING);
            assert!(
                page.len() <= cap,
                "case {case}: a page must hold at most {cap} entries, got {page:?}"
            );
            seen.extend(page);
            token = listed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case}: paging must terminate, got {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, names,
            "case {case}: paging must walk the whole ordered result exactly once"
        );
    }

    // No `MaxResults`: the page is this operation's documented default of 10, and the
    // token still walks the rest.
    let first = emulator
        .ok(
            "ListEndpoints",
            &json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
        )
        .await;
    assert_eq!(
        listed_names(&first).len(),
        DOCUMENTED_DEFAULT_MAX_RESULTS,
        "case documented-default: the page size must default to 10, got {first:?}"
    );
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case documented-default: expected a NextToken in {first}"))
        .to_string();
    let second = emulator
        .ok(
            "ListEndpoints",
            &json!({ "NextToken": token, "SortBy": "Name", "SortOrder": "Ascending" }),
        )
        .await;
    assert_eq!(
        listed_names(&second),
        names[DOCUMENTED_DEFAULT_MAX_RESULTS..].to_vec(),
        "case documented-default: the token must resume after the first page, got {second:?}"
    );
    assert!(
        second.get("NextToken").is_none(),
        "case documented-default: the last page must carry no NextToken, got {second:?}"
    );
}

/// RULE: the page is capped server-side, so no single call serialises the whole store
/// however large `MaxResults` is — and the resume token still walks the full set.
#[tokio::test]
async fn the_endpoint_page_is_capped_server_side_even_when_the_caller_asks_for_more() {
    let emulator = Emulator::new();
    seed_prerequisites(&emulator).await;
    // Seeded past the ceiling so a capped page and an uncapped one are distinguishable.
    let names: Vec<String> = (0..PAGE_CEILING + 3)
        .map(|index| format!("ep-{index:03}"))
        .collect();
    for name in &names {
        emulator
            .ok(
                "CreateEndpoint",
                &json!({ "EndpointName": name, "EndpointConfigName": CONFIG }),
            )
            .await;
    }

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({
            "MaxResults": 100,
            "SortBy": "Name",
            "SortOrder": "Ascending",
        });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let listed = emulator.ok("ListEndpoints", &body).await;
        let page = listed_names(&listed);
        assert!(
            page.len() <= PAGE_CEILING,
            "case ceiling: a page must hold at most {PAGE_CEILING} entries, got {}",
            page.len()
        );
        seen.extend(page);
        token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, names,
        "case ceiling: the resume token must still walk the whole result"
    );
}

/// RULE: an input outside a documented value set is refused with HTTP 400 rather than
/// silently ignored, and the refusal writes nothing.
#[tokio::test]
async fn list_endpoints_refuses_inputs_outside_the_documented_value_sets() {
    let emulator = Emulator::new();
    seed_settled_endpoints(&emulator).await;

    // Rows: one outside each documented value set — `EndpointSortKey`, `OrderKey`,
    // `EndpointStatus`, an unminted `NextToken`, `MaxResults` one past each published
    // bound (`range 1..100`), and `NameContains` outside `^[a-zA-Z0-9-]+$`/len 0..63.
    let too_long = "a".repeat(64);
    let cases: Vec<(&str, Value)> = vec![
        ("sort-by-outside-the-enum", json!({ "SortBy": "Nope" })),
        (
            "sort-order-outside-the-enum",
            json!({ "SortOrder": "Sideways" }),
        ),
        (
            "status-equals-outside-the-enum",
            json!({ "StatusEquals": "Nope" }),
        ),
        (
            "next-token-that-names-no-boundary",
            json!({ "NextToken": "not-a-token" }),
        ),
        ("max-results-below-1", json!({ "MaxResults": 0 })),
        ("max-results-above-100", json!({ "MaxResults": 101 })),
        (
            "name-contains-with-an-illegal-character",
            json!({ "NameContains": "bad name" }),
        ),
        (
            "name-contains-past-63",
            json!({ "NameContains": too_long.clone() }),
        ),
    ];

    for (case, body) in cases {
        let (status, parsed) = emulator.call("ListEndpoints", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: ListEndpoints must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: refusal code, got {parsed}"
        );
    }

    // Nothing above reached state: the seeded endpoints are all still listed.
    let listed = emulator
        .ok(
            "ListEndpoints",
            &json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
        )
        .await;
    assert_eq!(
        listed_names(&listed).len(),
        SEEDED.len(),
        "a refused request must write nothing, got {listed:?}"
    );
}
