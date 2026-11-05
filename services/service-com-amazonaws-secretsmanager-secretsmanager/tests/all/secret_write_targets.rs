// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what this service's MUTATING operations do with the
//! secret they are addressed to, driven over the wire through the multi-service
//! router and read back through `ListSecrets`, `DescribeSecret`,
//! `GetSecretValue` and `ListSecretVersionIds`.
//!
//! The rules under test:
//!
//! * a mutation whose `SecretId` names no secret this service holds is refused
//!   with `ResourceNotFoundException` ("Secrets Manager can't find the resource
//!   that you asked for.", this service's Smithy model) and writes NOTHING: it
//!   never brings the secret it could not find into being, and leaves the
//!   secrets the store does hold exactly as they were;
//! * `UpdateSecret` is answered from the store, so the metadata and the value
//!   ONE call carries are persisted together: every member the call names reads
//!   back from the next report, a member it does not name keeps what was
//!   stored, and a new version exists only when the call carried a value;
//! * `LastChangedDate` is "the last date and time that this secret was modified
//!   in any way" (`DescribeSecretResponse$LastChangedDate`, this service's
//!   Smithy model), so an operation that modifies a secret advances it and an
//!   operation that only reports on one leaves it where it was.
//!
//! Every test is a TABLE over the class its rule generalises: the operation that
//! resolved the id and the form of a `SecretId` that names nothing; the set of
//! members one `UpdateSecret` call carries and the type and size of its value;
//! the modification made and the report taken.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on
/// the credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The one sentence this service answers a secret it cannot find with.
const SECRET_NOT_FOUND: &str = "Secrets Manager can't find the specified secret.";

/// The ARN of a Lambda rotation function, as `RotationLambdaARN` carries it.
const LAMBDA_ARN: &str = "arn:aws:lambda:us-west-2:123456789012:function:rotate-me";

/// The managed key `KmsKeyId` names when a call sets it to the empty string: "If
/// you set this to an empty string, Secrets Manager uses the Amazon Web Services
/// managed key aws/secretsmanager." (`UpdateSecretRequest$KmsKeyId`, this
/// service's Smithy model.)
const DEFAULT_KMS_KEY: &str = "alias/aws/secretsmanager";

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

/// Send one request that must fail with a 400, and return its wire error code
/// (the `__type` discriminator without namespace) and its message.
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
    let code = raw.split('#').next_back().unwrap_or(raw).to_string();
    let message = parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in error body {parsed}"))
        .to_string();
    (code, message)
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

/// A policy document, built through `serde_json` so it is a document the service
/// has to parse rather than one it can match on. Its principal belongs to the
/// account this emulator serves, which is the one a principal can exist in.
fn policy() -> String {
    let account = engine::EMULATOR_ACCOUNT_ID;
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{account}:role/reader")},
            "Action": "secretsmanager:GetSecretValue",
            "Resource": "*"
        }]
    })
    .to_string()
}

/// Create a secret holding one version under a known id, and report its ARN.
async fn create(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateSecret",
        json!({
            "Name": name,
            "SecretString": "stored",
            "ClientRequestToken": version_id("v0"),
        }),
        case,
    )
    .await;
    text(&created, "ARN", case)
}

/// Every secret `ListSecrets` reports, by name, sorted.
async fn listed_names(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let listed = ok(router, "ListSecrets", json!({}), case).await;
    let mut names: Vec<String> = listed
        .get("SecretList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no SecretList in {listed}"))
        .iter()
        .map(|entry| text(entry, "Name", case))
        .collect();
    names.sort();
    names
}

/// The version ids this secret holds, deprecated ones included, sorted.
async fn version_ids(router: &MultiServiceRouter, secret_id: &str, case: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListSecretVersionIds",
        json!({"SecretId": secret_id, "IncludeDeprecated": true}),
        case,
    )
    .await;
    let mut ids: Vec<String> = listed
        .get("Versions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no Versions in {listed}"))
        .iter()
        .map(|entry| text(entry, "VersionId", case))
        .collect();
    ids.sort();
    ids
}

/// The instant `DescribeSecret` reports a secret was last modified at.
async fn last_changed(router: &MultiServiceRouter, secret_id: &str, case: &str) -> f64 {
    let described = ok(router, "DescribeSecret", json!({"SecretId": secret_id}), case).await;
    described
        .get("LastChangedDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: no LastChangedDate in {described}"))
}

/// A prepared call: the operation's name and the members its request carries.
type Call = (&'static str, Value);

/// One row of the `LastChangedDate` table: a label for the case, the calls that
/// set it up, and the operation under test with its own request.
type Modification = (&'static str, Vec<Call>, &'static str, Value);

/// The request body for a row: the operation's own members plus a `SecretId`.
fn addressed_to(members: &Value, secret_id: &str) -> Value {
    let mut body = members.clone();
    body.as_object_mut()
        .expect("every row's members are an object")
        .insert("SecretId".to_string(), json!(secret_id));
    body
}

/// Every operation of this service that MUTATES the secret its `SecretId` names,
/// with the other members its request needs so the call reaches the resolution
/// of that id rather than being refused before it. `GetRandomPassword`, the one
/// operation still scaffolded in this crate, resolves no `SecretId` at all.
fn mutations() -> Vec<(&'static str, Value)> {
    vec![
        ("DeleteSecret", json!({})),
        ("RestoreSecret", json!({})),
        ("UpdateSecret", json!({"Description": "a description"})),
        ("UpdateSecret", json!({"SecretString": "a value"})),
        ("UpdateSecret", json!({"SecretBinary": "AAECAwQ="})),
        ("PutSecretValue", json!({"SecretString": "a value"})),
        (
            "UpdateSecretVersionStage",
            json!({"VersionStage": "AWSCURRENT", "MoveToVersionId": version_id("v0")}),
        ),
        ("TagResource", json!({"Tags": [{"Key": "env", "Value": "test"}]})),
        ("UntagResource", json!({"TagKeys": ["env"]})),
        ("PutResourcePolicy", json!({"ResourcePolicy": policy()})),
        ("DeleteResourcePolicy", json!({})),
        ("RotateSecret", json!({"RotationLambdaARN": LAMBDA_ARN})),
        ("CancelRotateSecret", json!({})),
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

#[tokio::test]
async fn a_mutation_on_a_secret_this_service_does_not_hold_creates_nothing() {
    // The rows run over every operation that mutates the secret it names and
    // over the documented FORMS of a `SecretId` ("The ARN or name of the
    // secret", and a partial ARN — `GetSecretValueRequest$SecretId`, this
    // service's Smithy model), none of which names a secret stored here. The
    // refusal is the specific one, and the store is as it was afterwards: a
    // write is never the thing that brings its own target into being.
    let account = engine::EMULATOR_ACCOUNT_ID;
    let missing = [
        "never-created".to_string(),
        "folder/path/never-created".to_string(),
        format!("arn:aws:secretsmanager:us-west-2:{account}:secret:never-created-AbCdEf"),
        format!("arn:aws:secretsmanager:us-west-2:{account}:secret:never-created"),
    ];
    let held = "an-existing-secret";

    for (operation, members) in mutations() {
        for secret_id in &missing {
            let case = format!("{operation} addressed to {secret_id:?}");
            let router = emulator();
            // A secret that DOES exist, so the store is not merely empty and the
            // answer is about the id this request named.
            create(&router, held, &case).await;
            let before = ok(&router, "DescribeSecret", json!({"SecretId": held}), &case).await;

            let (code, message) =
                err(&router, operation, addressed_to(&members, secret_id), &case).await;
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

            // Nothing was created for the id the call could not resolve: it is
            // still absent from the listing and still reported missing.
            assert_eq!(
                listed_names(&router, &case).await,
                vec![held.to_string()],
                "case {case:?}: expected the refused {operation} to create no secret"
            );
            let (code, message) = err(
                &router,
                "DescribeSecret",
                json!({"SecretId": secret_id}),
                &case,
            )
            .await;
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected the id to still name nothing, got {code:?} with message \
                 {message:?}"
            );
            // And the secret the store does hold is untouched by the refusal.
            let after = ok(&router, "DescribeSecret", json!({"SecretId": held}), &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused {operation} to leave the stored secret alone"
            );
            assert_eq!(
                version_ids(&router, held, &case).await,
                vec![version_id("v0")],
                "case {case:?}: expected the refused {operation} to store no version"
            );
        }
    }
}

#[tokio::test]
async fn update_secret_persists_every_member_one_call_names() {
    // "Modifies the details of a secret, including metadata and the secret
    // value. ... If you include SecretString or SecretBinary to create a new
    // secret version, Secrets Manager automatically moves the staging label
    // AWSCURRENT to the new version." (`UpdateSecret`, this service's Smithy
    // model.) The rows run over WHICH members one call carries — each metadata
    // member alone, metadata and a value together, and every member at once —
    // and over the type and size of the value, including the documented empty
    // `KmsKeyId`.
    //
    // (label, the members the call carries, the values expected back per member)
    let rows: [(&str, Value, Value); 7] = [
        (
            "the description alone",
            json!({"Description": "a new description"}),
            json!({"Description": "a new description"}),
        ),
        (
            // `DescriptionType` has a minimum length of 0, so the empty string
            // is accepted on the wire — and it describes nothing, so the secret
            // is left carrying no description at all.
            "an emptied description",
            json!({"Description": ""}),
            json!({}),
        ),
        (
            "a key and a short value",
            json!({
                "KmsKeyId": "arn:aws:kms:us-west-2:111122223333:key/abcd",
                "SecretString": "second",
            }),
            json!({
                "KmsKeyId": "arn:aws:kms:us-west-2:111122223333:key/abcd",
                "SecretString": "second",
            }),
        ),
        (
            "a key alias and a JSON value",
            json!({"KmsKeyId": "alias/my-key", "SecretString": "{\"user\":\"b\"}"}),
            json!({"KmsKeyId": "alias/my-key", "SecretString": "{\"user\":\"b\"}"}),
        ),
        (
            "an external type and a binary value",
            json!({"Type": "ExamplePartner", "SecretBinary": "AAECAwQ="}),
            json!({"Type": "ExamplePartner", "SecretBinary": "AAECAwQ="}),
        ),
        (
            "every member at once, over a long value",
            json!({
                "Description": "everything",
                "KmsKeyId": "alias/another-key",
                "Type": "OtherPartner",
                "SecretString": "y".repeat(8192),
            }),
            json!({
                "Description": "everything",
                "KmsKeyId": "alias/another-key",
                "Type": "OtherPartner",
                "SecretString": "y".repeat(8192),
            }),
        ),
        (
            // "If you set this to an empty string, Secrets Manager uses the
            // Amazon Web Services managed key aws/secretsmanager."
            "an emptied key with a value",
            json!({"KmsKeyId": "", "SecretString": "under the managed key"}),
            json!({"KmsKeyId": DEFAULT_KMS_KEY, "SecretString": "under the managed key"}),
        ),
    ];

    for (label, members, expected) in rows {
        let case = label.to_string();
        let router = emulator();
        let name = "update-target";
        let arn = create(&router, name, &case).await;
        let before = ok(&router, "DescribeSecret", json!({"SecretId": name}), &case).await;

        let updated = ok(&router, "UpdateSecret", addressed_to(&members, name), &case).await;
        assert_eq!(
            text(&updated, "ARN", &case),
            arn,
            "case {case:?}: expected the stored secret's ARN, got {updated}"
        );
        assert_eq!(
            text(&updated, "Name", &case),
            name,
            "case {case:?}: expected the stored secret's name, got {updated}"
        );

        let carries_a_value =
            members.get("SecretString").is_some() || members.get("SecretBinary").is_some();
        let described = ok(&router, "DescribeSecret", json!({"SecretId": name}), &case).await;
        for member in ["Description", "KmsKeyId", "Type"] {
            match expected.get(member) {
                // A member the call named reads back as it was applied.
                Some(value) => assert_eq!(
                    described.get(member),
                    Some(value),
                    "case {case:?}: expected {member} to read back as {value}, got {described}"
                ),
                // A member it did not name keeps what was already stored.
                None => assert_eq!(
                    described.get(member),
                    before.get(member),
                    "case {case:?}: expected {member} to keep its stored value, got {described}"
                ),
            }
        }

        // "If Secrets Manager created a new version of the secret during this
        // operation, then VersionId contains the unique identifier of the new
        // version." — so a metadata-only call reports none and stores none.
        let read = ok(&router, "GetSecretValue", json!({"SecretId": name}), &case).await;
        let ids = version_ids(&router, name, &case).await;
        if carries_a_value {
            let version = text(&updated, "VersionId", &case);
            assert_ne!(
                version,
                version_id("v0"),
                "case {case:?}: expected a NEW version rather than the one already stored"
            );
            assert_eq!(
                text(&read, "VersionId", &case),
                version,
                "case {case:?}: expected the new version to be current, got {read}"
            );
            for member in ["SecretString", "SecretBinary"] {
                assert_eq!(
                    read.get(member),
                    expected.get(member),
                    "case {case:?}: expected {member} to read back as stored, got {read}"
                );
            }
            assert_eq!(
                ids.len(),
                2,
                "case {case:?}: expected the call to add exactly one version, got {ids:?}"
            );
        } else {
            assert!(
                updated.get("VersionId").is_none(),
                "case {case:?}: expected no VersionId when the call carried no value, got \
                 {updated}"
            );
            assert_eq!(
                text(&read, "VersionId", &case),
                version_id("v0"),
                "case {case:?}: expected the stored version to stay current, got {read}"
            );
            assert_eq!(
                ids,
                vec![version_id("v0")],
                "case {case:?}: expected a metadata-only call to add no version, got {ids:?}"
            );
        }
    }
}

#[tokio::test]
async fn last_changed_date_advances_on_a_modification_and_never_on_a_report() {
    // "The last date and time that this secret was modified in any way."
    // (`DescribeSecretResponse$LastChangedDate`, this service's Smithy model.)
    // The rows run over the modification made — metadata, a value, a staging
    // label, a tag, a policy, the rotation configuration and the deletion
    // schedule — and over the report taken, so the rule is asserted in both
    // directions rather than for one operation.
    let name = "last-changed";
    let v0 = version_id("v0");

    // (label, the calls that set the row up, the operation under test, its members)
    let modifications: Vec<Modification> = vec![
        (
            "metadata through UpdateSecret",
            vec![],
            "UpdateSecret",
            json!({"SecretId": name, "Description": "changed"}),
        ),
        (
            "a value through UpdateSecret",
            vec![],
            "UpdateSecret",
            json!({"SecretId": name, "SecretString": "changed"}),
        ),
        (
            "a value through PutSecretValue",
            vec![],
            "PutSecretValue",
            json!({"SecretId": name, "SecretString": "appended"}),
        ),
        (
            "a staging label moved",
            vec![],
            "UpdateSecretVersionStage",
            json!({"SecretId": name, "VersionStage": "blue", "MoveToVersionId": v0.clone()}),
        ),
        (
            "a tag attached",
            vec![],
            "TagResource",
            json!({"SecretId": name, "Tags": [{"Key": "env", "Value": "test"}]}),
        ),
        (
            "a tag detached",
            vec![(
                "TagResource",
                json!({"SecretId": name, "Tags": [{"Key": "env", "Value": "test"}]}),
            )],
            "UntagResource",
            json!({"SecretId": name, "TagKeys": ["env"]}),
        ),
        (
            "a policy attached",
            vec![],
            "PutResourcePolicy",
            json!({"SecretId": name, "ResourcePolicy": policy()}),
        ),
        (
            "a policy detached",
            vec![(
                "PutResourcePolicy",
                json!({"SecretId": name, "ResourcePolicy": policy()}),
            )],
            "DeleteResourcePolicy",
            json!({"SecretId": name}),
        ),
        (
            "rotation configured",
            vec![],
            "RotateSecret",
            json!({"SecretId": name, "RotationLambdaARN": LAMBDA_ARN}),
        ),
        (
            "rotation cancelled",
            vec![(
                "RotateSecret",
                json!({"SecretId": name, "RotationLambdaARN": LAMBDA_ARN}),
            )],
            "CancelRotateSecret",
            json!({"SecretId": name}),
        ),
        (
            "a deletion scheduled",
            vec![],
            "DeleteSecret",
            json!({"SecretId": name, "RecoveryWindowInDays": 7}),
        ),
        (
            "a deletion cancelled",
            vec![(
                "DeleteSecret",
                json!({"SecretId": name, "RecoveryWindowInDays": 7}),
            )],
            "RestoreSecret",
            json!({"SecretId": name}),
        ),
    ];

    for (label, setup, operation, request) in modifications {
        let case = format!("{label} advances LastChangedDate");
        let router = emulator();
        create(&router, name, &case).await;
        for (prepare, body) in setup {
            ok(&router, prepare, body, &case).await;
        }
        let before = last_changed(&router, name, &case).await;
        ok(&router, operation, request, &case).await;
        let after = last_changed(&router, name, &case).await;
        assert!(
            after > before,
            "case {case:?}: expected {operation} to advance LastChangedDate past {before}, got \
             {after}"
        );
    }

    // A report is not a modification, whatever it reads: every one of them
    // leaves `LastChangedDate` where the last write put it.
    let reports: Vec<Call> = vec![
        ("DescribeSecret", json!({"SecretId": name})),
        ("GetSecretValue", json!({"SecretId": name})),
        ("GetSecretValue", json!({"SecretId": name, "VersionId": v0.clone()})),
        ("ListSecrets", json!({})),
        ("ListSecretVersionIds", json!({"SecretId": name, "IncludeDeprecated": true})),
        ("GetResourcePolicy", json!({"SecretId": name})),
        ("BatchGetSecretValue", json!({"SecretIdList": [name]})),
        (
            "ValidateResourcePolicy",
            json!({"SecretId": name, "ResourcePolicy": policy()}),
        ),
    ];
    for (operation, request) in reports {
        let case = format!("{operation} leaves LastChangedDate alone");
        let router = emulator();
        create(&router, name, &case).await;
        let before = last_changed(&router, name, &case).await;
        ok(&router, operation, request.clone(), &case).await;
        // Twice, so a report that moved the instant only on a first call is
        // caught as well.
        ok(&router, operation, request, &case).await;
        let after = last_changed(&router, name, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected {operation} to report without modifying the secret, got \
             {after} where {before} was stored"
        );
    }
}
