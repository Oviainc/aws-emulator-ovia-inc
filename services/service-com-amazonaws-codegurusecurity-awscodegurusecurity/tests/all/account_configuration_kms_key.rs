// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP cover for the KMS key `UpdateAccountConfiguration` commits.
//!
//! RULE: the `encryptionConfig.kmsKeyArn` a request submits names a KMS resource, so
//! CodeGuru Security RESOLVES it against KMS before it commits anything — a key KMS
//! answers about is stored and reported back by `GetAccountConfiguration`, and a value
//! KMS says names no key this account can read is refused with the modelled
//! `ResourceNotFoundException` (HTTP 404) with the stored configuration left exactly as
//! it was. An `encryptionConfig` naming no key asks for the AWS-managed key, so there is
//! nothing to resolve.
//!
//! KMS is mounted beside CodeGuru Security on the SAME engine, because the handler
//! resolves the key by calling KMS in process, exactly as the assembled emulator does.
//! One suite below mounts CodeGuru Security ALONE, so the lookup establishes nothing
//! about the key: a condition nobody attributed to the key must not refuse the update.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codegurusecurity_awscodegurusecurity::EmulatedService;

use crate::scan_lifecycle::{call, store_of, Answer, REGION};

/// A second Region, for the rows that show a KMS key is a REGIONAL resource: it is
/// looked up where its own ARN says it lives.
const OTHER_REGION: &str = "eu-central-1";

/// An account that is not the emulator's own, for the row whose ARN names a key of
/// another account (12 digits, as `AccountIdType` requires).
const OTHER_ACCOUNT: &str = "123456789012";

/// Both services on one shared engine, driven in-process through the production router.
/// `with_kms` decides whether the handler's in-process `DescribeKey` reaches a KMS at
/// all.
fn harness_with_kms(with_kms: bool) -> (std::sync::Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(std::sync::Arc::clone(&engine)));
    if with_kms {
        router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ));
    }
    (engine, router)
}

/// Mint a KMS key through the mounted KMS service's `region` endpoint and answer its
/// ARN, so a key a row means to be REAL is one KMS can resolve.
async fn create_kms_key(router: &MultiServiceRouter, region: &str) -> String {
    let host = format!("kms.{region}.amazonaws.com");
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/kms/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=abc"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "TrentService.CreateKey")
        .header("authorization", authorization)
        .body(Bytes::from_static(b"{}"))
        .expect("a well-formed CreateKey request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "CreateKey in {region} must succeed so a row has a real key, got {status} {body}"
    );
    body["KeyMetadata"]["Arn"]
        .as_str()
        .expect("CreateKey reports the new key's ARN")
        .to_string()
}

/// `PUT /updateAccountConfiguration` carrying `encryptionConfig`.
async fn update(router: &MultiServiceRouter, encryption_config: Value) -> Answer {
    call(
        router,
        "PUT",
        "/updateAccountConfiguration",
        Some(json!({ "encryptionConfig": encryption_config })),
    )
    .await
}

/// `GET /accountConfiguration/get` — the independent reader of what was committed.
async fn read_back(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", "/accountConfiguration/get", None).await
}

/// The `encryptionConfig.kmsKeyArn` an answer reports, or `None` for the AWS-managed key.
fn reported_key(answer: &Answer) -> Option<String> {
    answer.get("encryptionConfig")["kmsKeyArn"]
        .as_str()
        .map(str::to_string)
}

/// The `encryptionConfig` payload for a request that configures `key`, or that asks for
/// the AWS-managed key by passing an `encryptionConfig` with nothing in it.
fn body_for(key: Option<&str>) -> Value {
    match key {
        Some(key) => json!({ "kmsKeyArn": key }),
        None => json!({}),
    }
}

/// A `kmsKeyArn` with `arn`'s account segment replaced, so the ARN names a key of
/// another account. The segment sits between the ARN's region and its `:key/`, as the
/// modelled `EncryptionConfig$kmsKeyArn` pattern spells it.
fn in_account(arn: &str, account: &str) -> String {
    let (head, key) = arn
        .split_once(":key/")
        .expect("a minted key ARN names its key after `:key/`");
    let region_end = head
        .rfind(':')
        .expect("a minted key ARN carries its account after the region");
    format!("{}:{account}:key/{key}", &head[..region_end])
}

/// A well-formed `kmsKeyArn` in `region` naming `key_id`, for a row whose key is meant
/// to be absent. Built to the modelled pattern, not taken from any recorded call.
fn key_arn(region: &str, account: &str, key_id: &str) -> String {
    format!("arn:aws:kms:{region}:{account}:key/{key_id}")
}

#[tokio::test]
async fn a_key_kms_resolves_is_committed_and_reported_back() {
    let (engine, router) = harness_with_kms(true);

    // Real keys, minted through KMS: one in the Region the request is signed for, one in
    // another Region. Both resolve where their own ARN says they live, so both are
    // committed.
    let here = create_kms_key(&router, REGION).await;
    let elsewhere = create_kms_key(&router, OTHER_REGION).await;

    // The rows differ along what the request asks for: a key of this Region, a key of
    // another Region, the same key again (an update that changes nothing), and the empty
    // configuration that asks for the AWS-managed key and so resolves nothing at all.
    let rows: Vec<(&str, Option<String>)> = vec![
        ("a key KMS holds in the signing region", Some(here.clone())),
        ("a key KMS holds in another region", Some(elsewhere.clone())),
        ("the same key again", Some(elsewhere.clone())),
        ("the AWS-managed key, named by an empty config", None),
        ("a key again after clearing", Some(here.clone())),
    ];

    for (case, key) in &rows {
        let answer = update(&router, body_for(key.as_deref())).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {}",
            answer.status, answer.json
        );
        let echoed = reported_key(&answer);
        assert_eq!(
            echoed.as_deref(),
            key.as_deref(),
            "case {case:?}: expected the committed key {key:?} back, got {echoed:?}"
        );

        let stored = store_of(&engine).lock().encryption_kms_key_arn();
        assert_eq!(
            stored.as_deref(),
            key.as_deref(),
            "case {case:?}: the store must hold {key:?}, got {stored:?}"
        );
        let reported = reported_key(&read_back(&router).await);
        assert_eq!(
            reported.as_deref(),
            key.as_deref(),
            "case {case:?}: GetAccountConfiguration must report {key:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_key_kms_cannot_resolve_is_refused_and_changes_nothing() {
    let (engine, router) = harness_with_kms(true);

    // A real key first, so a refused update is observable as "the key that was configured
    // is still the one configured" rather than merely as "nothing was configured".
    let held = create_kms_key(&router, REGION).await;
    let held_elsewhere = create_kms_key(&router, OTHER_REGION).await;
    let seeded = update(&router, body_for(Some(&held))).await;
    assert_eq!(
        seeded.status, 200,
        "seeding a real key: expected 200, got {} {}",
        seeded.status, seeded.json
    );

    // Every row satisfies the modelled `EncryptionConfig$kmsKeyArn` pattern — so none of
    // them is a boundary refusal — and names no key this account can read. They differ
    // along WHY: a single-region key id KMS never minted, a multi-region (`mrk-`) key id
    // it never minted, a real key addressed through another Region (a KMS key is
    // regional), and a real key's id under another account.
    let rows: Vec<(&str, String)> = vec![
        (
            "a single-region key id KMS does not hold",
            key_arn(
                REGION,
                engine::emulator_account_id(),
                "00000000-0000-0000-0000-000000000000",
            ),
        ),
        (
            "a multi-region key id KMS does not hold",
            key_arn(
                REGION,
                engine::emulator_account_id(),
                "mrk-1234567890abcdef1234567890abcdef",
            ),
        ),
        (
            "a key of another region, named through this one",
            key_arn(
                REGION,
                engine::emulator_account_id(),
                held_elsewhere
                    .rsplit_once("key/")
                    .expect("a minted key ARN names its key after `key/`")
                    .1,
            ),
        ),
        (
            "a key id under another account",
            in_account(&held, OTHER_ACCOUNT),
        ),
    ];

    for (case, key) in &rows {
        let answer = update(&router, body_for(Some(key))).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 for {key:?}, got {} {}",
            answer.status, answer.json
        );
        assert_eq!(
            answer.error_type, "ResourceNotFoundException",
            "case {case:?}: expected error code ResourceNotFoundException, got {} {}",
            answer.error_type, answer.json
        );
        assert_eq!(
            answer.str_of("message"),
            "No existing account configuration found.",
            "case {case:?}: expected this operation's not-found wording, got {}",
            answer.json
        );

        // Nothing was written: the key configured before the refusal is still the one the
        // store holds and the one the reader reports.
        let stored = store_of(&engine).lock().encryption_kms_key_arn();
        assert_eq!(
            stored.as_deref(),
            Some(held.as_str()),
            "case {case:?}: a refused update must leave the configured key, got {stored:?}"
        );
        let reported = reported_key(&read_back(&router).await);
        assert_eq!(
            reported.as_deref(),
            Some(held.as_str()),
            "case {case:?}: GetAccountConfiguration must still report the configured key, \
             got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_lookup_that_establishes_nothing_about_the_key_accepts_it() {
    // KMS is not mounted here, so the handler's in-process `DescribeKey` reaches no KMS:
    // the lookup answers nothing ABOUT THE KEY. A condition nobody attributed to the key
    // is not a key that does not exist, so the update is accepted and committed — the
    // refusal above is driven by what KMS said, never by a lookup that failed to ask.
    let (engine, router) = harness_with_kms(false);

    let rows: Vec<(&str, String)> = vec![
        (
            "a single-region key id",
            key_arn(
                REGION,
                engine::emulator_account_id(),
                "1234abcd-12ab-34cd-56ef-1234567890ab",
            ),
        ),
        (
            "a multi-region key id",
            key_arn(
                OTHER_REGION,
                OTHER_ACCOUNT,
                "mrk-80bd8ecdcd4342aebd84b7dc9da5a3cf",
            ),
        ),
    ];

    for (case, key) in &rows {
        let answer = update(&router, body_for(Some(key))).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {key:?}, got {} {}",
            answer.status, answer.json
        );
        let stored = store_of(&engine).lock().encryption_kms_key_arn();
        assert_eq!(
            stored.as_deref(),
            Some(key.as_str()),
            "case {case:?}: the accepted key must be the one stored, got {stored:?}"
        );
        let reported = reported_key(&read_back(&router).await);
        assert_eq!(
            reported.as_deref(),
            Some(key.as_str()),
            "case {case:?}: GetAccountConfiguration must report the accepted key, got \
             {reported:?}"
        );
    }
}
