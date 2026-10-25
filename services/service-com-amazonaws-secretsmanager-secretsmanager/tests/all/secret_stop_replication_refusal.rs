// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the wire BODY of the `StopReplicationToReplica` refusal
//! a secret that is not a replica gets, driven over the multi-service router.
//!
//! The rule under test: `StopReplicationToReplica` is the replica-side promotion
//! call, so a `SecretId` that in the signed Region names a secret which is NOT a
//! replica — a replication primary, or a standalone secret that has no replicas
//! at all — is refused with HTTP 400, `InvalidParameterException`, and ONE fixed
//! sentence naming that class of secret: the message carries no secret name, ARN
//! or Region from the request or from the store, and nothing else is appended to
//! it.
//!
//! The test is a TABLE over the class the rule generalises: whether the secret
//! has replicas and how many, the form of the `SecretId` (name, path-like name,
//! full ARN, partial ARN), the charset and size of the name, and whether a
//! replica is still `InProgress` or the clock has settled it to `InSync`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    REPLICAS_PER_SECRET, REPLICA_STATUS_IN_SYNC, REPLICA_SYNC_DELAY,
};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and so the Region every secret
/// it creates is homed in: the Region that holds the PRIMARY.
const REGION: &str = "us-west-2";

/// The one fixed sentence `StopReplicationToReplica` refuses a non-replica
/// secret with. Asserted as a literal rather than read from the crate, so the
/// test pins the text itself.
const NOT_A_REPLICA: &str = "Operation not permitted on a primary/standalone secret.";

/// A SigV4 header naming `secretsmanager` in `region`; the router routes on the
/// credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/secretsmanager/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request signed for [`REGION`], and return its status and
/// JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", authorization(REGION))
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

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// How a row names the secret in its `SecretId`: the documented forms, "The ARN
/// or name of the secret" plus the partial ARN
/// (`GetSecretValueRequest$SecretId`, this service's Smithy model).
#[derive(Clone, Copy, Debug)]
enum IdForm {
    Name,
    FullArn,
    PartialArn,
}

/// The `SecretId` a row sends, built from the stored name and ARN.
fn secret_id(form: IdForm, name: &str, arn: &str) -> String {
    match form {
        IdForm::Name => name.to_string(),
        IdForm::FullArn => arn.to_string(),
        // The partial ARN is the full ARN without the trailing hyphen and the
        // six random characters the service appends to the secret name.
        IdForm::PartialArn => arn[..arn.len().saturating_sub(7)].to_string(),
    }
}

/// Call `StopReplicationToReplica` and assert the whole refusal: status, wire
/// error code and the fixed sentence. Asserting the message by EQUALITY is what
/// pins the "no variable part" half of the rule: a sentence carrying any secret
/// name, ARN or Region would not be equal to this one.
async fn assert_refused(router: &MultiServiceRouter, sent_id: &str, case: &str) {
    let (status, parsed) =
        call(router, "StopReplicationToReplica", &json!({"SecretId": sent_id})).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {parsed}"
    );
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    let code = raw.split('#').next_back().unwrap_or(raw);
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected InvalidParameterException, got {code} in {parsed}"
    );
    let message = parsed
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no Message in error body {parsed}"));
    assert_eq!(
        message, NOT_A_REPLICA,
        "case {case:?}: expected the fixed sentence {NOT_A_REPLICA:?}, got {message:?}"
    );
}

#[tokio::test]
async fn stop_replication_to_replica_refuses_a_non_replica_with_one_fixed_sentence() {
    // The rows vary what the rule generalises over: the number of replicas the
    // secret holds (none at all — a standalone secret — through several), the
    // form of the `SecretId`, and the charset and length of the name.
    let rows: &[(&str, &str, &[&str], IdForm)] = &[
        ("standalone, by name", "standalone-secret", &[], IdForm::Name),
        (
            "standalone with a path-like name, by name",
            "team/prod/db-password",
            &[],
            IdForm::Name,
        ),
        (
            "standalone with a long mixed-charset name, by name",
            "Prod_Secret.With-Mixed+Chars=12345678901234567890",
            &[],
            IdForm::Name,
        ),
        (
            "standalone, by full ARN",
            "standalone-by-arn",
            &[],
            IdForm::FullArn,
        ),
        (
            "standalone, by partial ARN",
            "standalone-by-partial-arn",
            &[],
            IdForm::PartialArn,
        ),
        (
            "primary with one replica, by name",
            "primary-one-replica",
            &["us-east-1"],
            IdForm::Name,
        ),
        (
            "primary with two replicas, by full ARN",
            "primary-two-replicas",
            &["us-east-1", "eu-west-1"],
            IdForm::FullArn,
        ),
        (
            "primary with several replicas, by partial ARN",
            "primary-many-replicas",
            &["us-east-1", "eu-west-1", "ap-northeast-1", "sa-east-1"],
            IdForm::PartialArn,
        ),
    ];

    for (case, name, replicas, form) in rows {
        let router = emulator();
        let created = ok(
            &router,
            "CreateSecret",
            json!({"Name": name, "SecretString": "s3cret"}),
            case,
        )
        .await;
        let arn = text(&created, "ARN", case);
        let regions: Vec<String> = replicas.iter().map(|region| region.to_string()).collect();
        if !regions.is_empty() {
            ok(
                &router,
                "ReplicateSecretToRegions",
                json!({
                    "SecretId": name,
                    "AddReplicaRegions": regions
                        .iter()
                        .map(|region| json!({"Region": region}))
                        .collect::<Vec<Value>>(),
                }),
                case,
            )
            .await;
        }

        let sent = secret_id(*form, name, &arn);
        assert_refused(&router, &sent, case).await;
    }

    // A count at the bound the store puts on one secret's replicas is the same
    // refusal: the rule is about the secret not being a replica here, not about
    // how many replicas it has.
    let router = emulator();
    let name = "primary-at-the-replica-bound";
    let case = "primary holding the maximum number of replicas";
    ok(
        &router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "s3cret"}),
        case,
    )
    .await;
    let regions: Vec<String> = (0..REPLICAS_PER_SECRET)
        .map(|index| format!("emu-region-{index}"))
        .collect();
    ok(
        &router,
        "ReplicateSecretToRegions",
        json!({
            "SecretId": name,
            "AddReplicaRegions": regions
                .iter()
                .map(|region| json!({"Region": region}))
                .collect::<Vec<Value>>(),
        }),
        case,
    )
    .await;
    assert_refused(&router, name, case).await;
}

#[tokio::test]
async fn the_refusal_is_the_same_sentence_once_the_replica_has_settled() {
    // A fresh replica is created `InProgress` and the clock moves it to
    // `InSync`; the primary's refusal does not depend on which of the two the
    // replica is in, so the sentence is asserted on both sides of that move.
    let router = emulator();
    let name = "primary-across-the-sync";
    let case = "primary whose only replica is still InProgress";
    ok(
        &router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "s3cret"}),
        case,
    )
    .await;
    let replica_region = "us-east-1".to_string();
    ok(
        &router,
        "ReplicateSecretToRegions",
        json!({"SecretId": name, "AddReplicaRegions": [{"Region": replica_region}]}),
        case,
    )
    .await;
    assert_refused(&router, name, case).await;

    // Wait out the span the store owes the replica's move to `InSync`, with a
    // small margin, then confirm through `DescribeSecret` that the move happened
    // before asserting the refusal again.
    let settled = "primary whose replica has settled to InSync";
    tokio::time::sleep(REPLICA_SYNC_DELAY + Duration::from_millis(100)).await;
    let described = ok(&router, "DescribeSecret", json!({"SecretId": name}), settled).await;
    let statuses: Vec<String> = described
        .get("ReplicationStatus")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {settled:?}: no ReplicationStatus in {described}"))
        .iter()
        .map(|entry| text(entry, "Status", settled))
        .collect();
    assert_eq!(
        statuses,
        vec![REPLICA_STATUS_IN_SYNC.to_string()],
        "case {settled:?}: expected the replica to have settled, got {described}"
    );
    assert_refused(&router, name, settled).await;
}
