// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `GetStages`.
//!
//! The rule these cases assert, one level above the recorded case that filed the issue:
//!
//! > `GetStages` reports the `Stage` records the named api OWNS, each one exactly as the
//! > store holds it and in `StageName` order, in a page of at most `MaxResults` items
//! > (and never more than the service's page maximum) chained through `NextToken`, so
//! > walking the chain yields every one of that api's stages exactly once whatever page
//! > size the caller asks for; an api identifier nothing minted is a 404
//! > `NotFoundException`, a `MaxResults` that is not a positive integer is a 400
//! > `BadRequestException`, and the listing changes nothing.
//!
//! The rows vary what the rule generalises over: how many stages the api holds (0 up to
//! the per-api quota), the page size (1 up to one past the total, plus the page maximum
//! and one past it), the owning api's protocol type, the form of the stage name, which
//! optional members the stage carries (description, `AutoDeploy`, stage variables, route
//! settings, access log settings), and whether the CALLER created the stage or the
//! service synthesized it for a quick-create api. The recorded unit that filed the issue
//! — chaining `GetStages` over an api's stages — is one row.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The largest page a paginated ApiGatewayV2 reader serves, and the page size used when
/// a request names no `MaxResults` (`crate::state::MAX_PAGE_SIZE`).
const MAX_PAGE_SIZE: usize = 100;

/// AWS's published API Gateway quota of stages per API, the most stages one api can be
/// made to hold (`crate::state::MAX_STAGES_PER_API`).
const MAX_STAGES_PER_API: usize = 10;

/// The name of the stage that serves an api's root.
const DEFAULT_STAGE_NAME: &str = "$default";

/// The sentence API Gateway refuses a pagination cursor it cannot read with: it names
/// the cursor the POSITION in the collection (`crate::state::INVALID_NEXT_TOKEN_MESSAGE`).
const INVALID_POSITION_PARAMETER: &str = "Invalid position parameter";

fn http_api(name: &str) -> Value {
    json!({ "name": name, "protocolType": "HTTP" })
}

fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

/// Create one api and answer its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Create one stage under `api_id` and answer its `StageName`.
async fn create_stage(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: &Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateStage, got {status} {created}"
    );
    as_str(&created, "stageName")
}

/// Walk `GetStages` from the first page to the last and answer every `StageName` seen,
/// in the order the pages reported them.
async fn walk_pages(
    emulator: &MultiServiceRouter,
    label: &str,
    api_id: &str,
    page_size: usize,
    total: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than pages are possible, so a token that never clears is
    // caught instead of looping forever.
    for _ in 0..total + 2 {
        let uri = match &token {
            Some(t) => format!("/v2/apis/{api_id}/stages?maxResults={page_size}&nextToken={t}"),
            None => format!("/v2/apis/{api_id}/stages?maxResults={page_size}"),
        };
        let (status, body) = call(emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetStages, got {status} {body}"
        );
        let items = body["items"].as_array().cloned().unwrap_or_default();
        assert!(
            items.len() <= page_size,
            "case {label:?}: a page must hold at most {page_size} items, got {body}"
        );
        seen.extend(items.iter().map(|item| as_str(item, "stageName")));
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {label:?}: expected the chain to end, got a NextToken {token:?} after {} items",
        seen.len()
    );
    seen
}

#[tokio::test]
async fn get_stages_pages_every_stage_exactly_once_at_every_page_size() {
    // The rule generalises over how many stages exist and over page size, so the table
    // walks both: each count is paged at every size from 1 up to one past it. The
    // largest count is the per-api quota, the most stages an api can hold.
    for total in [0_usize, 1, 2, 5, MAX_STAGES_PER_API] {
        let emulator = emulator();
        let api_id = create_api(
            &emulator,
            &format!("total={total}"),
            &http_api(&format!("paged-stages-{total}")),
        )
        .await;

        let mut expected = BTreeSet::new();
        for index in 0..total {
            expected.insert(
                create_stage(
                    &emulator,
                    &format!("total={total}, stage #{index}"),
                    &api_id,
                    &json!({ "stageName": format!("stage-{index}") }),
                )
                .await,
            );
        }

        for page_size in 1..=total + 1 {
            let label = format!("total={total}, maxResults={page_size}");
            let seen = walk_pages(&emulator, &label, &api_id, page_size, total).await;
            assert_eq!(
                seen.len(),
                total,
                "case {label:?}: expected {total} stages across the chain, got {seen:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "case {label:?}: expected exactly the stored stages, got {seen:?}"
            );
            let mut sorted = seen.clone();
            sorted.sort();
            assert_eq!(
                seen, sorted,
                "case {label:?}: expected the chain in StageName order, got {seen:?}"
            );
        }

        // Unpaged: every stage in one answer, with nothing left to report. An api
        // holding none still reports an empty collection rather than no collection.
        let (status, body) = call(&emulator, "GET", &format!("/v2/apis/{api_id}/stages"), None).await;
        assert_eq!(
            status, 200,
            "case total={total}: expected 200 from an unpaged GetStages, got {status} {body}"
        );
        assert_eq!(
            body["items"].as_array().map(Vec::len),
            Some(total),
            "case total={total}: an unpaged GetStages must report every stage, got {body}"
        );
        assert!(
            body.get("nextToken").is_none() || body["nextToken"].is_null(),
            "case total={total}: an unpaged GetStages has nothing left to report, got {body}"
        );
    }
}

#[tokio::test]
async fn get_stages_reports_each_stored_record_and_only_the_owning_api_s() {
    let emulator = emulator();

    // The rows differ in the owning api's protocol type, the form of the stage name, and
    // which optional members the stage carries.
    let http = create_api(&emulator, "an http api", &http_api("listing-http-api")).await;
    let socket = create_api(
        &emulator,
        "a websocket api",
        &websocket_api("listing-socket-api"),
    )
    .await;
    // A quick-create api: its `$default` stage is one the SERVICE synthesized, and it
    // belongs to the listing exactly like a caller's does.
    let managed = create_api(
        &emulator,
        "a quick-create api",
        &json!({
            "name": "listing-managed-api",
            "protocolType": "HTTP",
            "target": "arn:aws:lambda:eu-west-1:000000000000:function:handler"
        }),
    )
    .await;

    // (owning api, the CreateStage body, the case label).
    let rows: Vec<(&String, Value, &'static str)> = vec![
        (
            &http,
            json!({ "stageName": DEFAULT_STAGE_NAME, "autoDeploy": true }),
            "the $default stage of an HTTP api, auto-deploying",
        ),
        (
            &http,
            json!({ "stageName": "prod", "description": "the live stage" }),
            "a named HTTP stage carrying a description",
        ),
        (
            &http,
            json!({
                "stageName": "with_settings",
                "stageVariables": { "table": "orders", "tier": "gold" },
                "routeSettings": {
                    "GET /items": { "throttlingBurstLimit": 10, "throttlingRateLimit": 5.5 }
                }
            }),
            "a stage carrying stage variables and route settings",
        ),
        (
            &http,
            json!({
                "stageName": "logged",
                "accessLogSettings": {
                    "destinationArn":
                        "arn:aws:logs:eu-west-1:000000000000:log-group:apigw-access",
                    "format": "$context.requestId $context.status"
                }
            }),
            "a stage carrying access log settings",
        ),
        (
            &socket,
            json!({ "stageName": "dev" }),
            "a WebSocket stage carrying no optional member at all",
        ),
    ];

    let mut expected_by_api: BTreeMap<&String, BTreeSet<String>> = BTreeMap::new();
    expected_by_api
        .entry(&managed)
        .or_default()
        .insert(DEFAULT_STAGE_NAME.to_string());

    for (api_id, request, case) in &rows {
        let stage_name = create_stage(&emulator, case, api_id, request).await;
        expected_by_api
            .entry(api_id)
            .or_default()
            .insert(stage_name.clone());

        let (status, body) = call(&emulator, "GET", &format!("/v2/apis/{api_id}/stages"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStages, got {status} {body}"
        );
        let items = body["items"].as_array().cloned().unwrap_or_default();
        let listed = items
            .iter()
            .find(|item| as_str(item, "stageName") == stage_name)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {stage_name} to be listed, got {body}")
            });
        assert!(
            listed.get("createdDate").is_some() && listed.get("lastUpdatedDate").is_some(),
            "case {case:?}: a listed stage reports its instants, got {listed}"
        );

        // The listed record is the one the single-resource reader reports, member for
        // member: the listing projects the store, not a shape of its own.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{stage_name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStage, got {status} {fetched}"
        );
        assert_eq!(
            listed, &fetched,
            "case {case:?}: expected GetStages to report the record GetStage reports, \
             got {listed} and {fetched}"
        );
    }

    // A stage belongs to the one api that owns it: each api lists only its own, and the
    // service-synthesized `$default` stage of the quick-create api reports as managed.
    for (api_id, label) in [
        (&http, "the HTTP api"),
        (&socket, "the WebSocket api"),
        (&managed, "the quick-create api"),
    ] {
        let (status, body) = call(&emulator, "GET", &format!("/v2/apis/{api_id}/stages"), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetStages, got {status} {body}"
        );
        let items = body["items"].as_array().cloned().unwrap_or_default();
        let listed: BTreeSet<String> = items
            .iter()
            .map(|item| as_str(item, "stageName"))
            .collect();
        assert_eq!(
            listed,
            expected_by_api.get(api_id).cloned().unwrap_or_default(),
            "case {label:?}: expected exactly the api's own stages, got {body}"
        );
    }

    let (_, body) = call(&emulator, "GET", &format!("/v2/apis/{managed}/stages"), None).await;
    let items = body["items"].as_array().cloned().unwrap_or_default();
    let synthesized = items
        .iter()
        .find(|item| as_str(item, "stageName") == DEFAULT_STAGE_NAME)
        .unwrap_or_else(|| {
            panic!("case \"the quick-create api\": expected a $default stage, got {body}")
        });
    assert_eq!(
        synthesized.get("apiGatewayManaged"),
        Some(&json!(true)),
        "case \"the quick-create api\": expected the synthesized stage to report \
         apiGatewayManaged=true, got {synthesized}"
    );
}

#[tokio::test]
async fn get_stages_is_a_report_that_changes_nothing() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "a repeated listing", &http_api("repeat-stages-api")).await;
    for index in 0..3 {
        create_stage(
            &emulator,
            &format!("stage #{index}"),
            &api_id,
            &json!({ "stageName": format!("stage-{index}") }),
        )
        .await;
    }

    let uri = format!("/v2/apis/{api_id}/stages?maxResults=2");
    let (_, first) = call(&emulator, "GET", &uri, None).await;
    for attempt in 0..3 {
        let (status, again) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case attempt={attempt}: expected 200 from GetStages, got {status} {again}"
        );
        assert_eq!(
            again, first,
            "case attempt={attempt}: a report must not change what a later caller sees, \
             got {again}"
        );
    }
}

#[tokio::test]
async fn get_stages_refuses_an_unknown_api_and_a_bad_max_results() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "a refusals host", &http_api("refusals-stages-api")).await;

    for case in ["nosuchapi00", "", "0123456789"] {
        let (status, body) = call(&emulator, "GET", &format!("/v2/apis/{case}/stages"), None).await;
        if case.is_empty() {
            // No api id at all does not address this operation at all; whatever the
            // router answers, it is never a listing.
            assert_ne!(
                status, 200,
                "case \"no api id\": expected a refusal, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetStages, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // `MaxResults` is modelled as `__string`, so the numeric rule is the service's: a
    // positive decimal integer. Each row breaks it in a different way.
    for raw in ["0", "-1", "abc", "1.5", "%20"] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages?maxResults={raw}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case maxResults={raw:?}: expected 400 from GetStages, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case maxResults={raw:?}: expected BadRequestException, got {body}"
        );
    }

    // Each published bound of the accepted range, the page maximum and one past it.
    for raw in ["1", "100", "101", "2147483647"] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages?maxResults={raw}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case maxResults={raw:?}: expected 200 from GetStages, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn get_stages_refuses_a_next_token_it_did_not_issue() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "a token host", &http_api("token-stages-api")).await;
    let other_api = create_api(&emulator, "another token host", &http_api("other-stages-api")).await;
    for index in 0..3 {
        for host in [&api_id, &other_api] {
            create_stage(
                &emulator,
                &format!("stage #{index}"),
                host,
                &json!({ "stageName": format!("stage-{index}") }),
            )
            .await;
        }
    }
    // Two routes under the same api, so the sibling collection issues a token of its own.
    for route_key in ["GET /a", "GET /b"] {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({ "routeKey": route_key })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {route_key:?}: expected 201 from CreateRoute, got {status} {created}"
        );
    }

    // A token this service issued for the api's own stages resumes the walk.
    let (status, first) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/stages?maxResults=1"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the first page\": expected 200 from GetStages, got {status} {first}"
    );
    let issued = first
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !issued.is_empty(),
        "case \"the first page\": expected a NextToken, got {first}"
    );
    let (status, resumed) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/stages?maxResults=1&nextToken={issued}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the issued token\": expected 200 from GetStages, got {status} {resumed}"
    );
    assert_eq!(
        resumed["items"].as_array().map(Vec::len),
        Some(1),
        "case \"the issued token\": expected the next page, got {resumed}"
    );

    // A token issued for ANOTHER collection names no position in this one: the token the
    // api's own ROUTES listing issued, and the one another api's stages listing issued.
    let (_, other_stages) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{other_api}/stages?maxResults=1"),
        None,
    )
    .await;
    let other_token = as_str(&other_stages, "nextToken");
    let (_, own_routes) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/routes?maxResults=1"),
        None,
    )
    .await;
    let routes_token = as_str(&own_routes, "nextToken");

    // The rows vary over what a token the service did not issue can look like: a word, a
    // resource name (the shape of a key), one with the alphabet but not the content of an
    // issued token, an empty value, a token of a sibling collection and one of the same
    // collection under another api.
    let mut cases: Vec<(String, String)> = vec![
        ("a word".to_string(), "not-a-token".to_string()),
        ("a stage name".to_string(), "stage-1".to_string()),
        ("hex of nothing this service minted".to_string(), "zzzz".to_string()),
        ("an empty token".to_string(), String::new()),
        (
            "an issued token with its last character dropped".to_string(),
            issued[..issued.len() - 1].to_string(),
        ),
        (
            "a token far longer than any this service mints".to_string(),
            "ab".repeat(4096),
        ),
    ];
    if !routes_token.is_empty() {
        cases.push((
            "a token issued for the api's routes".to_string(),
            routes_token,
        ));
    }
    if !other_token.is_empty() {
        cases.push((
            "a token issued for another api's stages".to_string(),
            other_token,
        ));
    }

    for (case, token) in &cases {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages?nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from GetStages for token {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            INVALID_POSITION_PARAMETER,
            "case {case:?}: expected the message {INVALID_POSITION_PARAMETER:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn a_get_stages_page_is_bounded_server_side() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "a clamped listing", &http_api("clamped-stages-api")).await;
    // The per-api quota is below the page maximum, so every stage of one api fits in one
    // clamped page: what a page size above the maximum must NOT do is serve more than
    // the maximum, and what it must still do is report the whole collection.
    for index in 0..MAX_STAGES_PER_API {
        create_stage(
            &emulator,
            &format!("stage #{index}"),
            &api_id,
            &json!({ "stageName": format!("stage-{index}") }),
        )
        .await;
    }

    for (label, uri) in [
        ("no MaxResults", format!("/v2/apis/{api_id}/stages")),
        (
            "MaxResults far above the maximum",
            format!("/v2/apis/{api_id}/stages?maxResults=1000000"),
        ),
        (
            "MaxResults at the maximum",
            format!("/v2/apis/{api_id}/stages?maxResults={MAX_PAGE_SIZE}"),
        ),
        (
            "MaxResults one past the maximum",
            format!("/v2/apis/{api_id}/stages?maxResults={}", MAX_PAGE_SIZE + 1),
        ),
    ] {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetStages, got {status} {body}"
        );
        let items = body["items"].as_array().cloned().unwrap_or_default();
        assert!(
            items.len() <= MAX_PAGE_SIZE,
            "case {label:?}: a page must hold at most {MAX_PAGE_SIZE} items, got {body}"
        );
        assert_eq!(
            items.len(),
            MAX_STAGES_PER_API,
            "case {label:?}: expected every stage of the api in one page, got {body}"
        );
        assert!(
            body.get("nextToken").is_none() || body["nextToken"].is_null(),
            "case {label:?}: a page holding the whole collection has nothing left to \
             report, got {body}"
        );
    }
}
