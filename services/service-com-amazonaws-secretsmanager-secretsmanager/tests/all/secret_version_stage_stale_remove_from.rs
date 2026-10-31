// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal `UpdateSecretVersionStage` answers when
//! `RemoveFromVersionId` names a version the staging label is not on, driven over
//! the wire through the multi-service router and read back through
//! `DescribeSecret` and `GetSecretValue`.
//!
//! The rule under test: `RemoveFromVersionId` carries a value the caller has to get
//! right — "If the staging label you are trying to attach to one version is already
//! attached to a different version, then you must include this parameter and specify
//! the version that the label is to be removed from. If the label is attached and
//! you either do not specify this parameter, or the version ID does not match, then
//! the operation fails."
//! (`UpdateSecretVersionStageRequest$RemoveFromVersionId`, this service's Smithy
//! model.) A version id that does not match is therefore a bad PARAMETER VALUE, not
//! a conflict with the secret's state: the refusal is HTTP 400
//! `InvalidParameterException` ("the parameter name or value is invalid"), never the
//! state refusal `InvalidRequestException` ("a parameter value is not valid for the
//! current state of the resource"), and its sentence states the rule and names the
//! version that currently holds the label — the label from the request, the version
//! id from this store — so a caller can re-read and retry. Nothing moves.
//!
//! This is the shape a lost race leaves behind: two moves of one label resolve with
//! a single winner, and the call that lands second names a version the label has
//! already left. Clients branch on the error CODE, so the second call has to come
//! back as the parameter refusal the first-choice catch names.
//!
//! Every test is a TABLE over the class the rule generalises: the staging label
//! (both reserved ones and a caller's own), how many calls contend for it, how the
//! current holder came to hold it (a move the caller asked for, or the implicit move
//! a value-storing write makes), the form of the `SecretId` (name and ARN), and
//! which version the stale `RemoveFromVersionId` names (the version the call is
//! moving the label to, and the version the label used to be on).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on the
/// credential scope and the emulator verifies signatures only when an authenticator
/// is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The code every refusal in this suite has to carry: the parameter refusal, not
/// the state refusal.
const PARAMETER_REFUSAL: &str = "InvalidParameterException";

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
            // A write that stores a value creates a version the caller has to
            // name; see `secret_client_request_token::with_staked_version`.
            serde_json::to_vec(&crate::secret_client_request_token::with_staked_version(
                operation, body,
            ))
            .expect("the request body is serializable"),
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

/// Send one request that must fail with a 400, and return its wire error code and
/// message.
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
    (code_of(&parsed, case), message_of(&parsed, case))
}

/// The wire error code of an error body, without its shape namespace.
fn code_of(parsed: &Value, case: &str) -> String {
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    raw.split('#').next_back().unwrap_or(raw).to_string()
}

/// The message of an error body.
fn message_of(parsed: &Value, case: &str) -> String {
    parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in error body {parsed}"))
        .to_string()
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// A version id of the modelled length (`SecretVersionIdType` is 32..64), built
/// from a readable seed so a failing case names which version it meant.
fn version_id(seed: &str) -> String {
    format!("{seed:0<32}")
}

/// The staging map `DescribeSecret` reports: version id to its labels, sorted.
async fn stages(
    router: &MultiServiceRouter,
    secret_id: &str,
    case: &str,
) -> Vec<(String, Vec<String>)> {
    let described = ok(router, "DescribeSecret", json!({"SecretId": secret_id}), case).await;
    let map = described
        .get("VersionIdsToStages")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut rows: Vec<(String, Vec<String>)> = map
        .into_iter()
        .map(|(version, labels)| {
            let mut labels: Vec<String> = labels
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: VersionIdsToStages holds a list"))
                .iter()
                .map(|label| label.as_str().unwrap_or_default().to_string())
                .collect();
            labels.sort();
            (version, labels)
        })
        .collect();
    rows.sort();
    rows
}

/// The version id carrying `label` right now, as this store reports it.
async fn holder(
    router: &MultiServiceRouter,
    secret_id: &str,
    label: &str,
    case: &str,
) -> Option<String> {
    stages(router, secret_id, case)
        .await
        .into_iter()
        .find(|(_, labels)| labels.iter().any(|held| held == label))
        .map(|(version, _)| version)
}

/// The sentence AWS refuses a `RemoveFromVersionId` that does not name the label's
/// current holder with: fixed text, the label from the request, and the holding
/// version id from the store.
fn expected_message(label: &str, held_by: &str) -> String {
    format!(
        "When you move staging label {label}, if you specify RemoveFromVersionId, it must be \
         set to the version that currently has the staging label {held_by}."
    )
}

/// Create a secret with one version, and report its ARN and that version's id.
async fn create(router: &MultiServiceRouter, name: &str, case: &str) -> (String, String) {
    let created = ok(
        router,
        "CreateSecret",
        json!({
            "Name": name,
            "SecretString": "version-1",
            "ClientRequestToken": version_id("v0"),
        }),
        case,
    )
    .await;
    (text(&created, "ARN", case), version_id("v0"))
}

/// Store another version under `token`. `labels` is the `VersionStages` the write
/// names, or `Value::Null` to let the service move `AWSCURRENT` itself.
async fn put(
    router: &MultiServiceRouter,
    secret_id: &str,
    token: &str,
    labels: Value,
    case: &str,
) -> String {
    let mut body = json!({
        "SecretId": secret_id,
        "ClientRequestToken": token,
        "SecretString": format!("value-for-{token}"),
    });
    if !labels.is_null() {
        body.as_object_mut()
            .expect("a request body is an object")
            .insert("VersionStages".to_string(), labels);
    }
    let stored = ok(router, "PutSecretValue", body, case).await;
    text(&stored, "VersionId", case)
}

#[tokio::test]
async fn the_loser_of_a_race_for_one_label_is_refused_as_a_bad_parameter_value() {
    // Two moves of one label resolve with a single winner, because the second one
    // names in `RemoveFromVersionId` a version the label has already left. What
    // that call has to correct is the VALUE it sent — the same call naming the
    // winner is served — so every refusal here is the parameter refusal naming the
    // current holder, and the label itself never moves to a loser.
    for label in ["AWSCURRENT", "AWSPENDING", "blue"] {
        for contenders in [2usize, 3, 5] {
            // The two addressing forms of `SecretId`: "the ARN or the name of the
            // secret" (`UpdateSecretVersionStageRequest$SecretId`, this service's
            // Smithy model).
            for id_form in ["name", "arn"] {
                let case = format!("{contenders} moves of {label:?} addressed by {id_form}");
                let router = emulator();
                let name = "stage-race-loser";
                let (arn, v0) = create(&router, name, &case).await;
                let secret_id = if id_form == "arn" { arn } else { name.to_string() };

                // Where the label starts: `AWSCURRENT` is on the version the
                // secret was created with, and any other label is put there by a
                // write that names it.
                let source = if label == "AWSCURRENT" {
                    v0.clone()
                } else {
                    put(
                        &router,
                        &secret_id,
                        &version_id("seed"),
                        json!([label]),
                        &case,
                    )
                    .await
                };

                // The versions the contenders move the label to. Each names its
                // own staging label, so storing it leaves the label under test
                // where it is.
                let mut targets = Vec::new();
                for index in 0..contenders {
                    targets.push(
                        put(
                            &router,
                            &secret_id,
                            &version_id(&format!("c{index}")),
                            json!([format!("candidate-{index}")]),
                            &case,
                        )
                        .await,
                    );
                }

                // Every contender moves the label off the SAME source version, so
                // all but one of them is stale by the time it lands.
                let mut winners = Vec::new();
                let mut losers = Vec::new();
                for target in &targets {
                    let case = format!("{case}, move to {target}");
                    let held_by = holder(&router, &secret_id, label, &case)
                        .await
                        .unwrap_or_else(|| panic!("case {case:?}: {label} is on some version"));
                    let body = json!({
                        "SecretId": secret_id.clone(),
                        "VersionStage": label,
                        "MoveToVersionId": target.clone(),
                        "RemoveFromVersionId": source.clone(),
                    });
                    let (status, parsed) = call(&router, "UpdateSecretVersionStage", &body).await;
                    if status == 200 {
                        winners.push(target.clone());
                        continue;
                    }
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected a 400 for the stale move, got {status} with \
                         body {parsed}"
                    );
                    let code = code_of(&parsed, &case);
                    assert_eq!(
                        code, PARAMETER_REFUSAL,
                        "case {case:?}: expected {PARAMETER_REFUSAL} for a RemoveFromVersionId \
                         that does not name the holder, got {code:?} with body {parsed}"
                    );
                    let message = message_of(&parsed, &case);
                    let expected = expected_message(label, &held_by);
                    assert_eq!(
                        message, expected,
                        "case {case:?}: expected {expected:?}, got {message:?}"
                    );
                    losers.push(target.clone());
                }
                assert_eq!(
                    winners.len(),
                    1,
                    "case {case:?}: expected exactly one winner, got {winners:?} against \
                     {losers:?}"
                );
                let winner = winners[0].clone();

                // The issue's own call: a loser retries the move with a
                // `RemoveFromVersionId` naming ITSELF — the version it believes
                // holds the label. The rows run over that value and over the
                // version the label used to be on, both stale.
                let before = stages(&router, &secret_id, &case).await;
                let loser = losers
                    .first()
                    .cloned()
                    .unwrap_or_else(|| panic!("case {case:?}: a race has at least one loser"));
                for (row, named) in [
                    ("the version the call moves the label to", loser.clone()),
                    ("the version the label used to be on", source.clone()),
                ] {
                    let case = format!("{case}, RemoveFromVersionId naming {row}");
                    let (code, message) = err(
                        &router,
                        "UpdateSecretVersionStage",
                        json!({
                            "SecretId": secret_id.clone(),
                            "VersionStage": label,
                            "MoveToVersionId": loser.clone(),
                            "RemoveFromVersionId": named.clone(),
                        }),
                        &case,
                    )
                    .await;
                    assert_eq!(
                        code, PARAMETER_REFUSAL,
                        "case {case:?}: expected {PARAMETER_REFUSAL}, got {code:?} with message \
                         {message:?}"
                    );
                    let expected = expected_message(label, &winner);
                    assert_eq!(
                        message, expected,
                        "case {case:?}: expected {expected:?}, got {message:?}"
                    );
                    assert_eq!(
                        stages(&router, &secret_id, &case).await,
                        before,
                        "case {case:?}: expected the refused move to leave every staging label \
                         where it was"
                    );
                }

                // Naming the winner is what makes the same move succeed, so the
                // refusals above are about the value the call sent.
                let case = format!("{case}, RemoveFromVersionId naming the winner");
                ok(
                    &router,
                    "UpdateSecretVersionStage",
                    json!({
                        "SecretId": secret_id.clone(),
                        "VersionStage": label,
                        "MoveToVersionId": loser.clone(),
                        "RemoveFromVersionId": winner.clone(),
                    }),
                    &case,
                )
                .await;
                assert_eq!(
                    holder(&router, &secret_id, label, &case).await,
                    Some(loser.clone()),
                    "case {case:?}: expected {label:?} to end up on {loser}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_stale_move_of_awscurrent_after_a_value_write_is_refused_and_changes_nothing() {
    // The same rule reached without any race: a write that stores a value moves
    // `AWSCURRENT` to the version it creates ("Secrets Manager automatically moves
    // the label AWSCURRENT to the new version", `PutSecretValueRequest`, this
    // service's Smithy model), so a caller still holding the older version id
    // names a version the label has left. The rows run over how many writes have
    // moved the label since, and over the form of the `SecretId`.
    for writes in [1usize, 2, 4] {
        for id_form in ["name", "arn"] {
            let case = format!("{writes} value writes, addressed by {id_form}");
            let router = emulator();
            let name = "stale-awscurrent-move";
            let (arn, v0) = create(&router, name, &case).await;
            let secret_id = if id_form == "arn" { arn } else { name.to_string() };

            let mut newest = v0.clone();
            for index in 0..writes {
                newest = put(
                    &router,
                    &secret_id,
                    &version_id(&format!("w{index}")),
                    Value::Null,
                    &case,
                )
                .await;
            }
            assert_eq!(
                holder(&router, &secret_id, "AWSCURRENT", &case).await,
                Some(newest.clone()),
                "case {case:?}: expected the newest version to hold AWSCURRENT"
            );
            let before = stages(&router, &secret_id, &case).await;

            let (code, message) = err(
                &router,
                "UpdateSecretVersionStage",
                json!({
                    "SecretId": secret_id.clone(),
                    "VersionStage": "AWSCURRENT",
                    "MoveToVersionId": v0.clone(),
                    "RemoveFromVersionId": v0.clone(),
                }),
                &case,
            )
            .await;
            assert_eq!(
                code, PARAMETER_REFUSAL,
                "case {case:?}: expected {PARAMETER_REFUSAL} for a stale RemoveFromVersionId, \
                 got {code:?} with message {message:?}"
            );
            let expected = expected_message("AWSCURRENT", &newest);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert_eq!(
                stages(&router, &secret_id, &case).await,
                before,
                "case {case:?}: expected the refused move to leave every staging label where it \
                 was"
            );

            // The refusal left the secret's current value where it was: the read
            // that asks for no version still answers with the holder.
            let value = ok(
                &router,
                "GetSecretValue",
                json!({"SecretId": secret_id.clone()}),
                &case,
            )
            .await;
            assert_eq!(
                text(&value, "VersionId", &case),
                newest,
                "case {case:?}: expected GetSecretValue to agree with the AWSCURRENT holder"
            );
        }
    }
}
