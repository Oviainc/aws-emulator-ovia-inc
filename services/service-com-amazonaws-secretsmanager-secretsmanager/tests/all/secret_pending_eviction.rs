// SPDX-License-Identifier: CC0-1.0

//! What minting a new CURRENT version does to a pending one.
//!
//! The rule these tables assert: `AWSPENDING` names the candidate for a secret's
//! next current value, so a write that mints a new current version without
//! re-staging `AWSPENDING` on the version it mints decides that value without the
//! candidate and cancels it — the label comes off whichever version carried it, and
//! off the id a started rotation reserved for a version it had not created yet, so
//! afterwards the staging index has no `AWSPENDING` entry at all. The cancelled
//! version keeps its bytes: it is simply deprecated, like any version the last label
//! left, and stays readable by `VersionId`.
//!
//! A write that does name `AWSPENDING` among the labels of the version it stores
//! names its own candidate, so the label moves to that version rather than being
//! cancelled; a write that mints no current version at all leaves the candidate
//! where it is.
//!
//! The rows span what the rule generalises over rather than one recorded request:
//! the OPERATION that mints the version (`UpdateSecret`, `PutSecretValue`), the
//! `VersionStages` the write claims (omitted, `AWSCURRENT`, `AWSCURRENT` beside a
//! caller's label, `AWSCURRENT` beside `AWSPENDING`, `AWSPENDING` alone, a caller's
//! label alone), and WHERE the pending candidate sits when the write lands (on a
//! version a `PutSecretValue` staged, and on the id a started rotation reserved).

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on the
/// credential scope and the emulator verifies signatures only when an authenticator
/// is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// A rotation function for the rows whose candidate is a started rotation's
/// reservation.
const LAMBDA_ARN: &str = "arn:aws:lambda:us-west-2:123456789012:function:rotate-me";

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

/// A member of a response that has to be there.
fn text(body: &Value, member: &str, case: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {body}"))
        .to_string()
}

/// `DescribeSecret`'s `VersionIdsToStages` as `(version id, labels)` pairs.
async fn staging_index(
    router: &MultiServiceRouter,
    secret_id: &str,
    case: &str,
) -> Vec<(String, Vec<String>)> {
    let described = ok(
        router,
        "DescribeSecret",
        json!({"SecretId": secret_id}),
        case,
    )
    .await;
    let mut rows: Vec<(String, Vec<String>)> = described
        .get("VersionIdsToStages")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(version, labels)| {
                    (
                        version.clone(),
                        labels
                            .as_array()
                            .map(|labels| {
                                labels
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    rows.sort();
    rows
}

/// The ids the staging index holds `label` against, sorted.
fn holders(rows: &[(String, Vec<String>)], label: &str) -> Vec<String> {
    let mut ids: Vec<String> = rows
        .iter()
        .filter(|(_, labels)| labels.iter().any(|held| held == label))
        .map(|(version, _)| version.clone())
        .collect();
    ids.sort();
    ids
}

/// The labels the staging index holds against `version_id`; a deprecated version
/// has no entry at all, which reads as no labels.
fn labels_of(rows: &[(String, Vec<String>)], version_id: &str) -> Vec<String> {
    rows.iter()
        .find(|(version, _)| version == version_id)
        .map(|(_, labels)| labels.clone())
        .unwrap_or_default()
}

/// Create a secret holding one current version, answered as its version id.
async fn create(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "the-first-value"}),
        case,
    )
    .await;
    text(&created, "VersionId", case)
}

/// Where the pending candidate sits when the write under test lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Candidate {
    /// On a version a `PutSecretValue` staged `AWSPENDING` on.
    StagedVersion,
    /// On the id a started rotation reserved for the version its rotation
    /// function has not created yet: "During rotation, Secrets Manager creates an
    /// AWSPENDING version ID before creating the new secret version."
    /// (`DescribeSecretResponse$VersionIdsToStages`, this service's Smithy model.)
    RotationReservation,
}

/// Stage a pending candidate on `name` and answer the version id it names.
async fn stage_candidate(
    router: &MultiServiceRouter,
    name: &str,
    candidate: Candidate,
    case: &str,
) -> String {
    let staged = match candidate {
        Candidate::StagedVersion => {
            ok(
                router,
                "PutSecretValue",
                json!({
                    "SecretId": name,
                    "SecretString": "the-candidate-value",
                    "VersionStages": ["AWSPENDING"],
                }),
                case,
            )
            .await
        }
        Candidate::RotationReservation => {
            ok(
                router,
                "RotateSecret",
                json!({"SecretId": name, "RotationLambdaARN": LAMBDA_ARN}),
                case,
            )
            .await
        }
    };
    let pending = text(&staged, "VersionId", case);
    // The setup put the candidate where the rule is about to be applied.
    let rows = staging_index(router, name, case).await;
    assert_eq!(
        holders(&rows, "AWSPENDING"),
        vec![pending.clone()],
        "case {case:?}: expected the setup to stage AWSPENDING on {pending} alone, got {rows:?}"
    );
    pending
}

/// What the write under test claims, and what the rule says about the candidate.
struct Row {
    case: &'static str,
    operation: &'static str,
    /// The `VersionStages` the write names, or `None` for a write that omits the
    /// member ("If you don't include VersionStages, then Secrets Manager
    /// automatically moves the staging label AWSCURRENT to this version",
    /// `PutSecretValueRequest$VersionStages`).
    stages: Option<&'static [&'static str]>,
    /// Whether the write mints the secret's new current version.
    mints_current: bool,
    /// Where `AWSPENDING` is once the write has landed: nowhere (cancelled), on
    /// the version this write stored, or still on the candidate.
    pending_after: PendingAfter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingAfter {
    /// The label is on no version and on no reservation.
    Cancelled,
    /// The write named its own candidate, so the label is on the version it
    /// stored.
    OnTheWrittenVersion,
    /// The write minted no current version, so the candidate stands.
    OnTheCandidate,
}

/// The writes the rule runs over: every form of a write that mints a new current
/// version, and the writes that do not.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "UpdateSecret storing a value",
            operation: "UpdateSecret",
            stages: None,
            mints_current: true,
            pending_after: PendingAfter::Cancelled,
        },
        Row {
            case: "PutSecretValue with no VersionStages",
            operation: "PutSecretValue",
            stages: None,
            mints_current: true,
            pending_after: PendingAfter::Cancelled,
        },
        Row {
            case: "PutSecretValue claiming AWSCURRENT",
            operation: "PutSecretValue",
            stages: Some(&["AWSCURRENT"]),
            mints_current: true,
            pending_after: PendingAfter::Cancelled,
        },
        Row {
            case: "PutSecretValue claiming AWSCURRENT beside a caller's label",
            operation: "PutSecretValue",
            stages: Some(&["AWSCURRENT", "blue"]),
            mints_current: true,
            pending_after: PendingAfter::Cancelled,
        },
        Row {
            case: "PutSecretValue claiming AWSCURRENT and AWSPENDING",
            operation: "PutSecretValue",
            stages: Some(&["AWSCURRENT", "AWSPENDING"]),
            mints_current: true,
            pending_after: PendingAfter::OnTheWrittenVersion,
        },
        Row {
            case: "PutSecretValue claiming AWSPENDING alone",
            operation: "PutSecretValue",
            stages: Some(&["AWSPENDING"]),
            mints_current: false,
            pending_after: PendingAfter::OnTheWrittenVersion,
        },
        Row {
            case: "PutSecretValue claiming a caller's label alone",
            operation: "PutSecretValue",
            stages: Some(&["release/2024-05"]),
            mints_current: false,
            pending_after: PendingAfter::OnTheCandidate,
        },
    ]
}

#[tokio::test]
async fn a_new_current_version_cancels_the_pending_candidate() {
    for (index, row) in rows().iter().enumerate() {
        for candidate in [Candidate::StagedVersion, Candidate::RotationReservation] {
            let case = format!("{}, candidate {candidate:?}", row.case);
            let router = emulator();
            let name = format!("pending-eviction-{index}");
            let first = create(&router, &name, &case).await;
            let pending = stage_candidate(&router, &name, candidate, &case).await;

            let mut body = Map::new();
            body.insert("SecretId".to_string(), json!(name.clone()));
            body.insert("SecretString".to_string(), json!("the-written-value"));
            if let Some(stages) = row.stages {
                body.insert("VersionStages".to_string(), json!(stages));
            }
            let written = ok(&router, row.operation, Value::Object(body), &case).await;
            let written_id = text(&written, "VersionId", &case);

            let rows_after = staging_index(&router, &name, &case).await;
            let expected_pending: Vec<String> = match row.pending_after {
                PendingAfter::Cancelled => Vec::new(),
                PendingAfter::OnTheWrittenVersion => vec![written_id.clone()],
                PendingAfter::OnTheCandidate => vec![pending.clone()],
            };
            assert_eq!(
                holders(&rows_after, "AWSPENDING"),
                expected_pending,
                "case {case:?}: expected AWSPENDING on {expected_pending:?}, got {rows_after:?}"
            );

            // The version the write minted is the one a read with no version or
            // label resolves, and the version it displaced carries AWSPREVIOUS.
            let read = ok(
                &router,
                "GetSecretValue",
                json!({"SecretId": name.clone()}),
                &case,
            )
            .await;
            let expected_current = if row.mints_current {
                written_id.clone()
            } else {
                first.clone()
            };
            assert_eq!(
                text(&read, "VersionId", &case),
                expected_current,
                "case {case:?}: expected AWSCURRENT on {expected_current}, got {read}"
            );
            if row.mints_current {
                assert_eq!(
                    holders(&rows_after, "AWSPREVIOUS"),
                    vec![first.clone()],
                    "case {case:?}: expected AWSPREVIOUS on the displaced version {first}, got \
                     {rows_after:?}"
                );
            }

            // A cancelled candidate that was a stored version keeps its bytes: it
            // is deprecated, so the staging index holds nothing against it while
            // `VersionId` still reads it.
            let cancelled_a_stored_version = candidate == Candidate::StagedVersion
                && row.pending_after != PendingAfter::OnTheCandidate;
            if cancelled_a_stored_version {
                assert_eq!(
                    labels_of(&rows_after, &pending),
                    Vec::<String>::new(),
                    "case {case:?}: expected no staging labels on the cancelled version \
                     {pending}, got {rows_after:?}"
                );
                let by_id = ok(
                    &router,
                    "GetSecretValue",
                    json!({"SecretId": name.clone(), "VersionId": pending.clone()}),
                    &case,
                )
                .await;
                assert_eq!(
                    text(&by_id, "SecretString", &case),
                    "the-candidate-value",
                    "case {case:?}: expected the cancelled version's bytes to survive, got \
                     {by_id}"
                );
                let listed = ok(
                    &router,
                    "ListSecretVersionIds",
                    json!({"SecretId": name.clone(), "IncludeDeprecated": true}),
                    &case,
                )
                .await;
                let ids: Vec<String> = listed["Versions"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: no Versions in {listed}"))
                    .iter()
                    .filter_map(|entry| entry.get("VersionId").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect();
                assert!(
                    ids.contains(&pending),
                    "case {case:?}: expected the cancelled version {pending} to still be stored, \
                     got {ids:?}"
                );
            }

            // A cancelled candidate leaves no rotation in flight, so a read for
            // the label is refused the way a label on no version is.
            if row.pending_after == PendingAfter::Cancelled {
                let (status, parsed) = call(
                    &router,
                    "GetSecretValue",
                    &json!({"SecretId": name.clone(), "VersionStage": "AWSPENDING"}),
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the cancelled label to resolve nothing, got \
                     {status} with {parsed}"
                );
                let message = parsed
                    .get("Message")
                    .or_else(|| parsed.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: no Message in {parsed}"));
                assert_eq!(
                    message,
                    "Secrets Manager can't find the specified secret value for staging label: \
                     AWSPENDING",
                    "case {case:?}: expected the staging-label sentence, got {parsed}"
                );
            }
        }
    }
}
