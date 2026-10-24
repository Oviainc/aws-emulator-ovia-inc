// SPDX-License-Identifier: CC0-1.0

//! The storage-configuration surface over the wire: `CreateStorageConfiguration`,
//! `GetStorageConfiguration` and `ListStorageConfigurations`.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. LISTING REPORTS PERSISTED STATE. `ListStorageConfigurations` answers a page of the
//!    storage configurations this account holds, in creation order, bounded by
//!    `maxResults` (default: the service quota or 100, whichever is smaller), with a
//!    `nextToken` exactly when configurations are left over — and that token resumes
//!    where the page ended.
//! 2. ROUND TRIP. `CreateStorageConfiguration` persists the configuration the request
//!    describes and mints what the request does not supply (the ARN), and both
//!    `GetStorageConfiguration` and a `ListStorageConfigurations` summary report that
//!    SAME configuration back — member for member, on every read.
//! 3. A MINTED ARN takes the modelled `StorageConfigurationArn` form, carries the region
//!    the creating request was signed for, is distinct between configurations and never
//!    changes once minted. A `name` is NOT an identity: the model says it "does not need
//!    to be unique".
//! 4. A READ CHANGES NOTHING: both read operations are `@readonly`, so repeated reads
//!    answer identically.
//! 5. REFUSALS: an ARN this store holds no configuration for is the modelled
//!    `ResourceNotFoundException` (HTTP 404); a value outside a member's modelled
//!    `@pattern`/`@range`/`@length` is the modelled `ValidationException` (HTTP 400); and
//!    a pagination token this listing did not issue is refused rather than read as "start
//!    again".
//! 6. The tagging operations reach a storage configuration: a tag written through
//!    `TagResource` on its ARN is read back by `ListTagsForResource` and by the
//!    configuration's own `tags` member.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The account id every ARN this emulator mints carries (`engine::emulator_account_id`),
/// and so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// `StorageConfigurationArn`'s `@pattern`, from
/// `smithy_query(query="shape",shape="StorageConfiguration")` — member `arn`.
const STORAGE_CONFIGURATION_ARN_PATTERN: &str =
    "^arn:aws:ivs:[a-z0-9-]+:[0-9]+:storage-configuration/[a-zA-Z0-9-]+$";

/// `PaginationToken`'s `@pattern`, from
/// `smithy_query(query="operation",operation="ListStorageConfigurations")` — member
/// `nextToken`.
const PAGINATION_TOKEN_PATTERN: &str = "^[a-zA-Z0-9+/=_-]*$";

/// The modelled `@length` maximum of `StorageConfigurationArn`.
const STORAGE_CONFIGURATION_ARN_MAX_LENGTH: usize = 128;

/// The published bounds of `MaxStorageConfigurationResults`' `@range`, from
/// `smithy_query(query="shape",shape="MaxStorageConfigurationResults")`: `1..100`.
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// The page size `ListStorageConfigurations` uses when the request omits `maxResults`.
///
/// From `ListStorageConfigurationsRequest$maxResults`: "Default: your service quota or
/// 100, whichever is smaller." This emulator's guard on the store is far above 100, so
/// the smaller of the two is the published 100.
const DEFAULT_PAGE_SIZE: usize = 100;

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
    /// scope naming `ivs` in `region`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        self.send(
            region,
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// [`Self::call_in`] against the region every test that does not vary it uses.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", operation, body).await
    }

    /// One of the three operations bound to `/tags/{resourceArn}`, carrying the ARN in
    /// the URI the way a client does: percent-encoded into one path segment.
    async fn tagging(&self, method: &str, arn: &str, body: Option<Value>) -> (u16, Value) {
        let path = format!("/tags/{}", encode_path_label(arn));
        self.send(
            "us-east-1",
            method,
            &path,
            body.map(|body| bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    async fn send(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<bytes::Bytes>,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivsrealtime.{region}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(body.unwrap_or_default())
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

    /// Create a storage configuration in `region` and return the `storageConfiguration`
    /// the service reported for it.
    async fn create_in(&self, case: &str, region: &str, body: &Value) -> Value {
        let (status, response) = self
            .call_in(region, "CreateStorageConfiguration", body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStorageConfiguration {body} in {region} must answer \
             HTTP 200, got {status} {response}"
        );
        response["storageConfiguration"].clone()
    }

    /// [`Self::create_in`] against the region every test that does not vary it uses.
    async fn create(&self, case: &str, body: &Value) -> Value {
        self.create_in(case, "us-east-1", body).await
    }

    /// Read `arn` back, asserting the read succeeded.
    async fn get(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self
            .call("GetStorageConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStorageConfiguration on {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["storageConfiguration"].clone()
    }

    /// One `ListStorageConfigurations` page, asserting the call succeeded.
    async fn list(&self, case: &str, request: &Value) -> Value {
        let (status, response) = self.call("ListStorageConfigurations", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListStorageConfigurations {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
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

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Does `value` satisfy one of the two model `@pattern`s this suite checks?
///
/// The patterns are quoted on the constants above, straight from `smithy_query`, and are
/// simple enough to check directly — which keeps this crate's dependency set exactly what
/// the service itself needs. Each arm names the pattern it implements; a pattern with no
/// arm panics rather than passing silently.
fn matches(pattern: &str, value: &str) -> bool {
    let region_like = |text: &str| {
        !text.is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    let id_like =
        |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    match pattern {
        // ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:storage-configuration/[a-zA-Z0-9-]+$
        STORAGE_CONFIGURATION_ARN_PATTERN => {
            let Some(rest) = value.strip_prefix("arn:aws:ivs:") else {
                return false;
            };
            let Some((region, rest)) = rest.split_once(':') else {
                return false;
            };
            let Some((account, resource)) = rest.split_once(':') else {
                return false;
            };
            let Some(id) = resource.strip_prefix("storage-configuration/") else {
                return false;
            };
            region_like(region)
                && !account.is_empty()
                && account.bytes().all(|b| b.is_ascii_digit())
                && id_like(id)
        }
        // ^[a-zA-Z0-9+/=_-]*$
        PAGINATION_TOKEN_PATTERN => value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'_' | b'-')),
        other => panic!("this suite has no checker for the pattern {other:?}"),
    }
}

/// An IVS ARN of `resource_type` in `region`, naming the resource `id`.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// The create requests this suite spans the class of inputs with: each row differs from
/// the others in what the rules generalise over — whether a name is given, how many tags
/// the request carries, and the form and size of the bucket name (`S3BucketName` is
/// `@length(3..63)` with `@pattern ^[a-z0-9-.]+$`, from
/// `smithy_query(query="shape",shape="S3StorageConfiguration")`, so both published
/// length bounds and both allowed punctuation characters appear below).
fn create_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the smallest request the model allows",
            json!({ "s3": { "bucketName": "abc" } }),
        ),
        (
            "every optional member supplied",
            json!({
                "s3": { "bucketName": "recordings-bucket" },
                "name": "studio-recordings",
                "tags": { "env": "test", "team": "media" },
            }),
        ),
        (
            "a bucket name at the modelled 63-character maximum, with dots and dashes",
            json!({ "s3": { "bucketName": format!("a.b-{}", "c".repeat(59)) } }),
        ),
        (
            "an explicitly empty name, which the name pattern admits",
            json!({ "s3": { "bucketName": "empty-name-bucket" }, "name": "" }),
        ),
        (
            "an explicitly empty tag map",
            json!({ "s3": { "bucketName": "untagged-bucket" }, "tags": {} }),
        ),
        (
            "a name using every character class the name pattern allows",
            json!({ "s3": { "bucketName": "mixed-case-name" }, "name": "Studio_9-A" }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// CreateStorageConfiguration, then GetStorageConfiguration
// ---------------------------------------------------------------------------

/// What a create persists is what a read reports: for every row of the input class, the
/// configuration `GetStorageConfiguration` answers is the one `CreateStorageConfiguration`
/// reported, member for member — and a second read answers the same again, because the
/// read writes nothing.
#[tokio::test]
async fn a_created_storage_configuration_is_read_back_exactly() {
    let emulator = RealTime::new();

    for (case, request) in create_cases() {
        let created = emulator.create(case, &request).await;
        let configuration_arn = member(&created, "arn");

        assert!(
            matches(STORAGE_CONFIGURATION_ARN_PATTERN, &configuration_arn),
            "case {case:?}: expected an arn matching {STORAGE_CONFIGURATION_ARN_PATTERN}, \
             got {configuration_arn:?}"
        );
        assert!(
            configuration_arn.len() <= STORAGE_CONFIGURATION_ARN_MAX_LENGTH,
            "case {case:?}: expected an arn of at most \
             {STORAGE_CONFIGURATION_ARN_MAX_LENGTH} characters, got {} in {configuration_arn:?}",
            configuration_arn.len()
        );
        assert_eq!(
            created["s3"],
            request["s3"],
            "case {case:?}: expected the s3 destination the request named, got {created}"
        );
        assert_eq!(
            created["name"],
            request.get("name").cloned().unwrap_or(Value::Null),
            "case {case:?}: expected the name the request gave, got {created}"
        );
        assert_eq!(
            created["tags"],
            request.get("tags").cloned().unwrap_or(json!({})),
            "case {case:?}: expected the tags the request gave, got {created}"
        );

        let read = emulator.get(case, &configuration_arn).await;
        assert_eq!(
            read, created,
            "case {case:?}: expected GetStorageConfiguration to report the created \
             configuration, got {read}"
        );
        let read_again = emulator.get(case, &configuration_arn).await;
        assert_eq!(
            read_again, created,
            "case {case:?}: expected a repeated read to answer identically, got {read_again}"
        );
    }
}

/// A minted ARN names the region the request was signed for, and is distinct per
/// configuration even when two requests are identical — a `name` names no resource:
/// `CreateStorageConfigurationRequest$name` states "The value does not need to be unique".
#[tokio::test]
async fn a_minted_arn_names_the_signing_region_and_identifies_the_configuration() {
    let emulator = RealTime::new();

    // Rows differ in the region the request is signed for, which is what the ARN's
    // region element generalises over.
    for region in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"] {
        let case = format!("signed for {region}");
        let first = emulator
            .create_in(&case, region, &json!({ "s3": { "bucketName": "shared" }, "name": "same" }))
            .await;
        let second = emulator
            .create_in(&case, region, &json!({ "s3": { "bucketName": "shared" }, "name": "same" }))
            .await;

        for created in [&first, &second] {
            let configuration_arn = member(created, "arn");
            assert_eq!(
                configuration_arn.split(':').nth(3).unwrap_or_default(),
                region,
                "case {case:?}: expected the arn to name the signing region, \
                 got {configuration_arn:?}"
            );
        }
        assert_ne!(
            member(&first, "arn"),
            member(&second, "arn"),
            "case {case:?}: expected two configurations of the same name to get distinct \
             arns, got {first} and {second}"
        );

        // Both are separately readable, so neither create overwrote the other.
        for created in [&first, &second] {
            let configuration_arn = member(created, "arn");
            let read = emulator.get(&case, &configuration_arn).await;
            assert_eq!(
                read, *created,
                "case {case:?}: expected {configuration_arn} to be readable on its own, \
                 got {read}"
            );
        }
    }
}

/// A well-formed ARN naming no configuration is a 404 `ResourceNotFoundException`; an
/// ARN the model's `@pattern`/`@length` refuses is a 400 `ValidationException`.
#[tokio::test]
async fn get_storage_configuration_separates_an_unknown_arn_from_a_malformed_one() {
    let emulator = RealTime::new();
    emulator
        .create("one stored configuration", &json!({ "s3": { "bucketName": "stored" } }))
        .await;

    // Rows differ in HOW the ARN fails: unknown but well-formed, wrong resource type,
    // wrong service, not an ARN at all, and the empty string (under `@length(1..128)`).
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "well-formed but naming nothing",
            arn("us-east-1", "storage-configuration", "0000deadbeef"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a stage arn, which this operation does not accept",
            arn("us-east-1", "stage", "0000deadbeef"),
            400,
            "ValidationException",
        ),
        (
            "an arn of another service",
            "arn:aws:s3:us-east-1:000000000000:storage-configuration/abc".to_string(),
            400,
            "ValidationException",
        ),
        (
            "not an arn at all",
            "storage-configuration".to_string(),
            400,
            "ValidationException",
        ),
        ("the empty arn", String::new(), 400, "ValidationException"),
    ];

    for (case, value, expected_status, expected_code) in cases {
        let (status, response) = emulator
            .call("GetStorageConfiguration", &json!({ "arn": value }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: got {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// ListStorageConfigurations
// ---------------------------------------------------------------------------

/// A listing over an empty store reports the required member as an empty list and no
/// `nextToken` — it never answers "not implemented" and never invents a configuration.
#[tokio::test]
async fn listing_an_empty_account_reports_no_storage_configurations() {
    let emulator = RealTime::new();

    // Rows differ in the page bound, which changes nothing when there is nothing stored.
    for request in [json!({}), json!({ "maxResults": 1 }), json!({ "maxResults": 100 })] {
        let case = format!("request {request}");
        let response = emulator.list(&case, &request).await;
        assert_eq!(
            response["storageConfigurations"],
            json!([]),
            "case {case:?}: expected an empty storageConfigurations list, got {response}"
        );
        assert_eq!(
            response.get("nextToken"),
            None,
            "case {case:?}: expected no nextToken on the only page, got {response}"
        );
    }
}

/// Whatever the page bound, paging through `ListStorageConfigurations` yields every
/// stored configuration exactly once, in creation order, each summary reporting the same
/// members the create reported, and only the last page omits `nextToken`.
#[tokio::test]
async fn list_storage_configurations_pages_over_every_configuration_in_creation_order() {
    let emulator = RealTime::new();
    let mut created: Vec<Value> = Vec::new();
    for index in 0..7 {
        created.push(
            emulator
                .create(
                    "a paged configuration",
                    &json!({
                        "s3": { "bucketName": format!("bucket-{index}") },
                        "name": format!("config-{index}"),
                    }),
                )
                .await,
        );
    }
    let expected: Vec<String> = created.iter().map(|value| member(value, "arn")).collect();

    // Rows differ in the page bound, which is what the rule generalises over: every
    // bound below, at and above the stored count, both published `@range` bounds, and
    // the absent member (the documented default).
    for max_results in [
        Some(MAX_RESULTS_RANGE.0),
        Some(2),
        Some(3),
        Some(6),
        Some(7),
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
            let response = emulator.list(&case, &request).await;
            let page = response["storageConfigurations"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= bound,
                "case {case}: expected at most {bound} configurations per page, got {}",
                page.len()
            );
            for summary in &page {
                let summary_arn = member(summary, "arn");
                let source = created
                    .iter()
                    .find(|value| member(value, "arn") == summary_arn)
                    .unwrap_or_else(|| {
                        panic!("case {case}: the page reported an unknown arn {summary_arn:?}")
                    });
                // `StorageConfigurationSummary` carries the same four members as
                // `StorageConfiguration`, so a summary and a read answer alike.
                assert_eq!(
                    summary, source,
                    "case {case}: expected the summary of {summary_arn} to report the \
                     created configuration, got {summary}"
                );
                seen.push(summary_arn);
            }

            pages += 1;
            assert!(pages <= 20, "case {case}: pagination did not terminate");
            match response["nextToken"].as_str() {
                Some(token) => {
                    assert!(
                        matches(PAGINATION_TOKEN_PATTERN, token),
                        "case {case}: expected a nextToken matching \
                         {PAGINATION_TOKEN_PATTERN}, got {token:?}"
                    );
                    assert_eq!(
                        page.len(),
                        bound,
                        "case {case}: expected a full page before a nextToken, got {response}"
                    );
                    next_token = Some(token.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {case}: expected every created configuration once, in creation order"
        );
    }
}

/// A pagination token `ListStorageConfigurations` did not issue names no position, so it
/// is refused rather than read as "start again" — a token another listing of this same
/// service issued included.
#[tokio::test]
async fn list_storage_configurations_refuses_a_pagination_token_it_did_not_issue() {
    let emulator = RealTime::new();
    for index in 0..3 {
        emulator
            .create(
                "a stored configuration",
                &json!({ "s3": { "bucketName": format!("bucket-{index}") } }),
            )
            .await;
    }
    // A token from the stage listing, which pages over a different store: it names no
    // position here.
    emulator
        .call("CreateStage", &json!({ "name": "s0" }))
        .await;
    emulator
        .call("CreateStage", &json!({ "name": "s1" }))
        .await;
    let stage_page = emulator
        .call("ListStages", &json!({ "maxResults": 1 }))
        .await
        .1;
    let foreign_token = stage_page["nextToken"]
        .as_str()
        .expect("a nextToken while stages remain")
        .to_string();

    // Rows differ in how the token is wrong: not base64, base64 of foreign text, base64
    // of this listing's own marker with a non-numeric position, a token issued by
    // another listing of this service — and the empty token, which means the first page.
    let cases: Vec<(&str, String, u16)> = vec![
        ("not base64 at all", "++++".to_string(), 400),
        ("base64 of foreign text", "c29tZXRoaW5nLWVsc2U=".to_string(), 400),
        (
            "base64 of a non-numeric position",
            "aXZzLXJlYWx0aW1lLXN0b3JhZ2UtY29uZmlndXJhdGlvbi1hZnRlcjpub3Bl".to_string(),
            400,
        ),
        ("a token issued by ListStages", foreign_token, 400),
        ("the empty token, which means the first page", String::new(), 200),
    ];

    for (case, token, expected_status) in cases {
        let (status, response) = emulator
            .call("ListStorageConfigurations", &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: got {response}"
            );
        } else {
            assert_eq!(
                response["storageConfigurations"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or_default(),
                3,
                "case {case:?}: expected the first page of every configuration, got {response}"
            );
        }
    }
}

/// `maxResults` is held to the modelled `@range(1..100)`: both published bounds are
/// accepted and a value just past either is refused with the modelled
/// `ValidationException`.
#[tokio::test]
async fn list_storage_configurations_holds_the_modeled_max_results_range() {
    let emulator = RealTime::new();
    emulator
        .create("one stored configuration", &json!({ "s3": { "bucketName": "only" } }))
        .await;

    let (minimum, maximum) = MAX_RESULTS_RANGE;
    let cases: Vec<(&str, i64, u16)> = vec![
        ("the published minimum", minimum, 200),
        ("the published maximum", maximum, 200),
        ("one below the minimum", minimum - 1, 400),
        ("one above the maximum", maximum + 1, 400),
        ("far below the minimum", -1, 400),
        ("far above the maximum", 1_000, 400),
    ];
    for (case, max_results, expected_status) in cases {
        let (status, response) = emulator
            .call(
                "ListStorageConfigurations",
                &json!({ "maxResults": max_results }),
            )
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
                "case {case:?}: got {response}"
            );
        }
    }
}

/// Listing is a read: it changes nothing a later caller sees. Repeated pages over an
/// unchanged store are identical, and a read of the resource in between answers the same
/// too.
#[tokio::test]
async fn listing_storage_configurations_changes_nothing() {
    let emulator = RealTime::new();
    let mut created: Vec<Value> = Vec::new();
    for index in 0..4 {
        created.push(
            emulator
                .create(
                    "a stored configuration",
                    &json!({ "s3": { "bucketName": format!("bucket-{index}") } }),
                )
                .await,
        );
    }

    // Rows differ in the page bound, so the invariant is checked on a paged listing as
    // well as on a single-page one.
    for max_results in [Some(1), Some(2), None] {
        let case = format!("maxResults={max_results:?}");
        let mut request = json!({});
        if let Some(max) = max_results {
            request["maxResults"] = json!(max);
        }
        let first = emulator.list(&case, &request).await;
        for configuration in &created {
            let configuration_arn = member(configuration, "arn");
            let read = emulator.get(&case, &configuration_arn).await;
            assert_eq!(
                read, *configuration,
                "case {case:?}: expected the read between listings to answer the created \
                 configuration, got {read}"
            );
        }
        let second = emulator.list(&case, &request).await;
        assert_eq!(
            second, first,
            "case {case:?}: expected a repeated listing to answer identically, got {second}"
        );
    }
}

// ---------------------------------------------------------------------------
// Tagging reaches a storage configuration
// ---------------------------------------------------------------------------

/// A storage configuration is a taggable resource of this service: `TagResource` on its
/// ARN is read back by `ListTagsForResource`, by `GetStorageConfiguration` and by its
/// `ListStorageConfigurations` summary, and `UntagResource` removes what it names.
#[tokio::test]
async fn tags_written_on_a_storage_configuration_are_read_back_everywhere() {
    let emulator = RealTime::new();
    let created = emulator
        .create(
            "a tagged configuration",
            &json!({ "s3": { "bucketName": "tagged" }, "tags": { "env": "test" } }),
        )
        .await;
    let configuration_arn = member(&created, "arn");

    // Rows differ in the tag set written: one added key, a key that overwrites, and a
    // write of several keys at once.
    let writes: Vec<(&str, Value, HashMap<&str, &str>)> = vec![
        (
            "one added key",
            json!({ "tags": { "team": "media" } }),
            HashMap::from([("env", "test"), ("team", "media")]),
        ),
        (
            "a key that overwrites",
            json!({ "tags": { "env": "prod" } }),
            HashMap::from([("env", "prod"), ("team", "media")]),
        ),
        (
            "several keys at once",
            json!({ "tags": { "a": "1", "b": "2" } }),
            HashMap::from([("env", "prod"), ("team", "media"), ("a", "1"), ("b", "2")]),
        ),
    ];

    for (case, body, expected) in writes {
        let (status, response) = emulator
            .tagging("POST", &configuration_arn, Some(body.clone()))
            .await;
        // `TagResource` answers the no-content 204 this service gives both tag writes
        // (`crate::state::NO_CONTENT_SUCCESS_STATUS`).
        assert_eq!(
            status, 204,
            "case {case:?}: TagResource {body} must answer HTTP 204, got {status} {response}"
        );

        let expected_tags: Value = json!(expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect::<serde_json::Map<String, Value>>());

        let (status, listed) = emulator.tagging("GET", &configuration_arn, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must answer HTTP 200, got {status} {listed}"
        );
        assert_eq!(
            listed["tags"], expected_tags,
            "case {case:?}: expected ListTagsForResource to report the written tags, \
             got {listed}"
        );

        let read = emulator.get(case, &configuration_arn).await;
        assert_eq!(
            read["tags"], expected_tags,
            "case {case:?}: expected the configuration's own tags member to report the \
             written tags, got {read}"
        );

        let page = emulator.list(case, &json!({})).await;
        let summary = page["storageConfigurations"][0].clone();
        assert_eq!(
            summary["tags"], expected_tags,
            "case {case:?}: expected the summary's tags member to report the written \
             tags, got {summary}"
        );
    }

    // And a removal is a removal: `UntagResource` takes the keys out.
    let path = format!(
        "/tags/{}?tagKeys=a&tagKeys=b",
        encode_path_label(&configuration_arn)
    );
    let (status, response) = emulator.send("us-east-1", "DELETE", &path, None).await;
    assert_eq!(
        status, 204,
        "UntagResource must answer HTTP 204, got {status} {response}"
    );
    let read = emulator.get("after untagging", &configuration_arn).await;
    assert_eq!(
        read["tags"],
        json!({ "env": "prod", "team": "media" }),
        "expected the untagged keys to be gone, got {read}"
    );
}
