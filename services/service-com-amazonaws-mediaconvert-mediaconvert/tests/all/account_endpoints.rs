// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DescribeEndpoints`: the account's API endpoint per
//! region, the `DEFAULT`/`GET_ONLY` modes, and the request window `MaxResults` and
//! `NextToken` are refused outside.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from the
//! request's SigV4 scope, the framework deserializes and validates, the handler runs, and
//! the response is serialized — so what is asserted is the bytes a client would see.
//!
//! The sibling `job_lifecycle` suite keeps its harness private to itself, and this suite
//! needs only one `POST`; the few lines below are that sender, not a second copy of a
//! shared fixture.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::MAX_ENDPOINT_RESULTS;
use service_com_amazonaws_mediaconvert_mediaconvert::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The regions this suite spans: the one the recorded corpus uses, two more commercial
/// regions and one in another partition-wide prefix, so the per-region rule is asserted
/// over a set rather than on one row.
const REGIONS: [&str; 4] = ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"];

/// The operation's URI, from the model's `http` binding
/// (`smithy_query(query="operation",operation="DescribeEndpoints")` -> `POST
/// /2017-08-29/endpoints`).
const ENDPOINTS_URI: &str = "/2017-08-29/endpoints";

/// A SigV4 `Authorization` header in this service's scope, signed for `region`. It is
/// what the router reads to pick the service, and what the handler reads the region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDTESTTESTTESTTEST/20260101/{region}/mediaconvert/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
    )
}

/// One emulator: a single shared engine behind the multi-service router, exactly as the
/// production router assembles it.
struct Emulator {
    router: MultiServiceRouter,
}

/// One HTTP reply, decoded far enough to assert on.
struct Reply {
    status: u16,
    /// `x-amzn-errortype`, the header restJson1 resolves an error shape from.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The `url` of every endpoint in the response, in response order.
    fn urls(&self) -> Vec<String> {
        self.body
            .get("endpoints")
            .and_then(Value::as_array)
            .map(|endpoints| {
                endpoints
                    .iter()
                    .map(|endpoint| {
                        endpoint
                            .get("url")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The single endpoint URL the response carries, or `None` when it carries none.
    fn only_url(&self) -> Option<String> {
        let urls = self.urls();
        (urls.len() == 1).then(|| urls.into_iter().next().unwrap_or_default())
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is the transport's business, not this suite's; the
        // Authorization header is still present because routing and the endpoint's region
        // read the credential scope out of it.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router }
    }

    /// `DescribeEndpoints` in `region`, with `body` as the request payload.
    async fn describe_endpoints(&self, region: &str, body: Value) -> Reply {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(ENDPOINTS_URI)
            .header("host", format!("mediaconvert.{region}.amazonaws.com"))
            .header("authorization", authorization(region))
            .header("content-type", "application/json")
            .body(::bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.body().clone();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }
}

/// Whether `url` is an account-specific MediaConvert endpoint in `region`: an HTTPS URL
/// whose host is a hexadecimal, account-specific label under the regional service host
/// (`https://<label>.mediaconvert.<region>.amazonaws.com`).
///
/// Checked structurally rather than against the handler's own formatter, so the test is
/// not a second copy of the code under test.
fn is_account_endpoint(url: &str, region: &str) -> bool {
    let suffix = format!(".mediaconvert.{region}.amazonaws.com");
    let Some(host) = url.strip_prefix("https://") else {
        return false;
    };
    let Some(label) = host.strip_suffix(&suffix) else {
        return false;
    };
    !label.is_empty() && label.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
}

/// The endpoint a region reports is the SAME URL on every later call, in either mode: it
/// is minted once, persisted, and read back from the store afterwards. Different regions
/// report different endpoints, because the URL names the region it serves.
#[tokio::test]
async fn the_reported_endpoint_is_stable_per_region() {
    let emulator = Emulator::new();
    let mut seen: Vec<(String, String)> = Vec::new();

    for region in REGIONS {
        // The first ask mints it; the request carries no `mode`, so the documented
        // default (`DEFAULT`) applies.
        let first = emulator.describe_endpoints(region, json!({})).await;
        assert_eq!(
            first.status, 200,
            "case {region}: expected HTTP 200, got {} with body {}",
            first.status, first.body
        );
        let Some(url) = first.only_url() else {
            panic!(
                "case {region}: expected exactly one endpoint, got {:?}",
                first.urls()
            );
        };
        assert!(
            is_account_endpoint(&url, region),
            "case {region}: expected an https://<label>.mediaconvert.{region}.amazonaws.com \
             endpoint, got {url:?}"
        );

        // Every later ask reports what was stored, whichever mode asks for it.
        for body in [
            json!({}),
            json!({ "mode": "DEFAULT" }),
            json!({ "mode": "GET_ONLY" }),
            json!({ "maxResults": 20 }),
        ] {
            let again = emulator.describe_endpoints(region, body.clone()).await;
            assert_eq!(
                again.status, 200,
                "case {region} / {body}: expected HTTP 200, got {} with body {}",
                again.status, again.body
            );
            assert_eq!(
                again.only_url().as_deref(),
                Some(url.as_str()),
                "case {region} / {body}: expected the stored endpoint {url:?}, got {:?}",
                again.urls()
            );
            assert!(
                again.body.get("nextToken").is_none(),
                "case {region} / {body}: one endpoint fits in one page, so no nextToken is \
                 due, got {}",
                again.body
            );
        }
        seen.push((region.to_string(), url));
    }

    for (region, url) in &seen {
        for (other_region, other_url) in &seen {
            if region == other_region {
                continue;
            }
            assert_ne!(
                url, other_url,
                "case {region} vs {other_region}: an account endpoint names its own region, \
                 so the two must differ, got {url:?} for both"
            );
        }
    }
}

/// `GET_ONLY` reports an endpoint only once one exists, and creates nothing: "Specify
/// GET_ONLY to return your endpoints if any exist, or an empty list if none exist."
/// `DEFAULT` is what creates it, and the created endpoint reads back through `GET_ONLY`.
#[tokio::test]
async fn get_only_reports_an_empty_list_until_default_creates_the_endpoint() {
    let emulator = Emulator::new();
    let region = "eu-west-1";

    // Two reads in a row: the second proves the first created nothing.
    for attempt in 1..=2 {
        let reply = emulator
            .describe_endpoints(region, json!({ "mode": "GET_ONLY" }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case GET_ONLY #{attempt} on a fresh account: expected HTTP 200, got {} with \
             body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.urls(),
            Vec::<String>::new(),
            "case GET_ONLY #{attempt} on a fresh account: expected an empty endpoint list, \
             got {:?}",
            reply.urls()
        );
    }

    let created = emulator
        .describe_endpoints(region, json!({ "mode": "DEFAULT" }))
        .await;
    assert_eq!(
        created.status, 200,
        "case DEFAULT: expected HTTP 200, got {} with body {}",
        created.status, created.body
    );
    let Some(url) = created.only_url() else {
        panic!(
            "case DEFAULT: expected exactly one endpoint, got {:?}",
            created.urls()
        );
    };

    // The round trip: what DEFAULT created is read back by the reporting mode.
    let read_back = emulator
        .describe_endpoints(region, json!({ "mode": "GET_ONLY" }))
        .await;
    assert_eq!(
        read_back.only_url().as_deref(),
        Some(url.as_str()),
        "case GET_ONLY after DEFAULT: expected the created endpoint {url:?}, got {:?}",
        read_back.urls()
    );

    // A region nobody asked about in DEFAULT mode still has no endpoint, so the write
    // DEFAULT makes is scoped to the region that asked.
    let elsewhere = emulator
        .describe_endpoints("sa-east-1", json!({ "mode": "GET_ONLY" }))
        .await;
    assert_eq!(
        elsewhere.urls(),
        Vec::<String>::new(),
        "case GET_ONLY in a region with no endpoint: expected an empty list, got {:?}",
        elsewhere.urls()
    );
}

/// `MaxResults` is accepted across the window its documentation publishes — "Max number
/// of endpoints, up to twenty" — and refused on either side of it, as a request problem
/// (HTTP 400 `BadRequestException`) rather than a clamp.
#[tokio::test]
async fn max_results_is_accepted_inside_the_documented_window_and_refused_outside() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    // Both published bounds, a value between them, and one past each bound. The upper
    // bound comes from the state module so the case cannot drift from the handler.
    let accepted = [1, 7, MAX_ENDPOINT_RESULTS];
    let refused = [MAX_ENDPOINT_RESULTS + 1, 0, -1];

    for max_results in accepted {
        let reply = emulator
            .describe_endpoints(region, json!({ "maxResults": max_results }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case maxResults={max_results}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.only_url().is_some(),
            "case maxResults={max_results}: expected the account endpoint, got {:?}",
            reply.urls()
        );
    }

    for max_results in refused {
        let reply = emulator
            .describe_endpoints(region, json!({ "maxResults": max_results }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case maxResults={max_results}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequestException"),
            "case maxResults={max_results}: expected BadRequestException, got {:?}",
            reply.error_type
        );
    }
}

/// This operation never issues a `nextToken` — an account has one endpoint per region, so
/// the first page is the only page — and a token it did not issue is a request problem,
/// never a silent empty page. An unmodelled `mode` is refused the same way (HTTP 400,
/// whether the boundary validator or the handler is what refuses it).
#[tokio::test]
async fn a_token_or_mode_this_service_does_not_issue_is_refused() {
    let emulator = Emulator::new();
    let region = "us-east-1";

    for token in ["", "not-a-token", "6162636465666768"] {
        let reply = emulator
            .describe_endpoints(region, json!({ "nextToken": token }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case nextToken={token:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequestException"),
            "case nextToken={token:?}: expected BadRequestException, got {:?}",
            reply.error_type
        );
    }

    for mode in ["default", "GET", "ALL"] {
        let reply = emulator
            .describe_endpoints(region, json!({ "mode": mode }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case mode={mode:?}: expected HTTP 400 for a value DescribeEndpointsMode does \
             not model, got {} with body {}",
            reply.status, reply.body
        );
    }
}
