// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `GetDeployments`.
//!
//! The rule these cases assert, one level above the recorded case that filed the issue:
//!
//! > `GetDeployments` reports the `Deployment` records the named api OWNS, each one as
//! > the store holds it, in a page of at most `MaxResults` items (and never more than
//! > the service's page maximum) chained through `NextToken`, so walking the chain
//! > yields every one of that api's deployments exactly once whatever page size the
//! > caller asks for; an api identifier nothing minted is a 404 `NotFoundException`, a
//! > `MaxResults` that is not a positive integer is a 400 `BadRequestException`, and the
//! > listing changes nothing.
//!
//! The rows vary what the rule generalises over: how many deployments the api holds (0
//! to 5, plus one past the page maximum), the page size (1 up to one past the total, the
//! page maximum and one past it), the owning api's protocol type, whether the deployment
//! carries a description, and whether the CALLER asked for it or the service released it
//! by itself for an auto-deploying stage. The recorded unit that filed the issue —
//! chaining `GetDeployments` over an api's deployments — is one row.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, assert_minted_id, call, emulator, error_code};

/// The largest page a paginated ApiGatewayV2 reader serves, and the page size used when
/// a request names no `MaxResults` (`crate::state::MAX_PAGE_SIZE`).
const MAX_PAGE_SIZE: usize = 100;

/// The `DeploymentStatus` a released deployment carries, from the values
/// `DeploymentStatus` declares — `PENDING | FAILED | DEPLOYED`
/// (`smithy_query(query="shape", shape="DeploymentStatus")`).
const DEPLOYED: &str = "DEPLOYED";

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

/// Create one deployment under `api_id` and answer its `DeploymentId`.
async fn create_deployment(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: Option<Value>,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/deployments"),
        request,
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateDeployment, got {status} {created}"
    );
    let id = as_str(&created, "deploymentId");
    assert_minted_id(case, "DeploymentId", &id);
    id
}

/// One row of the listing table: the api that owns the deployment, the id the create
/// minted for it, and the members the listing must report for it.
struct Stored<'a> {
    case: &'static str,
    api_id: &'a String,
    deployment_id: String,
    expected: Vec<(&'static str, Value)>,
}

/// Walk `GetDeployments` from the first page to the last and answer every
/// `DeploymentId` seen, in the order the pages reported them.
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
            Some(t) => {
                format!("/v2/apis/{api_id}/deployments?maxResults={page_size}&nextToken={t}")
            }
            None => format!("/v2/apis/{api_id}/deployments?maxResults={page_size}"),
        };
        let (status, body) = call(emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetDeployments, got {status} {body}"
        );
        let items = body["items"].as_array().cloned().unwrap_or_default();
        assert!(
            items.len() <= page_size,
            "case {label:?}: a page must hold at most {page_size} items, got {body}"
        );
        seen.extend(items.iter().map(|item| as_str(item, "deploymentId")));
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
async fn get_deployments_pages_every_deployment_exactly_once_at_every_page_size() {
    // The rule generalises over how many deployments exist and over page size, so the
    // table walks both: each count is paged at every size from 1 up to one past it.
    for total in [0_usize, 1, 2, 5] {
        let emulator = emulator();
        let api_id = create_api(
            &emulator,
            &format!("total={total}"),
            &http_api(&format!("paged-deployments-{total}")),
        )
        .await;

        let mut expected = BTreeSet::new();
        for index in 0..total {
            expected.insert(
                create_deployment(
                    &emulator,
                    &format!("total={total}, deployment #{index}"),
                    &api_id,
                    Some(json!({ "description": format!("deployment #{index}") })),
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
                "case {label:?}: expected {total} deployments across the chain, got {seen:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "case {label:?}: expected exactly the stored deployments, got {seen:?}"
            );
        }

        // Unpaged: every deployment in one answer, with nothing left to report. An api
        // holding none still reports an empty collection rather than no collection.
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/deployments"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case total={total}: expected 200 from an unpaged GetDeployments, got {status} {body}"
        );
        assert_eq!(
            body["items"].as_array().map(Vec::len),
            Some(total),
            "case total={total}: an unpaged GetDeployments must report every deployment, got {body}"
        );
        assert!(
            body.get("nextToken").is_none() || body["nextToken"].is_null(),
            "case total={total}: an unpaged GetDeployments has nothing left to report, got {body}"
        );
    }
}

#[tokio::test]
async fn get_deployments_reports_each_stored_record_and_only_the_owning_api_s() {
    let emulator = emulator();

    // The rows differ in the owning api's protocol type, whether the deployment carries
    // a description, and whether the caller asked for it or the service released it.
    let http = create_api(&emulator, "an http api", &http_api("listing-http-api")).await;
    let socket = create_api(
        &emulator,
        "a websocket api",
        &websocket_api("listing-socket-api"),
    )
    .await;

    let mut stored: Vec<Stored<'_>> = Vec::new();

    let case = "an HTTP api deployed by a request carrying only the api id";
    stored.push(Stored {
        case,
        api_id: &http,
        deployment_id: create_deployment(&emulator, case, &http, Some(json!({}))).await,
        expected: vec![
            ("deploymentStatus", json!(DEPLOYED)),
            ("autoDeployed", json!(false)),
        ],
    });
    let case = "an HTTP api deployed with a description";
    stored.push(Stored {
        case,
        api_id: &http,
        deployment_id: create_deployment(
            &emulator,
            case,
            &http,
            Some(json!({ "description": "the first cut" })),
        )
        .await,
        expected: vec![
            ("deploymentStatus", json!(DEPLOYED)),
            ("autoDeployed", json!(false)),
            ("description", json!("the first cut")),
        ],
    });
    let case = "a WebSocket api deployed by a request with no payload at all";
    stored.push(Stored {
        case,
        api_id: &socket,
        deployment_id: create_deployment(&emulator, case, &socket, None).await,
        expected: vec![
            ("deploymentStatus", json!(DEPLOYED)),
            ("autoDeployed", json!(false)),
        ],
    });

    // A deployment the SERVICE released by itself, for a stage whose `AutoDeploy` is on:
    // it belongs to the api's collection exactly like a caller's does, and reports
    // `AutoDeployed` true.
    let (status, stage) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{http}/stages"),
        Some(json!({ "stageName": "auto", "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"an auto-deploying stage\": expected 201 from CreateStage, got {status} {stage}"
    );
    stored.push(Stored {
        case: "a deployment the service released for an auto-deploying stage",
        api_id: &http,
        deployment_id: as_str(&stage, "deploymentId"),
        expected: vec![
            ("deploymentStatus", json!(DEPLOYED)),
            ("autoDeployed", json!(true)),
        ],
    });

    for Stored {
        case,
        api_id,
        deployment_id,
        expected,
    } in &stored
    {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/deployments"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetDeployments, got {status} {body}"
        );
        let items = body["items"].as_array().cloned().unwrap_or_default();
        let listed = items
            .iter()
            .find(|item| as_str(item, "deploymentId") == *deployment_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {deployment_id} to be listed, got {body}")
            });
        for (member, value) in expected {
            assert_eq!(
                listed.get(member),
                Some(value),
                "case {case:?}: expected the listed deployment to report {member}={value}, \
                 got {listed}"
            );
        }
        assert!(
            listed.get("createdDate").is_some(),
            "case {case:?}: a listed deployment reports its creation instant, got {listed}"
        );

        // The listed record is the one the single-resource reader reports, member for
        // member: the listing projects the store, not a shape of its own.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetDeployment, got {status} {fetched}"
        );
        assert_eq!(
            listed, &fetched,
            "case {case:?}: expected GetDeployments to report the record GetDeployment \
             reports, got {listed} and {fetched}"
        );
    }

    // A deployment belongs to the one api that owns it: each api lists only its own.
    let mut expected_by_api: BTreeMap<&String, BTreeSet<String>> = BTreeMap::new();
    for entry in &stored {
        expected_by_api
            .entry(entry.api_id)
            .or_default()
            .insert(entry.deployment_id.clone());
    }
    for (api_id, label) in [(&http, "the HTTP api"), (&socket, "the WebSocket api")] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/deployments"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetDeployments, got {status} {body}"
        );
        let listed: BTreeSet<String> = body["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| as_str(item, "deploymentId"))
            .collect();
        assert_eq!(
            listed,
            expected_by_api.get(api_id).cloned().unwrap_or_default(),
            "case {label:?}: expected exactly the api's own deployments, got {body}"
        );
    }
}

#[tokio::test]
async fn get_deployments_is_a_report_that_changes_nothing() {
    let emulator = emulator();
    let api_id = create_api(
        &emulator,
        "a repeated listing",
        &http_api("repeat-deployments-api"),
    )
    .await;
    for index in 0..3 {
        create_deployment(
            &emulator,
            &format!("deployment #{index}"),
            &api_id,
            Some(json!({ "description": format!("deployment #{index}") })),
        )
        .await;
    }

    let uri = format!("/v2/apis/{api_id}/deployments?maxResults=2");
    let (_, first) = call(&emulator, "GET", &uri, None).await;
    for attempt in 0..3 {
        let (status, again) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case attempt={attempt}: expected 200 from GetDeployments, got {status} {again}"
        );
        assert_eq!(
            again, first,
            "case attempt={attempt}: a report must not change what a later caller sees, \
             got {again}"
        );
    }
}

#[tokio::test]
async fn a_get_deployments_page_is_bounded_server_side() {
    let emulator = emulator();
    let api_id = create_api(
        &emulator,
        "a clamped listing",
        &http_api("clamped-deployments-api"),
    )
    .await;
    // One past the page maximum, so a page that is not clamped is visible.
    let total = MAX_PAGE_SIZE + 1;
    for index in 0..total {
        create_deployment(
            &emulator,
            &format!("deployment #{index}"),
            &api_id,
            Some(json!({})),
        )
        .await;
    }

    for (label, uri) in [
        ("no MaxResults", format!("/v2/apis/{api_id}/deployments")),
        (
            "MaxResults far above the maximum",
            format!("/v2/apis/{api_id}/deployments?maxResults=1000000"),
        ),
        (
            "MaxResults at the maximum",
            format!("/v2/apis/{api_id}/deployments?maxResults={MAX_PAGE_SIZE}"),
        ),
        (
            "MaxResults one past the maximum",
            format!(
                "/v2/apis/{api_id}/deployments?maxResults={}",
                MAX_PAGE_SIZE + 1
            ),
        ),
    ] {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetDeployments, got {status} {body}"
        );
        assert_eq!(
            body["items"].as_array().map(Vec::len),
            Some(MAX_PAGE_SIZE),
            "case {label:?}: a page must be clamped to {MAX_PAGE_SIZE} items, got {body}"
        );
        assert!(
            body.get("nextToken").and_then(Value::as_str).is_some(),
            "case {label:?}: a clamped page must report what is left through NextToken, got {body}"
        );
    }

    // And the clamped chain still reports every deployment exactly once.
    let seen = walk_pages(&emulator, "clamped chain", &api_id, MAX_PAGE_SIZE, total).await;
    assert_eq!(
        seen.len(),
        total,
        "case \"clamped chain\": expected {total} deployments, got {}",
        seen.len()
    );
    assert_eq!(
        seen.iter().cloned().collect::<BTreeSet<_>>().len(),
        total,
        "case \"clamped chain\": expected every deployment exactly once, got {seen:?}"
    );
}

#[tokio::test]
async fn get_deployments_refuses_an_unknown_api_and_a_bad_max_results() {
    let emulator = emulator();
    let api_id = create_api(
        &emulator,
        "a refusals host",
        &http_api("refusals-deployments-api"),
    )
    .await;

    for case in ["nosuchapi00", "", "0123456789"] {
        let (status, body) =
            call(&emulator, "GET", &format!("/v2/apis/{case}/deployments"), None).await;
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
            "case {case:?}: expected 404 from GetDeployments, got {status} {body}"
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
            &format!("/v2/apis/{api_id}/deployments?maxResults={raw}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case maxResults={raw:?}: expected 400 from GetDeployments, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case maxResults={raw:?}: expected BadRequestException, got {body}"
        );
    }

    // Each published bound of the accepted range, and one past the page maximum.
    for raw in ["1", "100", "101", "2147483647"] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/deployments?maxResults={raw}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case maxResults={raw:?}: expected 200 from GetDeployments, got {status} {body}"
        );
    }
}
