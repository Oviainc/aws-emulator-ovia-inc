// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Inspector account-activation and
//! suppression-filter slice: `Enable`, `BatchGetAccountStatus`, `CreateFilter`,
//! `ListFilters`, `UpdateFilter` and `DeleteFilter`.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The single account id every generated emulator presents
/// (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// Every `ResourceScanType` token, in model order.
/// Source: `smithy_query(query="shape", shape="ResourceScanType")`.
const RESOURCE_SCAN_TYPES: [&str; 5] = ["EC2", "ECR", "LAMBDA", "LAMBDA_CODE", "CODE_REPOSITORY"];

/// The `ResourceStatus`/`ResourceState` member each `ResourceScanType` is reported under.
/// Source: `smithy_query(query="shape", shape="ResourceStatus,ResourceState")`.
fn resource_member(resource_type: &str) -> &'static str {
    match resource_type {
        "EC2" => "ec2",
        "ECR" => "ecr",
        "LAMBDA" => "lambda",
        "LAMBDA_CODE" => "lambdaCode",
        "CODE_REPOSITORY" => "codeRepository",
        other => panic!("unmodelled ResourceScanType {other:?}"),
    }
}

/// A router holding only this service, over its own engine, so each test starts from
/// empty state.
pub(crate) fn router() -> MultiServiceRouter {
    router_over(&Arc::new(engine::InMemoryEngine::new()))
}

/// A router holding only this service, over `engine` — for a test that also reaches the
/// store the handlers read.
pub(crate) fn router_over(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_inspector2_inspector2::EmulatedService::new(Arc::clone(engine)),
    );
    router
}

/// Send a restJson1 request with `method`, signed for `region` so the service claims it
/// and can read the region out of the SigV4 credential scope. `body` is `None` for a
/// request with no payload, such as a GET whose members are bound to the query string.
pub(crate) async fn send_in(
    router: &MultiServiceRouter,
    method: &str,
    region: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Option<String>, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |body| {
        bytes::Bytes::from(serde_json::to_vec(body).expect("a serializable request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{uri}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=TESTKEY/20240101/{region}/inspector2/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, error_type, parsed)
}

/// POST a restJson1 request, signed for `region` so the service claims it and can read
/// the region out of the SigV4 credential scope.
pub(crate) async fn post_in(
    router: &MultiServiceRouter,
    region: &str,
    uri: &str,
    body: &Value,
) -> (u16, Option<String>, Value) {
    send_in(router, "POST", region, uri, Some(body)).await
}

async fn post(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let (status, _, parsed) = post_in(router, "us-east-1", uri, body).await;
    (status, parsed)
}

/// `Enable` records exactly the resource scan types the request named, and every other
/// modelled type stays off: the same table is then read back through
/// `BatchGetAccountStatus`, which must agree with what `Enable` stored.
#[tokio::test]
async fn enable_records_exactly_the_resource_types_requested() {
    // Rows differ along WHICH and HOW MANY scan types are named, including the
    // modelled list bounds (1 and all 5) and a type the issue never cited.
    let cases: [&[&str]; 6] = [
        &["EC2"],
        &["ECR"],
        &["CODE_REPOSITORY"],
        &["EC2", "ECR"],
        &["LAMBDA", "LAMBDA_CODE"],
        &RESOURCE_SCAN_TYPES,
    ];

    for requested in cases {
        let router = router();

        // Before Enable, the account reports everything off.
        let (status, before) = post(&router, "/status/batch/get", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {requested:?}: BatchGetAccountStatus before Enable: expected 200, got \
             {status} / {before}"
        );
        assert_eq!(
            before["accounts"][0]["state"]["status"], "DISABLED",
            "case {requested:?}: a never-enabled account must report DISABLED, got {before}"
        );

        let (status, enabled) =
            post(&router, "/enable", &json!({ "resourceTypes": requested })).await;
        assert_eq!(
            status, 200,
            "case {requested:?}: Enable: expected 200, got {status} / {enabled}"
        );
        assert_eq!(
            enabled["accounts"][0]["accountId"], ACCOUNT,
            "case {requested:?}: Enable must default to the calling account, got {enabled}"
        );
        assert_eq!(
            enabled["accounts"][0]["status"], "ENABLED",
            "case {requested:?}: Enable must report the account ENABLED, got {enabled}"
        );

        let (status, after) = post(&router, "/status/batch/get", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {requested:?}: BatchGetAccountStatus after Enable: expected 200, got \
             {status} / {after}"
        );

        for resource_type in RESOURCE_SCAN_TYPES {
            let expected = if requested.contains(&resource_type) {
                "ENABLED"
            } else {
                "DISABLED"
            };
            let member = resource_member(resource_type);
            assert_eq!(
                enabled["accounts"][0]["resourceStatus"][member], expected,
                "case {requested:?}: Enable's resourceStatus.{member}: expected {expected}, \
                 got {enabled}"
            );
            assert_eq!(
                after["accounts"][0]["resourceState"][member]["status"], expected,
                "case {requested:?}: BatchGetAccountStatus's resourceState.{member}: expected \
                 {expected}, got {after}"
            );
        }
    }
}

/// `Enable` acts on the accounts the request names, and on the calling account only when
/// it names none. `BatchGetAccountStatus` reports each requested account from its own
/// record, so an account that was not enabled still reports DISABLED.
#[tokio::test]
async fn enable_and_status_are_per_account() {
    let other = "012345678901";
    let third = "109876543210";
    let router = router();

    let (status, enabled) = post(
        &router,
        "/enable",
        &json!({ "resourceTypes": ["EC2"], "accountIds": [ACCOUNT, other] }),
    )
    .await;
    assert_eq!(status, 200, "Enable: expected 200, got {status} / {enabled}");
    let ids: Vec<&str> = enabled["accounts"]
        .as_array()
        .expect("accounts is a list")
        .iter()
        .map(|account| account["accountId"].as_str().expect("an accountId"))
        .collect();
    assert_eq!(
        ids,
        vec![ACCOUNT, other],
        "Enable must report every requested account, got {enabled}"
    );

    let (status, reported) = post(
        &router,
        "/status/batch/get",
        &json!({ "accountIds": [ACCOUNT, other, third] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetAccountStatus: expected 200, got {status} / {reported}"
    );
    for (index, (account, expected)) in [(ACCOUNT, "ENABLED"), (other, "ENABLED"), (third, "DISABLED")]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            reported["accounts"][index]["accountId"], account,
            "case {account}: reported out of request order: {reported}"
        );
        assert_eq!(
            reported["accounts"][index]["state"]["status"], expected,
            "case {account}: expected {expected}, got {reported}"
        );
    }
}

/// A repeated `@idempotencyToken` makes `Enable` a replay: the second call reports the
/// enablement the first one stored rather than widening it.
#[tokio::test]
async fn enable_is_idempotent_for_a_repeated_client_token() {
    let router = router();
    let first = json!({ "resourceTypes": ["EC2"], "clientToken": "token-1" });
    let (status, _) = post(&router, "/enable", &first).await;
    assert_eq!(status, 200, "first Enable: expected 200, got {status}");

    // Same token, more resource types: a replay, so nothing changes.
    let replay = json!({ "resourceTypes": ["EC2", "ECR"], "clientToken": "token-1" });
    let (status, replayed) = post(&router, "/enable", &replay).await;
    assert_eq!(
        status, 200,
        "replayed Enable: expected 200, got {status} / {replayed}"
    );
    assert_eq!(
        replayed["accounts"][0]["resourceStatus"]["ecr"], "DISABLED",
        "a replayed clientToken must not widen the stored enablement, got {replayed}"
    );

    // A different token is a new request and does widen it.
    let fresh = json!({ "resourceTypes": ["EC2", "ECR"], "clientToken": "token-2" });
    let (status, widened) = post(&router, "/enable", &fresh).await;
    assert_eq!(
        status, 200,
        "second Enable: expected 200, got {status} / {widened}"
    );
    assert_eq!(
        widened["accounts"][0]["resourceStatus"]["ecr"], "ENABLED",
        "a new clientToken must apply the request, got {widened}"
    );
}

/// `CreateFilter` stores exactly what it was sent and `ListFilters` renders it back: the
/// rows differ along action, criteria shape, and which optional members are present.
#[tokio::test]
async fn created_filters_round_trip_through_list_filters() {
    let cases: [(&str, Value); 4] = [
        (
            "suppress-lows",
            json!({
                "action": "SUPPRESS",
                "name": "suppress-lows",
                "filterCriteria": { "severity": [{ "comparison": "EQUALS", "value": "LOW" }] },
                "description": "tune out the noise",
                "reason": "accepted risk",
                "tags": { "team": "appsec" }
            }),
        ),
        (
            "no-op-filter",
            json!({
                "action": "NONE",
                "name": "no-op-filter",
                "filterCriteria": { "awsAccountId": [{ "comparison": "EQUALS", "value": ACCOUNT }] }
            }),
        ),
        (
            "multi-criteria",
            json!({
                "action": "SUPPRESS",
                "name": "multi-criteria",
                "filterCriteria": {
                    "resourceType": [{ "comparison": "NOT_EQUALS", "value": "AWS_EC2_INSTANCE" }],
                    "ecrImageTags": [
                        { "comparison": "EQUALS", "value": "latest" },
                        { "comparison": "PREFIX", "value": "release-" }
                    ],
                    "inspectorScore": [{ "lowerInclusive": 1.0, "upperInclusive": 7.5 }]
                },
                "tags": { "team": "appsec", "env": "prod", "owner": "platform" }
            }),
        ),
        (
            "x",
            json!({
                "action": "NONE",
                "name": "x",
                "filterCriteria": { "title": [{ "comparison": "PREFIX", "value": "CVE-" }] }
            }),
        ),
    ];

    for (name, request) in cases {
        let router = router();
        let (status, _, created) =
            post_in(&router, "eu-central-1", "/filters/create", &request).await;
        assert_eq!(
            status, 200,
            "case {name}: CreateFilter: expected 200, got {status} / {created}"
        );
        let arn = created["arn"].as_str().unwrap_or_default().to_string();
        let expected_prefix =
            format!("arn:aws:inspector2:eu-central-1:{ACCOUNT}:owner/{ACCOUNT}/filter/");
        assert!(
            arn.starts_with(&expected_prefix),
            "case {name}: expected an ARN starting {expected_prefix}, got {arn:?}"
        );
        assert!(
            !arn.is_empty() && arn.len() <= 128,
            "case {name}: FilterArn is modelled length 1..128, got {} chars: {arn:?}",
            arn.len()
        );

        let (status, listed) = post(&router, "/filters/list", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {name}: ListFilters: expected 200, got {status} / {listed}"
        );
        let filters = listed["filters"].as_array().expect("filters is a list");
        assert_eq!(
            filters.len(),
            1,
            "case {name}: expected exactly the one created filter, got {listed}"
        );
        let filter = &filters[0];
        assert_eq!(filter["arn"], arn, "case {name}: ARN differs: {filter}");
        assert_eq!(
            filter["ownerId"], ACCOUNT,
            "case {name}: ownerId differs: {filter}"
        );
        assert_eq!(filter["name"], request["name"], "case {name}: name differs: {filter}");
        assert_eq!(
            filter["action"], request["action"],
            "case {name}: action differs: {filter}"
        );
        assert_eq!(
            filter["criteria"], request["filterCriteria"],
            "case {name}: criteria must survive the round trip: {filter}"
        );
        for member in ["description", "reason"] {
            let sent = request.get(member).cloned().unwrap_or(Value::Null);
            let got = filter.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                got, sent,
                "case {name}: {member} must be stored as sent (sent {sent}, got {got})"
            );
        }
        // `tags` is always reported, so a filter created without tags reports an empty
        // map rather than leaving the member out (see `filter_tags_are_always_reported`).
        let sent_tags = request.get("tags").cloned().unwrap_or_else(|| json!({}));
        let got_tags = filter.get("tags").cloned().unwrap_or(Value::Null);
        assert_eq!(
            got_tags, sent_tags,
            "case {name}: tags must be reported as stored (sent {sent_tags}, got {got_tags})"
        );
        assert!(
            filter["createdAt"].is_number() && filter["updatedAt"].is_number(),
            "case {name}: createdAt/updatedAt must be timestamps, got {filter}"
        );
        assert_eq!(
            filter["createdAt"], filter["updatedAt"],
            "case {name}: a never-updated filter reports its creation instant as its last \
             update, got {filter}"
        );
    }
}

/// The `arns` and `action` selectors are applied to the stored filters.
#[tokio::test]
async fn list_filters_honours_the_arns_and_action_selectors() {
    let router = router();
    let mut arns = Vec::new();
    for (name, action) in [
        ("one", "SUPPRESS"),
        ("two", "NONE"),
        ("three", "SUPPRESS"),
        ("four", "NONE"),
    ] {
        let (status, created) = post(
            &router,
            "/filters/create",
            &json!({
                "action": action,
                "name": name,
                "filterCriteria": { "title": [{ "comparison": "EQUALS", "value": name }] }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "creating {name}: expected 200, got {status} / {created}"
        );
        arns.push(created["arn"].as_str().expect("an arn").to_string());
    }

    let names = |listed: &Value| -> Vec<String> {
        listed["filters"]
            .as_array()
            .expect("filters is a list")
            .iter()
            .map(|filter| filter["name"].as_str().unwrap_or_default().to_string())
            .collect()
    };

    let cases: [(&str, Value, Vec<&str>); 5] = [
        ("no selector", json!({}), vec!["one", "two", "three", "four"]),
        (
            "action SUPPRESS",
            json!({ "action": "SUPPRESS" }),
            vec!["one", "three"],
        ),
        ("action NONE", json!({ "action": "NONE" }), vec!["two", "four"]),
        (
            "one arn",
            json!({ "arns": [arns[2]] }),
            vec!["three"],
        ),
        (
            "two arns and an action",
            json!({ "arns": [arns[1], arns[3]], "action": "NONE" }),
            vec!["two", "four"],
        ),
    ];

    for (case, request, expected) in cases {
        let (status, listed) = post(&router, "/filters/list", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: ListFilters: expected 200, got {status} / {listed}"
        );
        let mut got = names(&listed);
        got.sort();
        let mut want: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        want.sort();
        assert_eq!(
            got, want,
            "case {case}: expected filters {want:?}, got {got:?} ({listed})"
        );
    }
}

/// `maxResults` truncates the page and the returned `nextToken` fetches exactly the
/// remainder, with no filter seen twice or skipped. Rows cover both published bounds of
/// the modelled 1..100 range and a page size larger than the result set.
#[tokio::test]
async fn list_filters_pages_at_max_results() {
    for (total, page_size) in [(1usize, 1i64), (5, 1), (5, 2), (5, 4), (5, 5), (3, 100)] {
        let router = router();
        let mut created = Vec::new();
        for index in 0..total {
            let name = format!("filter-{index}");
            let (status, response) = post(
                &router,
                "/filters/create",
                &json!({
                    "action": "SUPPRESS",
                    "name": name,
                    "filterCriteria": { "title": [{ "comparison": "EQUALS", "value": name }] }
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case total={total} maxResults={page_size}: CreateFilter: got {status} / {response}"
            );
            created.push(response["arn"].as_str().expect("an arn").to_string());
        }

        let mut seen = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let request = match &next_token {
                Some(token) => json!({ "maxResults": page_size, "nextToken": token }),
                None => json!({ "maxResults": page_size }),
            };
            let (status, listed) = post(&router, "/filters/list", &request).await;
            assert_eq!(
                status, 200,
                "case total={total} maxResults={page_size}: ListFilters: got {status} / {listed}"
            );
            let page = listed["filters"].as_array().expect("filters is a list");
            assert!(
                page.len() as i64 <= page_size,
                "case total={total} maxResults={page_size}: a page returned {} filters",
                page.len()
            );
            for filter in page {
                seen.push(filter["arn"].as_str().expect("an arn").to_string());
            }
            pages += 1;
            assert!(
                pages <= total + 1,
                "case total={total} maxResults={page_size}: pagination did not terminate \
                 after {pages} pages"
            );
            next_token = listed["nextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, created,
            "case total={total} maxResults={page_size}: paging must yield every stored \
             filter exactly once, in the stored order"
        );
    }
}

/// `maxResults` outside the modelled 1..100 range is refused at the boundary, before any
/// filter is read.
#[tokio::test]
async fn list_filters_refuses_a_max_results_outside_the_modelled_range() {
    let router = router();
    for max_results in [0, 101, -1] {
        let (status, error_type, body) = post_in(
            &router,
            "us-east-1",
            "/filters/list",
            &json!({ "maxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case maxResults={max_results}: expected 400, got {status} / {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case maxResults={max_results}: expected ValidationException, got {error_type:?}"
        );
    }
}

/// A `nextToken` this service never minted names no position, so it is refused rather
/// than silently answered from the start of the list.
#[tokio::test]
async fn list_filters_refuses_an_unparseable_next_token() {
    let router = router();
    // Rows: non-hex, odd-length hex, and WELL-FORMED hex that decodes to something this
    // service never minted (hex of "AA" and of a filter ARN of another service).
    let tokens = [
        "not-a-token",
        "zz",
        "0",
        "4141",
        "61726e3a6177733a73333a3a3a6d792d6275636b6574",
    ];
    for token in tokens {
        let (status, error_type, body) = post_in(
            &router,
            "us-east-1",
            "/filters/list",
            &json!({ "nextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status} / {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {token:?}: expected ValidationException, got {error_type:?}"
        );
    }
}

/// `AccountIdSet` is `@uniqueItems`, so a repeated account id is refused at the boundary —
/// no operation ever sees one, and so none can report an account twice.
#[tokio::test]
async fn a_repeated_account_id_is_refused() {
    let other = "012345678901";
    let cases: [(&str, Vec<&str>); 3] = [
        ("one id twice", vec![ACCOUNT, ACCOUNT]),
        ("two ids, one repeated", vec![ACCOUNT, other, ACCOUNT]),
        ("one id four times", vec![other, other, other, other]),
    ];

    for (case, requested) in cases {
        let router = router();
        for (operation, uri) in [("Enable", "/enable"), ("BatchGetAccountStatus", "/status/batch/get")]
        {
            let body = if uri == "/enable" {
                json!({ "resourceTypes": ["EC2"], "accountIds": requested })
            } else {
                json!({ "accountIds": requested })
            };
            let (status, error_type, response) = post_in(&router, "us-east-1", uri, &body).await;
            assert_eq!(
                status, 400,
                "case {case}: {operation}: expected 400, got {status} / {response}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("ValidationException"),
                "case {case}: {operation}: expected ValidationException, got {error_type:?}"
            );
        }
    }

    // And a list of DISTINCT ids is accepted, so the refusal is about the repeat.
    let router = router();
    let (status, enabled) = post(
        &router,
        "/enable",
        &json!({ "resourceTypes": ["EC2"], "accountIds": [ACCOUNT, other] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "distinct ids must be accepted, got {status} / {enabled}"
    );
}

/// `UpdateFilter` applies only the members the request supplied, leaves the rest alone,
/// and refreshes `updatedAt`.
#[tokio::test]
async fn update_filter_mutates_only_the_supplied_members() {
    let original = json!({
        "action": "SUPPRESS",
        "name": "original-name",
        "filterCriteria": { "severity": [{ "comparison": "EQUALS", "value": "LOW" }] },
        "description": "original description",
        "reason": "original reason",
        "tags": { "team": "appsec" }
    });

    let cases: [(&str, Value); 5] = [
        ("description only", json!({ "description": "tuned" })),
        ("name only", json!({ "name": "renamed" })),
        ("action only", json!({ "action": "NONE" })),
        (
            "criteria only",
            json!({
                "filterCriteria": {
                    "severity": [{ "comparison": "EQUALS", "value": "CRITICAL" }]
                }
            }),
        ),
        (
            "several at once",
            json!({ "name": "renamed", "reason": "reviewed", "action": "NONE" }),
        ),
    ];

    for (case, update) in cases {
        let router = router();
        let (status, created) = post(&router, "/filters/create", &original).await;
        assert_eq!(
            status, 200,
            "case {case}: CreateFilter: expected 200, got {status} / {created}"
        );
        let arn = created["arn"].as_str().expect("an arn").to_string();

        let (status, before) = post(&router, "/filters/list", &json!({})).await;
        assert_eq!(status, 200, "case {case}: ListFilters: got {status} / {before}");
        let before = before["filters"][0].clone();

        // A measurable gap, so a refreshed `updatedAt` is strictly later than the
        // creation instant rather than merely not earlier.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let mut request = update.clone();
        request["filterArn"] = Value::String(arn.clone());
        let (status, updated) = post(&router, "/filters/update", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateFilter: expected 200, got {status} / {updated}"
        );
        assert_eq!(
            updated["arn"], arn,
            "case {case}: UpdateFilter must return the filter's ARN, got {updated}"
        );

        let (status, listed) = post(&router, "/filters/list", &json!({})).await;
        assert_eq!(status, 200, "case {case}: ListFilters: got {status} / {listed}");
        let after = listed["filters"][0].clone();

        for (member, input_member) in [
            ("name", "name"),
            ("action", "action"),
            ("criteria", "filterCriteria"),
            ("description", "description"),
            ("reason", "reason"),
        ] {
            let expected = update
                .get(input_member)
                .cloned()
                .unwrap_or_else(|| before[member].clone());
            assert_eq!(
                after[member], expected,
                "case {case}: {member}: expected {expected}, got {}",
                after[member]
            );
        }
        assert_eq!(
            after["tags"], before["tags"],
            "case {case}: UpdateFilter does not take tags, so they must be untouched: {after}"
        );
        assert_eq!(
            after["createdAt"], before["createdAt"],
            "case {case}: createdAt must not move, got {after}"
        );
        let created_at = after["createdAt"].as_f64().expect("a createdAt timestamp");
        let updated_at = after["updatedAt"].as_f64().expect("an updatedAt timestamp");
        assert!(
            updated_at > created_at,
            "case {case}: updatedAt must be refreshed (createdAt {created_at}, updatedAt \
             {updated_at})"
        );
    }
}

/// An ARN that names no stored filter is a 404 `ResourceNotFoundException` for both
/// `UpdateFilter` and `DeleteFilter` — including the ARN of a filter already deleted.
#[tokio::test]
async fn unknown_filter_arns_are_not_found() {
    let router = router();
    let (status, created) = post(
        &router,
        "/filters/create",
        &json!({
            "action": "SUPPRESS",
            "name": "doomed",
            "filterCriteria": { "title": [{ "comparison": "EQUALS", "value": "doomed" }] }
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateFilter: got {status} / {created}");
    let arn = created["arn"].as_str().expect("an arn").to_string();

    let (status, deleted) = post(&router, "/filters/delete", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "DeleteFilter: expected 200, got {status} / {deleted}"
    );
    assert_eq!(
        deleted["arn"], arn,
        "DeleteFilter must return the deleted filter's ARN, got {deleted}"
    );

    let (status, listed) = post(&router, "/filters/list", &json!({})).await;
    assert_eq!(status, 200, "ListFilters: got {status} / {listed}");
    assert_eq!(
        listed["filters"].as_array().map(Vec::len),
        Some(0),
        "a deleted filter must be gone from ListFilters, got {listed}"
    );

    // Rows: the ARN of a filter that was deleted, and a well-formed ARN that was never
    // minted. A value that is not a well-formed filter ARN at all is a 400
    // `ValidationException` instead, which `malformed_filter_arns_are_refused_as_invalid`
    // covers.
    let unknown = [
        ("already deleted", arn.clone()),
        (
            "never minted",
            format!("arn:aws:inspector2:us-east-1:{ACCOUNT}:owner/{ACCOUNT}/filter/deadbeef"),
        ),
        (
            "another account's",
            "arn:aws:inspector2:us-east-1:111122223333:owner/111122223333/filter/0123456789abcdef"
                .to_string(),
        ),
    ];
    for (case, candidate) in unknown {
        let (status, error_type, body) = post_in(
            &router,
            "us-east-1",
            "/filters/delete",
            &json!({ "arn": candidate }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: DeleteFilter: expected 404, got {status} / {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: DeleteFilter: expected ResourceNotFoundException, got {error_type:?}"
        );

        let (status, error_type, body) = post_in(
            &router,
            "us-east-1",
            "/filters/update",
            &json!({ "filterArn": candidate, "description": "tuned" }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: UpdateFilter: expected 404, got {status} / {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: UpdateFilter: expected ResourceNotFoundException, got {error_type:?}"
        );
    }
}

/// A regional ARN is built from the region the request is signed for, not from a fixed
/// one.
#[tokio::test]
async fn a_filter_arn_carries_the_requests_region() {
    for region in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"] {
        let router = router();
        let (status, _, created) = post_in(
            &router,
            region,
            "/filters/create",
            &json!({
                "action": "NONE",
                "name": "regional",
                "filterCriteria": { "title": [{ "comparison": "EQUALS", "value": "regional" }] }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region}: CreateFilter: expected 200, got {status} / {created}"
        );
        let arn = created["arn"].as_str().unwrap_or_default();
        let expected = format!("arn:aws:inspector2:{region}:{ACCOUNT}:owner/{ACCOUNT}/filter/");
        assert!(
            arn.starts_with(&expected),
            "case {region}: expected an ARN starting {expected}, got {arn:?}"
        );
    }
}

/// The filter store is bounded: once it holds `MAX_STORED_FILTERS` records, `CreateFilter`
/// answers its own modelled `ServiceQuotaExceededException` (402) instead of growing the
/// store, and deleting one makes room again.
#[tokio::test]
async fn create_filter_refuses_past_the_stored_filter_cap() {
    const CAP: usize = 1_000;
    let router = router();
    let new_filter = |index: usize| {
        json!({
            "action": "SUPPRESS",
            "name": format!("filter-{index}"),
            "filterCriteria": { "title": [{ "comparison": "EQUALS", "value": "x" }] }
        })
    };

    let mut last = String::new();
    for index in 0..CAP {
        let (status, created) = post(&router, "/filters/create", &new_filter(index)).await;
        assert_eq!(
            status, 200,
            "case filter #{index}: expected 200 up to the cap, got {status} / {created}"
        );
        last = created["arn"].as_str().expect("an arn").to_string();
    }

    let (status, error_type, body) =
        post_in(&router, "us-east-1", "/filters/create", &new_filter(CAP)).await;
    assert_eq!(
        status, 402,
        "case one past the cap: expected 402, got {status} / {body}"
    );
    assert_eq!(
        error_type.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case one past the cap: expected ServiceQuotaExceededException, got {error_type:?}"
    );

    // The refused request stored nothing.
    let (status, listed) = post(&router, "/filters/list", &json!({ "maxResults": 100 })).await;
    assert_eq!(status, 200, "ListFilters: got {status} / {listed}");
    assert!(
        listed["nextToken"].is_string(),
        "the store should still hold a full cap's worth of filters, got {listed}"
    );

    // Deleting one makes room for exactly one more.
    let (status, deleted) = post(&router, "/filters/delete", &json!({ "arn": last })).await;
    assert_eq!(status, 200, "DeleteFilter: got {status} / {deleted}");
    let (status, created) = post(&router, "/filters/create", &new_filter(CAP)).await;
    assert_eq!(
        status, 200,
        "after a delete there is room for one more filter, got {status} / {created}"
    );
}

/// The account store is bounded: `Enable` refuses to take it past `MAX_ENABLED_ACCOUNTS`,
/// naming the member and the bound, and the refused request stores nothing.
#[tokio::test]
async fn enable_refuses_past_the_enabled_account_cap() {
    const CAP: usize = 1_000;
    // `accountIds` is modelled length 0..100, so the cap is reached in batches of 100.
    const BATCH: usize = 100;
    let router = router();

    for batch in 0..CAP / BATCH {
        let ids: Vec<String> = (0..BATCH)
            .map(|offset| format!("{:012}", batch * BATCH + offset))
            .collect();
        let (status, enabled) = post(
            &router,
            "/enable",
            &json!({ "resourceTypes": ["EC2"], "accountIds": ids }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case batch {batch}: expected 200 up to the cap, got {status} / {enabled}"
        );
    }

    let (status, error_type, body) = post_in(
        &router,
        "us-east-1",
        "/enable",
        &json!({ "resourceTypes": ["EC2"], "accountIds": ["999999999999"] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the cap: expected 400, got {status} / {body}"
    );
    assert_eq!(
        error_type.as_deref(),
        Some("ValidationException"),
        "case one past the cap: expected ValidationException, got {error_type:?}"
    );
    assert_eq!(
        body["fields"][0]["name"], "accountIds",
        "the refusal must name the member it is about, got {body}"
    );

    // Nothing was stored for the refused account, and an account already in the store can
    // still be enabled again.
    let (status, reported) = post(
        &router,
        "/status/batch/get",
        &json!({ "accountIds": ["999999999999", "000000000042"] }),
    )
    .await;
    assert_eq!(status, 200, "BatchGetAccountStatus: got {status} / {reported}");
    assert_eq!(
        reported["accounts"][0]["state"]["status"], "DISABLED",
        "the refused account must not have been stored, got {reported}"
    );
    assert_eq!(
        reported["accounts"][1]["state"]["status"], "ENABLED",
        "an account inside the cap stays enabled, got {reported}"
    );
}

/// Remembered `clientToken`s are a bounded, oldest-first queue: a token older than the
/// most recent `MAX_CLIENT_TOKENS_PER_ACCOUNT` is no longer remembered, so a request
/// carrying it is applied rather than replayed.
#[tokio::test]
async fn remembered_client_tokens_are_bounded() {
    const CAP: usize = 64;
    let router = router();
    let enable = |token: &str, types: &[&str]| {
        json!({ "resourceTypes": types, "clientToken": token })
    };

    let (status, _) = post(&router, "/enable", &enable("token-oldest", &["EC2"])).await;
    assert_eq!(status, 200, "first Enable: expected 200, got {status}");

    // Still remembered: a replay does not widen the enablement.
    let (status, replayed) = post(&router, "/enable", &enable("token-oldest", &["ECR"])).await;
    assert_eq!(status, 200, "replay: expected 200, got {status} / {replayed}");
    assert_eq!(
        replayed["accounts"][0]["resourceStatus"]["ecr"], "DISABLED",
        "a remembered token must still replay, got {replayed}"
    );

    // Push the oldest token out of the queue.
    for index in 0..CAP {
        let token = format!("token-{index}");
        let (status, pushed) = post(&router, "/enable", &enable(&token, &["EC2"])).await;
        assert_eq!(
            status, 200,
            "case {token}: expected 200, got {status} / {pushed}"
        );
    }

    let (status, applied) = post(&router, "/enable", &enable("token-oldest", &["ECR"])).await;
    assert_eq!(
        status, 200,
        "evicted token: expected 200, got {status} / {applied}"
    );
    assert_eq!(
        applied["accounts"][0]["resourceStatus"]["ecr"], "ENABLED",
        "a token past the bound is no longer remembered, so the request applies: {applied}"
    );
}

/// A filter's stored tags round-trip through `ListFilters`: the entries it was created
/// with, and an EMPTY map when it was created with none or with an empty one — the member
/// is never left out. The table over presence and size lives in
/// `filter_tags_are_always_reported`; this row set only pins the round trip.
#[tokio::test]
async fn filter_tags_round_trip_as_sent() {
    let cases: [(&str, Value, Value); 3] = [
        ("absent", Value::Null, json!({})),
        ("empty map", json!({}), json!({})),
        (
            "one entry",
            json!({ "team": "appsec" }),
            json!({ "team": "appsec" }),
        ),
    ];

    for (case, tags, expected) in cases {
        let router = router();
        let mut request = json!({
            "action": "NONE",
            "name": "tagged",
            "filterCriteria": { "title": [{ "comparison": "EQUALS", "value": "tagged" }] }
        });
        if !tags.is_null() {
            request["tags"] = tags.clone();
        }
        let (status, created) = post(&router, "/filters/create", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: CreateFilter: expected 200, got {status} / {created}"
        );

        let (status, listed) = post(&router, "/filters/list", &json!({})).await;
        assert_eq!(status, 200, "case {case}: ListFilters: got {status} / {listed}");
        let got = listed["filters"][0]
            .get("tags")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            got, expected,
            "case {case}: tags must read back as stored (sent {tags}, expected {expected}, \
             got {got})"
        );
    }
}
