// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the wire BODIES of this service's client errors, driven
//! over the multi-service router.
//!
//! The rules under test:
//!
//! * a `SecretId` that names no secret this service holds is
//!   `ResourceNotFoundException` carrying ONE constant sentence, with no part of
//!   the request in it — whichever operation resolved the id;
//! * a secret inside its recovery window still EXISTS, so an operation it
//!   forbids answers `InvalidRequestException` ("The secret is scheduled for
//!   deletion.", `InvalidRequestException`, this service's Smithy model), never
//!   `ResourceNotFoundException`;
//! * a pagination token this service cannot resume from is
//!   `InvalidNextTokenException` carrying one constant sentence, in every
//!   paginated operation;
//! * `DeleteSecret` refuses a `RecoveryWindowInDays` outside the documented
//!   7..30 range with a message stating that constraint and not echoing the
//!   rejected value;
//! * `PutSecretValue` reports a `ClientRequestToken` collision with a message
//!   BUILT from the request — the offending token and the secret's ARN — so the
//!   caller can attribute which write collided.
//!
//! Every test is a TABLE over the class its rule generalises: the operation that
//! resolved the id, the form of the `SecretId`, the form of the token, the
//! recovery window, the type and size of the colliding value.

use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::FORCE_DELETE_PURGE_DELAY;
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on
/// the credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The one sentence `ResourceNotFoundException` carries for a secret this
/// service does not hold ("Secrets Manager can't find the resource that you
/// asked for.", `ResourceNotFoundException`, this service's Smithy model).
const SECRET_NOT_FOUND: &str = "Secrets Manager can't find the specified secret.";

/// The one sentence `InvalidNextTokenException` carries ("The `NextToken` value
/// is invalid.", `InvalidNextTokenException`, this service's Smithy model).
const INVALID_NEXT_TOKEN: &str = "The pagination token is no longer valid.";

/// The sentence an operation a secret's state forbids carries.
const MARKED_FOR_DELETION: &str =
    "You can't perform this operation on the secret because it was marked for deletion.";

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request and return its status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Send one request that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Send one request that must fail with a 400, and return its wire error code
/// (the `__type` discriminator without namespace) and its `Message`.
async fn err(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> (String, String) {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 client error from {operation}, got {status} with body \
         {parsed}"
    );
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    let message = parsed
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no Message in error body {parsed}"));
    (
        raw.split('#').next_back().unwrap_or(raw).to_string(),
        message.to_string(),
    )
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// Every operation that resolves a `SecretId`, with the other members its
/// request needs so the call reaches the resolution rather than being refused
/// earlier. The list is this crate's implemented surface as `OPERATIONS.md`
/// records it; the stubs (`UpdateSecret`, `RestoreSecret`,
/// `UpdateSecretVersionStage`, `CancelRotateSecret`, `BatchGetSecretValue`, the
/// resource-policy operations) answer 501 and carry no modelled message yet.
fn operations_resolving_a_secret_id() -> Vec<(&'static str, Value)> {
    vec![
        ("DescribeSecret", json!({})),
        ("GetSecretValue", json!({})),
        ("ListSecretVersionIds", json!({})),
        ("DeleteSecret", json!({})),
        ("PutSecretValue", json!({"SecretString": "a-value"})),
        ("RotateSecret", json!({})),
        ("TagResource", json!({"Tags": [{"Key": "env", "Value": "test"}]})),
        ("UntagResource", json!({"TagKeys": ["env"]})),
        (
            "ReplicateSecretToRegions",
            json!({"AddReplicaRegions": [{"Region": "eu-west-1"}]}),
        ),
        (
            "RemoveRegionsFromReplication",
            json!({"RemoveReplicaRegions": ["eu-west-1"]}),
        ),
        ("StopReplicationToReplica", json!({})),
    ]
}

/// A request body: the operation's own members plus the `SecretId` under test.
fn with_secret_id(extra: &Value, secret_id: &str) -> Value {
    let mut body = extra.clone();
    body.as_object_mut()
        .expect("every row's extra members are an object")
        .insert("SecretId".to_string(), json!(secret_id));
    body
}

#[tokio::test]
async fn a_secret_id_naming_nothing_is_one_constant_not_found_sentence() {
    // The rows run over the operation that resolved the id and over the
    // documented FORMS of a `SecretId` ("The ARN or name of the secret", and a
    // partial ARN — `GetSecretValueRequest$SecretId`, this service's Smithy
    // model), none of which names a stored secret here.
    let account = engine::EMULATOR_ACCOUNT_ID;
    let secret_ids = [
        "no-such-secret",
        "folder/path/no-such-secret",
        &format!("arn:aws:secretsmanager:us-west-2:{account}:secret:no-such-secret-AbCdEf"),
        &format!("arn:aws:secretsmanager:us-west-2:{account}:secret:no-such-secret"),
    ]
    .map(str::to_string);

    for (operation, extra) in operations_resolving_a_secret_id() {
        for secret_id in &secret_ids {
            let case = format!("{operation} on SecretId {secret_id:?}");
            let router = emulator();
            // A secret that DOES exist, so the store is not merely empty and the
            // answer is about the id the request named.
            ok(
                &router,
                "CreateSecret",
                json!({"Name": "an-existing-secret", "SecretString": "stored"}),
                &case,
            )
            .await;

            let (code, message) =
                err(&router, operation, with_secret_id(&extra, secret_id), &case).await;
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {code:?} with message \
                 {message:?}"
            );
            assert_eq!(
                message, SECRET_NOT_FOUND,
                "case {case:?}: expected the constant sentence {SECRET_NOT_FOUND:?}, got \
                 {message:?}"
            );
            assert!(
                !message.contains(secret_id.as_str()),
                "case {case:?}: expected no part of the request in the message, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_secret_marked_for_deletion_is_refused_by_state_not_reported_missing() {
    // The rows run over HOW the deletion was asked for — each documented bound
    // of the recovery window, a value between them, the default a caller names
    // no window for, and `ForceDeleteWithoutRecovery`, which skips the window
    // but is still performed "with an asynchronous background process, so there
    // might be a short delay before the secret is permanently deleted"
    // (`DeleteSecretRequest$ForceDeleteWithoutRecovery`, this service's Smithy
    // model) — and over the form of the `SecretId` the read uses. A secret that
    // is marked for deletion still EXISTS, so it is still found and the refusal
    // is about its state.
    let deletions: [(&str, Value); 5] = [
        ("the default window", json!({})),
        ("the minimum window", json!({"RecoveryWindowInDays": 7})),
        ("a window inside the range", json!({"RecoveryWindowInDays": 19})),
        ("the maximum window", json!({"RecoveryWindowInDays": 30})),
        ("no window at all", json!({"ForceDeleteWithoutRecovery": true})),
    ];
    for (label, request) in &deletions {
        for by_arn in [false, true] {
            let case = format!("{label}, read by ARN: {by_arn}");
            let router = emulator();
            let name = "marked-for-deletion";
            let created = ok(
                &router,
                "CreateSecret",
                json!({"Name": name, "SecretString": "stored"}),
                &case,
            )
            .await;
            let arn = text(&created, "ARN", &case);

            let mut delete = request.clone();
            delete
                .as_object_mut()
                .expect("every row's request is an object")
                .insert("SecretId".to_string(), json!(name));
            let started = Instant::now();
            let deleted = ok(&router, "DeleteSecret", delete, &case).await;
            assert!(
                deleted.get("DeletionDate").is_some(),
                "case {case:?}: expected a DeletionDate from DeleteSecret, got {deleted}"
            );
            // A forced deletion is removed once its short span has elapsed, so
            // the transient state is only asserted while the span cannot yet
            // have passed; a windowed one is days away and always asserted.
            if started.elapsed() >= FORCE_DELETE_PURGE_DELAY {
                continue;
            }

            let secret_id = if by_arn { arn.clone() } else { name.to_string() };

            // The secret is still there: a report of it is served, and it
            // carries the date its deletion falls due.
            let described = ok(
                &router,
                "DescribeSecret",
                json!({"SecretId": secret_id.clone()}),
                &case,
            )
            .await;
            assert!(
                described.get("DeletedDate").is_some(),
                "case {case:?}: expected DescribeSecret to still find the secret with a \
                 DeletedDate, got {described}"
            );

            let (code, message) = err(
                &router,
                "GetSecretValue",
                json!({"SecretId": secret_id.clone()}),
                &case,
            )
            .await;
            assert_eq!(
                code, "InvalidRequestException",
                "case {case:?}: expected InvalidRequestException for a secret marked for \
                 deletion, got {code:?} with message {message:?}"
            );
            assert_eq!(
                message, MARKED_FOR_DELETION,
                "case {case:?}: expected {MARKED_FOR_DELETION:?}, got {message:?}"
            );

            // And the OTHER error still means what it means: a secret that was
            // never created is not found.
            let (code, message) = err(
                &router,
                "GetSecretValue",
                json!({"SecretId": "never-created"}),
                &case,
            )
            .await;
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException for a secret that never \
                 existed, got {code:?} with message {message:?}"
            );
            assert_eq!(
                message, SECRET_NOT_FOUND,
                "case {case:?}: expected the constant sentence {SECRET_NOT_FOUND:?}, got \
                 {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_pagination_token_this_service_cannot_resume_from_is_one_constant_sentence() {
    // The rows run over every paginated operation this crate implements and over
    // the forms a token can take that no page of this listing can resume from:
    // not the encoding this service mints, the encoding of a cursor that names
    // nothing, and a long opaque blob. `NextTokenType` is modelled with a length
    // bound of 1..4096, so every row stays inside it.
    let tokens = [
        "not-a-token".to_string(),
        "zzzz".to_string(),
        // The service's own encoding (hex of the cursor) over a name/version id
        // that this listing does not hold.
        "6e6f742d696e2d746869732d6c697374696e67".to_string(),
        "f".repeat(4000),
    ];
    for operation in ["ListSecrets", "ListSecretVersionIds"] {
        for token in &tokens {
            let case = format!("{operation} with NextToken {:.32?}", token);
            let router = emulator();
            let name = "a-paged-secret";
            ok(
                &router,
                "CreateSecret",
                json!({"Name": name, "SecretString": "stored"}),
                &case,
            )
            .await;

            let mut body = json!({"NextToken": token});
            if operation == "ListSecretVersionIds" {
                body.as_object_mut()
                    .expect("a request body is an object")
                    .insert("SecretId".to_string(), json!(name));
            }
            let (code, message) = err(&router, operation, body, &case).await;
            assert_eq!(
                code, "InvalidNextTokenException",
                "case {case:?}: expected InvalidNextTokenException, got {code:?} with message \
                 {message:?}"
            );
            assert_eq!(
                message, INVALID_NEXT_TOKEN,
                "case {case:?}: expected the constant sentence {INVALID_NEXT_TOKEN:?}, got \
                 {message:?}"
            );
            assert!(
                !message.contains(token.as_str()),
                "case {case:?}: expected no part of the request in the message, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn delete_secret_states_the_documented_recovery_window_range() {
    // "The number of days from 7 to 30 that Secrets Manager waits before
    // permanently deleting the secret."
    // (`DeleteSecretRequest$RecoveryWindowInDays`, this service's Smithy model.)
    // The rows run over each published bound, one day past each of them, and
    // further values on either side.
    const EXPECTED: &str = "The RecoveryWindowInDays value must be between 7 and 30 days \
                            (inclusive).";
    let refused = [-1i64, 0, 1, 2, 6, 31, 45, 3650];
    for window in refused {
        let case = format!("RecoveryWindowInDays {window}");
        let router = emulator();
        ok(
            &router,
            "CreateSecret",
            json!({"Name": "window-validation", "SecretString": "stored"}),
            &case,
        )
        .await;
        let (code, message) = err(
            &router,
            "DeleteSecret",
            json!({"SecretId": "window-validation", "RecoveryWindowInDays": window}),
            &case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {code:?} with message \
             {message:?}"
        );
        assert_eq!(
            message, EXPECTED,
            "case {case:?}: expected {EXPECTED:?}, got {message:?} (the message states the \
             documented range and never echoes the rejected value)"
        );
    }

    // Each published bound, and a value between them, is ACCEPTED — the refusal
    // is the range's, not a blanket one.
    for window in [7i64, 8, 21, 29, 30] {
        let case = format!("RecoveryWindowInDays {window} (inside the range)");
        let router = emulator();
        ok(
            &router,
            "CreateSecret",
            json!({"Name": "window-accepted", "SecretString": "stored"}),
            &case,
        )
        .await;
        let deleted = ok(
            &router,
            "DeleteSecret",
            json!({"SecretId": "window-accepted", "RecoveryWindowInDays": window}),
            &case,
        )
        .await;
        assert!(
            deleted.get("DeletionDate").is_some(),
            "case {case:?}: expected a DeletionDate from an in-range DeleteSecret, got {deleted}"
        );
    }
}

#[tokio::test]
async fn delete_secret_refuses_a_window_and_a_forced_delete_in_one_call() {
    // "You can't use both this parameter and RecoveryWindowInDays in the same
    // call." (`DeleteSecretRequest$ForceDeleteWithoutRecovery`, this service's
    // Smithy model.) The refusal is of the two members APPEARING together, so
    // the rows run over the value each one carries, including a window that is
    // out of range on its own: the combination is refused first.
    const EXPECTED: &str =
        "You can't use ForceDeleteWithoutRecovery in conjunction with RecoveryWindowInDays.";
    for window in [2i64, 7, 30, 45] {
        for force in [true, false] {
            let case = format!("RecoveryWindowInDays {window} with ForceDeleteWithoutRecovery \
                                {force}");
            let router = emulator();
            ok(
                &router,
                "CreateSecret",
                json!({"Name": "both-parameters", "SecretString": "stored"}),
                &case,
            )
            .await;
            let (code, message) = err(
                &router,
                "DeleteSecret",
                json!({
                    "SecretId": "both-parameters",
                    "RecoveryWindowInDays": window,
                    "ForceDeleteWithoutRecovery": force,
                }),
                &case,
            )
            .await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {code:?} with message \
                 {message:?}"
            );
            assert_eq!(
                message, EXPECTED,
                "case {case:?}: expected {EXPECTED:?}, got {message:?}"
            );
            // The refused call stored nothing: the secret is untouched.
            let described = ok(
                &router,
                "DescribeSecret",
                json!({"SecretId": "both-parameters"}),
                &case,
            )
            .await;
            assert!(
                described.get("DeletedDate").is_none(),
                "case {case:?}: expected the refused call to leave the secret undeleted, got \
                 {described}"
            );
        }
    }
}

#[tokio::test]
async fn a_client_request_token_collision_names_the_token_and_the_secret() {
    // `PutSecretValue$ClientRequestToken` is "a unique identifier for the new
    // version of the secret" and an @idempotencyToken: the same token with the
    // same value is the same request, while the same token with a DIFFERENT
    // value collides. The rows run over the form and length of the token
    // (`SecretVersionIdType` is modelled with a length bound of 32..64) and over
    // the type and size of the colliding value.
    let tokens = [
        "aaaaaaaa-1111-4bbb-8ccc-dddddddddddd".to_string(), // a UUID, 36 characters
        "0".repeat(32),                                     // the modelled minimum length
        "e".repeat(64),                                     // the modelled maximum length
        "Token_With.Mixed-Case/And+Digits0123456789".to_string(),
    ];
    // (label, the first value stored, the colliding second value)
    let values: [(&str, Value, Value); 4] = [
        ("short strings", json!({"SecretString": "first"}), json!({"SecretString": "second"})),
        (
            "a long string",
            json!({"SecretString": "a"}),
            json!({"SecretString": "b".repeat(4096)}),
        ),
        (
            "binary over string",
            json!({"SecretString": "first"}),
            json!({"SecretBinary": "AAECAwQ="}),
        ),
        (
            "binary over binary",
            json!({"SecretBinary": "AAECAwQ="}),
            json!({"SecretBinary": "BQYHCAk="}),
        ),
    ];

    for token in &tokens {
        for (label, first, second) in &values {
            let case = format!("token {token:?} with {label}");
            let router = emulator();
            let name = "token-conflict";
            let created = ok(
                &router,
                "CreateSecret",
                json!({"Name": name, "SecretString": "initial"}),
                &case,
            )
            .await;
            let arn = text(&created, "ARN", &case);

            let mut put = first.clone();
            let members = put.as_object_mut().expect("a request body is an object");
            members.insert("SecretId".to_string(), json!(name));
            members.insert("ClientRequestToken".to_string(), json!(token));
            let stored = ok(&router, "PutSecretValue", put.clone(), &case).await;
            assert_eq!(
                text(&stored, "VersionId", &case),
                *token,
                "case {case:?}: expected the token to be the new version's id, got {stored}"
            );

            // The same token with the same value is the same request: it changes
            // nothing and is not a collision.
            let repeated = ok(&router, "PutSecretValue", put, &case).await;
            assert_eq!(
                text(&repeated, "VersionId", &case),
                *token,
                "case {case:?}: expected an idempotent repeat to report the same version, got \
                 {repeated}"
            );

            let mut conflicting = second.clone();
            let members = conflicting
                .as_object_mut()
                .expect("a request body is an object");
            members.insert("SecretId".to_string(), json!(name));
            members.insert("ClientRequestToken".to_string(), json!(token));
            let (code, message) = err(&router, "PutSecretValue", conflicting, &case).await;
            assert_eq!(
                code, "ResourceExistsException",
                "case {case:?}: expected ResourceExistsException, got {code:?} with message \
                 {message:?}"
            );
            let expected = format!(
                "You can't use ClientRequestToken {token} because that value is already in use \
                 for a version of secret {arn}."
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}
