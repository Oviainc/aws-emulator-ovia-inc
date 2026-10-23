// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account-level recommendation loop: `ListChecks`,
//! `ListRecommendations`, `GetRecommendation`, `ListRecommendationResources`,
//! `UpdateRecommendationLifecycle` and
//! `BatchUpdateRecommendationResourceExclusion`, driven over HTTP through the
//! same `MultiServiceRouter` the emulator serves with.
//!
//! Every assertion is about a RULE, exercised over a table of inputs rather
//! than one recorded call:
//!
//! * a filterable list reports exactly the stored entries its filters keep, in
//!   key order (`ListChecks`, `ListRecommendations`,
//!   `ListRecommendationResources`);
//! * `nextToken` walks every entry exactly once at any page size, and a token
//!   the service did not issue is refused;
//! * `resourcesAggregates` equals the per-status counts of the
//!   recommendation's own resources;
//! * an identifier that resolves to nothing is 404, and one outside its
//!   modelled pattern is 400;
//! * a lifecycle update persists and the next read reports it.
//!
//! The expected catalog below is written out by hand (its source is the seed in
//! `src/state.rs`), so a response is compared against stated values rather than
//! against the store it came from.
//!
//! Every call here is made as an account whose support plan includes the Trusted
//! Advisor API: the service refuses every request from an account whose plan does
//! not, ahead of each handler, and that gate is pinned in
//! `tests/all/support_level_gate.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use service_com_amazonaws_trustedadvisor_trustedadvisor::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service, so
/// the router's `can_handle` claims the request. Request authentication is off
/// in these tests, so the signature itself is never verified.
pub(crate) const SIGV4: &str = "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/\
                     trustedadvisor/aws4_request, SignedHeaders=host, Signature=00";

/// The seeded check ids, in the order the store keys them (check id order).
const CHECK_IDS: &[&str] = &[
    "amazon-ebs-snapshots",
    "iam-access-key-rotation",
    "low-utilization-ec2-instances",
    "rds-idle-db-instances",
];

/// A mounted emulator with its own engine, so each test starts from the seeded
/// store and sees only its own writes.
///
/// The account is put on a support plan that includes the Trusted Advisor API:
/// the service's support-level gate refuses EVERY request from an account whose
/// plan does not, and no operation changes a plan (the API models none), so the
/// plan is recorded in process here. The gate itself is pinned in
/// `tests/all/support_level_gate.rs`.
fn emulator() -> MultiServiceRouter {
    emulator_on_plan(SupportPlan::Business).0
}

/// A mounted emulator whose account holds `plan`, together with the engine it
/// was built on so a caller can move that plan and watch the gate's answer
/// change over one store.
pub(crate) fn emulator_on_plan(plan: SupportPlan) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>)
{
    let engine = Arc::new(engine::InMemoryEngine::new());
    set_support_plan(&engine, plan);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Put the emulator's account on `plan`, the way something outside the API would
/// (nothing on the wire changes a support plan).
pub(crate) fn set_support_plan(engine: &engine::InMemoryEngine, plan: SupportPlan) {
    service_state(engine)
        .expect("the in-memory engine carries this service's state")
        .lock()
        .set_support_plan(plan);
}

/// Drive one request through the router and read its status and JSON body (a
/// `Unit` output answers 200 with an empty body, reported as `Value::Null`).
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (status, _errortype, json) = dispatch(router, method, path_and_query, body).await;
    (status, json)
}

/// Drive one request through the router and read its status, the
/// `x-amzn-errortype` header restJson1 resolves an error SHAPE from, and its
/// JSON body.
pub(crate) async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> (u16, Option<String>, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("a serializable request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://trustedadvisor.us-east-1.amazonaws.com{path_and_query}"
        ))
        .header("authorization", SIGV4)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    (status, errortype, json)
}

/// One ARN or `nextToken` as it goes into a request URI, path label or query
/// value.
///
/// Both carry a known, small alphabet: an ARN is RFC 3986 unreserved characters
/// plus `:` and `/`, and a `nextToken` is base64, i.e. unreserved plus `+`, `/`
/// and `=`. A path segment may hold `:` unescaped (RFC 3986 §3.3: `pchar`
/// includes `":"`), so the characters that must be escaped are exactly `/`
/// (which would otherwise split the segment), `+` (decoded as a space in a
/// query) and `=` (which separates a query parameter from its value). Every
/// other byte either alphabet can hold is already safe in both positions. `%`
/// itself is escaped first so the function stays total on input outside those
/// alphabets; there is no general escaping rule to implement here.
pub(crate) fn label(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('/', "%2F")
        .replace('+', "%2B")
        .replace('=', "%3D")
}

/// `arn:aws:trustedadvisor:::check/<id>` — the ARN pattern `CheckSummary$arn`
/// declares (no account segment).
pub(crate) fn check_arn(check_id: &str) -> String {
    format!("arn:aws:trustedadvisor:::check/{check_id}")
}

/// `arn:aws:trustedadvisor::<account>:recommendation/<id>` — the ARN pattern
/// `AccountRecommendationArn` declares. Each seeded check produced the
/// recommendation `<check id>-recommendation`.
pub(crate) fn recommendation_arn(check_id: &str) -> String {
    format!(
        "arn:aws:trustedadvisor::{}:recommendation/{check_id}-recommendation",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// `arn:aws:trustedadvisor::<account>:recommendation-resource/<rec>/<resource>`
/// — the ARN pattern `RecommendationResourceArn` declares.
pub(crate) fn resource_arn(check_id: &str, resource_id: &str) -> String {
    format!(
        "arn:aws:trustedadvisor::{}:recommendation-resource/{check_id}-recommendation/{resource_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The `field` of every member of the `list` in a response body.
pub(crate) fn field_of_each(body: &Value, list: &str, field: &str) -> Vec<String> {
    body[list]
        .as_array()
        .unwrap_or_else(|| panic!("response has no {list} array: {body}"))
        .iter()
        .map(|item| {
            item[field]
                .as_str()
                .unwrap_or_else(|| panic!("{list} item has no {field}: {item}"))
                .to_string()
        })
        .collect()
}

/// The wire error code of a failure response (`__type`, which restJson1 carries
/// in the error body).
pub(crate) fn error_code(body: &Value) -> &str {
    body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("error body carries no __type: {body}"))
}

/// A modelled timestamp as seconds since the epoch, whichever wire form the
/// protocol wrote: restJson1's default epoch-seconds number, or an RFC 3339
/// string. Read through this helper so the assertions are about the INSTANT and
/// not about the encoding.
pub(crate) fn timestamp_secs(value: &Value) -> f64 {
    if let Some(number) = value.as_f64() {
        return number;
    }
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("not a timestamp: {value}"));
    aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| panic!("not an RFC 3339 timestamp {text}: {error}"))
        .as_secs_f64()
}

#[tokio::test]
async fn list_checks_reports_the_catalog_and_honours_every_filter() {
    let router = emulator();

    // Each row: the query string, and the check ids it must select. Rows differ
    // along the member filtered on, the number of matches (all, several, one,
    // none) and the combination of two filters.
    let cases: &[(&str, &[&str])] = &[
        ("", CHECK_IDS),
        (
            "?pillar=cost_optimizing",
            &[
                "low-utilization-ec2-instances",
                "rds-idle-db-instances",
            ],
        ),
        ("?pillar=security", &["iam-access-key-rotation"]),
        // `performance` is the second pillar of a check that also carries
        // `cost_optimizing`: the filter is membership, not equality.
        ("?pillar=performance", &["rds-idle-db-instances"]),
        ("?pillar=operational_excellence", &[]),
        ("?awsService=ec2", &["low-utilization-ec2-instances"]),
        ("?awsService=iam", &["iam-access-key-rotation"]),
        ("?awsService=lambda", &[]),
        (
            "?source=ta_check",
            &[
                "amazon-ebs-snapshots",
                "iam-access-key-rotation",
                "low-utilization-ec2-instances",
            ],
        ),
        ("?source=rds", &["rds-idle-db-instances"]),
        ("?source=well_architected", &[]),
        (
            "?pillar=cost_optimizing&source=ta_check",
            &["low-utilization-ec2-instances"],
        ),
        ("?pillar=cost_optimizing&awsService=rds", &["rds-idle-db-instances"]),
        // `language` selects which stored copy of the check text is reported
        // (pinned in `tests/all/localized_text.rs`); it is not a filter, so it
        // must not change which checks are reported.
        ("?language=ja&source=rds", &["rds-idle-db-instances"]),
    ];

    for (query, expected) in cases {
        let (status, body) = call(&router, "GET", &format!("/v1/checks{query}"), None).await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected 200, got {status} with {body}"
        );
        let ids = field_of_each(&body, "checkSummaries", "id");
        assert_eq!(
            ids, *expected,
            "case {query:?}: expected checks {expected:?}, got {ids:?}"
        );
    }

    // Every required `CheckSummary` member is present, computed from the store.
    let (_, body) = call(&router, "GET", "/v1/checks", None).await;
    for summary in body["checkSummaries"].as_array().expect("checkSummaries") {
        let id = summary["id"].as_str().expect("id");
        assert_eq!(
            summary["arn"].as_str(),
            Some(check_arn(id).as_str()),
            "case {id:?}: expected arn {:?}, got {:?}",
            check_arn(id),
            summary["arn"]
        );
        for member in ["name", "description", "source"] {
            assert!(
                summary[member].as_str().is_some_and(|text| !text.is_empty()),
                "case {id:?}: expected a non-empty {member}, got {:?}",
                summary[member]
            );
        }
        assert!(
            summary["pillars"].as_array().is_some_and(|list| !list.is_empty()),
            "case {id:?}: expected at least one pillar, got {:?}",
            summary["pillars"]
        );
        assert!(
            summary["awsServices"].as_array().is_some(),
            "case {id:?}: expected awsServices, got {:?}",
            summary["awsServices"]
        );
        assert!(
            summary["metadata"].is_object(),
            "case {id:?}: expected a metadata map, got {:?}",
            summary["metadata"]
        );
    }
}

#[tokio::test]
async fn paginating_a_list_walks_every_entry_exactly_once() {
    let router = emulator();

    // Rows differ along the page size: one entry per page, two, exactly the
    // whole catalog, and a page larger than it.
    for page_size in [1usize, 2, 3, 4, 200] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One extra round beyond the number of pages the size implies, so a
        // token that never clears is caught as a loop rather than hanging.
        for _ in 0..=CHECK_IDS.len() + 1 {
            let query = match &token {
                Some(value) => format!("?maxResults={page_size}&nextToken={}", label(value)),
                None => format!("?maxResults={page_size}"),
            };
            let (status, body) = call(&router, "GET", &format!("/v1/checks{query}"), None).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size}: expected 200, got {status} with {body}"
            );
            let page = field_of_each(&body, "checkSummaries", "id");
            assert!(
                page.len() <= page_size,
                "case maxResults={page_size}: expected at most {page_size} per page, got {}",
                page.len()
            );
            seen.extend(page);
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case maxResults={page_size}: expected the token to clear, got {token:?}"
        );
        assert_eq!(
            seen, CHECK_IDS,
            "case maxResults={page_size}: expected every check once in key order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    let recommendation = recommendation_arn("amazon-ebs-snapshots");

    // Every paginated read on the slice, each with a token that is not base64
    // of a key (the modelled `length 4..10000` is satisfied, so the refusal is
    // the handler's own).
    let cases: &[(&str, String)] = &[
        ("ListChecks", "/v1/checks?nextToken=%21%21%21%21".to_string()),
        (
            "ListRecommendations",
            "/v1/recommendations?nextToken=%21%21%21%21".to_string(),
        ),
        (
            "ListRecommendationResources",
            format!(
                "/v1/recommendations/{}/resources?nextToken=%21%21%21%21",
                label(&recommendation)
            ),
        ),
    ];

    for (operation, path) in cases {
        let (status, body) = call(&router, "GET", path, None).await;
        assert_eq!(
            status, 400,
            "case {operation}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {operation}: expected ValidationException, got {body}"
        );
    }

    // A token this service DID issue, replayed on a different list: the key it
    // names belongs to another keyspace, so it is refused rather than silently
    // resuming from the wrong place.
    let (_, first_checks_page) = call(&router, "GET", "/v1/checks?maxResults=1", None).await;
    let checks_token = first_checks_page["nextToken"]
        .as_str()
        .expect("a nextToken on a truncated ListChecks page")
        .to_string();
    let (_, first_resources_page) = call(
        &router,
        "GET",
        &format!(
            "/v1/recommendations/{}/resources?maxResults=1",
            label(&recommendation)
        ),
        None,
    )
    .await;
    let resources_token = first_resources_page["nextToken"]
        .as_str()
        .expect("a nextToken on a truncated ListRecommendationResources page")
        .to_string();

    let replays: &[(&str, String)] = &[
        (
            "a ListChecks token on ListRecommendations",
            format!("/v1/recommendations?nextToken={}", label(&checks_token)),
        ),
        (
            "a ListChecks token on ListRecommendationResources",
            format!(
                "/v1/recommendations/{}/resources?nextToken={}",
                label(&recommendation),
                label(&checks_token)
            ),
        ),
        (
            "a resources token on ListChecks",
            format!("/v1/checks?nextToken={}", label(&resources_token)),
        ),
        (
            "one recommendation's resources token on another's resources",
            format!(
                "/v1/recommendations/{}/resources?nextToken={}",
                label(&recommendation_arn("rds-idle-db-instances")),
                label(&resources_token)
            ),
        ),
    ];

    for (name, path) in replays {
        let (status, body) = call(&router, "GET", path, None).await;
        assert_eq!(
            status, 400,
            "case {name}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {name}: expected ValidationException, got {body}"
        );
    }

    // And the token IS accepted by the list that issued it.
    let (status, body) = call(
        &router,
        "GET",
        &format!("/v1/checks?maxResults=1&nextToken={}", label(&checks_token)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case the issuing list accepts its own token: expected 200, got {status} with {body}"
    );
    assert_eq!(
        field_of_each(&body, "checkSummaries", "id"),
        vec![CHECK_IDS[1].to_string()],
        "case the issuing list accepts its own token: expected the second check, got {body}"
    );
}

#[tokio::test]
async fn list_recommendations_reports_the_store_and_honours_every_filter() {
    let router = emulator();

    // Expected values are check ids; the recommendation of check `c` is
    // `<c>-recommendation`, and the store keys recommendations by ARN, which
    // orders them by that id.
    let cases: &[(&str, &[&str])] = &[
        ("", CHECK_IDS),
        (
            "?type=priority",
            &[
                "low-utilization-ec2-instances",
                "rds-idle-db-instances",
            ],
        ),
        (
            "?type=standard",
            &["amazon-ebs-snapshots", "iam-access-key-rotation"],
        ),
        ("?status=error", &["amazon-ebs-snapshots"]),
        ("?status=ok", &["iam-access-key-rotation"]),
        (
            "?status=warning",
            &[
                "low-utilization-ec2-instances",
                "rds-idle-db-instances",
            ],
        ),
        ("?pillar=fault_tolerance", &["amazon-ebs-snapshots"]),
        ("?pillar=service_limits", &[]),
        ("?awsService=ebs", &["amazon-ebs-snapshots"]),
        ("?source=rds", &["rds-idle-db-instances"]),
        (
            "?type=priority&status=warning&pillar=performance",
            &["rds-idle-db-instances"],
        ),
        // Two filters that cannot hold together select nothing.
        ("?type=standard&source=rds", &[]),
        // The seeded `lastUpdatedAt` values, from the seed table in
        // `src/state.rs` (one day apart in SEED order, which is not check-id
        // order):
        //   low-utilization-ec2-instances  2024-01-02T00:00:00Z
        //   amazon-ebs-snapshots                  2024-01-03T00:00:00Z
        //   iam-access-key-rotation               2024-01-04T00:00:00Z
        //   rds-idle-db-instances                 2024-01-05T00:00:00Z
        (
            "?afterLastUpdatedAt=2024-01-04T00:00:00Z",
            &["iam-access-key-rotation", "rds-idle-db-instances"],
        ),
        (
            "?beforeLastUpdatedAt=2024-01-03T12:00:00Z",
            &[
                "amazon-ebs-snapshots",
                "low-utilization-ec2-instances",
            ],
        ),
        (
            "?afterLastUpdatedAt=2024-01-03T12:00:00Z&beforeLastUpdatedAt=2024-01-04T12:00:00Z",
            &["iam-access-key-rotation"],
        ),
        ("?afterLastUpdatedAt=2030-01-01T00:00:00Z", &[]),
    ];

    for (query, expected) in cases {
        let (status, body) = call(&router, "GET", &format!("/v1/recommendations{query}"), None).await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected 200, got {status} with {body}"
        );
        let arns = field_of_each(&body, "recommendationSummaries", "arn");
        let expected_arns: Vec<String> = expected.iter().map(|id| recommendation_arn(id)).collect();
        assert_eq!(
            arns, expected_arns,
            "case {query:?}: expected recommendations {expected_arns:?}, got {arns:?}"
        );
    }

    // `checkIdentifier` selects by the check ARN each recommendation carries.
    for check_id in CHECK_IDS {
        let query = format!("?checkIdentifier={}", label(&check_arn(check_id)));
        let (status, body) = call(&router, "GET", &format!("/v1/recommendations{query}"), None).await;
        assert_eq!(
            status, 200,
            "case checkIdentifier={check_id}: expected 200, got {status} with {body}"
        );
        let arns = field_of_each(&body, "recommendationSummaries", "arn");
        assert_eq!(
            arns,
            vec![recommendation_arn(check_id)],
            "case checkIdentifier={check_id}: expected only that check's recommendation, got {arns:?}"
        );
    }

    // `checkIdentifier` is modelled `length 20..64`: a check ARN at the upper
    // bound is accepted (it names nothing, so the page is empty), and one byte
    // past it is refused. `arn:aws:trustedadvisor:::check/` is 31 characters.
    let at_bound = format!("arn:aws:trustedadvisor:::check/{}", "c".repeat(33));
    assert_eq!(at_bound.len(), 64, "the bound case must be exactly 64 bytes");
    let (status, body) = call(
        &router,
        "GET",
        &format!("/v1/recommendations?checkIdentifier={}", label(&at_bound)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case checkIdentifier at the 64-byte bound: expected 200, got {status} with {body}"
    );
    assert_eq!(
        field_of_each(&body, "recommendationSummaries", "arn"),
        Vec::<String>::new(),
        "case checkIdentifier at the 64-byte bound: expected no match, got {body}"
    );

    let past_bound = format!("arn:aws:trustedadvisor:::check/{}", "c".repeat(34));
    let (status, body) = call(
        &router,
        "GET",
        &format!("/v1/recommendations?checkIdentifier={}", label(&past_bound)),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "case checkIdentifier one byte past the bound: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case checkIdentifier one byte past the bound: expected ValidationException, got {body}"
    );
}

#[tokio::test]
async fn a_recommendation_resolves_and_its_aggregates_count_its_resources() {
    let router = emulator();

    let (status, listed) = call(&router, "GET", "/v1/recommendations", None).await;
    assert_eq!(status, 200, "expected 200 from ListRecommendations, got {status}");

    for summary in listed["recommendationSummaries"]
        .as_array()
        .expect("recommendationSummaries")
    {
        let arn = summary["arn"].as_str().expect("arn").to_string();

        // The ARN from the list resolves in GetRecommendation, and the full
        // shape repeats the summary's values.
        let (status, body) =
            call(&router, "GET", &format!("/v1/recommendations/{}", label(&arn)), None).await;
        assert_eq!(
            status, 200,
            "case {arn}: expected 200 from GetRecommendation, got {status} with {body}"
        );
        let recommendation = &body["recommendation"];
        for member in ["id", "arn", "type", "status", "name", "source", "checkArn"] {
            assert_eq!(
                recommendation[member], summary[member],
                "case {arn}: expected {member} {:?} from GetRecommendation, got {:?}",
                summary[member], recommendation[member]
            );
        }
        assert!(
            recommendation["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "case {arn}: expected a non-empty description, got {:?}",
            recommendation["description"]
        );

        // The resource list and the aggregates are the same facts counted twice.
        let (status, resources) = call(
            &router,
            "GET",
            &format!("/v1/recommendations/{}/resources", label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {arn}: expected 200 from ListRecommendationResources, got {status} with {resources}"
        );
        let summaries = resources["recommendationResourceSummaries"]
            .as_array()
            .expect("recommendationResourceSummaries");
        assert!(
            !summaries.is_empty(),
            "case {arn}: expected at least one resource, got none"
        );

        let count = |wanted: &str| -> i64 {
            summaries
                .iter()
                .filter(|item| item["status"].as_str() == Some(wanted))
                .count() as i64
        };
        let excluded = summaries
            .iter()
            .filter(|item| item["exclusionStatus"].as_str() == Some("excluded"))
            .count() as i64;
        let aggregates = &recommendation["resourcesAggregates"];
        for (member, expected) in [
            ("okCount", count("ok")),
            ("warningCount", count("warning")),
            ("errorCount", count("error")),
            ("excludedCount", excluded),
        ] {
            assert_eq!(
                aggregates[member].as_i64(),
                Some(expected),
                "case {arn}: expected {member} {expected}, got {:?}",
                aggregates[member]
            );
        }

        // Every resource names this recommendation and carries the modelled
        // ARN shape and the default exclusion status.
        for resource in summaries {
            let id = resource["id"].as_str().expect("id");
            assert_eq!(
                resource["recommendationArn"].as_str(),
                Some(arn.as_str()),
                "case {id}: expected recommendationArn {arn}, got {:?}",
                resource["recommendationArn"]
            );
            let prefix = format!("{}:recommendation-resource/", engine::EMULATOR_ACCOUNT_ID);
            assert!(
                resource["arn"].as_str().is_some_and(|value| value.contains(&prefix)
                    && value.ends_with(&format!("/{id}"))),
                "case {id}: expected a recommendation-resource ARN ending in /{id}, got {:?}",
                resource["arn"]
            );
            for member in ["awsResourceId", "regionCode", "status"] {
                assert!(
                    resource[member].as_str().is_some_and(|text| !text.is_empty()),
                    "case {id}: expected a non-empty {member}, got {:?}",
                    resource[member]
                );
            }
            assert!(
                timestamp_secs(&resource["lastUpdatedAt"]) > 0.0,
                "case {id}: expected a lastUpdatedAt instant, got {:?}",
                resource["lastUpdatedAt"]
            );
        }
    }
}

#[tokio::test]
async fn resource_filters_select_the_stored_resources() {
    let router = emulator();
    let arn = recommendation_arn("low-utilization-ec2-instances");
    let base = format!("/v1/recommendations/{}/resources", label(&arn));

    // Rows differ along the member filtered on and the number of matches.
    let cases: &[(&str, &[&str])] = &[
        ("", &["res-ec2-0001", "res-ec2-0002", "res-ec2-0003"]),
        ("?status=warning", &["res-ec2-0001", "res-ec2-0003"]),
        ("?status=ok", &["res-ec2-0002"]),
        ("?status=error", &[]),
        ("?regionCode=us-east-1", &["res-ec2-0001"]),
        ("?regionCode=eu-west-1", &["res-ec2-0003"]),
        ("?regionCode=sa-east-1", &[]),
        (
            "?exclusionStatus=included",
            &["res-ec2-0001", "res-ec2-0002", "res-ec2-0003"],
        ),
        ("?exclusionStatus=excluded", &[]),
        ("?status=warning&regionCode=eu-west-1", &["res-ec2-0003"]),
        ("?status=ok&regionCode=eu-west-1", &[]),
        ("?maxResults=2", &["res-ec2-0001", "res-ec2-0002"]),
    ];

    for (query, expected) in cases {
        let (status, body) = call(&router, "GET", &format!("{base}{query}"), None).await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected 200, got {status} with {body}"
        );
        let ids = field_of_each(&body, "recommendationResourceSummaries", "id");
        assert_eq!(
            ids, *expected,
            "case {query:?}: expected resources {expected:?}, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn an_identifier_that_resolves_to_nothing_is_404_and_a_malformed_one_is_400() {
    let router = emulator();

    // A well-formed ARN naming no stored recommendation, and an identifier
    // outside the modelled `length 20..200` / ARN pattern.
    let unknown = format!(
        "arn:aws:trustedadvisor::{}:recommendation/does-not-exist",
        engine::EMULATOR_ACCOUNT_ID
    );
    let malformed = "not-an-arn-but-long-enough-to-pass-the-length-bound";
    let lifecycle_body = json!({
        "lifecycleStage": "resolved",
        "updateReason": "fixed in production",
        "updateReasonCode": "other",
    });

    let cases: &[(&str, &str, &str, bool, u16, &str)] = &[
        (
            "GetRecommendation/unknown",
            "GET",
            "/v1/recommendations/{}",
            false,
            404,
            "ResourceNotFoundException",
        ),
        (
            "ListRecommendationResources/unknown",
            "GET",
            "/v1/recommendations/{}/resources",
            false,
            404,
            "ResourceNotFoundException",
        ),
        (
            "UpdateRecommendationLifecycle/unknown",
            "PUT",
            "/v1/recommendations/{}/lifecycle",
            true,
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (name, method, template, needs_body, expected_status, expected_code) in cases {
        let body = needs_body.then(|| lifecycle_body.clone());
        let path = template.replace("{}", &label(&unknown));
        let (status, response) = call(&router, method, &path, body.clone()).await;
        assert_eq!(
            status, *expected_status,
            "case {name}: expected {expected_status}, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            *expected_code,
            "case {name}: expected {expected_code}, got {response}"
        );

        // The same call with an identifier outside the modelled ARN pattern is
        // a 400, not a 404: the request never names a resource at all.
        let path = template.replace("{}", &label(malformed));
        let (status, response) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 400,
            "case {name}/malformed: expected 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {name}/malformed: expected ValidationException, got {response}"
        );
    }
}

#[tokio::test]
async fn a_lifecycle_update_persists_and_the_next_read_reports_it() {
    let router = emulator();
    let arn = recommendation_arn("low-utilization-ec2-instances");
    let lifecycle_path = format!("/v1/recommendations/{}/lifecycle", label(&arn));
    let get_path = format!("/v1/recommendations/{}", label(&arn));

    let (_, before) = call(&router, "GET", &get_path, None).await;
    let seeded_last_updated = timestamp_secs(&before["recommendation"]["lastUpdatedAt"]);

    // Every `UpdateRecommendationLifecycleStage` value, ending on a stage that
    // leaves `resolved` again so the `resolvedAt` rule is tested both ways, plus
    // a last row that OMITS `updateReason`/`updateReasonCode`: the operation is
    // `@idempotent` PUT, so the request REPLACES the lifecycle and the previous
    // update's wording does not stay attached to the new stage.
    // Reason codes come from `UpdateRecommendationLifecycleStageReasonCode`.
    let cases: &[(&str, Option<&str>, Option<&str>)] = &[
        (
            "in_progress",
            Some("working the finding now"),
            Some("valid_business_case"),
        ),
        (
            "pending_response",
            Some("waiting on the account owner"),
            Some("other"),
        ),
        (
            "resolved",
            Some("right-sized the instances"),
            Some("other_methods_available"),
        ),
        (
            "dismissed",
            Some("reopened then dismissed"),
            Some("low_priority"),
        ),
        ("pending_response", None, None),
    ];

    for (stage, reason, reason_code) in cases {
        let mut request = json!({ "lifecycleStage": stage });
        if let Some(reason) = reason {
            request["updateReason"] = json!(reason);
        }
        if let Some(reason_code) = reason_code {
            request["updateReasonCode"] = json!(reason_code);
        }
        let (status, body) = call(&router, "PUT", &lifecycle_path, Some(request)).await;
        assert_eq!(
            status, 200,
            "case {stage}/{reason:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body,
            Value::Null,
            "case {stage}/{reason:?}: expected an empty body, got {body}"
        );

        let (status, body) = call(&router, "GET", &get_path, None).await;
        assert_eq!(
            status, 200,
            "case {stage}/{reason:?}: expected 200 from GetRecommendation, got {status} with {body}"
        );
        let recommendation = &body["recommendation"];
        assert_eq!(
            recommendation["lifecycleStage"].as_str(),
            Some(*stage),
            "case {stage}/{reason:?}: expected lifecycleStage {stage}, got {:?}",
            recommendation["lifecycleStage"]
        );
        assert_eq!(
            recommendation["updateReason"].as_str(),
            *reason,
            "case {stage}/{reason:?}: expected updateReason {reason:?}, got {:?}",
            recommendation["updateReason"]
        );
        assert_eq!(
            recommendation["updateReasonCode"].as_str(),
            *reason_code,
            "case {stage}/{reason:?}: expected updateReasonCode {reason_code:?}, got {:?}",
            recommendation["updateReasonCode"]
        );
        let last_updated = timestamp_secs(&recommendation["lastUpdatedAt"]);
        assert!(
            last_updated > seeded_last_updated,
            "case {stage}/{reason:?}: expected lastUpdatedAt after the seeded {seeded_last_updated}, got {last_updated}"
        );
        if *stage == "resolved" {
            let resolved_at = timestamp_secs(&recommendation["resolvedAt"]);
            // A millisecond of tolerance: both come from the same stored
            // instant, so this only absorbs a change of wire encoding.
            assert!(
                (resolved_at - last_updated).abs() < 1e-3,
                "case {stage}/{reason:?}: expected resolvedAt to be the update time {last_updated}, got {resolved_at}"
            );
        } else {
            assert!(
                recommendation["resolvedAt"].is_null(),
                "case {stage}/{reason:?}: expected no resolvedAt, got {:?}",
                recommendation["resolvedAt"]
            );
        }

        // The same change is visible on the list read, which recomputes the
        // summary from the same stored entry.
        let (_, listed) = call(&router, "GET", "/v1/recommendations", None).await;
        let staged: Vec<String> = listed["recommendationSummaries"]
            .as_array()
            .expect("recommendationSummaries")
            .iter()
            .filter(|item| item["arn"].as_str() == Some(arn.as_str()))
            .map(|item| item["lifecycleStage"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(
            staged,
            vec![(*stage).to_string()],
            "case {stage}/{reason:?}: expected ListRecommendations to report the new stage, got {staged:?}"
        );
    }
}

#[tokio::test]
async fn a_lifecycle_update_of_a_non_priority_recommendation_conflicts() {
    let router = emulator();

    // Every seeded recommendation whose `type` is `standard`: the operation's
    // own doc says it "only supports prioritized recommendations".
    for check_id in ["amazon-ebs-snapshots", "iam-access-key-rotation"] {
        let arn = recommendation_arn(check_id);
        let (status, body) = call(
            &router,
            "PUT",
            &format!("/v1/recommendations/{}/lifecycle", label(&arn)),
            Some(json!({
                "lifecycleStage": "dismissed",
                "updateReason": "not applicable to this account",
                "updateReasonCode": "not_applicable",
            })),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {check_id}: expected 409, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {check_id}: expected ConflictException, got {body}"
        );

        // And nothing was written: the stage is still the seeded one.
        let (_, body) =
            call(&router, "GET", &format!("/v1/recommendations/{}", label(&arn)), None).await;
        assert_eq!(
            body["recommendation"]["lifecycleStage"],
            Value::Null,
            "case {check_id}: expected the seeded (absent) lifecycleStage, got {:?}",
            body["recommendation"]["lifecycleStage"]
        );
    }
}

#[tokio::test]
async fn an_exclusion_update_persists_and_is_counted() {
    let router = emulator();
    let arn = recommendation_arn("amazon-ebs-snapshots");
    let resources_path = format!("/v1/recommendations/{}/resources", label(&arn));
    let included = resource_arn("amazon-ebs-snapshots", "res-ebs-0001");
    let excluded = resource_arn("amazon-ebs-snapshots", "res-ebs-0002");

    // Rows differ along the requested exclusion flag and the number of entries
    // in one call: the stored status after each call is exactly what the call
    // asked for, and `excludedCount` counts it. Each row's expectation is the
    // set of excluded resource ids, in resource-ARN order.
    struct Row {
        name: &'static str,
        entries: Vec<(String, bool)>,
        expected_excluded: Vec<&'static str>,
    }
    let rows: Vec<Row> = vec![
        Row {
            name: "exclude the included one",
            entries: vec![(included.clone(), true)],
            expected_excluded: vec!["res-ebs-0001", "res-ebs-0002"],
        },
        Row {
            name: "include both in one call",
            entries: vec![(included.clone(), false), (excluded.clone(), false)],
            expected_excluded: vec![],
        },
        Row {
            name: "exclude both in one call",
            entries: vec![(included.clone(), true), (excluded.clone(), true)],
            expected_excluded: vec!["res-ebs-0001", "res-ebs-0002"],
        },
        Row {
            name: "include only the first again",
            entries: vec![(included.clone(), false)],
            expected_excluded: vec!["res-ebs-0002"],
        },
    ];

    // The seeded state first: one resource excluded, and `excludedCount` agrees.
    let (status, body) = call(&router, "GET", &format!("{resources_path}?exclusionStatus=excluded"), None).await;
    assert_eq!(status, 200, "case seeded: expected 200, got {status} with {body}");
    assert_eq!(
        field_of_each(&body, "recommendationResourceSummaries", "id"),
        vec!["res-ebs-0002".to_string()],
        "case seeded: expected only res-ebs-0002 excluded, got {body}"
    );

    for Row {
        name,
        entries,
        expected_excluded,
    } in rows
    {
        let payload = json!({
            "recommendationResourceExclusions": entries
                .iter()
                .map(|(arn, is_excluded)| json!({"arn": arn, "isExcluded": is_excluded}))
                .collect::<Vec<Value>>(),
        });
        let (status, body) = call(
            &router,
            "PUT",
            "/v1/batch-update-recommendation-resource-exclusion",
            Some(payload),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: expected 200, got {status} with {body}"
        );
        let errors = body["batchUpdateRecommendationResourceExclusionErrors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name}: no error list in {body}"));
        assert!(
            errors.is_empty(),
            "case {name}: expected no per-entry errors, got {errors:?}"
        );

        let (_, listed) = call(
            &router,
            "GET",
            &format!("{resources_path}?exclusionStatus=excluded"),
            None,
        )
        .await;
        let ids = field_of_each(&listed, "recommendationResourceSummaries", "id");
        assert_eq!(
            ids, expected_excluded,
            "case {name}: expected excluded {expected_excluded:?}, got {ids:?}"
        );

        // `excludedCount` is derived, so it tracks the same change.
        let (_, recommendation) =
            call(&router, "GET", &format!("/v1/recommendations/{}", label(&arn)), None).await;
        assert_eq!(
            recommendation["recommendation"]["resourcesAggregates"]["excludedCount"].as_i64(),
            Some(expected_excluded.len() as i64),
            "case {name}: expected excludedCount {}, got {:?}",
            expected_excluded.len(),
            recommendation["recommendation"]["resourcesAggregates"]["excludedCount"]
        );
    }
}

#[tokio::test]
async fn an_exclusion_entry_the_service_cannot_apply_comes_back_per_entry() {
    let router = emulator();
    let unknown = resource_arn("amazon-ebs-snapshots", "res-ebs-9999");
    // The operation's doc says it "currently doesn't support prioritized
    // recommendation resources"; this resource belongs to a `priority` one.
    let prioritized = resource_arn("low-utilization-ec2-instances", "res-ec2-0001");
    let applicable = resource_arn("amazon-ebs-snapshots", "res-ebs-0001");

    let cases: &[(&str, &str, &str)] = &[
        ("unknown resource", unknown.as_str(), "ResourceNotFoundException"),
        ("prioritized resource", prioritized.as_str(), "ConflictException"),
    ];

    for (name, bad_arn, expected_code) in cases {
        let (status, body) = call(
            &router,
            "PUT",
            "/v1/batch-update-recommendation-resource-exclusion",
            Some(json!({
                "recommendationResourceExclusions": [
                    {"arn": bad_arn, "isExcluded": true},
                    {"arn": applicable, "isExcluded": true},
                ],
            })),
        )
        .await;
        // The call itself succeeds: a refused ENTRY is reported per entry.
        assert_eq!(
            status, 200,
            "case {name}: expected 200, got {status} with {body}"
        );
        let errors = body["batchUpdateRecommendationResourceExclusionErrors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name}: no error list in {body}"));
        assert_eq!(
            errors.len(),
            1,
            "case {name}: expected exactly one per-entry error, got {errors:?}"
        );
        assert_eq!(
            errors[0]["arn"].as_str(),
            Some(*bad_arn),
            "case {name}: expected the error to name {bad_arn}, got {:?}",
            errors[0]
        );
        assert_eq!(
            errors[0]["errorCode"].as_str(),
            Some(*expected_code),
            "case {name}: expected errorCode {expected_code}, got {:?}",
            errors[0]
        );

        // The entry beside it still applied, and the refused one did not.
        let (_, listed) = call(
            &router,
            "GET",
            &format!(
                "/v1/recommendations/{}/resources?exclusionStatus=excluded",
                label(&recommendation_arn("amazon-ebs-snapshots"))
            ),
            None,
        )
        .await;
        let ids = field_of_each(&listed, "recommendationResourceSummaries", "id");
        assert!(
            ids.contains(&"res-ebs-0001".to_string()),
            "case {name}: expected the applicable entry to have been excluded, got {ids:?}"
        );

        let (_, prioritized_resources) = call(
            &router,
            "GET",
            &format!(
                "/v1/recommendations/{}/resources?exclusionStatus=excluded",
                label(&recommendation_arn("low-utilization-ec2-instances"))
            ),
            None,
        )
        .await;
        let ids = field_of_each(&prioritized_resources, "recommendationResourceSummaries", "id");
        assert!(
            ids.is_empty(),
            "case {name}: expected no prioritized resource to be excluded, got {ids:?}"
        );
    }
}
