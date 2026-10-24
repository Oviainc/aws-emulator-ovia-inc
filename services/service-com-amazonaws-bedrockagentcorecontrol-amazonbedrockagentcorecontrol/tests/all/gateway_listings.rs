// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListGateways`.
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what `CreateGateway` writes is what the listing reads.
//!
//! The rule under test: a gateway listing reports every gateway persisted in this
//! account, each summarised from that gateway's own record — so the listing cannot
//! disagree with `GetGateway` — in one deterministic order, cut into pages of at most
//! `maxResults` entries that resume exactly where the previous page ended. It writes
//! nothing of its own: the only state change a read may materialise is one the clock had
//! already made due (the removal an accepted `DeleteGateway` owed).
//!
//! The target listing on one gateway is `gateway_target_listing.rs`; the create/read round
//! trip is `gateway_lifecycle.rs`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The settle span `src/state.rs` gives a gateway's `CREATING` -> `READY` transition (and
/// the removal an accepted deletion owes), plus slack, so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GatewayRole";

/// The modelled bounds of `maxResults` (`GatewayMaxResults`, range 1..1000, from
/// `smithy_query operation=ListGateways`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// The members `GatewaySummary` shares with `GetGateway`'s envelope
/// (`smithy_query shape=GatewaySummary`), which a listing must report as the read does.
const SHARED_MEMBERS: [&str; 8] = [
    "gatewayId",
    "name",
    "status",
    "description",
    "createdAt",
    "updatedAt",
    "authorizerType",
    "protocolType",
];

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service, which is what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `ListGateways` with an optional page size and resume token.
async fn list_gateways(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut query = Vec::new();
    if let Some(max) = max_results {
        query.push(format!("maxResults={max}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let uri = if query.is_empty() {
        "/gateways/".to_string()
    } else {
        format!("/gateways/?{}", query.join("&"))
    };
    send(router, "GET", &uri, None).await
}

async fn get_gateway(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    send(router, "GET", &format!("/gateways/{id}/"), None).await
}

async fn delete_gateway(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    send(router, "DELETE", &format!("/gateways/{id}/"), None).await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `items` of a listing response.
fn items(body: &Value) -> Vec<Value> {
    body.get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `gatewayId` of every entry of a listing response, in the order reported.
fn listed_ids(body: &Value) -> Vec<String> {
    items(body)
        .iter()
        .map(|entry| text(entry, "gatewayId"))
        .collect()
}

/// A `CreateGateway` body, with `description` and the `authorizerConfiguration` that
/// `CUSTOM_JWT` requires when the row asks for it.
fn gateway_body(name: &str, authorizer_type: &str, description: Option<&str>) -> Value {
    let mut body = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "authorizerType": authorizer_type,
    });
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    if authorizer_type == "CUSTOM_JWT" {
        body["authorizerConfiguration"] = json!({
            "customJWTAuthorizer": {
                "discoveryUrl": "https://example.com/.well-known/openid-configuration",
                "allowedClients": ["client-1"]
            }
        });
    }
    body
}

/// Create a gateway and return the `gatewayId` it minted.
async fn create_gateway(
    router: &MultiServiceRouter,
    name: &str,
    authorizer_type: &str,
    description: Option<&str>,
) -> String {
    let body = gateway_body(name, authorizer_type, description);
    let (status, created) = send(router, "POST", "/gateways/", Some(&body)).await;
    assert_eq!(
        status, 202,
        "case {name:?}: expected HTTP 202 from CreateGateway, got {status} ({created})"
    );
    text(&created, "gatewayId")
}

/// Every stored gateway is listed, summarised off the same record `GetGateway` reports,
/// in one deterministic order.
///
/// Rows differ along what the summary is built from: the name's shape (which the id is
/// minted from, so the list order is exercised over names that sort differently from
/// their ids), whether a description was stored, and the `authorizerType` — every value
/// `AuthorizerType` declares (`smithy_query shape=AuthorizerType`).
#[tokio::test]
async fn every_stored_gateway_is_listed_as_the_read_reports_it() {
    let router = router();

    // An empty store lists nothing, and that is an empty page rather than a refusal.
    let (status, empty) = list_gateways(&router, None, None).await;
    assert_eq!(
        status, 200,
        "case \"an empty store\": expected HTTP 200 from ListGateways, got {status} ({empty})"
    );
    assert!(
        items(&empty).is_empty(),
        "case \"an empty store\": expected no items, got {empty}"
    );
    assert!(
        empty.get("nextToken").is_none_or(Value::is_null),
        "case \"an empty store\": expected no nextToken, got {}",
        empty["nextToken"]
    );

    // (case, name, authorizerType, description)
    let rows: [(&str, &str, &str, Option<&str>); 5] = [
        ("a name that sorts last, AWS_IAM", "zulu", "AWS_IAM", None),
        (
            "mixed case, which the id lowers, with a description",
            "MixedCase",
            "NONE",
            Some("the mixed-case gateway"),
        ),
        (
            "dashes between characters, AUTHENTICATE_ONLY",
            "alpha-gateway",
            "AUTHENTICATE_ONLY",
            None,
        ),
        (
            "a single character, CUSTOM_JWT with its configuration",
            "g",
            "CUSTOM_JWT",
            Some("one character"),
        ),
        ("a name ending in a dash", "trailing-dash-", "AWS_IAM", None),
    ];

    let mut created_ids = Vec::new();
    for (case, name, authorizer_type, description) in rows {
        let id = create_gateway(&router, name, authorizer_type, description).await;
        assert!(
            !id.is_empty(),
            "case {case:?}: expected CreateGateway to mint a gatewayId"
        );
        created_ids.push(id);
    }
    created_ids.sort();

    // The clock-owed transition is settled first, so the listing and the read are compared
    // at an instant neither of them can be mid-transition at.
    tokio::time::sleep(PAST_SETTLE).await;

    let (status, listed) = list_gateways(&router, None, None).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ListGateways, got {status} ({listed})"
    );
    assert_eq!(
        listed_ids(&listed),
        created_ids,
        "expected every stored gateway, in ascending gatewayId order; got {:?} for {created_ids:?}",
        listed_ids(&listed)
    );
    assert!(
        listed.get("nextToken").is_none_or(Value::is_null),
        "a page that holds every gateway carries no nextToken, got {}",
        listed["nextToken"]
    );

    for entry in items(&listed) {
        let id = text(&entry, "gatewayId");
        let (status, fetched) = get_gateway(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {id:?}: expected HTTP 200 from GetGateway, got {status} ({fetched})"
        );
        for member in SHARED_MEMBERS {
            assert_eq!(
                entry[member], fetched[member],
                "case {id:?}: the listing must report the {member} GetGateway reports, got {} \
                 for {}",
                entry[member], fetched[member]
            );
        }
        // `DateTimestamp` carries `@timestampFormat("date-time")`, so every timestamp of a
        // summary is an RFC 3339 STRING on this service's restJson1 wire, never an epoch
        // number (`smithy_query shape=DateTimestamp`).
        for member in ["createdAt", "updatedAt"] {
            let rendered = entry[member].as_str().unwrap_or_default();
            assert!(
                rendered.contains('T') && rendered.ends_with('Z'),
                "case {id:?}: expected {member} as an RFC 3339 date-time string, got {}",
                entry[member]
            );
        }
        assert_eq!(
            text(&entry, "status"),
            "READY",
            "case {id:?}: after the settle span a gateway is READY, got {}",
            text(&entry, "status")
        );
    }
}

/// The status a listing reports is the one the clock owes, materialised on a read and not
/// by it: `CREATING` until the span passes, `READY` afterwards, and every later listing
/// reports the same.
#[tokio::test]
async fn a_gateway_listing_reports_the_status_the_clock_owes() {
    let router = router();
    let id = create_gateway(&router, "settling", "AWS_IAM", None).await;

    let (_, listed) = list_gateways(&router, None, None).await;
    assert_eq!(
        text(&items(&listed)[0], "status"),
        "CREATING",
        "case \"straight after the create\": expected CREATING, got {}",
        text(&items(&listed)[0], "status")
    );

    tokio::time::sleep(PAST_SETTLE).await;
    for case in ["the first listing past the settle span", "a second listing"] {
        let (_, listed) = list_gateways(&router, None, None).await;
        assert_eq!(
            listed_ids(&listed),
            vec![id.clone()],
            "case {case:?}: expected the one stored gateway, got {:?}",
            listed_ids(&listed)
        );
        assert_eq!(
            text(&items(&listed)[0], "status"),
            "READY",
            "case {case:?}: expected READY, got {}",
            text(&items(&listed)[0], "status")
        );
    }
}

/// A listing pages over the stored gateways: every page holds at most `maxResults`
/// entries, each page resumes where the previous one ended, the last carries no token,
/// and the whole walk reports every gateway exactly once in the one order — for page
/// sizes below, at and above the number stored, including each modelled bound.
///
/// The gateways are created in an order that is NOT their minted-id order (each id
/// carries a fresh suffix), so comparing the walk against the sorted ids asserts the
/// cursor really resumes by key rather than by position.
#[tokio::test]
async fn a_gateway_listing_pages_over_every_stored_gateway() {
    let router = router();
    let mut created_ids = Vec::new();
    for index in 0..5 {
        created_ids.push(create_gateway(&router, &format!("page{index}"), "AWS_IAM", None).await);
    }
    created_ids.sort();

    // (case, maxResults)
    let rows: [(&str, Option<i64>); 6] = [
        ("no maxResults, so the modelled maximum", None),
        ("one at a time (the modelled minimum)", Some(MAX_RESULTS_MIN)),
        ("two at a time", Some(2)),
        ("exactly the number stored", Some(5)),
        ("more than the number stored", Some(7)),
        ("the modelled maximum", Some(MAX_RESULTS_MAX)),
    ];

    for (case, max_results) in rows {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (status, page) = list_gateways(&router, max_results, token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from ListGateways, got {status} ({page})"
            );
            let page_ids = listed_ids(&page);
            if let Some(max) = max_results {
                assert!(
                    page_ids.len() as i64 <= max,
                    "case {case:?}: a page holds at most maxResults entries, got {} ({page})",
                    page_ids.len()
                );
            }
            walked.extend(page_ids.iter().cloned());
            match page.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        !page_ids.is_empty(),
                        "case {case:?}: a page that hands back a token is not empty ({page})"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            walked, created_ids,
            "case {case:?}: the whole walk reports every stored gateway exactly once, in order; \
             got {walked:?} for {created_ids:?}"
        );
    }
}

/// What `ListGateways` refuses, and with which modelled error: each published bound of
/// `maxResults` is served and one past it is refused, and a `nextToken` this service never
/// issued is refused rather than silently answered with page one.
#[tokio::test]
async fn a_gateway_listing_refuses_what_the_model_states_it_must() {
    let router = router();
    create_gateway(&router, "refusals", "AWS_IAM", None).await;

    // (case, maxResults, expected status, expected error code)
    let rows: [(&str, i64, u16, &str); 4] = [
        ("maxResults at the modelled minimum", MAX_RESULTS_MIN, 200, ""),
        ("maxResults at the modelled maximum", MAX_RESULTS_MAX, 200, ""),
        (
            "maxResults one below the minimum",
            MAX_RESULTS_MIN - 1,
            400,
            "ValidationException",
        ),
        (
            "maxResults one above the maximum",
            MAX_RESULTS_MAX + 1,
            400,
            "ValidationException",
        ),
    ];
    for (case, max_results, expected_status, expected_code) in rows {
        let (status, answer) = list_gateways(&router, Some(max_results), None).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({answer})"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&answer),
                expected_code,
                "case {case:?}: expected the modelled {expected_code}, got {} ({answer})",
                error_code(&answer)
            );
        }
    }

    // Every token below is inside `GatewayNextToken`'s length (1..2048) and pattern
    // (`^\S*$`), so each refusal is the handler's and not the boundary's.
    let bogus: [(&str, &str); 3] = [
        ("a token that is not the form this service issues", "not-a-token"),
        ("a token with an odd number of hex digits", "abc"),
        ("a token whose bytes are not text", "ff"),
    ];
    for (case, token) in bogus {
        let (status, answer) = list_gateways(&router, None, Some(token)).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({token}): expected HTTP 400 ValidationException, got {status} {} \
             ({answer})",
            error_code(&answer)
        );
    }

    // A token this service DID issue still resumes the walk after the refusals above: a
    // refused page changed nothing.
    let (_, page) = list_gateways(&router, Some(1), None).await;
    assert_eq!(
        items(&page).len(),
        1,
        "case \"a page of one after the refusals\": expected one entry, got {page}"
    );
}

/// A listing is a read: it reports the state the store holds and changes nothing a later
/// caller sees. The one change it may materialise is the removal an accepted
/// `DeleteGateway` already owed to the clock.
#[tokio::test]
async fn a_gateway_listing_changes_nothing_and_drops_what_the_clock_removed() {
    let router = router();
    let kept = create_gateway(&router, "kept", "AWS_IAM", None).await;
    let doomed = create_gateway(&router, "doomed", "AWS_IAM", None).await;
    let mut both = vec![kept.clone(), doomed.clone()];
    both.sort();

    tokio::time::sleep(PAST_SETTLE).await;

    // Two consecutive listings answer identically: nothing was consumed by the read.
    let (_, first) = list_gateways(&router, None, None).await;
    let (_, again) = list_gateways(&router, None, None).await;
    assert_eq!(
        again, first,
        "case \"a second listing\": expected the same answer, got {again} for {first}"
    );
    assert_eq!(
        listed_ids(&first),
        both,
        "case \"both gateways stored\": expected both, got {:?}",
        listed_ids(&first)
    );

    // An accepted deletion is reported as `DELETING` while the clock still owes the
    // removal, and the gateway leaves the listing once that removal is due.
    let (status, deleted) = delete_gateway(&router, &doomed).await;
    assert_eq!(
        status, 202,
        "case \"DeleteGateway on a READY gateway\": expected HTTP 202, got {status} ({deleted})"
    );
    let (_, deleting) = list_gateways(&router, None, None).await;
    let still_listed: Vec<Value> = items(&deleting)
        .into_iter()
        .filter(|entry| text(entry, "gatewayId") == doomed)
        .collect();
    assert_eq!(
        still_listed.len(),
        1,
        "case \"a deletion the clock has not finished\": the gateway is still listed, got \
         {deleting}"
    );
    assert_eq!(
        text(&still_listed[0], "status"),
        "DELETING",
        "case \"a deletion the clock has not finished\": expected DELETING, got {}",
        text(&still_listed[0], "status")
    );

    tokio::time::sleep(PAST_SETTLE).await;
    let (status, settled) = list_gateways(&router, None, None).await;
    assert_eq!(
        status, 200,
        "case \"past the removal the clock owed\": expected HTTP 200, got {status} ({settled})"
    );
    assert_eq!(
        listed_ids(&settled),
        vec![kept.clone()],
        "case \"past the removal the clock owed\": expected only the gateway that was kept, got \
         {:?}",
        listed_ids(&settled)
    );
    let (status, gone) = get_gateway(&router, &doomed).await;
    assert_eq!(
        (status, error_code(&gone).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"GetGateway on the removed gateway\": expected HTTP 404 ResourceNotFoundException, \
         got {status} {} ({gone})",
        error_code(&gone)
    );
}
