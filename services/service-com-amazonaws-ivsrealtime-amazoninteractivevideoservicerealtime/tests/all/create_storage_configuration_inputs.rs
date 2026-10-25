// SPDX-License-Identifier: CC0-1.0

//! `CreateStorageConfiguration`'s input class and the bound on the store it grows.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code, the stated message and
//! the JSON body — never an internal call.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A value INSIDE a member's modelled `@length`/`@pattern` is persisted as sent and
//!    read back unchanged by `GetStorageConfiguration`, whatever it is.
//! 2. A value OUTSIDE one of those constraints is the modelled `ValidationException`
//!    (HTTP 400) naming the operation, the dotted member path and the bound it violated,
//!    and the call persists nothing: the configurations the accepted requests created are
//!    untouched.
//! 3. The store this operation grows is BOUNDED: once it holds as many configurations as
//!    the service admits, a further create is the modelled
//!    `ServiceQuotaExceededException` (HTTP 402) stating the resource and the limit, and
//!    the store does not grow.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The published bounds of `StorageConfigurationName`'s `@length`, from
/// `smithy_query(query="operation",operation="CreateStorageConfiguration")` — member
/// `name`.
const NAME_LENGTH: (usize, usize) = (0, 128);

/// `StorageConfigurationName`'s `@pattern`, from the same query.
const NAME_PATTERN: &str = "^[a-zA-Z0-9-_]*$";

/// The published bounds of `S3BucketName`'s `@length`, from
/// `smithy_query(query="shape",shape="S3StorageConfiguration")` — member `bucketName`.
const BUCKET_NAME_LENGTH: (usize, usize) = (3, 63);

/// `S3BucketName`'s `@pattern`, from the same query.
const BUCKET_NAME_PATTERN: &str = "^[a-z0-9-.]+$";

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

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential scope
    /// naming `ivs`.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00";
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.us-east-1.amazonaws.com/{operation}"
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

    /// Create a storage configuration, asserting the call succeeded, and return the
    /// `storageConfiguration` the service reported.
    async fn create(&self, case: &str, body: &Value) -> Value {
        let (status, response) = self.call("CreateStorageConfiguration", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStorageConfiguration {body} must answer HTTP 200, \
             got {status} {response}"
        );
        response["storageConfiguration"].clone()
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

    /// How many storage configurations this account holds, read by paging
    /// `ListStorageConfigurations` to exhaustion.
    async fn held(&self, case: &str) -> usize {
        let mut total = 0usize;
        let mut token: Option<String> = None;
        loop {
            let request = match &token {
                Some(next) => json!({ "nextToken": next }),
                None => json!({}),
            };
            let (status, response) = self.call("ListStorageConfigurations", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListStorageConfigurations {request} must answer HTTP 200, \
                 got {status} {response}"
            );
            total += response["storageConfigurations"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            token = response["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                return total;
            }
        }
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// The stated message of a failed call.
fn error_message(response: &Value) -> &str {
    response["message"].as_str().unwrap_or("<no message>")
}

/// The message rule a `@pattern` refusal follows, as `error_envelope.rs` states it.
fn pattern_message(member: &str, pattern: &str) -> String {
    format!("CreateStorageConfiguration.{member} does not match the required pattern {pattern}")
}

/// The message rule a `@length` refusal follows, as `error_envelope.rs` states it.
fn length_message(member: &str, kind: &str, bound: usize, found: usize) -> String {
    format!(
        "CreateStorageConfiguration.{member} is required to have a {kind} length of {bound}, \
         found {found}"
    )
}

/// Assert `response` is the modelled `ValidationException` (HTTP 400) stating `expected`
/// and carrying no configuration.
fn assert_refused(case: &str, status: u16, response: &Value, expected: &str) {
    assert_eq!(
        (status, error_code(response), error_message(response)),
        (400, "ValidationException", expected),
        "case {case:?}: expected the stated refusal {expected:?}, got {status} {response}"
    );
    assert!(
        response.get("storageConfiguration").is_none(),
        "case {case:?}: a refused create must report no configuration, got {response}"
    );
}

// ---------------------------------------------------------------------------
// The modelled `name` constraints
// ---------------------------------------------------------------------------

/// A `name` inside the member's modelled `@length` and `@pattern` is persisted as sent and
/// read back unchanged; one outside them is the modelled `ValidationException` (HTTP 400)
/// carrying no configuration, and the configurations the accepted requests created are
/// untouched by a refused one.
///
/// The accepted rows cover both published length bounds and each character class the
/// pattern admits on its own; the refused rows sit one character past the maximum, or
/// outside the character class.
#[tokio::test]
async fn the_modeled_storage_configuration_name_constraints_are_enforced() {
    let emulator = RealTime::new();

    let shortest_name = "x".repeat(NAME_LENGTH.0);
    let longest_name = "n".repeat(NAME_LENGTH.1);
    let accepted: Vec<(&str, &str)> = vec![
        ("the modelled minimum length: the empty name", &shortest_name),
        ("one character", "a"),
        ("upper case only", "STUDIO"),
        ("digits only", "1080"),
        ("the hyphen the pattern admits", "-"),
        ("the underscore the pattern admits", "_"),
        ("every admitted class at once", "Studio-720p_v2"),
        ("the modelled maximum length", &longest_name),
    ];

    let mut stored: Vec<(String, String, Value)> = Vec::new();
    for (case, name) in &accepted {
        let request = json!({ "s3": { "bucketName": "name-cases-bucket" }, "name": name });
        let created = emulator.create(case, &request).await;
        assert_eq!(
            created["name"], json!(name),
            "case {case:?}: expected the name the request sent ({} characters), got {created}",
            name.chars().count()
        );

        // Persisted, not just echoed: the read reports the same configuration.
        let configuration_arn = created["arn"].as_str().unwrap_or_default().to_string();
        let read = emulator.get(case, &configuration_arn).await;
        assert_eq!(
            read, created,
            "case {case:?}: GetStorageConfiguration must report the created configuration, \
             expected {created}, got {read}"
        );
        stored.push((case.to_string(), configuration_arn, created));
    }

    let over_long_name = "n".repeat(NAME_LENGTH.1 + 1);
    let refused: Vec<(&str, &str, String)> = vec![
        (
            "one character past the modelled maximum length",
            &over_long_name,
            length_message("name", "maximum", NAME_LENGTH.1, NAME_LENGTH.1 + 1),
        ),
        (
            "a space, which the pattern excludes",
            "studio feed",
            pattern_message("name", NAME_PATTERN),
        ),
        (
            "a dot, which the pattern excludes",
            "studio.recordings",
            pattern_message("name", NAME_PATTERN),
        ),
        (
            "a slash, which the pattern excludes",
            "studio/recordings",
            pattern_message("name", NAME_PATTERN),
        ),
        (
            "a non-ASCII letter",
            "naïve",
            pattern_message("name", NAME_PATTERN),
        ),
        (
            "a tab inside an otherwise legal name",
            "studio\trecordings",
            pattern_message("name", NAME_PATTERN),
        ),
    ];
    for (case, name, expected) in &refused {
        let request = json!({ "s3": { "bucketName": "refused-bucket" }, "name": name });
        let (status, response) = emulator.call("CreateStorageConfiguration", &request).await;
        assert_refused(case, status, &response, expected);
    }

    // A refused create left every accepted configuration exactly as it was.
    for (case, configuration_arn, created) in &stored {
        let read = emulator.get(case, configuration_arn).await;
        assert_eq!(
            &read, created,
            "case {case:?}: the refused creates must leave {configuration_arn} as created, \
             expected {created}, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// The modelled `s3.bucketName` constraints
// ---------------------------------------------------------------------------

/// The S3 destination the request names is persisted as sent for every bucket name inside
/// the member's modelled `@length` and `@pattern`, and a name outside them is the modelled
/// `ValidationException` (HTTP 400) naming the dotted member path — the member is nested,
/// so the same rule reaches inside the structure.
#[tokio::test]
async fn the_modeled_s3_bucket_name_constraints_are_enforced() {
    let emulator = RealTime::new();

    let shortest_bucket = "a".repeat(BUCKET_NAME_LENGTH.0);
    let longest_bucket = format!("a.b-{}", "c".repeat(BUCKET_NAME_LENGTH.1 - 4));
    let accepted: Vec<(&str, &str)> = vec![
        ("the modelled minimum length", &shortest_bucket),
        ("digits only", "12345"),
        ("the dot the pattern admits", "studio.recordings"),
        ("the hyphen the pattern admits", "studio-recordings"),
        (
            "the modelled maximum length, with both admitted punctuation characters",
            &longest_bucket,
        ),
    ];

    let mut stored: Vec<(String, String, Value)> = Vec::new();
    for (case, bucket) in &accepted {
        let request = json!({ "s3": { "bucketName": bucket } });
        let created = emulator.create(case, &request).await;
        assert_eq!(
            created["s3"], request["s3"],
            "case {case:?}: expected the s3 destination the request named, got {created}"
        );

        let configuration_arn = created["arn"].as_str().unwrap_or_default().to_string();
        let read = emulator.get(case, &configuration_arn).await;
        assert_eq!(
            read, created,
            "case {case:?}: GetStorageConfiguration must report the created configuration, \
             expected {created}, got {read}"
        );
        stored.push((case.to_string(), configuration_arn, created));
    }

    let under_long_bucket = "a".repeat(BUCKET_NAME_LENGTH.0 - 1);
    let over_long_bucket = "a".repeat(BUCKET_NAME_LENGTH.1 + 1);
    let refused: Vec<(&str, &str, String)> = vec![
        (
            "one character short of the modelled minimum length",
            &under_long_bucket,
            length_message(
                "s3.bucketName",
                "minimum",
                BUCKET_NAME_LENGTH.0,
                BUCKET_NAME_LENGTH.0 - 1,
            ),
        ),
        (
            "one character past the modelled maximum length",
            &over_long_bucket,
            length_message(
                "s3.bucketName",
                "maximum",
                BUCKET_NAME_LENGTH.1,
                BUCKET_NAME_LENGTH.1 + 1,
            ),
        ),
        (
            "upper case, which the pattern excludes",
            "Studio-Recordings",
            pattern_message("s3.bucketName", BUCKET_NAME_PATTERN),
        ),
        (
            "an underscore, which the pattern excludes",
            "studio_recordings",
            pattern_message("s3.bucketName", BUCKET_NAME_PATTERN),
        ),
        (
            "a slash, which the pattern excludes",
            "studio/recordings",
            pattern_message("s3.bucketName", BUCKET_NAME_PATTERN),
        ),
    ];
    for (case, bucket, expected) in &refused {
        let request = json!({ "s3": { "bucketName": bucket } });
        let (status, response) = emulator.call("CreateStorageConfiguration", &request).await;
        assert_refused(case, status, &response, expected);
    }

    // Nothing the refusals carried was persisted, and every accepted configuration is
    // exactly as it was created.
    for (case, configuration_arn, created) in &stored {
        let read = emulator.get(case, configuration_arn).await;
        assert_eq!(
            &read, created,
            "case {case:?}: the refused creates must leave {configuration_arn} as created, \
             expected {created}, got {read}"
        );
    }
    assert_eq!(
        emulator.held("after the refusals").await,
        stored.len(),
        "expected only the accepted creates to be held, got {} for {} accepted requests",
        emulator.held("after the refusals").await,
        stored.len()
    );
}

// ---------------------------------------------------------------------------
// The bound on the store this operation grows
// ---------------------------------------------------------------------------

/// The storage-configuration store is bounded: once it holds as many configurations as the
/// service admits, a further create is the modelled `ServiceQuotaExceededException` (HTTP
/// 402) stating the resource the quota is on and the limit reached, and the store does not
/// grow — the configurations already held are all still readable.
#[tokio::test]
async fn the_storage_configuration_store_is_bounded_and_reports_the_modeled_quota_error() {
    // Read the bound from the crate rather than restating it here, so this test cannot
    // drift from the bound the service actually enforces.
    use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::state::MAX_STORAGE_CONFIGURATIONS;

    let emulator = RealTime::new();

    // Fill the store, varying the request shape as it fills so the bound is reached with
    // configurations of several shapes rather than one repeated request.
    let mut arns = Vec::with_capacity(MAX_STORAGE_CONFIGURATIONS);
    for index in 0..MAX_STORAGE_CONFIGURATIONS {
        let request = match index % 3 {
            0 => json!({ "s3": { "bucketName": format!("bucket-{index}") } }),
            1 => json!({
                "s3": { "bucketName": format!("bucket-{index}") },
                "name": format!("configuration-{index}"),
            }),
            _ => json!({
                "s3": { "bucketName": format!("bucket.{index}") },
                "name": format!("configuration_{index}"),
                "tags": { "index": index.to_string() },
            }),
        };
        let created = emulator
            .create(&format!("configuration {index} of {MAX_STORAGE_CONFIGURATIONS}"), &request)
            .await;
        arns.push(created["arn"].as_str().unwrap_or_default().to_string());
    }

    let (status, response) = emulator
        .call(
            "CreateStorageConfiguration",
            &json!({ "s3": { "bucketName": "one-past-the-bound" } }),
        )
        .await;
    let case = "one configuration past the bound";
    assert_eq!(
        status, 402,
        "case {case:?}: expected HTTP 402, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ServiceQuotaExceededException",
        "case {case:?}: expected ServiceQuotaExceededException, got {response}"
    );
    // The quota-refusal wording this service states: the resource the quota is on and the
    // limit that was reached.
    assert_eq!(
        error_message(&response),
        format!("Resource: storage-configuration quota exceeded limit: {MAX_STORAGE_CONFIGURATIONS}"),
        "case {case:?}: expected the service's quota refusal wording, got {response}"
    );
    assert!(
        response.get("storageConfiguration").is_none(),
        "case {case:?}: a refused create must report no configuration, got {response}"
    );

    // The store did not grow, and every configuration it holds is still readable.
    assert_eq!(
        emulator.held(case).await,
        MAX_STORAGE_CONFIGURATIONS,
        "case {case:?}: expected the store to hold exactly \
         {MAX_STORAGE_CONFIGURATIONS} configurations"
    );
    let first = arns.first().expect("the store was filled");
    let last = arns.last().expect("the store was filled");
    for arn in [first, last] {
        let read = emulator.get(case, arn).await;
        assert_eq!(
            read["arn"], json!(arn),
            "case {case:?}: expected {arn} to still read back, got {read}"
        );
    }
}
