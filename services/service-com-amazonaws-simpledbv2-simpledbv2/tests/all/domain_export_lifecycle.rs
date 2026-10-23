// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the whole SimpleDBv2 surface: the domain-export
//! lifecycle (`StartDomainExport`, `GetExport`, `ListExports`).
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: each of the three operations computes its response from the
//! ONE persisted export record, so what `StartDomainExport` stored is what `GetExport`
//! and `ListExports` report, and an identifier that names no stored record is the
//! modelled client error rather than a fabricated answer.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_simpledbv2_simpledbv2::state::{
    ComAmazonawsSimpledbv2Simpledbv2State, StoredExport, IN_PROGRESS_SPAN, MANIFEST_SUMMARY_FILE,
    MAX_EXPORTS, MAX_EXPORT_ARN_LEN, MAX_PAGE_SIZE, MIN_EXPORT_ARN_LEN, STATUS_PENDING,
    SUCCEEDED_SPAN,
};
use service_com_amazonaws_simpledbv2_simpledbv2::EmulatedService;

/// The region every request below signs for; the minted ARNs must carry exactly it.
pub(crate) const REGION: &str = "us-west-2";
/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// Margin added to a documented span before asserting the move it owes has happened.
const SETTLE_MARGIN: Duration = Duration::from_millis(80);

/// One emulator with an empty store — no exports and NO domains — mounted exactly as
/// `build_router` mounts it.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// One emulator whose account already holds `domains`.
///
/// No modelled operation of this service creates a SimpleDB domain (that is the
/// SimpleDB 2009-04-15 `CreateDomain`), so a domain reaches the store only through its
/// in-process seam — `create_domain`, the same one `SIMPLEDB_EXISTING_DOMAINS` reaches
/// when the store is built. Every test that starts an export therefore declares the
/// domains it exports, exactly as an operator does.
pub(crate) fn emulator_with_domains(domains: &[&str]) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    declare_domains(&engine, domains);
    emulator_over(engine)
}

/// Declare `domains` in the store of `engine`, through the seam described on
/// [`emulator_with_domains`].
pub(crate) fn declare_domains(engine: &Arc<engine::InMemoryEngine>, domains: &[&str]) {
    let state = engine.state::<ComAmazonawsSimpledbv2Simpledbv2State>();
    let mut store = state.lock();
    for domain in domains {
        store.create_domain(domain);
    }
}

/// One emulator over an engine the caller already holds, so a test can seed this
/// service's store before any request runs.
pub(crate) fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return `(http status, parsed body)`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, _error_type, parsed) = call_reporting_error_type(router, operation, body).await;
    (status, parsed)
}

/// Call one operation and return `(http status, the `x-amzn-errortype` response
/// header, parsed body)` — the header verbatim, for a case that asserts the wire
/// signal an SDK dispatches its error handling on.
///
/// The routing signal an SDK request carries is the SigV4 credential scope, which
/// names this service's signing name (`sdb`) and the region the handlers read.
pub(crate) async fn call_reporting_error_type(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Option<String>, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/v2/{operation}"))
        .header("host", format!("sdb.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDSDBTEST/20260101/{REGION}/sdb/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`; the framework also sets the
    // `x-amzn-errortype` header, so fold the header in and one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type.clone(), &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, error_type, parsed)
}

/// A call that must answer 200.
pub(crate) async fn ok_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected 200, got {status:?} with body {parsed:?}"
    );
    parsed
}

/// The modelled error shape a failed call reported.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.simpledbv2#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A member of a JSON object as a string, or `None` when the member is absent.
pub(crate) fn member(body: &Value, name: &str) -> Option<String> {
    body.get(name).and_then(Value::as_str).map(str::to_string)
}

/// Whether `arn` is the export ARN this service mints for `domain`.
///
/// Checked by hand rather than with a regex: `ExportArn` carries only
/// `@length(20, 2048)` (source: `smithy_query(query="shape",shape="ExportArn")`), and
/// the resource shape asserted here is the one the service's own smoke test uses,
/// `arn:aws:sdb:<region>:<account>:domain/<domainName>/export/<id>` — whose id
/// segment, in that same smoke test, is the hyphenated RFC 4122 rendering
/// [`is_minted_id`] pins.
fn export_arn_of(arn: &str, domain: &str) -> bool {
    let prefix = format!("arn:aws:sdb:{REGION}:{ACCOUNT}:domain/{domain}/export/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        return false;
    };
    is_minted_id(id) && (MIN_EXPORT_ARN_LEN..=MAX_EXPORT_ARN_LEN).contains(&arn.len())
}

/// Whether `id` is the hyphenated RFC 4122 version-4 rendering this service mints —
/// `8-4-4-4-12` lower-case hex with the version nibble `4` and the variant nibble in
/// `8..=b`, the form the service's own `GetExport` smoke test shows an export id
/// taking (`3eb4eaed-872b-4e08-b4b6-ff6999a83e01`).
fn is_minted_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    if groups.iter().map(|g| g.len()).ne([8, 4, 4, 4, 12]) {
        return false;
    }
    if !id
        .chars()
        .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c))
    {
        return false;
    }
    groups[2].starts_with('4') && ('8'..='b').contains(&groups[3].chars().next().unwrap_or(' '))
}

/// The id segment (last path segment) of an export ARN.
fn export_id_of(arn: &str) -> String {
    arn.rsplit_once('/')
        .map_or_else(|| arn.to_string(), |(_, id)| id.to_string())
}

/// One `StartDomainExport` request and the optional members it supplies, so a table
/// row reads as the case it is.
#[derive(Debug, Clone)]
struct StartCase {
    name: &'static str,
    domain_name: &'static str,
    s3_bucket: &'static str,
    s3_key_prefix: Option<&'static str>,
    s3_sse_algorithm: Option<&'static str>,
    s3_sse_kms_key_id: Option<&'static str>,
    s3_bucket_owner: Option<&'static str>,
}

impl StartCase {
    fn new(name: &'static str, domain_name: &'static str, s3_bucket: &'static str) -> Self {
        Self {
            name,
            domain_name,
            s3_bucket,
            s3_key_prefix: None,
            s3_sse_algorithm: None,
            s3_sse_kms_key_id: None,
            s3_bucket_owner: None,
        }
    }

    fn prefix(mut self, prefix: &'static str) -> Self {
        self.s3_key_prefix = Some(prefix);
        self
    }

    fn sse(mut self, algorithm: &'static str) -> Self {
        self.s3_sse_algorithm = Some(algorithm);
        self
    }

    fn kms_key(mut self, key_id: &'static str) -> Self {
        self.s3_sse_kms_key_id = Some(key_id);
        self
    }

    fn bucket_owner(mut self, account: &'static str) -> Self {
        self.s3_bucket_owner = Some(account);
        self
    }

    /// The request body, carrying only the members this row supplies.
    fn body(&self, client_token: Option<&str>) -> Value {
        let mut request = json!({
            "domainName": self.domain_name,
            "s3Bucket": self.s3_bucket,
        });
        let object = request.as_object_mut().expect("a JSON object");
        for (name, value) in [
            ("clientToken", client_token),
            ("s3KeyPrefix", self.s3_key_prefix),
            ("s3SseAlgorithm", self.s3_sse_algorithm),
            ("s3SseKmsKeyId", self.s3_sse_kms_key_id),
            ("s3BucketOwner", self.s3_bucket_owner),
        ] {
            if let Some(value) = value {
                object.insert(name.to_string(), Value::String(value.to_string()));
            }
        }
        request
    }
}

/// The optional-member and value space `StartDomainExport` accepts, one row per shape
/// of request. The `s3SseAlgorithm` values are the modelled `S3SseAlgorithm` enum's
/// own (`AES256`, `KMS`); the bucket names span the forms the modelled
/// `S3BucketName` pattern `^[a-z0-9A-Z]+[\.\-\w]*[a-z0-9A-Z]+$` admits — lower case,
/// dotted, hyphenated, underscored and mixed case.
fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase::new("only the required members", "my-domain", "my-export-bucket"),
        StartCase::new("a key prefix", "orders", "exports.example.com").prefix("dumps/"),
        StartCase::new("SSE-S3", "customers", "customer_exports").sse("AES256"),
        StartCase::new("SSE-KMS with a key", "invoices", "InvoiceExports")
            .sse("KMS")
            .kms_key("arn:aws:kms:us-west-2:000000000000:key/1234abcd"),
        // The emulator's own account, the other end of the `^[0-9]{12}$` range from
        // the row that names a foreign one.
        StartCase::new("a bucket owner", "events", "ev3nts-bucket").bucket_owner("000000000000"),
        StartCase::new("every optional member at once", "warehouse", "wh-exports.v2")
            .prefix("tenant/42/")
            .sse("KMS")
            .kms_key("alias/simpledb-exports")
            .bucket_owner("210987654321"),
        StartCase::new("a one-character domain name", "d", "tiny-bucket"),
        StartCase::new(
            "a long key prefix at the modelled 850 bound",
            "bulk",
            "bulk-bucket",
        )
        .prefix(LONGEST_KEY_PREFIX),
    ]
}

/// A prefix of exactly 850 characters — the published maximum of `S3KeyPrefix`
/// (`@length(1, 850)`, source: `smithy_query(query="shape",shape="S3KeyPrefix")`).
//
// 850 = 17 * 50: fifty copies of a 17-character group, ten per source line. The
// assertion in `start_then_get_reports_every_stored_member` pins the length.
const LONGEST_KEY_PREFIX: &str = "0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg\
     0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg\
     0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg\
     0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg\
     0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg\
     0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg\
     0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg\
     0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg\
     0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg\
     0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg0123456789abcdefg";

/// The domain every row of [`start_cases`] exports, which an account has to hold
/// before any of those starts can succeed.
fn start_case_domains() -> Vec<&'static str> {
    start_cases()
        .into_iter()
        .map(|case| case.domain_name)
        .collect()
}

/// `StartDomainExport` persists a record and `GetExport` reports exactly it, for every
/// shape of request the model admits.
#[tokio::test]
async fn start_then_get_reports_every_stored_member() {
    let router = emulator_with_domains(&start_case_domains());
    // No two starts may mint the same identifier, whether an export id or a generated
    // client token.
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for case in start_cases() {
        let started = ok_call(&router, "StartDomainExport", &case.body(None)).await;

        let export_arn = member(&started, "exportArn")
            .unwrap_or_else(|| panic!("case {}: no exportArn in {started:?}", case.name));
        assert!(
            export_arn_of(&export_arn, case.domain_name),
            "case {}: expected an exportArn of the form \
             arn:aws:sdb:{REGION}:{ACCOUNT}:domain/{}/export/<id> within {MIN_EXPORT_ARN_LEN}..\
             {MAX_EXPORT_ARN_LEN} characters, got {export_arn:?}",
            case.name,
            case.domain_name
        );
        let client_token = member(&started, "clientToken")
            .unwrap_or_else(|| panic!("case {}: no clientToken in {started:?}", case.name));
        assert!(
            is_minted_id(&client_token),
            "case {}: a request that names no clientToken gets a minted one, got \
             {client_token:?}",
            case.name
        );
        for identifier in [export_id_of(&export_arn), client_token.clone()] {
            assert!(
                minted.insert(identifier.clone()),
                "case {}: {identifier:?} was already minted for an earlier export",
                case.name
            );
        }
        let requested_at = started
            .get("requestedAt")
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            requested_at.is_number(),
            "case {}: expected a requestedAt timestamp, got {requested_at:?}",
            case.name
        );

        // The round trip: everything the start stored is what the read reports.
        let fetched = ok_call(
            &router,
            "GetExport",
            &json!({ "exportArn": export_arn.clone() }),
        )
        .await;
        for (name, expected) in [
            ("exportArn", Some(export_arn.clone())),
            ("clientToken", Some(client_token.clone())),
            ("domainName", Some(case.domain_name.to_string())),
            ("s3Bucket", Some(case.s3_bucket.to_string())),
            ("s3KeyPrefix", case.s3_key_prefix.map(str::to_string)),
            ("s3SseAlgorithm", case.s3_sse_algorithm.map(str::to_string)),
            ("s3SseKmsKeyId", case.s3_sse_kms_key_id.map(str::to_string)),
            ("s3BucketOwner", case.s3_bucket_owner.map(str::to_string)),
        ] {
            let actual = member(&fetched, name);
            assert_eq!(
                actual, expected,
                "case {}: GetExport's {name} must be the stored value; expected \
                 {expected:?}, got {actual:?} in {fetched:?}",
                case.name
            );
        }
        assert_eq!(
            fetched.get("requestedAt"),
            Some(&requested_at),
            "case {}: GetExport's requestedAt must be the stored one, got {fetched:?}",
            case.name
        );
        // A fresh export is reported in a non-terminal status derived from the record,
        // never a fabricated terminal one.
        assert_eq!(
            member(&fetched, "exportStatus").as_deref(),
            Some("PENDING"),
            "case {}: a just-started export is PENDING, got {fetched:?}",
            case.name
        );
        for terminal_member in ["exportManifest", "itemsCount", "exportDataCutoffTime"] {
            assert!(
                fetched.get(terminal_member).is_none(),
                "case {}: {terminal_member} belongs to a SUCCEEDED export, got {fetched:?}",
                case.name
            );
        }
        for failure_member in ["failureCode", "failureMessage"] {
            assert!(
                fetched.get(failure_member).is_none(),
                "case {}: {failure_member} belongs to a FAILED export, got {fetched:?}",
                case.name
            );
        }
    }

    assert_eq!(
        LONGEST_KEY_PREFIX.len(),
        850,
        "the longest-prefix row must sit exactly on S3KeyPrefix's published 850 bound, \
         got {}",
        LONGEST_KEY_PREFIX.len()
    );
}

/// Inside the client token's window, the same token names the same export: identical
/// parameters replay it, changed parameters conflict.
#[tokio::test]
async fn a_repeat_with_the_same_client_token_is_idempotent() {
    let router = emulator_with_domains(&start_case_domains());

    for (index, case) in start_cases().into_iter().enumerate() {
        let token = format!("token-{index}");
        let first = ok_call(&router, "StartDomainExport", &case.body(Some(&token))).await;
        let second = ok_call(&router, "StartDomainExport", &case.body(Some(&token))).await;
        assert_eq!(
            first, second,
            "case {}: a repeat with the same clientToken and parameters must answer the \
             same export, got {first:?} then {second:?}",
            case.name
        );
        assert_eq!(
            member(&first, "clientToken").as_deref(),
            Some(token.as_str()),
            "case {}: the supplied clientToken is what the output carries, got {first:?}",
            case.name
        );

        // Exactly one export was created for the token, whatever the repeat count.
        let listed = ok_call(
            &router,
            "ListExports",
            &json!({ "domainName": case.domain_name }),
        )
        .await;
        let arns: Vec<String> = summary_arns(&listed);
        let expected_arn = member(&first, "exportArn").expect("the start minted an ARN");
        assert_eq!(
            arns,
            vec![expected_arn],
            "case {}: the repeat must have created no second export for this domain, got \
             {arns:?}",
            case.name
        );
    }
}

/// The parameter changes a same-token repeat may differ in, one row each.
#[tokio::test]
async fn a_repeat_with_the_same_token_and_changed_parameters_conflicts() {
    let router = emulator_with_domains(&["conflicts", "other-domain"]);
    let base = StartCase::new("base", "conflicts", "conflict-bucket")
        .prefix("one/")
        .sse("KMS")
        .kms_key("alias/first")
        .bucket_owner("000000000000");

    let changed = [
        (
            "a different domainName",
            StartCase::new("x", "other-domain", "conflict-bucket")
                .prefix("one/")
                .sse("KMS")
                .kms_key("alias/first")
                .bucket_owner("000000000000"),
        ),
        (
            "a different s3Bucket",
            StartCase::new("x", "conflicts", "other-bucket")
                .prefix("one/")
                .sse("KMS")
                .kms_key("alias/first")
                .bucket_owner("000000000000"),
        ),
        (
            "a different s3KeyPrefix",
            StartCase::new("x", "conflicts", "conflict-bucket")
                .prefix("two/")
                .sse("KMS")
                .kms_key("alias/first")
                .bucket_owner("000000000000"),
        ),
        (
            "a dropped s3KeyPrefix",
            StartCase::new("x", "conflicts", "conflict-bucket")
                .sse("KMS")
                .kms_key("alias/first")
                .bucket_owner("000000000000"),
        ),
        (
            "a different s3SseKmsKeyId",
            StartCase::new("x", "conflicts", "conflict-bucket")
                .prefix("one/")
                .sse("KMS")
                .kms_key("alias/second")
                .bucket_owner("000000000000"),
        ),
        (
            "a different s3BucketOwner",
            StartCase::new("x", "conflicts", "conflict-bucket")
                .prefix("one/")
                .sse("KMS")
                .kms_key("alias/first")
                .bucket_owner("210987654321"),
        ),
    ];

    for (index, (what, case)) in changed.into_iter().enumerate() {
        let token = format!("conflict-token-{index}");
        ok_call(&router, "StartDomainExport", &base.body(Some(&token))).await;
        let (status, body) = call(&router, "StartDomainExport", &case.body(Some(&token))).await;
        assert_eq!(
            status, 400,
            "case {what}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {what}: expected ConflictException, got {body:?}"
        );
    }
}

/// `s3SseKmsKeyId` "is only expected with KMS as the S3 SSE algorithm", so every other
/// pairing is the modelled parameter-combination refusal.
#[tokio::test]
async fn a_kms_key_without_the_kms_algorithm_is_refused() {
    let router = emulator_with_domains(&["combo", "combo-ok"]);
    let refused = [
        (
            "a KMS key with SSE-S3",
            StartCase::new("x", "combo", "combo-bucket")
                .sse("AES256")
                .kms_key("alias/k"),
        ),
        (
            "a KMS key with no algorithm at all",
            StartCase::new("x", "combo", "combo-bucket").kms_key("alias/k"),
        ),
    ];
    for (what, case) in refused {
        let (status, body) = call(&router, "StartDomainExport", &case.body(None)).await;
        assert_eq!(
            status, 400,
            "case {what}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterCombinationException",
            "case {what}: expected InvalidParameterCombinationException, got {body:?}"
        );
    }

    // The accepted pairings, for contrast: KMS with a key, and each algorithm alone.
    for (what, case) in [
        (
            "KMS with a key",
            StartCase::new("x", "combo-ok", "combo-bucket")
                .sse("KMS")
                .kms_key("alias/k"),
        ),
        (
            "KMS without a key",
            StartCase::new("x", "combo-ok", "combo-bucket").sse("KMS"),
        ),
        (
            "SSE-S3 without a key",
            StartCase::new("x", "combo-ok", "combo-bucket").sse("AES256"),
        ),
    ] {
        let (status, body) = call(&router, "StartDomainExport", &case.body(None)).await;
        assert_eq!(
            status, 200,
            "case {what}: expected HTTP 200, got {status} with {body:?}"
        );
    }
}

/// `GetExport` separates "not an export ARN" from "an export ARN naming nothing".
#[tokio::test]
async fn get_export_refuses_a_non_arn_and_reports_an_unknown_one() {
    let router = emulator();

    // Every row is at least 20 characters, so the modelled `@length(20, 2048)` passes
    // and the handler's own rule is what answers.
    let malformed = [
        ("no arn prefix at all", "this-is-not-an-arn-at-all"),
        (
            "another service's namespace",
            "arn:aws:sqs:us-west-2:000000000000:export/abc123",
        ),
        (
            "an empty partition field",
            "arn::sdb:us-west-2:000000000000:export/abc123",
        ),
        (
            "too few ARN fields",
            "arn:aws:sdb:us-west-2:export/abc123xyz",
        ),
        (
            "no export segment",
            "arn:aws:sdb:us-west-2:000000000000:domain/my-domain",
        ),
        (
            "a path element merely ending in 'export'",
            "arn:aws:sdb:us-west-2:000000000000:notexport/abc123",
        ),
        (
            "an empty export id",
            "arn:aws:sdb:us-west-2:000000000000:domain/my-domain/export/",
        ),
    ];
    for (what, arn) in malformed {
        let (status, body) = call(&router, "GetExport", &json!({ "exportArn": arn })).await;
        assert_eq!(
            status, 400,
            "case {what}: expected HTTP 400 for {arn:?}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {what}: expected InvalidParameterValueException for {arn:?}, got {body:?}"
        );
    }

    // Well-formed export ARNs that name nothing. Both resource forms the service's own
    // documents use are admitted, and a foreign account is well-formed too.
    let unknown = [
        (
            "the plain export/<id> form of the model's GetExport example",
            "arn:aws:sdb:us-east-1:123456789012:export/abc123",
        ),
        (
            "the domain/<name>/export/<uuid> form of the model's smoke test",
            "arn:aws:sdb:us-east-1:123456789012:domain/testDomain/export/\
             3eb4eaed-872b-4e08-b4b6-ff6999a83e01",
        ),
        (
            "this emulator's own region and account",
            "arn:aws:sdb:us-west-2:000000000000:domain/my-domain/export/not-a-stored-id",
        ),
        (
            "another partition",
            "arn:aws-cn:sdb:cn-north-1:000000000000:export/abc123",
        ),
    ];
    for (what, arn) in unknown {
        let (status, body) = call(&router, "GetExport", &json!({ "exportArn": arn })).await;
        assert_eq!(
            status, 400,
            "case {what}: expected HTTP 400 for {arn:?}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchExportException",
            "case {what}: expected NoSuchExportException for {arn:?}, got {body:?}"
        );
    }
}

/// The `exportArn`s of a `ListExports` page, in the order the page carries them.
pub(crate) fn summary_arns(listed: &Value) -> Vec<String> {
    listed
        .get("exportSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("exportSummaries is required, got {listed:?}"))
        .iter()
        .map(|summary| {
            member(summary, "exportArn")
                .unwrap_or_else(|| panic!("a summary without an exportArn: {summary:?}"))
        })
        .collect()
}

/// Walk `ListExports` from the first page to the last, following `nextToken`, and
/// return every ARN it reported.
async fn walk_pages(
    router: &MultiServiceRouter,
    request: &Value,
    page_size: Option<i32>,
) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..64 {
        let mut body = request.clone();
        let object = body.as_object_mut().expect("a JSON object");
        if let Some(size) = page_size {
            object.insert("maxResults".to_string(), json!(size));
        }
        match &token {
            Some(value) => {
                object.insert("nextToken".to_string(), json!(value));
            }
            None => {
                object.remove("nextToken");
            }
        }
        let listed = ok_call(router, "ListExports", &body).await;
        let page = summary_arns(&listed);
        if let Some(size) = page_size {
            assert!(
                page.len() <= size as usize,
                "maxResults {size}: a page carried {} summaries",
                page.len()
            );
        }
        seen.extend(page);
        token = member(&listed, "nextToken");
        if token.is_none() {
            return seen;
        }
    }
    panic!("ListExports never ran out of pages for {request:?} at page size {page_size:?}");
}

/// `ListExports` enumerates the stored records, filters by domain, and honours
/// `maxResults` as a page bound across every page size.
#[tokio::test]
async fn list_exports_filters_and_pages_over_the_stored_records() {
    // `gamma` is a domain this account holds that no export names, so a filter on it
    // resolves and answers the required member as an empty list.
    let router = emulator_with_domains(&["alpha", "beta", "gamma"]);

    // Two domains, interleaved, so neither the filter nor the order can be faked by
    // grouping.
    let layout = [
        ("alpha", "alpha-bucket"),
        ("beta", "beta-bucket"),
        ("alpha", "alpha-bucket-two"),
        ("beta", "beta-bucket-two"),
        ("alpha", "alpha-bucket-three"),
    ];
    let mut created: Vec<(String, String)> = Vec::new();
    for (domain, bucket) in layout {
        let started = ok_call(
            &router,
            "StartDomainExport",
            &json!({ "domainName": domain, "s3Bucket": bucket }),
        )
        .await;
        created.push((
            domain.to_string(),
            member(&started, "exportArn").expect("the start minted an ARN"),
        ));
    }
    let all: Vec<String> = created.iter().map(|(_, arn)| arn.clone()).collect();

    // An unfiltered single page carries every record, in creation order, with no token.
    let listed = ok_call(&router, "ListExports", &json!({})).await;
    assert_eq!(
        summary_arns(&listed),
        all,
        "case unfiltered: ListExports must report every stored export in creation order, \
         got {listed:?}"
    );
    assert!(
        listed.get("nextToken").is_none(),
        "case unfiltered: the last page carries no nextToken, got {listed:?}"
    );
    // Each summary's other three members come from the same record.
    for summary in listed
        .get("exportSummaries")
        .and_then(Value::as_array)
        .expect("exportSummaries is required")
    {
        let arn = member(summary, "exportArn").expect("an ARN");
        let fetched = ok_call(&router, "GetExport", &json!({ "exportArn": arn.clone() })).await;
        for name in ["exportStatus", "domainName"] {
            assert_eq!(
                member(summary, name),
                member(&fetched, name),
                "case {arn}: the summary's {name} must match the record GetExport reports, \
                 summary {summary:?} record {fetched:?}"
            );
        }
        assert_eq!(
            summary.get("requestedAt"),
            fetched.get("requestedAt"),
            "case {arn}: the summary's requestedAt must match the record's, summary \
             {summary:?} record {fetched:?}"
        );
    }

    // A filter selects exactly that domain's exports; a domain the account holds that
    // has none answers the required member as an empty list.
    for domain in ["alpha", "beta", "gamma"] {
        let expected: Vec<String> = created
            .iter()
            .filter(|(stored_domain, _)| stored_domain == domain)
            .map(|(_, arn)| arn.clone())
            .collect();
        let listed = ok_call(&router, "ListExports", &json!({ "domainName": domain })).await;
        assert_eq!(
            summary_arns(&listed),
            expected,
            "case filter {domain}: expected {expected:?}, got {listed:?}"
        );
    }

    // Every page size from 1 past the record count sees each export exactly once, in
    // creation order, and the final page carries no token. The unbounded request and
    // one past the server's own ceiling are the ends of the range.
    for page_size in [
        Some(1),
        Some(2),
        Some(3),
        Some(5),
        Some(7),
        Some(MAX_PAGE_SIZE),
        Some(MAX_PAGE_SIZE + 1),
        None,
    ] {
        let walked = walk_pages(&router, &json!({}), page_size).await;
        assert_eq!(
            walked, all,
            "case maxResults {page_size:?}: paging must yield every export exactly once in \
             creation order, got {walked:?}"
        );
        let unique: BTreeSet<&String> = walked.iter().collect();
        assert_eq!(
            unique.len(),
            walked.len(),
            "case maxResults {page_size:?}: an export was reported twice in {walked:?}"
        );

        let walked_alpha = walk_pages(&router, &json!({ "domainName": "alpha" }), page_size).await;
        let expected_alpha: Vec<String> = created
            .iter()
            .filter(|(domain, _)| domain == "alpha")
            .map(|(_, arn)| arn.clone())
            .collect();
        assert_eq!(
            walked_alpha, expected_alpha,
            "case maxResults {page_size:?} filtered on alpha: expected {expected_alpha:?}, \
             got {walked_alpha:?}"
        );
    }
}

/// A `nextToken` this service did not issue is the modelled refusal, not a page.
#[tokio::test]
async fn an_unparseable_next_token_is_refused() {
    let router = emulator_with_domains(&["tokens"]);
    ok_call(
        &router,
        "StartDomainExport",
        &json!({ "domainName": "tokens", "s3Bucket": "token-bucket" }),
    )
    .await;

    let garbage = [
        ("plain text", "garbage"),
        ("odd-length hex", "abc"),
        ("non-hex characters", "zzzz"),
        // Hex of "0": a position before the first record, which no page ever starts at.
        // The offset is carried with the filter digest, so a bare offset is also not a
        // token this service issued.
        ("the bare zero offset", "30"),
        // Hex of "9999": a position past everything the store holds.
        ("a bare offset past the store", "39393939"),
        // Hex of "not-a-number".
        (
            "hex of something that is not a number",
            "6e6f742d612d6e756d626572",
        ),
    ];
    for (what, token) in garbage {
        let (status, body) = call(&router, "ListExports", &json!({ "nextToken": token })).await;
        assert_eq!(
            status, 400,
            "case {what}: expected HTTP 400 for nextToken {token:?}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidNextTokenException",
            "case {what}: expected InvalidNextTokenException for nextToken {token:?}, got {body:?}"
        );
    }
}

/// A `nextToken` is valid only for the enumeration it was issued for: continuing one
/// stream with another's cursor would hand back a page the caller never started.
#[tokio::test]
async fn a_next_token_from_another_enumeration_is_refused() {
    let router = emulator_with_domains(&["alpha", "beta"]);
    for (domain, bucket) in [
        ("alpha", "alpha-bucket"),
        ("beta", "beta-bucket"),
        ("alpha", "alpha-bucket-two"),
        ("beta", "beta-bucket-two"),
    ] {
        ok_call(
            &router,
            "StartDomainExport",
            &json!({ "domainName": domain, "s3Bucket": bucket }),
        )
        .await;
    }

    /// The first page of one enumeration, and the token it hands back.
    async fn first_page_token(router: &MultiServiceRouter, request: &Value) -> String {
        let mut body = request.clone();
        body.as_object_mut()
            .expect("a JSON object")
            .insert("maxResults".to_string(), json!(1));
        let listed = ok_call(router, "ListExports", &body).await;
        member(&listed, "nextToken")
            .unwrap_or_else(|| panic!("expected a nextToken from {body:?}, got {listed:?}"))
    }

    // Every pairing of "the enumeration a token came from" with "the enumeration it is
    // then offered to". The diagonal continues its own stream and must be accepted;
    // every off-diagonal pairing must be refused.
    let enumerations = [
        ("no filter", json!({})),
        ("the alpha filter", json!({ "domainName": "alpha" })),
        ("the beta filter", json!({ "domainName": "beta" })),
    ];
    for (issued_by, issuing_request) in &enumerations {
        let token = first_page_token(&router, issuing_request).await;
        for (offered_to, continuing_request) in &enumerations {
            let mut body = continuing_request.clone();
            let object = body.as_object_mut().expect("a JSON object");
            object.insert("maxResults".to_string(), json!(1));
            object.insert("nextToken".to_string(), json!(token.clone()));
            let (status, answered) = call(&router, "ListExports", &body).await;
            if issued_by == offered_to {
                assert_eq!(
                    status, 200,
                    "case a token issued by {issued_by} continuing its own enumeration: \
                     expected HTTP 200, got {status} with {answered:?}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case a token issued by {issued_by} offered to {offered_to}: expected HTTP \
                 400, got {status} with {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "InvalidNextTokenException",
                "case a token issued by {issued_by} offered to {offered_to}: expected \
                 InvalidNextTokenException, got {answered:?}"
            );
        }
    }
}

/// The export's status advances on the CLOCK, not on being read, and the terminal
/// members appear exactly when it reaches `SUCCEEDED`.
#[tokio::test]
async fn export_status_advances_on_the_clock() {
    let router = emulator_with_domains(&["clocked"]);
    let case = StartCase::new("clock", "clocked", "clock-bucket").prefix("snapshots/");
    let started = ok_call(&router, "StartDomainExport", &case.body(None)).await;
    let export_arn = member(&started, "exportArn").expect("the start minted an ARN");
    let get = json!({ "exportArn": export_arn.clone() });

    // Straight after the call the transient status is still observable: the span is
    // never zero.
    let pending = ok_call(&router, "GetExport", &get).await;
    assert_eq!(
        member(&pending, "exportStatus").as_deref(),
        Some("PENDING"),
        "case immediately after the start: expected PENDING, got {pending:?}"
    );

    // A blocking sleep is deliberate: this test owns its thread, nothing in the
    // handler sleeps, and this crate's tokio does not enable the `time` feature
    // `tokio::time::sleep` needs.
    std::thread::sleep(IN_PROGRESS_SPAN + SETTLE_MARGIN);
    let in_progress = ok_call(&router, "GetExport", &get).await;
    assert_eq!(
        member(&in_progress, "exportStatus").as_deref(),
        Some("IN_PROGRESS"),
        "case past the IN_PROGRESS span: expected IN_PROGRESS, got {in_progress:?}"
    );
    assert!(
        in_progress.get("exportManifest").is_none(),
        "case past the IN_PROGRESS span: the manifest belongs to SUCCEEDED, got {in_progress:?}"
    );

    std::thread::sleep(SUCCEEDED_SPAN + SETTLE_MARGIN);
    let succeeded = ok_call(&router, "GetExport", &get).await;
    assert_eq!(
        member(&succeeded, "exportStatus").as_deref(),
        Some("SUCCEEDED"),
        "case past the SUCCEEDED span: expected SUCCEEDED, got {succeeded:?}"
    );
    let expected_manifest = format!(
        "snapshots/{}/{MANIFEST_SUMMARY_FILE}",
        export_id_of(&export_arn)
    );
    assert_eq!(
        member(&succeeded, "exportManifest").as_deref(),
        Some(expected_manifest.as_str()),
        "case past the SUCCEEDED span: the manifest key is built from the stored prefix and \
         the export id; expected {expected_manifest:?}, got {succeeded:?}"
    );
    assert_eq!(
        succeeded.get("itemsCount").and_then(Value::as_i64),
        Some(0),
        "case past the SUCCEEDED span: this service models no item-level operation, so the \
         exported-item count is zero, got {succeeded:?}"
    );
    let requested_at = started
        .get("requestedAt")
        .and_then(Value::as_f64)
        .expect("a requestedAt timestamp");
    let cutoff = succeeded
        .get("exportDataCutoffTime")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("case past the SUCCEEDED span: expected an exportDataCutoffTime, got {succeeded:?}")
        });
    assert!(
        cutoff >= requested_at,
        "case past the SUCCEEDED span: the cutoff must not precede the request; requestedAt \
         {requested_at}, cutoff {cutoff}"
    );

    // Terminal is terminal: reading again neither advances the status nor moves the
    // members that came with it.
    let again = ok_call(&router, "GetExport", &get).await;
    assert_eq!(
        again, succeeded,
        "case a second read of a SUCCEEDED export: a read must not change what a later \
         caller sees; first {succeeded:?}, second {again:?}"
    );

    // And the list reports the same status as the record.
    let listed = ok_call(&router, "ListExports", &json!({})).await;
    let summaries = listed
        .get("exportSummaries")
        .and_then(Value::as_array)
        .expect("exportSummaries is required");
    assert_eq!(
        summaries.len(),
        1,
        "case the clock test's store: expected exactly one export, got {listed:?}"
    );
    assert_eq!(
        member(&summaries[0], "exportStatus").as_deref(),
        Some("SUCCEEDED"),
        "case the summary of a SUCCEEDED export: expected SUCCEEDED, got {listed:?}"
    );
}

/// `domainName` lands inside the minted `exportArn`, whose `:`-separated fields and
/// `/`-separated resource path identify the export, so a name the ARN cannot carry is
/// refused rather than answered with a value the model does not admit.
#[tokio::test]
async fn a_domain_name_the_export_arn_cannot_carry_is_refused() {
    // The fixed part of the ARN (`arn:aws:sdb:<region>:<account>:domain//export/<uuid>`)
    // is well under 150 characters, so these two length rows sit either side of the
    // bound.
    let just_inside = "d".repeat(MAX_EXPORT_ARN_LEN - 150);
    let past_the_bound = "d".repeat(MAX_EXPORT_ARN_LEN + 1);
    // Only the accepted row's domain is one the account holds: every refused row is
    // refused on the value it supplies, before any domain is resolved.
    let router = emulator_with_domains(&[just_inside.as_str()]);

    let accepted = ok_call(
        &router,
        "StartDomainExport",
        &json!({ "domainName": just_inside, "s3Bucket": "overflow-bucket" }),
    )
    .await;
    let arn = member(&accepted, "exportArn").expect("the start minted an ARN");
    assert!(
        arn.len() <= MAX_EXPORT_ARN_LEN,
        "case a domainName just inside the bound: the minted ARN is {} characters, past the \
         {MAX_EXPORT_ARN_LEN} an exportArn may carry",
        arn.len()
    );

    let refused = [
        ("a name past the ARN's length bound", past_the_bound.as_str()),
        ("a path separator", "two/segments"),
        ("an ARN field separator", "two:fields"),
        ("a carriage return", "cr\rinjected"),
        ("a line feed", "lf\ninjected"),
        ("a NUL", "nul\0injected"),
    ];
    for (what, domain_name) in refused {
        let (status, body) = call(
            &router,
            "StartDomainExport",
            &json!({ "domainName": domain_name, "s3Bucket": "overflow-bucket" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {what}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {what}: expected InvalidParameterValueException, got {body:?}"
        );
    }

    // Nothing the refusals named reached the store: only the accepted row is listed.
    let listed = ok_call(&router, "ListExports", &json!({})).await;
    assert_eq!(
        summary_arns(&listed),
        vec![arn],
        "case after six refusals: only the accepted start may have stored a record, got \
         {listed:?}"
    );
}

/// A store already holding its ceiling refuses another start with the modelled quota
/// error, and stores nothing: the bound lives in the store, so it holds whoever calls.
#[tokio::test]
async fn a_start_past_the_store_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // Seed the store directly, through the same shared engine the service is about to
    // be mounted on: driving MAX_EXPORTS starts over the wire would test the ceiling
    // by the slowest possible route.
    {
        let state = engine.state::<ComAmazonawsSimpledbv2Simpledbv2State>();
        let mut store = state.lock();
        // The domain the start below names: the account holds it, so what refuses that
        // start is the store's ceiling and nothing else.
        store.create_domain("one-too-many");
        let now = std::time::SystemTime::now();
        for index in 0..MAX_EXPORTS {
            let export_id = format!("seeded-{index}");
            let stored = store.insert_export(StoredExport {
                export_arn: format!(
                    "arn:aws:sdb:{REGION}:{ACCOUNT}:domain/seeded/export/{export_id}"
                ),
                export_id,
                client_token: format!("seeded-token-{index}"),
                domain_name: "seeded".to_string(),
                export_status: STATUS_PENDING.to_string(),
                requested_at: now,
                s3_bucket: "seeded-bucket".to_string(),
                s3_key_prefix: None,
                s3_sse_algorithm: None,
                s3_sse_kms_key_id: None,
                s3_bucket_owner: None,
                export_manifest: None,
                items_count: None,
                export_data_cutoff_time: None,
                failure_code: None,
                failure_message: None,
                in_progress_due_at: now + IN_PROGRESS_SPAN,
                succeeded_due_at: now + SUCCEEDED_SPAN,
            });
            assert!(
                stored,
                "case seeding record {index}: the store must accept every record up to its \
                 {MAX_EXPORTS} ceiling"
            );
        }
        assert_eq!(
            store.export_count(),
            MAX_EXPORTS,
            "case the seeded store: expected {MAX_EXPORTS} records, got {}",
            store.export_count()
        );
    }

    let router = emulator_over(engine);
    let (status, body) = call(
        &router,
        "StartDomainExport",
        &json!({ "domainName": "one-too-many", "s3Bucket": "ceiling-bucket" }),
    )
    .await;
    assert_eq!(
        status, 409,
        "case a full store: expected HTTP 409, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "NumberExportsLimitExceeded",
        "case a full store: expected NumberExportsLimitExceeded, got {body:?}"
    );

    // And nothing was stored for the refused start.
    let listed = ok_call(&router, "ListExports", &json!({ "domainName": "one-too-many" })).await;
    assert_eq!(
        summary_arns(&listed),
        Vec::<String>::new(),
        "case a full store: a refused start must store nothing, got {listed:?}"
    );
}
