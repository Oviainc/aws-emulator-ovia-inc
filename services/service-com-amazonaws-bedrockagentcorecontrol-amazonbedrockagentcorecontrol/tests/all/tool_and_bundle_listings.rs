// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for two listings of this service: `ListBrowsers` and
//! `ListConfigurationBundles`.
//!
//! The rule under test: a listing reports exactly the resources the account holds, from
//! the same records a read of one of them answers from — so a refused create contributes
//! nothing to it, a create replayed under one `clientToken` contributes ONE entry, and
//! `ListBrowsers`'s `type` filter selects between the browsers this account created and
//! the browser tool the SERVICE provides in every account.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request in this suite signs for.
const REGION: &str = "ap-south-1";

/// The browser tool the SERVICE provides in every account, which `ResourceType`'s
/// `SYSTEM` member names (`smithy_query(query="shape", shape="ResourceType")`).
const SYSTEM_BROWSER_ID: &str = "aws.browser.v1";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
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
        .header("host", "bedrock-agentcore-control.amazonaws.com")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        );
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

/// `ListBrowsers`: `POST /browsers`, optionally filtered by `type`.
async fn list_browsers(router: &MultiServiceRouter, kind: Option<&str>) -> (u16, Value) {
    let uri = match kind {
        Some(kind) => format!("/browsers?type={kind}"),
        None => "/browsers".to_string(),
    };
    send(router, "POST", &uri, Some(&json!({}))).await
}

/// `ListConfigurationBundles`: `POST /configuration-bundles`.
async fn list_bundles(router: &MultiServiceRouter) -> (u16, Value) {
    send(router, "POST", "/configuration-bundles", Some(&json!({}))).await
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

/// The ids a browser listing reported, sorted.
fn browser_ids(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> = body["browserSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["browserId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// `ListBrowsers` reports the browsers this account created and the one the service
/// provides, and its `type` filter selects between them.
///
/// The browser a refused create would have made is in none of the three listings: a
/// `networkMode` of `VPC` with no `vpcConfig` is refused before any state is written, so
/// the name it asked for appears nowhere.
#[tokio::test]
async fn list_browsers_reports_what_the_account_holds_and_filters_by_type() {
    let router = router();

    // Nothing created yet: only the tool the service provides is there, and the CUSTOM
    // filter selects none of it.
    let (status, listed) = list_browsers(&router, None).await;
    assert_eq!(
        (status, browser_ids(&listed)),
        (200, vec![SYSTEM_BROWSER_ID.to_string()]),
        "an account that created no browser still holds the service's own, got {status} \
         ({listed})"
    );
    let (status, listed) = list_browsers(&router, Some("CUSTOM")).await;
    assert_eq!(
        (status, browser_ids(&listed)),
        (200, Vec::<String>::new()),
        "the CUSTOM filter selects only what this account created, got {status} ({listed})"
    );

    // A create whose network configuration the model refuses leaves nothing behind: "this
    // configuration is required when the network mode is set to VPC".
    let (status, refused) = send(
        &router,
        "PUT",
        "/browsers",
        Some(&json!({
            "name": "VpcWithNoConfig",
            "networkConfiguration": {"networkMode": "VPC"},
        })),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "a VPC browser with no vpcConfig is refused, got {status} ({refused})"
    );

    // Two browsers this account really created, one of them in the VPC mode WITH the
    // configuration that mode requires.
    let mut created = Vec::new();
    for (case, name, network) in [
        ("a public browser", "PublicBrowser", json!({"networkMode": "PUBLIC"})),
        (
            "a VPC browser with the configuration that mode requires",
            "VpcBrowser",
            json!({
                "networkMode": "VPC",
                "vpcConfig": {
                    "subnets": ["subnet-0123456789abcdef0"],
                    "securityGroups": ["sg-0123456789abcdef0"],
                },
            }),
        ),
    ] {
        let (status, answer) = send(
            &router,
            "PUT",
            "/browsers",
            Some(&json!({"name": name, "networkConfiguration": network})),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateBrowser, got {status} ({answer})"
        );
        created.push(
            answer["browserId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the create reported no browserId"))
                .to_string(),
        );
    }
    created.sort();

    // (case, the filter, the ids expected)
    let mut everything = created.clone();
    everything.push(SYSTEM_BROWSER_ID.to_string());
    everything.sort();
    let rows: [(&str, Option<&str>, Vec<String>); 3] = [
        ("no filter, so every type", None, everything),
        ("only what this account created", Some("CUSTOM"), created),
        (
            "only what the service provides",
            Some("SYSTEM"),
            vec![SYSTEM_BROWSER_ID.to_string()],
        ),
    ];

    for (case, filter, expected) in rows {
        let (status, listed) = list_browsers(&router, filter).await;
        assert_eq!(
            (status, browser_ids(&listed)),
            (200, expected),
            "case {case:?}: unexpected listing, got {status} ({listed})"
        );
        assert!(
            !listed.to_string().contains("VpcWithNoConfig"),
            "case {case:?}: the refused create's name appears in the listing, in {listed}"
        );
    }

    // A listing reports the same records a read of one browser answers from.
    let (status, listed) = list_browsers(&router, Some("CUSTOM")).await;
    assert_eq!(status, 200, "the CUSTOM listing is served, got {status}");
    for summary in listed["browserSummaries"].as_array().unwrap_or(&Vec::new()) {
        let browser_id = summary["browserId"].as_str().unwrap_or_default();
        let (status, read) = send(
            &router,
            "GET",
            &format!("/browsers/{browser_id}"),
            None,
        )
        .await;
        assert_eq!(
            (
                status,
                read["name"].clone(),
                read["status"].clone(),
                read["browserArn"].clone()
            ),
            (
                200,
                summary["name"].clone(),
                summary["status"].clone(),
                summary["browserArn"].clone()
            ),
            "the listing and the read report one browser alike, got {status} ({read}) against \
             {summary}"
        );
    }
}

/// `CreateConfigurationBundle` is idempotent over `clientToken`: concurrent creates that
/// share one token and one request all resolve to the SAME bundle, and the account holds
/// exactly one bundle for them.
///
/// The rows differ along how many times the create is replayed under the one token.
#[tokio::test]
async fn a_bundle_create_replayed_under_one_token_holds_one_bundle() {
    let router = router();

    // (case, bundle name, the token, how many times the create is sent)
    let rows: [(&str, &str, &str, usize); 3] = [
        ("one create", "BundleOnce", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 1),
        ("the create replayed twice", "BundleTwice", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", 2),
        (
            "the create replayed three times, as a racing client retries it",
            "BundleThrice",
            "cccccccccccccccccccccccccccccccccc",
            3,
        ),
    ];

    for (case, bundle_name, token, attempts) in rows {
        let body = json!({
            "bundleName": bundle_name,
            "clientToken": token,
            "components": {"runtime": {"configuration": {"model": "a-model"}}},
        });
        let mut answers = Vec::new();
        for _ in 0..attempts {
            let (status, created) = send(
                &router,
                "POST",
                "/configuration-bundles/create",
                Some(&body),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: every replayed create succeeds, got {status} ({created})"
            );
            answers.push(created);
        }

        // Every answer names one bundle and one version of it.
        for (attempt, answer) in answers.iter().enumerate() {
            assert_eq!(
                (
                    answer["bundleId"].clone(),
                    answer["versionId"].clone(),
                    answer["bundleArn"].clone()
                ),
                (
                    answers[0]["bundleId"].clone(),
                    answers[0]["versionId"].clone(),
                    answers[0]["bundleArn"].clone()
                ),
                "case {case:?}: attempt {attempt} resolved to another bundle, got {answer}"
            );
        }

        // And the account holds exactly one bundle under that name.
        let (status, listed) = list_bundles(&router).await;
        let matching: Vec<&Value> = listed["bundles"]
            .as_array()
            .map(|bundles| {
                bundles
                    .iter()
                    .filter(|bundle| bundle["bundleName"] == json!(bundle_name))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            (status, matching.len()),
            (200, 1),
            "case {case:?}: expected exactly one bundle named {bundle_name}, got {status} \
             ({listed})"
        );
        assert_eq!(
            matching[0]["bundleId"],
            answers[0]["bundleId"],
            "case {case:?}: the listing names the bundle the create answered with, got {listed}"
        );
    }

    // The three bundles above are the three the account holds.
    let (status, listed) = list_bundles(&router).await;
    let mut names: Vec<String> = listed["bundles"]
        .as_array()
        .map(|bundles| {
            bundles
                .iter()
                .map(|bundle| bundle["bundleName"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    assert_eq!(
        (status, names),
        (
            200,
            vec![
                "BundleOnce".to_string(),
                "BundleThrice".to_string(),
                "BundleTwice".to_string()
            ]
        ),
        "the listing reports every bundle once, got {status} ({listed})"
    );
}
