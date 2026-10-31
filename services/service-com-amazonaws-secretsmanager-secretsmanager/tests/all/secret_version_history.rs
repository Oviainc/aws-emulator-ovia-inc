// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the version history a secret keeps, driven over the wire
//! through the multi-service router.
//!
//! The rules under test:
//!
//! * every write of a value appends a version and leaves the earlier ones in
//!   place, so each one stays readable by its own `VersionId` while the default
//!   `GetSecretValue` resolves the newest;
//! * a version reports every staging label attached to it, in the one order the
//!   service reports them, and each of those labels resolves that version;
//! * a version reports the date its value was last retrieved, at the date
//!   resolution the model documents, and reports none until it has been
//!   retrieved.
//!
//! Every test is a TABLE over the class its rule generalises: the operation that
//! wrote the version, the number of writes, the labels a version already
//! carried, and the form the retrieval took.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager`; the router routes on the credential
/// scope and the emulator verifies signatures only when an authenticator is
/// registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The length of the UTC day the access date's resolution is expressed in:
/// "Note that the resolution of this field is at the date level and does not
/// include the time." (`SecretVersionsListEntry$LastAccessedDate`, this
/// service's Smithy model.)
const SECONDS_PER_DAY: u64 = 86_400;

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

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// The `Versions` entries a listing reports, in the order it reports them.
fn versions(parsed: &Value, case: &str) -> Vec<Value> {
    parsed
        .get("Versions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no Versions in {parsed}"))
        .clone()
}

/// The staging labels a response reports, in the order it reports them; empty
/// when it reports none.
fn labels(parsed: &Value) -> Vec<String> {
    parsed
        .get("VersionStages")
        .and_then(Value::as_array)
        .map(|stages| {
            stages
                .iter()
                .map(|stage| stage.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The entry one `VersionId` has in a listing of every version.
async fn entry(router: &MultiServiceRouter, name: &str, version_id: &str, case: &str) -> Value {
    let listed = ok(
        router,
        "ListSecretVersionIds",
        json!({"SecretId": name, "IncludeDeprecated": true}),
        case,
    )
    .await;
    versions(&listed, case)
        .into_iter()
        .find(|entry| entry.get("VersionId").and_then(Value::as_str) == Some(version_id))
        .unwrap_or_else(|| panic!("case {case:?}: version {version_id} is not listed in {listed}"))
}

/// Midnight UTC of the day this runs, as the wire reports a timestamp.
fn today() -> f64 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs();
    (seconds - seconds % SECONDS_PER_DAY) as f64
}

#[tokio::test]
async fn every_write_appends_a_version_and_leaves_the_earlier_ones_readable() {
    // "If you include SecretString or SecretBinary to create a new secret
    // version, Secrets Manager automatically moves the staging label AWSCURRENT
    // to the new version. Then it attaches the label AWSPREVIOUS to the version
    // that AWSCURRENT was removed from." (`UpdateSecret`, this service's Smithy
    // model.) So no write overwrites a version: each one is still addressable by
    // its id afterwards, which is what makes a rollback by `VersionId` possible.
    // The rows run over the operation that writes the value and over how many
    // writes follow the create.
    for (case, operation) in [
        ("written with PutSecretValue", "PutSecretValue"),
        ("written with UpdateSecret", "UpdateSecret"),
    ] {
        for writes in 1..=3usize {
            let case = format!("{case}, {writes} write(s) after the create");
            let router = emulator();
            let name = format!("history/{operation}-{writes}");
            let created = ok(
                &router,
                "CreateSecret",
                json!({"Name": name.clone(), "SecretString": "value-0"}),
                &case,
            )
            .await;
            let mut stored = vec![(text(&created, "VersionId", &case), "value-0".to_string())];
            for index in 1..=writes {
                let value = format!("value-{index}");
                let written = ok(
                    &router,
                    operation,
                    json!({"SecretId": name.clone(), "SecretString": value.clone()}),
                    &case,
                )
                .await;
                stored.push((text(&written, "VersionId", &case), value));
            }

            // Every version written is still there, in the order it was made.
            let listed = ok(
                &router,
                "ListSecretVersionIds",
                json!({"SecretId": name.clone(), "IncludeDeprecated": true}),
                &case,
            )
            .await;
            let reported: Vec<String> = versions(&listed, &case)
                .iter()
                .map(|entry| text(entry, "VersionId", &case))
                .collect();
            let expected: Vec<String> =
                stored.iter().map(|(version_id, _)| version_id.clone()).collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected every version in creation order, got {listed}"
            );

            // Each one reads back its OWN value by id, superseded or not.
            for (version_id, value) in &stored {
                let read = ok(
                    &router,
                    "GetSecretValue",
                    json!({"SecretId": name.clone(), "VersionId": version_id.clone()}),
                    &case,
                )
                .await;
                assert_eq!(
                    text(&read, "SecretString", &case),
                    *value,
                    "case {case:?}: expected version {version_id} to still hold its own value, \
                     got {read}"
                );
            }

            // And the read that names no version resolves the newest one.
            let (newest, newest_value) = stored
                .last()
                .expect("the secret was created with a value")
                .clone();
            let current = ok(
                &router,
                "GetSecretValue",
                json!({"SecretId": name.clone()}),
                &case,
            )
            .await;
            assert_eq!(
                text(&current, "VersionId", &case),
                newest,
                "case {case:?}: expected the newest version to be current, got {current}"
            );
            assert_eq!(
                text(&current, "SecretString", &case),
                newest_value,
                "case {case:?}: expected the last value written back, got {current}"
            );
            assert_eq!(
                labels(&current),
                vec!["AWSCURRENT".to_string()],
                "case {case:?}: expected the newest version staged AWSCURRENT, got {current}"
            );
            // The version it was taken from carries AWSPREVIOUS, so a client can
            // roll back to it by label as well as by id.
            let (superseded, superseded_value) = stored[stored.len() - 2].clone();
            let previous = ok(
                &router,
                "GetSecretValue",
                json!({"SecretId": name.clone(), "VersionStage": "AWSPREVIOUS"}),
                &case,
            )
            .await;
            assert_eq!(
                text(&previous, "VersionId", &case),
                superseded,
                "case {case:?}: expected AWSPREVIOUS on the version AWSCURRENT left, got \
                 {previous}"
            );
            assert_eq!(
                text(&previous, "SecretString", &case),
                superseded_value,
                "case {case:?}: expected the superseded value under AWSPREVIOUS, got {previous}"
            );
        }
    }
}

#[tokio::test]
async fn a_version_reports_every_staging_label_attached_to_it() {
    // "When rotation is successful, the AWSPENDING staging label might be
    // attached to the same version as the AWSCURRENT version" (`RotateSecret`,
    // this service's Smithy model), so a version can carry several labels at
    // once and reports all of them — the labels the service moves itself first,
    // in the order its own lifecycle names them, then the caller's own in the
    // order they were attached. The rows run over the labels the version already
    // carried when AWSCURRENT was moved onto it.
    let rows: &[(&str, &[&str], &[&str])] = &[
        (
            "a version staged AWSPENDING",
            &["AWSPENDING"],
            &["AWSCURRENT", "AWSPENDING"],
        ),
        (
            "a version staged AWSPENDING and a caller's label",
            &["AWSPENDING", "blue"],
            &["AWSCURRENT", "AWSPENDING", "blue"],
        ),
        (
            "a version staged one caller label",
            &["teal"],
            &["AWSCURRENT", "teal"],
        ),
        (
            "a version staged two caller labels",
            &["one", "two"],
            &["AWSCURRENT", "one", "two"],
        ),
    ];
    for (case, attached, expected) in rows {
        let router = emulator();
        let name = "staging/labels";
        let created = ok(
            &router,
            "CreateSecret",
            json!({"Name": name, "SecretString": "current-value"}),
            case,
        )
        .await;
        let first = text(&created, "VersionId", case);
        let written = ok(
            &router,
            "PutSecretValue",
            json!({
                "SecretId": name,
                "SecretString": "candidate-value",
                "VersionStages": attached,
            }),
            case,
        )
        .await;
        let target = text(&written, "VersionId", case);

        // Move AWSCURRENT onto the version that already carries those labels.
        ok(
            &router,
            "UpdateSecretVersionStage",
            json!({
                "SecretId": name,
                "VersionStage": "AWSCURRENT",
                "MoveToVersionId": target.clone(),
                "RemoveFromVersionId": first.clone(),
            }),
            case,
        )
        .await;

        let expected: Vec<String> = expected.iter().map(|label| label.to_string()).collect();
        let current = ok(&router, "GetSecretValue", json!({"SecretId": name}), case).await;
        assert_eq!(
            text(&current, "VersionId", case),
            target,
            "case {case:?}: expected the version the label was moved to, got {current}"
        );
        assert_eq!(
            text(&current, "SecretString", case),
            "candidate-value",
            "case {case:?}: expected that version's own value, got {current}"
        );
        assert_eq!(
            labels(&current),
            expected,
            "case {case:?}: expected VersionStages {expected:?}, got {current}"
        );
        // The listing reports the same labels in the same order.
        let listed = entry(&router, name, &target, case).await;
        assert_eq!(
            labels(&listed),
            expected,
            "case {case:?}: expected the listing to report VersionStages {expected:?}, got \
             {listed}"
        );
        // Every label attached to the version resolves that version.
        for label in &expected {
            let read = ok(
                &router,
                "GetSecretValue",
                json!({"SecretId": name, "VersionStage": label}),
                case,
            )
            .await;
            assert_eq!(
                text(&read, "VersionId", case),
                target,
                "case {case:?}: expected staging label {label:?} to resolve version {target}, \
                 got {read}"
            );
        }
        // And AWSCURRENT left AWSPREVIOUS behind it on the version it came from.
        let superseded = entry(&router, name, &first, case).await;
        assert_eq!(
            labels(&superseded),
            vec!["AWSPREVIOUS".to_string()],
            "case {case:?}: expected AWSPREVIOUS on the version AWSCURRENT left, got {superseded}"
        );
    }
}

#[tokio::test]
async fn a_version_reports_the_date_its_value_was_last_retrieved() {
    // "The date that this version of the secret was last accessed. Note that the
    // resolution of this field is at the date level and does not include the
    // time." (`SecretVersionsListEntry$LastAccessedDate`, this service's Smithy
    // model), and the secret's own is "omitted if the secret has never been
    // retrieved in the Region" (`DescribeSecretResponse$LastAccessedDate`) — so
    // it is retrieving a value that records one, on the version retrieved. The
    // rows run over the form the retrieval takes and which version it names.
    let rows: &[(&str, &str, bool)] = &[
        ("GetSecretValue naming no version", "current", false),
        ("GetSecretValue naming the VersionId", "current", true),
        ("GetSecretValue naming AWSPREVIOUS", "previous", false),
        ("BatchGetSecretValue", "current", false),
    ];
    for (case, retrieved, by_id) in rows {
        let router = emulator();
        let name = "access/dates";
        let created = ok(
            &router,
            "CreateSecret",
            json!({"Name": name, "SecretString": "value-0"}),
            case,
        )
        .await;
        let previous = text(&created, "VersionId", case);
        let written = ok(
            &router,
            "PutSecretValue",
            json!({"SecretId": name, "SecretString": "value-1"}),
            case,
        )
        .await;
        let current = text(&written, "VersionId", case);
        let target = if *retrieved == "current" {
            current.clone()
        } else {
            previous.clone()
        };
        let other = if target == current {
            previous.clone()
        } else {
            current.clone()
        };

        // Nothing has been retrieved yet, so no version reports a date and
        // neither does the secret.
        for version_id in [&current, &previous] {
            let listed = entry(&router, name, version_id, case).await;
            assert_eq!(
                listed.get("LastAccessedDate"),
                None,
                "case {case:?}: version {version_id} has never been retrieved, got {listed}"
            );
        }
        let before = ok(&router, "DescribeSecret", json!({"SecretId": name}), case).await;
        assert_eq!(
            before.get("LastAccessedDate"),
            None,
            "case {case:?}: the secret has never been retrieved, got {before}"
        );

        // The retrieval this row is about.
        match *case {
            "BatchGetSecretValue" => {
                ok(
                    &router,
                    "BatchGetSecretValue",
                    json!({"SecretIdList": [name]}),
                    case,
                )
                .await;
            }
            _ => {
                let mut body = json!({"SecretId": name});
                if *by_id {
                    body["VersionId"] = json!(target.clone());
                } else if target == previous {
                    body["VersionStage"] = json!("AWSPREVIOUS");
                }
                ok(&router, "GetSecretValue", body, case).await;
            }
        }

        // The version retrieved carries the date, at date resolution; the one
        // that was not retrieved still carries none.
        let listed = entry(&router, name, &target, case).await;
        let reported = listed
            .get("LastAccessedDate")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a LastAccessedDate on version {target}, got {listed}")
            });
        assert_eq!(
            reported,
            today(),
            "case {case:?}: expected the retrieval dated midnight UTC of the day it happened, \
             got {listed}"
        );
        let untouched = entry(&router, name, &other, case).await;
        assert_eq!(
            untouched.get("LastAccessedDate"),
            None,
            "case {case:?}: version {other} was not retrieved, got {untouched}"
        );

        // The secret reports the same date, and nothing else about it moved.
        let after = ok(&router, "DescribeSecret", json!({"SecretId": name}), case).await;
        assert_eq!(
            after.get("LastAccessedDate").and_then(Value::as_f64),
            Some(today()),
            "case {case:?}: expected the secret to report the retrieval, got {after}"
        );
        assert_eq!(
            after.get("LastChangedDate"),
            before.get("LastChangedDate"),
            "case {case:?}: a retrieval changes nothing, so LastChangedDate stays put, got \
             {after}"
        );
        assert_eq!(
            after.get("VersionIdsToStages"),
            before.get("VersionIdsToStages"),
            "case {case:?}: a retrieval moves no staging label, got {after}"
        );
    }
}
