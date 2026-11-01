// SPDX-License-Identifier: CC0-1.0

//! `ListPublicKeys` over the wire: the page of public keys this account holds, in the
//! region the request was processed in.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. LISTING REPORTS PERSISTED STATE. The listing answers a page of the public keys this
//!    account holds, in import order, bounded by `maxResults` (default: 50), with a
//!    `nextToken` exactly when keys are left over — and that token resumes where the page
//!    ended. A key imported later joins the listing at the end.
//! 2. A SUMMARY AND A READ AGREE. Every member `PublicKeySummary` carries is the member of
//!    the same name `GetPublicKey` reports for that same key; the members the summary
//!    shape does not carry (`publicKeyMaterial`, `fingerprint`) are absent from a page.
//! 3. REGION. A page holds exactly the keys of the region its own request was processed in
//!    — "all public keys in your account, in the AWS region where the API request is
//!    processed".
//! 4. A READ CHANGES NOTHING: the listing is `@readonly`, so repeated pages over an
//!    unchanged store are identical and a read in between answers the same.
//! 5. REFUSALS: a `maxResults` outside the modelled `@range` is the modelled
//!    `ValidationException` (HTTP 400), and a pagination token this listing did not issue
//!    is refused in this service's one wording rather than read as "start again".

use std::sync::Arc;

use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384};
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region this suite signs for unless a row varies it.
const REGION: &str = "us-east-1";

/// `PaginationToken`'s `@pattern`, from
/// `smithy_query(query="operation",operation="ListPublicKeys")` — member `nextToken`.
const PAGINATION_TOKEN_PATTERN: &str = "^[a-zA-Z0-9+/=_-]*$";

/// The published bounds of `MaxPublicKeyResults`' `@range`, from
/// `smithy_query(query="shape",shape="MaxPublicKeyResults")`: `1..100`.
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// The page size the listing uses when the request omits `maxResults`, from
/// `ListPublicKeysRequest$maxResults`: "Maximum number of results to return. Default:
/// 50."
const DEFAULT_PAGE_SIZE: usize = 50;

/// The published maximum length of `PublicKeyName`, from
/// `smithy_query(query="shape",shape="PublicKeyName")` (`@length(0..128)`).
const MAX_NAME_LENGTH: usize = 128;

/// The members `PublicKeySummary` carries, from
/// `smithy_query(query="shape",shape="PublicKeySummary")`.
const SUMMARY_MEMBERS: [&str; 3] = ["arn", "name", "tags"];

/// The members the full `PublicKey` carries that the summary shape does NOT
/// (`smithy_query(query="shape",shape="PublicKey,PublicKeySummary")`), so a page reports
/// fewer members than a read rather than different ones — and no page carries key
/// material.
const MEMBERS_NOT_IN_A_SUMMARY: [&str; 2] = ["publicKeyMaterial", "fingerprint"];

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in `region` — which is how a request names the region it is
    /// processed in.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{region}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// [`Self::call_in`] against the region every test that does not vary it uses.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Import a public key in `region` and return the `publicKey` the service reported.
    async fn import_in(&self, case: &str, region: &str, body: &Value) -> Value {
        let (status, response) = self.call_in(region, "ImportPublicKey", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ImportPublicKey must answer HTTP 200, got {status} {response}"
        );
        response["publicKey"].clone()
    }

    /// Import a freshly generated key of `algorithm`, with the members `request` carries
    /// besides the material.
    async fn import_generated(
        &self,
        case: &str,
        region: &str,
        algorithm: &'static rcgen::SignatureAlgorithm,
        request: &Value,
    ) -> Value {
        let mut body = request.clone();
        body["publicKeyMaterial"] = json!(generated_pem(algorithm));
        self.import_in(case, region, &body).await
    }

    /// Read one key back, asserting the read succeeded.
    async fn get(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self.call("GetPublicKey", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPublicKey on {arn} must answer HTTP 200, got {status} {response}"
        );
        response["publicKey"].clone()
    }

    /// One `ListPublicKeys` page from `region`, asserting the call succeeded.
    async fn list_in(&self, case: &str, region: &str, request: &Value) -> Value {
        let (status, response) = self.call_in(region, "ListPublicKeys", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListPublicKeys {request} must answer HTTP 200, got {status} {response}"
        );
        response
    }

    /// [`Self::list_in`] in this suite's own region.
    async fn list(&self, case: &str, request: &Value) -> Value {
        self.list_in(case, REGION, request).await
    }

    /// The ARNs one page reports, in the order it reported them.
    async fn listed_arns(&self, case: &str, request: &Value) -> Vec<String> {
        let response = self.list(case, request).await;
        summaries(&response)
            .iter()
            .map(|summary| member(summary, "arn"))
            .collect()
    }
}

/// The error code a refusal states, or `"<absent>"` so a failing assertion names the
/// missing member rather than panicking inside the reader.
fn error_code(response: &Value) -> &str {
    response
        .get("Code")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
}

/// The text a refusal states.
fn error_message(response: &Value) -> &str {
    response
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
}

/// The sentence every listing of this service refuses a token it did not issue with.
fn foreign_page_token_message(operation: &str) -> String {
    format!("{operation}.nextToken is required to be a pagination token issued by {operation}")
}

/// A string member, read as the empty string when absent — what a client sees for a
/// member the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The summaries one page reported, as an empty list when the member is absent — so a
/// failing assertion names what came back rather than panicking inside the reader.
fn summaries(response: &Value) -> Vec<Value> {
    response["publicKeys"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `value` restricted to the members [`SUMMARY_MEMBERS`] names, so a summary and a full
/// read can be compared member for member on exactly the members both carry.
fn summary_members(value: &Value) -> Value {
    let mut projected = Map::new();
    for name in SUMMARY_MEMBERS {
        if let Some(found) = value.get(name) {
            projected.insert(name.to_string(), found.clone());
        }
    }
    Value::Object(projected)
}

/// Does `value` satisfy the one model `@pattern` this suite checks?
///
/// The pattern is quoted on [`PAGINATION_TOKEN_PATTERN`], straight from `smithy_query`,
/// and is simple enough to check directly — which keeps this crate's dependency set
/// exactly what the service itself needs. A pattern with no arm panics rather than
/// passing silently.
fn matches(pattern: &str, value: &str) -> bool {
    match pattern {
        PAGINATION_TOKEN_PATTERN => value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'_' | b'-')
        }),
        other => panic!("this suite checks no pattern {other:?}"),
    }
}

/// A freshly generated public key of `algorithm`, as the PEM `PUBLIC KEY` block
/// `openssl ... -pubout` writes.
///
/// Generated rather than hard-coded, so what every row imports is a real public key — and
/// a different one per call, since the material is a key's identity to this service.
fn generated_pem(algorithm: &'static rcgen::SignatureAlgorithm) -> String {
    let der = KeyPair::generate_for(algorithm)
        .expect("the test key pair is generated")
        .public_key_der();
    pem::encode(&pem::Pem::new("PUBLIC KEY", der)).replace("\r\n", "\n")
}

/// The import requests this suite spans the class of keys with: each row differs from the
/// others in what a summary reports or in what the full record holds — the key's
/// algorithm, whether the request names the key and how long that name is, and how many
/// tags accompany it.
fn import_rows() -> Vec<(&'static str, &'static rcgen::SignatureAlgorithm, Value)> {
    vec![
        (
            "ecdsa p-256, every optional member omitted",
            &PKCS_ECDSA_P256_SHA256,
            json!({}),
        ),
        (
            "ecdsa p-384, named, one tag",
            &PKCS_ECDSA_P384_SHA384,
            json!({ "name": "token-signing-key", "tags": { "env": "test" } }),
        ),
        (
            "ecdsa p-256, named, several tags",
            &PKCS_ECDSA_P256_SHA256,
            json!({ "name": "stage_token-key", "tags": { "env": "prod", "team": "video" } }),
        ),
        (
            "the empty name, which the modelled length 0..128 allows",
            &PKCS_ECDSA_P384_SHA384,
            json!({ "name": "" }),
        ),
        (
            "a name at the published maximum length",
            &PKCS_ECDSA_P256_SHA256,
            json!({ "name": "k".repeat(MAX_NAME_LENGTH) }),
        ),
        (
            "an empty tag map",
            &PKCS_ECDSA_P384_SHA384,
            json!({ "name": "no-tags", "tags": {} }),
        ),
    ]
}

/// The suite's fixture: the keys [`import_rows`] describes, in import order.
struct Fixture {
    emulator: RealTime,
    /// The `publicKey` each import reported, in import order.
    imported: Vec<Value>,
}

impl Fixture {
    async fn new() -> Self {
        let emulator = RealTime::new();
        let mut imported = Vec::new();
        for (case, algorithm, request) in import_rows() {
            imported.push(
                emulator
                    .import_generated(case, REGION, algorithm, &request)
                    .await,
            );
        }
        Self { emulator, imported }
    }

    /// The ARNs of the imported keys, in import order.
    fn arns(&self) -> Vec<String> {
        self.imported
            .iter()
            .map(|value| member(value, "arn"))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Rule 1: the listing reports persisted state, a page at a time
// ---------------------------------------------------------------------------

/// A listing over an empty account reports the required member as an empty list and no
/// `nextToken` — it never answers "not implemented" and never invents a key.
#[tokio::test]
async fn listing_an_empty_account_reports_no_public_keys() {
    let emulator = RealTime::new();

    // Rows differ in the page bound, which changes nothing when there is nothing stored.
    let requests = [
        json!({}),
        json!({ "maxResults": MAX_RESULTS_RANGE.0 }),
        json!({ "maxResults": 7 }),
        json!({ "maxResults": MAX_RESULTS_RANGE.1 }),
        json!({ "nextToken": "" }),
    ];
    for request in requests {
        let case = format!("request {request}");
        let response = emulator.list(&case, &request).await;
        assert_eq!(
            response["publicKeys"],
            json!([]),
            "case {case:?}: expected an empty publicKeys list, got {response}"
        );
        assert_eq!(
            response.get("nextToken"),
            None,
            "case {case:?}: expected no nextToken on the only page, got {response}"
        );
    }
}

/// Whatever the page bound, paging through the listing yields every stored key exactly
/// once, in import order, and only the last page omits `nextToken`.
#[tokio::test]
async fn the_listing_pages_over_every_public_key_in_import_order() {
    let fixture = Fixture::new().await;
    let expected = fixture.arns();
    let stored = expected.len();

    // Rows differ in the page bound, which is what the rule generalises over: every bound
    // below, at and above the stored count, both published `@range` bounds, and the absent
    // member (the documented default).
    for max_results in [
        Some(MAX_RESULTS_RANGE.0),
        Some(2),
        Some(3),
        Some(stored as i64 - 1),
        Some(stored as i64),
        Some(stored as i64 + 1),
        Some(MAX_RESULTS_RANGE.1),
        None,
    ] {
        let case = format!("maxResults={max_results:?}");
        let bound = max_results.map_or(DEFAULT_PAGE_SIZE, |max| max as usize);
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({});
            if let Some(max) = max_results {
                request["maxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let response = fixture.emulator.list(&case, &request).await;
            let page = summaries(&response);
            assert!(
                page.len() <= bound,
                "case {case:?}: expected at most {bound} public keys per page, got {}",
                page.len()
            );
            for summary in &page {
                seen.push(member(summary, "arn"));
            }

            pages += 1;
            assert!(pages <= 20, "case {case:?}: pagination did not terminate");
            match response["nextToken"].as_str() {
                Some(token) => {
                    assert!(
                        matches(PAGINATION_TOKEN_PATTERN, token),
                        "case {case:?}: expected a nextToken matching \
                         {PAGINATION_TOKEN_PATTERN}, got {token:?}"
                    );
                    assert_eq!(
                        page.len(),
                        bound,
                        "case {case:?}: expected a full page before a nextToken, got {response}"
                    );
                    next_token = Some(token.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: expected every imported key once, in import order, got {seen:?}"
        );
    }
}

/// The documented default bound is the one the listing applies: with more keys stored than
/// the default, a request that omits `maxResults` reports exactly that many and leaves the
/// rest for the next page.
#[tokio::test]
async fn omitting_max_results_pages_by_the_documented_default() {
    let emulator = RealTime::new();

    // One more than the default, so "every key" and "the default page" are different
    // answers.
    let stored = DEFAULT_PAGE_SIZE + 1;
    let mut expected: Vec<String> = Vec::new();
    for index in 0..stored {
        let imported = emulator
            .import_generated(
                "the default-bound fixture",
                REGION,
                &PKCS_ECDSA_P256_SHA256,
                &json!({ "name": format!("key-{index}") }),
            )
            .await;
        expected.push(member(&imported, "arn"));
    }

    let response = emulator.list("the default bound", &json!({})).await;
    let page = summaries(&response);
    assert_eq!(
        page.len(),
        DEFAULT_PAGE_SIZE,
        "expected the documented default of {DEFAULT_PAGE_SIZE} per page, got {}",
        page.len()
    );
    let token = response["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "expected a nextToken while a key is left over, got {response}"
    );
    let rest = emulator
        .listed_arns("the default bound", &json!({ "nextToken": token }))
        .await;
    assert_eq!(
        rest,
        expected[DEFAULT_PAGE_SIZE..].to_vec(),
        "expected the token to resume at the key the default page left, got {rest:?}"
    );
}

/// A key imported later joins the listing at the end — the page is read off the store
/// rather than remembered.
#[tokio::test]
async fn the_listing_follows_later_imports() {
    let fixture = Fixture::new().await;
    let mut expected = fixture.arns();

    // Rows differ in the key the late import carries: an unnamed one, a named one and a
    // tagged one, so "the end of the listing" is not a property of one request shape.
    let late: Vec<(&str, Value)> = vec![
        ("late and unnamed", json!({})),
        ("late and named", json!({ "name": "late" })),
        (
            "late, named and tagged",
            json!({ "name": "later", "tags": { "env": "test" } }),
        ),
    ];
    for (case, request) in late {
        let imported = fixture
            .emulator
            .import_generated(case, REGION, &PKCS_ECDSA_P384_SHA384, &request)
            .await;
        expected.push(member(&imported, "arn"));
        let listed = fixture.emulator.listed_arns(case, &json!({})).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the new key listed last, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a summary and a read agree
// ---------------------------------------------------------------------------

/// Every member a summary carries is the member of the same name `GetPublicKey` reports
/// for that same key, and the members the summary shape does not carry — the key material
/// and its fingerprint — are absent from a page.
#[tokio::test]
async fn every_summary_reports_what_a_read_reports() {
    let fixture = Fixture::new().await;

    let listed = fixture.emulator.list("the whole listing", &json!({})).await;
    let page = summaries(&listed);
    assert_eq!(
        page.len(),
        fixture.imported.len(),
        "expected every imported key on one page, got {listed}"
    );

    for (position, summary) in page.iter().enumerate() {
        let arn = member(summary, "arn");
        let case = format!("summary {position} ({arn})");
        let read = fixture.emulator.get(&case, &arn).await;
        assert_eq!(
            summary_members(summary),
            summary_members(&read),
            "case {case:?}: expected the summary to report what the read reports, \
             got {summary} against {read}"
        );
        // The import's own answer is the same key too, so a page, a read and an import
        // cannot report three different things.
        assert_eq!(
            summary_members(summary),
            summary_members(&fixture.imported[position]),
            "case {case:?}: expected the summary to report what the import reported, \
             got {summary}"
        );
        for absent in MEMBERS_NOT_IN_A_SUMMARY {
            assert!(
                summary.get(absent).is_none(),
                "case {case:?}: {absent} is not a member of PublicKeySummary, got {summary}"
            );
        }
    }

    // And no page carries key material under any key at all: the whole page is checked as
    // text, so a summary cannot smuggle the PEM in under another name.
    let rendered = listed.to_string();
    assert!(
        !rendered.contains("PUBLIC KEY"),
        "expected no key material anywhere in a page, got {listed}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the page is the request's own region's view
// ---------------------------------------------------------------------------

/// "In your account, in the AWS region where the API request is processed": a page holds
/// exactly the keys imported through the listing's own region's endpoint.
#[tokio::test]
async fn a_page_reports_only_the_public_keys_of_its_own_region() {
    let emulator = RealTime::new();

    // Rows differ in the region and in how many keys it holds, so neither "the first
    // region" nor "one each" can pass by accident.
    let rows: [(&str, usize); 3] = [("us-east-1", 2), ("eu-west-1", 1), ("ap-southeast-2", 3)];

    let mut expected: Vec<(&str, Vec<String>)> = Vec::new();
    for (region, count) in rows {
        let mut arns = Vec::new();
        for index in 0..count {
            let imported = emulator
                .import_generated(
                    region,
                    region,
                    &PKCS_ECDSA_P256_SHA256,
                    &json!({ "name": format!("key-{index}") }),
                )
                .await;
            arns.push(member(&imported, "arn"));
        }
        expected.push((region, arns));
    }

    for (region, arns) in &expected {
        let listed = emulator.list_in(region, region, &json!({})).await;
        let reported: Vec<String> = summaries(&listed)
            .iter()
            .map(|summary| member(summary, "arn"))
            .collect();
        assert_eq!(
            &reported, arns,
            "case {region:?}: expected only this region's keys, got {reported:?}"
        );
        for arn in &reported {
            assert!(
                arn.contains(&format!(":{region}:")),
                "case {region:?}: every listed ARN must name this region, got {arn:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the listing is a read
// ---------------------------------------------------------------------------

/// Listing is a read: it changes nothing a later caller sees. Repeated pages over an
/// unchanged store are identical, whatever the bound, and a read of a key in between
/// answers the same too.
#[tokio::test]
async fn listing_changes_nothing() {
    let fixture = Fixture::new().await;
    let probe_arn = member(&fixture.imported[1], "arn");

    // Rows differ in the request repeated: the whole listing, a one-at-a-time page and a
    // page bound at the published maximum.
    let requests = [
        json!({}),
        json!({ "maxResults": 1 }),
        json!({ "maxResults": MAX_RESULTS_RANGE.1 }),
    ];
    for request in requests {
        let case = format!("request {request}");
        let first = fixture.emulator.list(&case, &request).await;
        let before = fixture.emulator.get(&case, &probe_arn).await;
        let second = fixture.emulator.list(&case, &request).await;
        let after = fixture.emulator.get(&case, &probe_arn).await;
        assert_eq!(
            first, second,
            "case {case:?}: expected the second page to be identical, got {second}"
        );
        assert_eq!(
            before, after,
            "case {case:?}: expected listing to leave the key where it was, got {after}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: refusals
// ---------------------------------------------------------------------------

/// `maxResults` is held to the modelled `@range(1..100)`: both published bounds are
/// accepted and a value just past either is refused with the modelled
/// `ValidationException`.
#[tokio::test]
async fn the_listing_holds_the_modeled_max_results_range() {
    let fixture = Fixture::new().await;

    let (minimum, maximum) = MAX_RESULTS_RANGE;
    let rows: Vec<(&str, i64, u16)> = vec![
        ("the published minimum", minimum, 200),
        ("the published maximum", maximum, 200),
        ("one below the minimum", minimum - 1, 400),
        ("one above the maximum", maximum + 1, 400),
        ("far below the minimum", -1, 400),
        ("far above the maximum", 1_000, 400),
    ];
    for (case, max_results, expected_status) in rows {
        let (status, response) = fixture
            .emulator
            .call("ListPublicKeys", &json!({ "maxResults": max_results }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?} (maxResults={max_results}): expected HTTP {expected_status}, \
             got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {response}"
            );
            assert!(
                response.get("publicKeys").is_none(),
                "case {case:?}: a refused listing reports no page, got {response}"
            );
        }
    }
}

/// A pagination token this listing did not issue names no position, so it is refused
/// rather than read as "start again" — a token another listing of this same service
/// issued included — while the empty token is the first page and its own token resumes.
#[tokio::test]
async fn the_listing_refuses_a_pagination_token_it_did_not_issue() {
    let fixture = Fixture::new().await;
    let stored = fixture.imported.len();

    // A token from the stage listing, which pages over a different store: it names no
    // position here. Two stages, so that listing has a page left over to issue one for.
    for index in 0..2 {
        let (status, response) = fixture
            .emulator
            .call("CreateStage", &json!({ "name": format!("stage-{index}") }))
            .await;
        assert_eq!(
            status, 200,
            "fixture CreateStage: expected HTTP 200, got {status} {response}"
        );
    }
    let (status, stage_page) = fixture
        .emulator
        .call("ListStages", &json!({ "maxResults": 1 }))
        .await;
    assert_eq!(
        status, 200,
        "fixture ListStages must answer HTTP 200, got {status} {stage_page}"
    );
    let foreign_token = stage_page["nextToken"]
        .as_str()
        .expect("a nextToken while stages remain")
        .to_string();

    // Rows differ in how the token is wrong: not base64, base64 of foreign text, base64
    // of another listing's marker, a token issued by another listing of this service —
    // and the empty token, which means the first page.
    let rows: Vec<(&str, String, u16)> = vec![
        ("not base64 at all", "++++".to_string(), 400),
        (
            "base64 of foreign text",
            "c29tZXRoaW5nLWVsc2U=".to_string(),
            400,
        ),
        (
            "base64 of a marker with a non-numeric position",
            "aXZzLXJlYWx0aW1lLXB1YmxpYy1rZXktYWZ0ZXI6bm9wZQ==".to_string(),
            400,
        ),
        ("a token issued by ListStages", foreign_token, 400),
        (
            "the empty token, which means the first page",
            String::new(),
            200,
        ),
    ];
    for (case, token, expected_status) in rows {
        let (status, response) = fixture
            .emulator
            .call("ListPublicKeys", &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {response}"
            );
            assert_eq!(
                error_message(&response),
                foreign_page_token_message("ListPublicKeys"),
                "case {case:?}: expected this service's one foreign-token sentence, got {}",
                error_message(&response)
            );
        } else {
            assert_eq!(
                summaries(&response).len(),
                stored,
                "case {case:?}: expected the first page of every key, got {response}"
            );
        }
    }

    // Its OWN token resumes where the page ended: the keys it reports are the ones the
    // first page did not.
    let first = fixture
        .emulator
        .list("its own token", &json!({ "maxResults": 1 }))
        .await;
    let resumed = fixture
        .emulator
        .listed_arns(
            "its own token",
            &json!({ "nextToken": first["nextToken"].as_str().unwrap_or_default() }),
        )
        .await;
    assert_eq!(
        resumed,
        fixture.arns()[1..].to_vec(),
        "case \"its own token\": expected the keys the first page left, got {resumed:?}"
    );

    // And a token this listing issued in ANOTHER region names no position here either:
    // the page's scope is the region its request was processed in.
    let other_region = "eu-west-1";
    for index in 0..2 {
        fixture
            .emulator
            .import_generated(
                "the other region's fixture",
                other_region,
                &PKCS_ECDSA_P256_SHA256,
                &json!({ "name": format!("other-{index}") }),
            )
            .await;
    }
    let other_page = fixture
        .emulator
        .list_in("the other region", other_region, &json!({ "maxResults": 1 }))
        .await;
    let other_token = other_page["nextToken"]
        .as_str()
        .expect("a nextToken while the other region's keys remain")
        .to_string();
    let (status, response) = fixture
        .emulator
        .call("ListPublicKeys", &json!({ "nextToken": other_token }))
        .await;
    assert_eq!(
        status, 400,
        "case \"a token issued in another region\": expected HTTP 400, got {status} {response}"
    );
    assert_eq!(
        error_message(&response),
        foreign_page_token_message("ListPublicKeys"),
        "case \"a token issued in another region\": expected this service's one \
         foreign-token sentence, got {}",
        error_message(&response)
    );
}
