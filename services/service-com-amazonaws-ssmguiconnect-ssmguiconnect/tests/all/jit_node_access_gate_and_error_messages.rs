// SPDX-License-Identifier: CC0-1.0

//! The three rules this service answers a request by that are not about the stored
//! record itself, each asserted over a TABLE of inputs rather than one case.
//!
//! 1. THE ACCOUNT-LEVEL FEATURE GATE. Configuring the resource that records a
//!    feature's sessions is refused unless the account is enrolled in that feature:
//!    every `UpdateConnectionRecordingPreferences` on an account that is not enrolled
//!    in just-in-time node access answers the modeled `AccessDeniedException` (403),
//!    whatever the request holds, and leaves nothing behind. The gate is evaluated
//!    AFTER the request-shape validation the model declares and BEFORE anything the
//!    request names is examined. `DeleteConnectionRecordingPreferences` is not gated.
//! 2. THE IDEMPOTENT REMOVAL. An `@idempotent` delete is asked for a state, not for a
//!    resource: with nothing configured it succeeds (200) and reports the effective
//!    `ClientToken`, however many times it is called.
//! 3. THIS SERVICE'S ERROR TEXT. The absent singleton is reported through one
//!    not-found TEMPLATE naming the resource, and a `@length` bound is reported as one
//!    sentence naming the member path and the bound it broke — with no aggregated
//!    `N validation error(s) detected` prefix and no echo of the offending value.
//!
//! Every case drives real HTTP through the production
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter), so what is
//! asserted is wire behaviour (status, error code, message), not an internal call.

use std::sync::Arc;

use service_com_amazonaws_ssmguiconnect_ssmguiconnect::state::{
    resource_not_found_message, CONNECTION_RECORDING_PREFERENCES,
    JUST_IN_TIME_NODE_ACCESS_DISABLED,
};
use service_com_amazonaws_ssmguiconnect_ssmguiconnect::{
    ComAmazonawsSsmguiconnectSsmguiconnectState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying only this service over a fresh engine, so each case has its own
/// store. The account starts as AWS leaves one: NOT enrolled in just-in-time node
/// access.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the framework's job and is exercised elsewhere.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// Enroll (or un-enroll) the account in just-in-time node access.
///
/// Written to the account's own state rather than sent as a request because
/// enrollment belongs to SSM Quick Setup: no operation of THIS service performs it.
fn enroll(engine: &engine::InMemoryEngine, enrolled: bool) {
    engine
        .state::<ComAmazonawsSsmguiconnectSsmguiconnectState>()
        .lock()
        .set_just_in_time_node_access_enabled(enrolled);
}

/// A restJson1 request for `op`, routed by its SigV4 credential scope the way every
/// signed AWS SDK request is.
fn request(op: &str, body: &serde_json::Value) -> http::Request<bytes::Bytes> {
    http::Request::builder()
        .method("POST")
        .uri(format!("/{op}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/ssm-guiconnect/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("build the request")
}

/// Status code plus parsed JSON body of one call.
async fn call(
    router: &MultiServiceRouter,
    op: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let response = router.dispatch(request(op, &body)).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

/// The `__type` of a restJson1 error body, shortened to the shape name.
fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` of an error body, under either casing a protocol spells it with.
fn error_message(body: &serde_json::Value) -> String {
    for key in ["message", "Message"] {
        if let Some(text) = body.get(key).and_then(serde_json::Value::as_str) {
            return text.to_string();
        }
    }
    format!("<no message in {body}>")
}

/// A well-formed `ConnectionRecordingPreferences` body.
fn preferences(owner: &str, bucket: &str, key_arn: &str) -> serde_json::Value {
    serde_json::json!({
        "RecordingDestinations": {
            "S3Buckets": [{ "BucketOwner": owner, "BucketName": bucket }]
        },
        "KMSKeyArn": key_arn,
    })
}

const KEY_ARN: &str = "arn:aws:kms:us-east-1:123456789012:key/1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";

const UPDATE: &str = "UpdateConnectionRecordingPreferences";
const GET: &str = "GetConnectionRecordingPreferences";
const DELETE: &str = "DeleteConnectionRecordingPreferences";

/// Assert that the singleton is reported absent, which is what "the call left nothing
/// behind" means to the resource's own reader.
async fn assert_absent(router: &MultiServiceRouter, case: &str) {
    let (status, got) = call(router, GET, serde_json::json!({})).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the singleton to still be absent, got {status:?} with {got:?}"
    );
    assert_eq!(
        error_code(&got),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {got:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1 — the account-level just-in-time node access gate
// ---------------------------------------------------------------------------

/// Every Update on an account that is not enrolled is refused with the modeled
/// `AccessDeniedException` (403), and persists nothing.
///
/// The rows differ along everything the request can vary that the rule does NOT look
/// at: the bucket (including one no bucket-existence check could resolve), the key
/// (including a value that is not an ARN at all, and the shortest the `@length(1,
/// 2048)` admits), whether a `ClientToken` is sent, and its value.
#[tokio::test]
async fn an_unenrolled_account_is_denied_every_update() {
    // The gate's own denial text, as AWS recorded it. The REQUIREMENT is the
    // rejection class (403 / `AccessDeniedException`); this message is what
    // distinguishes the feature gate from an IAM denial, so it is pinned once here
    // and asserted through the crate's constant below.
    assert_eq!(
        JUST_IN_TIME_NODE_ACCESS_DISABLED,
        "Just-in-time node access is not enabled. You must set up just-in-time node access to \
         record RDP connections.",
        "the gate's denial message must stay the one that names the feature, got {:?}",
        JUST_IN_TIME_NODE_ACCESS_DISABLED
    );

    // (case, request body)
    let cases: [(&str, serde_json::Value); 6] = [
        (
            "a well-formed request",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", KEY_ARN),
                "ClientToken": "gate-1",
            }),
        ),
        (
            "a bucket that does not exist",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "no-such-bucket-anywhere", KEY_ARN),
                "ClientToken": "gate-2",
            }),
        ),
        (
            "a KMSKeyArn that is not an ARN",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", "not-a-valid-kms-arn"),
                "ClientToken": "gate-3",
            }),
        ),
        (
            "the shortest KMSKeyArn the bound admits",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("123456789012", "rdp-recordings", "k"),
                "ClientToken": "gate-4",
            }),
        ),
        (
            "no ClientToken at all",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("210987654321", "other.recordings", KEY_ARN),
            }),
        ),
        (
            "the longest ClientToken the bound admits",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("210987654321", "another-bucket", KEY_ARN),
                "ClientToken": "t".repeat(64),
            }),
        ),
    ];

    for (case, body) in cases {
        let router = router();
        let (status, response) = call(&router, UPDATE, body).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected the gate to deny the call with 403, got {status:?} with \
             {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {response:?}"
        );
        assert_eq!(
            error_message(&response),
            JUST_IN_TIME_NODE_ACCESS_DISABLED,
            "case {case:?}: expected the feature gate's own denial message, got {response:?}"
        );
        // A denied Update leaves no partial state.
        assert_absent(&router, case).await;
    }
}

/// The denial is STABLE: it does not depend on how often the call is made, on the
/// `ClientToken` being new or replayed, or on the calls overlapping — and it never
/// turns into a conflict or a partial success.
#[tokio::test]
async fn the_denial_is_stable_across_retries_and_concurrent_calls() {
    let body = |token: &str| {
        serde_json::json!({
            "ConnectionRecordingPreferences":
                preferences("123456789012", "rdp-recordings", KEY_ARN),
            "ClientToken": token,
        })
    };

    // (case, the tokens sent one after another)
    let sequential: [(&str, Vec<&str>); 3] = [
        ("the same token twice", vec!["same-token", "same-token"]),
        (
            "the same token five times",
            vec!["retried"; 5],
        ),
        (
            "three distinct tokens",
            vec!["first", "second", "third"],
        ),
    ];
    for (case, tokens) in sequential {
        let router = router();
        for (attempt, token) in tokens.iter().enumerate() {
            let (status, response) = call(&router, UPDATE, body(token)).await;
            assert_eq!(
                status, 403,
                "case {case:?}, attempt {attempt}: expected 403, got {status:?} with {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "AccessDeniedException",
                "case {case:?}, attempt {attempt}: expected AccessDeniedException, got \
                 {response:?}"
            );
        }
        assert_absent(&router, case).await;
    }

    // Overlapping calls: all are denied, none succeeds, and none is answered with a
    // conflict — the gate is not a lock.
    let router = router();
    let (a, b, c) = tokio::join!(
        call(&router, UPDATE, body("concurrent-a")),
        call(&router, UPDATE, body("concurrent-b")),
        call(&router, UPDATE, body("concurrent-c")),
    );
    for (racer, (status, response)) in [("a", a), ("b", b), ("c", c)] {
        assert_eq!(
            status, 403,
            "case \"three concurrent updates\", racer {racer:?}: expected 403, got {status:?} \
             with {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "AccessDeniedException",
            "case \"three concurrent updates\", racer {racer:?}: expected AccessDeniedException, \
             got {response:?}"
        );
    }
    assert_absent(&router, "three concurrent updates").await;
}

/// WHERE the gate sits in the order of checks, asserted from both sides on one
/// un-enrolled account:
///
/// * a request outside a modeled constraint is refused as invalid (400) — shape
///   validation runs FIRST, so a malformed request never reaches the gate;
/// * a request whose shape is legal but whose contents could only be refused by
///   looking something up (a bucket that does not exist, a key that is not an ARN) is
///   refused by the GATE (403) — nothing the request names is examined.
#[tokio::test]
async fn shape_validation_precedes_the_gate_and_the_gate_precedes_everything_else() {
    let long_token = "t".repeat(65);
    let long_key = "k".repeat(2049);
    // (case, request body, expected status, expected error code)
    let cases: [(&str, serde_json::Value, u16, &str); 9] = [
        (
            "BucketName outside its pattern",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("123456789012", "Not-A-Bucket", KEY_ARN),
                "ClientToken": "order-1",
            }),
            400,
            "ValidationException",
        ),
        (
            "BucketOwner outside its pattern",
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("12345", "rdp-recordings", KEY_ARN),
                "ClientToken": "order-2",
            }),
            400,
            "ValidationException",
        ),
        (
            "ClientToken over its length bound",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", KEY_ARN),
                "ClientToken": long_token,
            }),
            400,
            "ValidationException",
        ),
        (
            "S3Buckets below its cardinality bound",
            serde_json::json!({
                "ConnectionRecordingPreferences": {
                    "RecordingDestinations": { "S3Buckets": [] },
                    "KMSKeyArn": KEY_ARN,
                },
                "ClientToken": "order-3",
            }),
            400,
            "ValidationException",
        ),
        (
            "S3Buckets above its cardinality bound",
            serde_json::json!({
                "ConnectionRecordingPreferences": {
                    "RecordingDestinations": {
                        "S3Buckets": [
                            { "BucketOwner": "123456789012", "BucketName": "one-bucket" },
                            { "BucketOwner": "123456789012", "BucketName": "two-bucket" },
                        ]
                    },
                    "KMSKeyArn": KEY_ARN,
                },
                "ClientToken": "order-4",
            }),
            400,
            "ValidationException",
        ),
        (
            "KMSKeyArn over its length bound",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", &long_key),
                "ClientToken": "order-5",
            }),
            400,
            "ValidationException",
        ),
        (
            "a required member absent",
            serde_json::json!({ "ClientToken": "order-6" }),
            400,
            "ValidationException",
        ),
        (
            "a well-formed bucket name that resolves to nothing",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "bucket-that-does-not-exist", KEY_ARN),
                "ClientToken": "order-7",
            }),
            403,
            "AccessDeniedException",
        ),
        (
            "a KMSKeyArn that is inside its bound but is not an ARN",
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", "not-a-valid-kms-arn"),
                "ClientToken": "order-8",
            }),
            403,
            "AccessDeniedException",
        ),
    ];

    for (case, body, expected_status, expected_code) in cases {
        let router = router();
        let (status, response) = call(&router, UPDATE, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status:?} with {response:?}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: expected {expected_code}, got {response:?}"
        );
        assert_absent(&router, case).await;
    }
}

/// The gate is read from account STATE on every call, not decided once: the same
/// request that the un-enrolled account denies is accepted once the account is
/// enrolled, and denied again when the enrollment goes away — while the stored record
/// written in between stays readable.
#[tokio::test]
async fn the_gate_follows_the_accounts_enrollment() {
    let (router, engine) = router_with_engine();
    let body = |token: &str| {
        serde_json::json!({
            "ConnectionRecordingPreferences":
                preferences("123456789012", "rdp-recordings", KEY_ARN),
            "ClientToken": token,
        })
    };

    // (case, enrolled, expected status)
    let cases: [(&str, bool, u16); 4] = [
        ("not enrolled", false, 403),
        ("enrolled", true, 200),
        ("still enrolled", true, 200),
        ("enrollment withdrawn", false, 403),
    ];

    for (case, enrolled, expected) in cases {
        enroll(&engine, enrolled);
        let (status, response) = call(&router, UPDATE, body(&format!("token-{expected}"))).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status:?} with {response:?}"
        );
    }

    // The write the enrolled account made is real persisted state, readable through
    // the resource's own reader after the enrollment was withdrawn: the gate refuses
    // CONFIGURING the resource, it does not retract what is configured.
    let (status, got) = call(&router, GET, serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "case \"read after the enrollment was withdrawn\": expected the stored record, got \
         {status:?} with {got:?}"
    );
    assert_eq!(
        got["ConnectionRecordingPreferences"]["RecordingDestinations"]["S3Buckets"][0]
            ["BucketName"]
            .as_str(),
        Some("rdp-recordings"),
        "case \"read after the enrollment was withdrawn\": expected the stored bucket, got {got:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2 — the idempotent removal
// ---------------------------------------------------------------------------

/// Delete is not gated and succeeds with nothing configured, echoing the effective
/// `ClientToken`. Rows vary what the rule generalises over: the token's value, its
/// absence, how many times the call is repeated, and whether the account is enrolled.
#[tokio::test]
async fn deleting_an_unconfigured_singleton_succeeds_idempotently() {
    // (case, enrolled, the token sent or None, repeats)
    let cases: [(&str, bool, Option<&str>, u32); 5] = [
        ("one call with a token", false, Some("del-1"), 1),
        ("the same token twice", false, Some("del-1"), 2),
        ("the same token five times", false, Some("del-repeat"), 5),
        ("no token", false, None, 3),
        ("an enrolled account", true, Some("del-enrolled"), 2),
    ];

    for (case, enrolled, token, repeats) in cases {
        let (router, engine) = router_with_engine();
        enroll(&engine, enrolled);
        for attempt in 1..=repeats {
            let body = match token {
                Some(token) => serde_json::json!({ "ClientToken": token }),
                None => serde_json::json!({}),
            };
            let (status, response) = call(&router, DELETE, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}, attempt {attempt}: expected Delete to succeed, got {status:?} \
                 with {response:?}"
            );
            let reported = response["ClientToken"].as_str().unwrap_or_default();
            match token {
                Some(token) => assert_eq!(
                    reported, token,
                    "case {case:?}, attempt {attempt}: expected the ClientToken echoed back, got \
                     {response:?}"
                ),
                // Service-provided, so what is asserted is the modeled bound, not a
                // value: `ClientToken` is `@length(1, 64)`.
                None => assert!(
                    (1..=64).contains(&reported.chars().count()),
                    "case {case:?}, attempt {attempt}: expected a minted ClientToken inside \
                     @length(1, 64), got {response:?}"
                ),
            }
            assert_absent(&router, case).await;
        }
    }
}

/// A denied Update followed by a Delete, repeated: each cycle answers exactly as the
/// first did, and the singleton is absent throughout. This is the teardown loop a
/// client writes, so it must not drift with the number of rounds.
#[tokio::test]
async fn repeated_denied_update_and_delete_cycles_stay_consistent() {
    let router = router();
    for round in 1..=4 {
        let case = format!("round {round}");
        let (status, denied) = call(
            &router,
            UPDATE,
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", KEY_ARN),
                "ClientToken": format!("cycle-update-{round}"),
            }),
        )
        .await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected the Update to be denied, got {status:?} with {denied:?}"
        );
        assert_eq!(
            error_message(&denied),
            JUST_IN_TIME_NODE_ACCESS_DISABLED,
            "case {case:?}: expected the same denial each round, got {denied:?}"
        );

        let token = format!("cycle-delete-{round}");
        let (status, deleted) = call(
            &router,
            DELETE,
            serde_json::json!({ "ClientToken": token.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the Delete to succeed, got {status:?} with {deleted:?}"
        );
        assert_eq!(
            deleted["ClientToken"].as_str(),
            Some(token.as_str()),
            "case {case:?}: expected the Delete to echo its token, got {deleted:?}"
        );
        assert_absent(&router, &case).await;
    }
}

// ---------------------------------------------------------------------------
// Rule 3 — this service's error text
// ---------------------------------------------------------------------------

/// The absent singleton is reported through one TEMPLATE naming the resource, from
/// every state that can produce it: never configured, and configured then deleted.
#[tokio::test]
async fn the_absent_singleton_is_reported_through_the_not_found_template() {
    let expected = resource_not_found_message(CONNECTION_RECORDING_PREFERENCES);
    // The template, pinned against the text the service emits, so a change to either
    // has to be deliberate.
    assert_eq!(
        expected, "The following  resource cannot be found: 'ConnectionRecordingPreferences'",
        "the not-found template must name the resource exactly as the service does, got \
         {expected:?}"
    );

    let (router, engine) = router_with_engine();
    for case in ["never configured", "configured then deleted"] {
        if case == "configured then deleted" {
            enroll(&engine, true);
            let (status, _) = call(
                &router,
                UPDATE,
                serde_json::json!({
                    "ConnectionRecordingPreferences":
                        preferences("123456789012", "rdp-recordings", KEY_ARN),
                    "ClientToken": "configured",
                }),
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: expected the setup Update to succeed");
            let (status, _) = call(
                &router,
                DELETE,
                serde_json::json!({ "ClientToken": "teardown" }),
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: expected the setup Delete to succeed");
        }

        for read in 1..=2 {
            let (status, got) = call(&router, GET, serde_json::json!({})).await;
            assert_eq!(
                status, 404,
                "case {case:?} (read {read}): expected 404, got {status:?} with {got:?}"
            );
            assert_eq!(
                error_code(&got),
                "ResourceNotFoundException",
                "case {case:?} (read {read}): expected ResourceNotFoundException, got {got:?}"
            );
            assert_eq!(
                error_message(&got),
                expected,
                "case {case:?} (read {read}): expected the not-found template, got {got:?}"
            );
        }
    }
}

/// The template is a TEMPLATE: it states the resource it is given and nothing of the
/// request, so a later reader of another resource gets that resource's name in the
/// same sentence.
#[test]
fn the_not_found_template_names_whichever_resource_it_is_given() {
    for resource in [
        CONNECTION_RECORDING_PREFERENCES,
        "SomeOtherResource",
        "A",
    ] {
        let message = resource_not_found_message(resource);
        assert!(
            message.contains(&format!("'{resource}'")),
            "case {resource:?}: the template must name the resource, got {message:?}"
        );
        assert!(
            message.starts_with("The following") && message.ends_with(&format!("'{resource}'")),
            "case {resource:?}: the template's shape must not vary with the resource, got \
             {message:?}"
        );
    }
}

/// A `@length` bound this service refuses is stated as ONE sentence naming the member
/// path and the bound it broke: no `N validation error(s) detected` prefix, and no
/// echo of the value the caller sent.
///
/// One row per constrained member and per side of its bound. The members and their
/// bounds come from `smithy_query(query="closure",
/// operation="UpdateConnectionRecordingPreferences")` on this service's model:
/// `ClientToken` `@length(1, 64)`, `ConnectionRecordingPreferences$KMSKeyArn`
/// `@length(1, 2048)` and `RecordingDestinations$S3Buckets` `@length(1, 1)`.
#[tokio::test]
async fn a_length_bound_is_refused_with_this_services_own_sentence() {
    let long_token = "t".repeat(65);
    let long_key = "k".repeat(2049);
    let good = preferences("123456789012", "rdp-recordings", KEY_ARN);

    // (case, operation, request body, the sentence the service answers with)
    let cases: [(&str, &str, serde_json::Value, String); 7] = [
        (
            "ClientToken one over its upper bound",
            UPDATE,
            serde_json::json!({
                "ConnectionRecordingPreferences": good.clone(),
                "ClientToken": long_token.clone(),
            }),
            "The length for ClientToken cannot be greater than 64.".to_string(),
        ),
        (
            "ClientToken under its lower bound",
            UPDATE,
            serde_json::json!({
                "ConnectionRecordingPreferences": good.clone(),
                "ClientToken": "",
            }),
            "The length for ClientToken cannot be less than 1.".to_string(),
        ),
        (
            "the same member on another operation",
            DELETE,
            serde_json::json!({ "ClientToken": long_token.clone() }),
            "The length for ClientToken cannot be greater than 64.".to_string(),
        ),
        (
            "S3Buckets below its cardinality bound",
            UPDATE,
            serde_json::json!({
                "ConnectionRecordingPreferences": {
                    "RecordingDestinations": { "S3Buckets": [] },
                    "KMSKeyArn": KEY_ARN,
                },
                "ClientToken": "tok",
            }),
            "ConnectionRecordingPreferences.RecordingDestinations.S3Buckets must contain at \
             least 1 value."
                .to_string(),
        ),
        (
            "S3Buckets above its cardinality bound",
            UPDATE,
            serde_json::json!({
                "ConnectionRecordingPreferences": {
                    "RecordingDestinations": {
                        "S3Buckets": [
                            { "BucketOwner": "123456789012", "BucketName": "one-bucket" },
                            { "BucketOwner": "123456789012", "BucketName": "two-bucket" },
                        ]
                    },
                    "KMSKeyArn": KEY_ARN,
                },
                "ClientToken": "tok",
            }),
            "ConnectionRecordingPreferences.RecordingDestinations.S3Buckets cannot contain more \
             than 1 value."
                .to_string(),
        ),
        // The third @length member of the model, which the issue did not cite: a
        // NESTED member, so its sentence names the whole path down to it.
        (
            "KMSKeyArn one over its upper bound",
            UPDATE,
            serde_json::json!({
                "ConnectionRecordingPreferences":
                    preferences("123456789012", "rdp-recordings", &long_key),
                "ClientToken": "tok",
            }),
            "The length for ConnectionRecordingPreferences.KMSKeyArn cannot be greater than 2048."
                .to_string(),
        ),
        (
            "KMSKeyArn under its lower bound",
            UPDATE,
            serde_json::json!({
                "ConnectionRecordingPreferences": preferences("123456789012", "rdp-recordings", ""),
                "ClientToken": "tok",
            }),
            "The length for ConnectionRecordingPreferences.KMSKeyArn cannot be less than 1."
                .to_string(),
        ),
    ];

    for (case, operation, body, sentence) in cases {
        let router = router();
        let (status, response) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status:?} with {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
        let message = error_message(&response);
        assert_eq!(
            message, sentence,
            "case {case:?}: expected {sentence:?}, got {message:?}"
        );
        assert!(
            !message.contains("validation error"),
            "case {case:?}: the aggregated envelope prefix must be gone, got {message:?}"
        );
        for withheld in [long_token.as_str(), long_key.as_str()] {
            assert!(
                !message.contains(withheld),
                "case {case:?}: the offending value must not be echoed, got {message:?}"
            );
        }
    }
}
