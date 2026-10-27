// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListCodeSigningConfigs`: "Returns a list of code signing
//! configurations. A request returns up to 10,000 configurations per call. You can use
//! the MaxItems parameter to return fewer configurations per call."
//!
//! The rule under test: `ListCodeSigningConfigs` reports the code-signing configurations
//! this account holds — each one exactly as `GetCodeSigningConfig` reports it — in a
//! stable order (by `CodeSigningConfigArn`, the identifier the store keys them by), one
//! page of at most `MaxItems` entries starting after the `Marker` the previous page
//! answered, carrying a `NextMarker` only while configurations remain; and it changes
//! nothing a later caller sees.
//!
//! Every row drives real HTTP requests through the router and compares the listing
//! against `GetCodeSigningConfig`, so a configuration the listing invented rather than
//! read fails here. The rows differ along what the rule generalises over: the number of
//! configurations held (none, one, several), the configuration each one carries (profile
//! count, policy, description), the page size (each published bound of `MaxItems`, sizes
//! between, sizes outside the range, and no size at all), and where the page starts (no
//! marker, a marker naming a configuration, a marker ordered before every configuration,
//! a marker ordered after every one).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateCodeSigningConfig` and `ListCodeSigningConfigs` are bound, from
/// `smithy_query(query="operation", operation="ListCodeSigningConfigs")`
/// (`GET /2020-04-22/code-signing-configs`) and
/// `smithy_query(query="operation", operation="CreateCodeSigningConfig")`
/// (`POST /2020-04-22/code-signing-configs`).
const CODE_SIGNING_CONFIGS_URI: &str = "/2020-04-22/code-signing-configs";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The `CodeSigningPolicy` values the model declares, from
/// `smithy_query(query="shape", shape="CodeSigningPolicy")` (`Warn | Enforce`).
const POLICY_WARN: &str = "Warn";
/// See [`POLICY_WARN`].
const POLICY_ENFORCE: &str = "Enforce";

/// The bounds the model puts on `MaxItems` (`MaxListItems`, `@range(min: 1, max: 10000)`),
/// from `smithy_query(query="shape", shape="MaxListItems")`.
const MAX_ITEMS_MIN: i64 = 1;
/// See [`MAX_ITEMS_MIN`].
const MAX_ITEMS_MAX: i64 = 10_000;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The `CodeSigningConfig` a create or a get answers with.
    fn config(&self) -> Value {
        self.json()
            .get("CodeSigningConfig")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// The `CodeSigningConfigs` a listing answers with.
    fn configs(&self) -> Vec<Value> {
        match self.json().get("CodeSigningConfigs") {
            Some(Value::Array(entries)) => entries.clone(),
            other => panic!(
                "expected a CodeSigningConfigs array, got {other:?}: {}",
                self.text()
            ),
        }
    }

    /// The `CodeSigningConfigArn` of every entry a listing answered, in answer order.
    fn arns(&self) -> Vec<String> {
        self.configs()
            .iter()
            .map(|entry| {
                entry
                    .get("CodeSigningConfigArn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The `NextMarker` a listing answered, when it carried one.
    fn next_marker(&self) -> Option<String> {
        self.json()
            .get("NextMarker")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// `CreateCodeSigningConfig`.
async fn create_config(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "POST", CODE_SIGNING_CONFIGS_URI, body).await
}

/// `GetCodeSigningConfig` on the configuration `arn` names.
async fn get_config(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{CODE_SIGNING_CONFIGS_URI}/{arn}"),
        &Value::Null,
    )
    .await
}

/// `ListCodeSigningConfigs` with the query string `query` (`""` for no parameters).
async fn list_configs(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{CODE_SIGNING_CONFIGS_URI}{query}"),
        &Value::Null,
    )
    .await
}

/// A signing profile version ARN satisfying the model's `Arn` pattern, in the form AWS
/// Signer vends one.
fn signing_profile(index: usize) -> String {
    format!("arn:aws:signer:{REGION}:{ACCOUNT}:/signing-profiles/Profile{index}/{index:08x}")
}

/// `count` signing profile version ARNs, as a JSON list.
fn signing_profiles(count: usize) -> Value {
    Value::Array((0..count).map(|index| json!(signing_profile(index))).collect())
}

/// The create requests the listing tests persist, differing in the number of signing
/// profiles, the policy stated and the description — the dimensions a listed entry can
/// differ along.
fn create_requests() -> Vec<Value> {
    vec![
        json!({
            "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(1) },
        }),
        json!({
            "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(3) },
            "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": POLICY_ENFORCE },
            "Description": "three profiles, enforced",
        }),
        json!({
            "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(2) },
            "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": POLICY_WARN },
            "Description": "",
            "Tags": { "team": "platform" },
        }),
        json!({
            "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(20) },
            "Description": "d".repeat(256),
        }),
        json!({
            "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(1) },
            "Description": "the same publishers as the first row",
        }),
    ]
}

/// Create one configuration per request in `requests` and answer the ARN each one got,
/// in the order the next page of a listing must report them: by ARN, which is the
/// identifier the store keys a configuration by and is NOT the creation order (the id
/// inside the ARN is minted per configuration).
async fn create_all(router: &MultiServiceRouter, requests: &[Value]) -> Vec<String> {
    let mut arns = Vec::new();
    for (index, request) in requests.iter().enumerate() {
        let created = create_config(router, request).await;
        assert_eq!(
            created.status,
            201,
            "case create {index}: expected 201 from CreateCodeSigningConfig, got {} {}",
            created.status,
            created.text()
        );
        arns.push(
            created
                .config()
                .get("CodeSigningConfigArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
    }
    arns.sort();
    arns
}

#[tokio::test]
async fn list_code_signing_configs_reports_every_stored_configuration() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Nothing stored: the answer is an empty list and no marker, never a refusal and
    // never an invented configuration.
    let empty = list_configs(&router, "").await;
    assert_eq!(
        (empty.status, empty.arns(), empty.next_marker()),
        (200, Vec::new(), None),
        "case no configurations: expected 200 with an empty list and no marker, got {} {}",
        empty.status,
        empty.text()
    );

    let expected = create_all(&router, &create_requests()).await;

    let listed = list_configs(&router, "").await;
    assert_eq!(
        (listed.status, listed.arns(), listed.next_marker()),
        (200, expected.clone(), None),
        "case every configuration: expected {expected:?} in ARN order and no marker, got {} {}",
        listed.status,
        listed.text()
    );

    // Each listed entry is the record `GetCodeSigningConfig` answers for that ARN —
    // every member of it, not a summary of the listing's own making.
    for (index, arn) in expected.iter().enumerate() {
        let fetched = get_config(&router, arn).await;
        assert_eq!(
            fetched.status,
            200,
            "case {arn:?}: expected 200 from GetCodeSigningConfig, got {} {}",
            fetched.status,
            fetched.text()
        );
        assert_eq!(
            listed.configs()[index],
            fetched.config(),
            "case {arn:?}: expected the listed entry to be what GetCodeSigningConfig \
             reports, {}, got {}",
            fetched.text(),
            listed.configs()[index]
        );
    }

    // The listing only reports: a second identical call answers the same thing, and the
    // store still holds exactly the configurations the creates persisted.
    let again = list_configs(&router, "").await;
    assert_eq!(
        (again.arns(), again.next_marker()),
        (listed.arns(), listed.next_marker()),
        "case repeated listing: expected the same answer twice, got {} then {}",
        listed.text(),
        again.text()
    );
    let stored: Vec<String> = engine
        .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
        .lock()
        .code_signing_configs
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        stored, expected,
        "case store after listing: expected the store to still hold {expected:?}, got \
         {stored:?}"
    );
}

/// One row of the paging table: the page size asked for, and the most entries a page may
/// then carry.
struct PagingCase {
    name: &'static str,
    max_items: Option<i64>,
    page_limit: usize,
}

#[tokio::test]
async fn list_code_signing_configs_pages_over_every_configuration() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Enough configurations that a small page size is not the whole answer, so a marker
    // has to be followed to see them all.
    let requests: Vec<Value> = (0..7)
        .map(|index| {
            json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(1) },
                "Description": format!("paged {index}"),
            })
        })
        .collect();
    let expected = create_all(&router, &requests).await;

    let cases = [
        PagingCase {
            name: "one at a time, the smallest page the model admits",
            max_items: Some(MAX_ITEMS_MIN),
            page_limit: MAX_ITEMS_MIN as usize,
        },
        PagingCase {
            name: "two at a time, a size that does not divide the total",
            max_items: Some(2),
            page_limit: 2,
        },
        PagingCase {
            name: "a size one short of the total",
            max_items: Some(6),
            page_limit: 6,
        },
        PagingCase {
            name: "a size exactly the total",
            max_items: Some(7),
            page_limit: 7,
        },
        PagingCase {
            name: "a size larger than the total",
            max_items: Some(50),
            page_limit: 50,
        },
        PagingCase {
            name: "the largest page the model admits",
            max_items: Some(MAX_ITEMS_MAX),
            page_limit: expected.len(),
        },
        PagingCase {
            name: "no page size at all",
            max_items: None,
            page_limit: expected.len(),
        },
    ];

    for case in cases {
        let label = case.name;
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        // One more round than pages can possibly be needed, so a marker that never
        // settles fails the assertion below rather than looping forever.
        for _ in 0..expected.len() + 2 {
            let mut query: Vec<String> = Vec::new();
            if let Some(max_items) = case.max_items {
                query.push(format!("MaxItems={max_items}"));
            }
            if let Some(marker) = marker.as_deref() {
                query.push(format!("Marker={marker}"));
            }
            let query = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };

            let page = list_configs(&router, &query).await;
            assert_eq!(
                page.status,
                200,
                "case {label:?}: expected 200 from ListCodeSigningConfigs{query}, got {} {}",
                page.status,
                page.text()
            );
            let arns = page.arns();
            assert!(
                arns.len() <= case.page_limit,
                "case {label:?}: expected at most {} entries per page, got {} ({arns:?})",
                case.page_limit,
                arns.len()
            );
            seen.extend(arns.iter().cloned());

            match page.next_marker() {
                Some(next) => {
                    // A marker is answered only while configurations remain, and it is
                    // the key of the last entry this page reported.
                    assert_eq!(
                        Some(next.clone()),
                        arns.last().cloned(),
                        "case {label:?}: expected the marker to be the last entry of the \
                         page, got {next:?} after {arns:?}"
                    );
                    assert!(
                        seen.len() < expected.len(),
                        "case {label:?}: expected no marker once every configuration was \
                         reported, got {next:?} after {seen:?}"
                    );
                    marker = Some(next);
                }
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {label:?}: expected the pages to concatenate to {expected:?} with \
             nothing skipped or repeated, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn list_code_signing_configs_starts_the_page_after_the_marker() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let expected = create_all(&router, &create_requests()).await;

    // A marker naming a configuration this account holds: the page starts at the first
    // configuration ordered after it, so that configuration is not reported twice.
    for (index, arn) in expected.iter().enumerate() {
        let page = list_configs(&router, &format!("?Marker={arn}")).await;
        let remaining: Vec<String> = expected[index + 1..].to_vec();
        assert_eq!(
            (page.status, page.arns()),
            (200, remaining.clone()),
            "case marker {arn:?}: expected {remaining:?} after it, got {} {}",
            page.status,
            page.text()
        );
    }

    // A marker ordered BEFORE every configuration names none of them, and the page still
    // starts at the first configuration after it: the whole ordered sequence.
    let before = list_configs(&router, "?Marker=arn:aws:lambda:").await;
    assert_eq!(
        (before.status, before.arns()),
        (200, expected.clone()),
        "case a marker before every configuration: expected {expected:?}, got {} {}",
        before.status,
        before.text()
    );

    // A marker ordered AFTER every configuration leaves nothing to report, and nothing to
    // continue after either.
    let after = list_configs(&router, "?Marker=zzz-past-every-configuration").await;
    assert_eq!(
        (after.status, after.arns(), after.next_marker()),
        (200, Vec::new(), None),
        "case a marker past every configuration: expected an empty page and no marker, \
         got {} {}",
        after.status,
        after.text()
    );
}

#[tokio::test]
async fn list_code_signing_configs_refuses_a_page_size_outside_the_modelled_range() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let requests: Vec<Value> = (0..3)
        .map(|index| {
            json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(1) },
                "Description": format!("bounded {index}"),
            })
        })
        .collect();
    let expected = create_all(&router, &requests).await;

    // `MaxItems` is `@range(min: 1, max: 10000)`: below the minimum and past the maximum
    // are refused rather than clamped into a page.
    for (label, max_items) in [
        ("zero", "0"),
        ("negative", "-1"),
        ("one past the maximum", "10001"),
        ("far past the maximum", "1000000"),
    ] {
        let answer = list_configs(&router, &format!("?MaxItems={max_items}")).await;
        assert_eq!(
            answer.status,
            400,
            "case MaxItems {label}: expected 400, got {} {}",
            answer.status,
            answer.text()
        );
    }

    // Each published bound is served: the minimum answers one configuration and a
    // marker, the maximum answers all three and none.
    let smallest = list_configs(&router, &format!("?MaxItems={MAX_ITEMS_MIN}")).await;
    assert_eq!(
        (smallest.status, smallest.arns(), smallest.next_marker()),
        (200, expected[..1].to_vec(), Some(expected[0].clone())),
        "case MaxItems at the minimum: expected the first configuration and its marker, \
         got {} {}",
        smallest.status,
        smallest.text()
    );
    let largest = list_configs(&router, &format!("?MaxItems={MAX_ITEMS_MAX}")).await;
    assert_eq!(
        (largest.status, largest.arns(), largest.next_marker()),
        (200, expected.clone(), None),
        "case MaxItems at the maximum: expected {expected:?} and no marker, got {} {}",
        largest.status,
        largest.text()
    );
}
