// SPDX-License-Identifier: CC0-1.0

//! A READ of a secret value that resolves nothing says WHICH value it could not find.
//!
//! The rule under test: `GetSecretValue` resolves the value either by the `VersionId` the
//! request names or by a staging label, so a read that resolves no value answers 400
//! `ResourceNotFoundException` naming the member it resolved by and that member's value —
//! "Secrets Manager can't find the specified secret value for VersionId: <VersionId>" for a
//! read that named a `VersionId`, and "… for staging label: <label>" for one that resolved by
//! label. The only variable part is the member value echoed from the request.
//!
//! The second table drives the same rule through a rotation in flight, which is the state
//! that makes a version id name no value: `RotateSecret` stakes `AWSPENDING` on the id of a
//! version the rotation function has not created yet, a further `RotateSecret` is refused
//! while that candidate is outstanding, and the refused call mints no version — so the token
//! it carried resolves to no value either, and the staging labels are where they were.
//!
//! Every test is a TABLE over what the rule generalises over: which member the read resolved
//! by, the form and length of the `VersionId` (`SecretVersionIdType` is modelled with
//! `@length` 32..64, so the rows run over both published bounds and a value between them),
//! the staging label asked for, how many versions the secret holds, and the form of the
//! `SecretId`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::ROTATION_IN_PROGRESS_MESSAGE;
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on the credential
/// scope and the emulator verifies signatures only when an authenticator is registered, which
/// this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The ARN of a Lambda rotation function, as `RotationLambdaARN` carries it.
const LAMBDA_ARN: &str = "arn:aws:lambda:us-west-2:123456789012:function:RotateMySecret";

/// The published bounds of `SecretVersionIdType` and `ClientRequestTokenType` (`@length` 32
/// and 64, this service's Smithy model).
const VERSION_ID_MIN_LEN: usize = 32;
const VERSION_ID_MAX_LEN: usize = 64;

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

/// Send one request that must be refused, and return its status, wire error code and message.
async fn err(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> (u16, String, String) {
    let (status, parsed) = call(router, operation, &body).await;
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    let code = raw.split('#').next_back().unwrap_or(raw).to_string();
    let message = parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in error body {parsed}"))
        .to_string();
    (status, code, message)
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// A version id (and so a `ClientRequestToken`) of `length` characters, unique per `seed`.
fn version_id(seed: &str, length: usize) -> String {
    let mut id = format!("v-{seed}-");
    while id.len() < length {
        id.push('z');
    }
    id.chars().take(length).collect()
}

/// Create a secret holding an initial value under a version id the caller names, and answer
/// `(ARN, that version id)`.
async fn create_with_value(
    router: &MultiServiceRouter,
    name: &str,
    value: &str,
    case: &str,
) -> (String, String) {
    let token = version_id(&format!("init-{name}"), VERSION_ID_MIN_LEN + 4);
    let created = ok(
        router,
        "CreateSecret",
        json!({"Name": name, "SecretString": value, "ClientRequestToken": token.clone()}),
        case,
    )
    .await;
    (text(&created, "ARN", case), token)
}

/// The expected sentence for a read that resolved by `VersionId`.
fn expected_for_version_id(version_id: &str) -> String {
    format!("Secrets Manager can't find the specified secret value for VersionId: {version_id}")
}

/// The expected sentence for a read that resolved by staging label.
fn expected_for_stage(stage: &str) -> String {
    format!("Secrets Manager can't find the specified secret value for staging label: {stage}")
}

/// What `DescribeSecret` reports for `secret_id`.
async fn described(router: &MultiServiceRouter, secret_id: &str, case: &str) -> Value {
    ok(router, "DescribeSecret", json!({"SecretId": secret_id}), case).await
}

#[tokio::test]
async fn a_value_read_that_resolves_nothing_names_the_member_it_resolved_by() {
    let router = emulator();
    let name = "lookup/messages";
    let (_, first) = create_with_value(&router, name, "value-0", "setup").await;
    // A second secret, so one row can ask THIS secret for a version id that really exists —
    // on another secret, where it names no value of the secret being read.
    let other = "lookup/messages-other";
    let (_, other_version) = create_with_value(&router, other, "value-other", "setup").await;
    // A secret holding no version at all: "If you include SecretString or SecretBinary then
    // Secrets Manager creates an initial secret version" (`CreateSecret`, this service's
    // Smithy model), so one created with neither has no AWSCURRENT to resolve.
    let empty = "lookup/messages-empty";
    ok(
        &router,
        "CreateSecret",
        json!({
            "Name": empty,
            "ClientRequestToken": version_id("empty", VERSION_ID_MIN_LEN),
        }),
        "setup",
    )
    .await;

    // (case, the secret read, the request members beside SecretId, the expected sentence)
    let rows: Vec<(String, &str, Value, String)> = vec![
        (
            format!("a VersionId of the minimum modelled length ({VERSION_ID_MIN_LEN})"),
            name,
            json!({"VersionId": version_id("absent-min", VERSION_ID_MIN_LEN)}),
            expected_for_version_id(&version_id("absent-min", VERSION_ID_MIN_LEN)),
        ),
        (
            "a VersionId in UUID form".to_string(),
            name,
            json!({"VersionId": "11111111-2222-4333-8444-555555555555"}),
            expected_for_version_id("11111111-2222-4333-8444-555555555555"),
        ),
        (
            format!("a VersionId of the maximum modelled length ({VERSION_ID_MAX_LEN})"),
            name,
            json!({"VersionId": version_id("absent-max", VERSION_ID_MAX_LEN)}),
            expected_for_version_id(&version_id("absent-max", VERSION_ID_MAX_LEN)),
        ),
        (
            "a VersionId that names a version of ANOTHER secret".to_string(),
            name,
            json!({"VersionId": other_version.clone()}),
            expected_for_version_id(&other_version),
        ),
        (
            "a VersionId the secret holds, under a staging label it does not carry".to_string(),
            name,
            json!({"VersionId": first.clone(), "VersionStage": "AWSPREVIOUS"}),
            expected_for_version_id(&first),
        ),
        (
            "an absent VersionId alongside a staging label the secret does carry".to_string(),
            name,
            json!({
                "VersionId": version_id("absent-with-stage", VERSION_ID_MIN_LEN),
                "VersionStage": "AWSCURRENT",
            }),
            expected_for_version_id(&version_id("absent-with-stage", VERSION_ID_MIN_LEN)),
        ),
        (
            "the staging label AWSPREVIOUS, which no version carries yet".to_string(),
            name,
            json!({"VersionStage": "AWSPREVIOUS"}),
            expected_for_stage("AWSPREVIOUS"),
        ),
        (
            "the staging label AWSPENDING, which no version carries".to_string(),
            name,
            json!({"VersionStage": "AWSPENDING"}),
            expected_for_stage("AWSPENDING"),
        ),
        (
            "a staging label of the caller's own".to_string(),
            name,
            json!({"VersionStage": "blue"}),
            expected_for_stage("blue"),
        ),
        (
            "no member at all, on a secret holding no version".to_string(),
            empty,
            json!({}),
            expected_for_stage("AWSCURRENT"),
        ),
    ];

    for (case, secret, members, expected) in rows {
        let mut body = json!({"SecretId": secret});
        for (member, value) in members.as_object().expect("the row states an object") {
            body[member] = value.clone();
        }
        let (status, code, message) = err(&router, "GetSecretValue", body.clone(), &case).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {body}, got {status} with message {message:?}"
        );
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for {body}, got {code:?} with \
             message {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The read resolved nothing, so it created nothing: the secret still reports exactly the
    // version it was created with.
    let after = described(&router, name, "after the refused reads").await;
    let stages = after["VersionIdsToStages"]
        .as_object()
        .unwrap_or_else(|| panic!("case \"after the refused reads\": no VersionIdsToStages in {after}"));
    assert_eq!(
        stages.keys().cloned().collect::<Vec<String>>(),
        vec![first.clone()],
        "case \"after the refused reads\": expected only the created version, got {after}"
    );
}

#[tokio::test]
async fn a_rotation_in_flight_makes_its_staked_version_name_no_value() {
    // RULE: `RotateSecret` stakes `AWSPENDING` on the id of a version the rotation function
    // has not created yet, so that id names NO secret value until the function writes one;
    // while that candidate sits on a version other than the `AWSCURRENT` one, a further
    // `RotateSecret` is refused and mints nothing. The rows run over the form of the
    // `SecretId` the second call names the secret by, and over whether it names its own
    // `ClientRequestToken`.
    for (index, by_arn, second_token) in [
        (0usize, false, Some(version_id("second-a", VERSION_ID_MIN_LEN))),
        (1, true, Some(version_id("second-b", VERSION_ID_MAX_LEN))),
        (2, false, None),
    ] {
        let case = format!(
            "a second rotation naming the secret {} with {} token",
            if by_arn { "by ARN" } else { "by name" },
            if second_token.is_some() { "its own" } else { "no" }
        );
        let router = emulator();
        let name = format!("lookup/rotating-{index}");
        let (arn, current) = create_with_value(&router, &name, "credential-0", &case).await;
        let staked = version_id(&format!("staked-{index}"), VERSION_ID_MIN_LEN + 8);

        let started = ok(
            &router,
            "RotateSecret",
            json!({
                "SecretId": name.clone(),
                "ClientRequestToken": staked.clone(),
                "RotationLambdaARN": LAMBDA_ARN,
                "RotationRules": {"AutomaticallyAfterDays": 30},
                "RotateImmediately": true,
            }),
            &case,
        )
        .await;
        assert_eq!(
            text(&started, "VersionId", &case),
            staked,
            "case {case:?}: expected the staked version id back, got {started}"
        );

        // The call answered while the rotation is in flight, so `AWSPENDING` names a version
        // the function has not created — and reading a value under that id finds none.
        let in_flight = described(&router, &name, &case).await;
        assert_eq!(
            in_flight["VersionIdsToStages"][&staked]
                .as_array()
                .map(|labels| labels.iter().any(|label| label == "AWSPENDING")),
            Some(true),
            "case {case:?}: expected AWSPENDING staked on {staked}, got {in_flight}"
        );
        let expected = expected_for_version_id(&staked);
        let (status, code, message) = err(
            &router,
            "GetSecretValue",
            json!({"SecretId": name.clone(), "VersionId": staked.clone()}),
            &case,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 reading the staked version, got {status}"
        );
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException reading the staked version, got \
             {code:?} with message {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // "If the AWSPENDING staging label is present but not attached to the same version as
        // AWSCURRENT, then any later invocation of RotateSecret assumes that a previous
        // rotation request is still in progress and returns an error" (`RotateSecret`, this
        // service's Smithy model).
        let mut second = json!({
            "SecretId": if by_arn { arn.clone() } else { name.clone() },
            "RotationLambdaARN": LAMBDA_ARN,
        });
        if let Some(token) = &second_token {
            second["ClientRequestToken"] = json!(token);
        }
        let (status, code, message) = err(&router, "RotateSecret", second.clone(), &case).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for the second rotation {second}, got {status} \
             with message {message:?}"
        );
        assert_eq!(
            code, "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException for the second rotation, got \
             {code:?} with message {message:?}"
        );
        assert_eq!(
            message, ROTATION_IN_PROGRESS_MESSAGE,
            "case {case:?}: expected {ROTATION_IN_PROGRESS_MESSAGE:?}, got {message:?}"
        );

        // The refused call minted nothing and moved no staging label.
        if let Some(token) = &second_token {
            let expected = expected_for_version_id(token);
            let (status, code, message) = err(
                &router,
                "GetSecretValue",
                json!({"SecretId": name.clone(), "VersionId": token}),
                &case,
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 reading the refused call's token, got {status}"
            );
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected the refused token to name no value, got {code:?} with \
                 message {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
        let after = described(&router, &name, &case).await;
        assert_eq!(
            after["VersionIdsToStages"], in_flight["VersionIdsToStages"],
            "case {case:?}: expected the staging labels unchanged by the refused rotation, got \
             {after}"
        );
        // The value the rotation has not replaced is still the readable one.
        let value = ok(
            &router,
            "GetSecretValue",
            json!({"SecretId": name.clone(), "VersionId": current.clone()}),
            &case,
        )
        .await;
        assert_eq!(
            text(&value, "SecretString", &case),
            "credential-0",
            "case {case:?}: expected the current value still readable, got {value}"
        );
    }
}
