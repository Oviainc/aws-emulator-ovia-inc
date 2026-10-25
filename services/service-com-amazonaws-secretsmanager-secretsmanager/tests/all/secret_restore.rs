// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RestoreSecret`, driven over the wire through the
//! multi-service router and read back through `DescribeSecret`,
//! `GetSecretValue`, `PutSecretValue` and `ListSecrets`.
//!
//! The rules under test:
//!
//! * `RestoreSecret` cancels a scheduled deletion by removing the secret's
//!   deletion stamp, so the secret is readable and writable again and is listed
//!   without asking for planned deletions — whatever recovery window scheduled
//!   it, and whichever form of `SecretId` names it;
//! * cancelling a deletion touches nothing else about the secret: every version
//!   and the whole staging map survive it, and a secret no deletion is scheduled
//!   for is left exactly as it was while the call still reports its ARN and name;
//! * a deletion is cancellable only while the secret's deletion date is still
//!   ahead — past it the deletion is final, so a `SecretId` naming such a secret,
//!   or no secret at all, is `ResourceNotFoundException`.
//!
//! Every test is a TABLE over the class its rule generalises: the recovery
//! window, the form of the `SecretId` (name, full ARN, partial ARN), how many
//! versions and staging labels the secret holds, how many times the call is
//! repeated, and whether the secret was deleted with a window, without one, or
//! never.

use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// The one sentence this service answers a secret it cannot find with, whichever
/// operation raised it (`ResourceNotFoundException` is documented as "Secrets
/// Manager can't find the resource that you asked for.", this service's Smithy
/// model).
const SECRET_NOT_FOUND: &str = "Secrets Manager can't find the specified secret.";

/// The trailing hyphen and six random characters Secrets Manager appends to a
/// secret's name in its ARN, which a partial ARN leaves off
/// (`CreateSecretRequest$Name`, this service's Smithy model).
const ARN_SUFFIX_LEN: usize = 7;

/// How one row names the secret in its call: the secret's name and its full ARN
/// go in, the `SecretId` the call carries comes out.
type SecretIdForm = fn(&str, &str) -> String;

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

/// Send one request that must fail, and return its wire error code and message.
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

/// Assert an operation answers `ResourceNotFoundException`, with the one message
/// this service reports for a secret it does not hold.
async fn assert_not_found(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) {
    let (code, message) = err(router, operation, body, case).await;
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException from {operation}, got {code}"
    );
    assert_eq!(
        message, SECRET_NOT_FOUND,
        "case {case:?}: expected the not-found message from {operation}, got {message:?}"
    );
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// The secret names `ListSecrets` reports for a request.
async fn listed_names(router: &MultiServiceRouter, body: Value, case: &str) -> Vec<String> {
    let listed = ok(router, "ListSecrets", body, case).await;
    listed
        .get("SecretList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no SecretList in {listed}"))
        .iter()
        .map(|entry| text(entry, "Name", case))
        .collect()
}

/// The three forms of `SecretId` this service resolves, as the table's rows name
/// them.
fn secret_id_forms() -> Vec<(&'static str, SecretIdForm)> {
    vec![
        ("by name", |name, _arn| name.to_string()),
        ("by full ARN", |_name, arn| arn.to_string()),
        ("by partial ARN", |_name, arn| {
            arn[..arn.len() - ARN_SUFFIX_LEN].to_string()
        }),
    ]
}

#[tokio::test]
async fn restore_secret_cancels_a_scheduled_deletion_whatever_window_scheduled_it() {
    // The rows run over the documented recovery-window range — "The number of
    // days from 7 to 30 that Secrets Manager waits before permanently deleting
    // the secret" (DeleteSecretRequest$RecoveryWindowInDays, this service's
    // Smithy model) — at each published bound, inside it, and with the window
    // left out so the documented default applies, crossed with every form of
    // SecretId the service resolves.
    for (window_case, window) in [
        ("the published minimum window", Some(7_i64)),
        ("a window inside the range", Some(15)),
        ("the published maximum window", Some(30)),
        ("no window, so the documented default", None),
    ] {
        for (id_case, id_form) in secret_id_forms() {
            let case = format!("{window_case}, {id_case}");
            let case = case.as_str();
            let router = emulator();
            let name = "restore/scheduled";
            let created = ok(
                &router,
                "CreateSecret",
                json!({"Name": name, "SecretString": "the value"}),
                case,
            )
            .await;
            let arn = text(&created, "ARN", case);

            let mut delete = json!({"SecretId": name});
            if let Some(window) = window {
                delete["RecoveryWindowInDays"] = json!(window);
            }
            let deleted = ok(&router, "DeleteSecret", delete, case).await;
            assert!(
                deleted.get("DeletionDate").is_some(),
                "case {case:?}: expected the delete to report a DeletionDate, got {deleted}"
            );
            let scheduled = ok(&router, "DescribeSecret", json!({"SecretId": name}), case).await;
            assert!(
                scheduled.get("DeletedDate").is_some(),
                "case {case:?}: expected the scheduled secret to carry a DeletedDate, got \
                 {scheduled}"
            );

            let restored = ok(
                &router,
                "RestoreSecret",
                json!({"SecretId": id_form(name, &arn)}),
                case,
            )
            .await;
            assert_eq!(
                text(&restored, "ARN", case),
                arn,
                "case {case:?}: expected the restore to report the secret's ARN, got {restored}"
            );
            assert_eq!(
                text(&restored, "Name", case),
                name,
                "case {case:?}: expected the restore to report the secret's name, got {restored}"
            );

            // The deletion stamp is gone, and with it every refusal the
            // scheduled state caused.
            let described = ok(&router, "DescribeSecret", json!({"SecretId": name}), case).await;
            assert!(
                described.get("DeletedDate").is_none(),
                "case {case:?}: expected the restored secret to carry no DeletedDate, got \
                 {described}"
            );
            let fetched = ok(&router, "GetSecretValue", json!({"SecretId": name}), case).await;
            assert_eq!(
                text(&fetched, "SecretString", case),
                "the value",
                "case {case:?}: expected the restored secret's value to read back, got {fetched}"
            );
            ok(
                &router,
                "PutSecretValue",
                json!({"SecretId": name, "SecretString": "written after the restore"}),
                case,
            )
            .await;
            assert_eq!(
                listed_names(&router, json!({}), case).await,
                vec![name.to_string()],
                "case {case:?}: expected the restored secret to be listed without asking for \
                 planned deletions"
            );
        }
    }
}

#[tokio::test]
async fn restore_secret_preserves_every_version_and_the_whole_staging_map() {
    // The rows run over how much staging state the secret holds: one version,
    // several (so AWSPREVIOUS is in play), and several plus a caller-named
    // label, which is the full staging map a restore must leave alone.
    // Each row lists the puts made after the create, and the staging labels each
    // put claims (none, so it takes AWSCURRENT, or a caller-named label, which
    // leaves AWSCURRENT where it was).
    for (case, puts) in [
        ("one version", vec![]),
        ("two versions", vec![None]),
        ("four versions", vec![None, None, None]),
        (
            "four versions and a caller-named label",
            vec![None, None, Some("AWSTESTING")],
        ),
    ] {
        let router = emulator();
        let name = "restore/versions";
        ok(
            &router,
            "CreateSecret",
            json!({"Name": name, "SecretString": "value 0"}),
            case,
        )
        .await;
        for (index, stage) in puts.iter().enumerate() {
            let mut body = json!({
                "SecretId": name,
                "SecretString": format!("value {}", index + 1),
            });
            if let Some(stage) = stage {
                body["VersionStages"] = json!([stage]);
            }
            ok(&router, "PutSecretValue", body, case).await;
        }

        let before = ok(&router, "DescribeSecret", json!({"SecretId": name}), case).await;
        let stages_before = before
            .get("VersionIdsToStages")
            .unwrap_or_else(|| panic!("case {case:?}: no VersionIdsToStages in {before}"))
            .clone();
        let versions_before = ok(
            &router,
            "ListSecretVersionIds",
            json!({"SecretId": name, "IncludeDeprecated": true}),
            case,
        )
        .await;

        ok(
            &router,
            "DeleteSecret",
            json!({"SecretId": name, "RecoveryWindowInDays": 7}),
            case,
        )
        .await;
        ok(&router, "RestoreSecret", json!({"SecretId": name}), case).await;

        let after = ok(&router, "DescribeSecret", json!({"SecretId": name}), case).await;
        assert_eq!(
            after.get("VersionIdsToStages"),
            Some(&stages_before),
            "case {case:?}: expected the staging map to survive the restore, got {after}"
        );
        let versions_after = ok(
            &router,
            "ListSecretVersionIds",
            json!({"SecretId": name, "IncludeDeprecated": true}),
            case,
        )
        .await;
        assert_eq!(
            versions_after.get("Versions"),
            versions_before.get("Versions"),
            "case {case:?}: expected every version to survive the restore, got {versions_after}"
        );
        // Each version still reads back under its own id, deprecated ones
        // included.
        for index in 0..=puts.len() {
            let value = format!("value {index}");
            let fetched = ok(
                &router,
                "GetSecretValue",
                json!({"SecretId": name, "VersionId": version_id_of(&versions_before, index, case)}),
                case,
            )
            .await;
            assert_eq!(
                text(&fetched, "SecretString", case),
                value,
                "case {case:?}: expected version {index} to read back unchanged, got {fetched}"
            );
        }
    }
}

/// The id of the `index`-th version a `ListSecretVersionIds` body reports.
fn version_id_of(listed: &Value, index: usize, case: &str) -> String {
    let versions = listed
        .get("Versions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no Versions in {listed}"));
    let version = versions
        .get(index)
        .unwrap_or_else(|| panic!("case {case:?}: no version {index} in {listed}"));
    text(version, "VersionId", case)
}

#[tokio::test]
async fn restoring_a_secret_no_deletion_is_scheduled_for_changes_nothing() {
    // The rows differ along how the secret got into a state with no scheduled
    // deletion — never deleted, or deleted and already restored — and over how
    // many further restores are piled on, since each must answer like the first
    // and leave the secret as it is.
    for (case, delete_first, repeats) in [
        ("never deleted, one restore", false, 1_usize),
        ("never deleted, three restores", false, 3),
        ("deleted and already restored, one restore", true, 1),
        ("deleted and already restored, three restores", true, 3),
    ] {
        let router = emulator();
        let name = "restore/idempotent";
        let created = ok(
            &router,
            "CreateSecret",
            json!({"Name": name, "SecretString": "v", "Description": "unchanging"}),
            case,
        )
        .await;
        let arn = text(&created, "ARN", case);
        if delete_first {
            ok(
                &router,
                "DeleteSecret",
                json!({"SecretId": name, "RecoveryWindowInDays": 7}),
                case,
            )
            .await;
            ok(&router, "RestoreSecret", json!({"SecretId": name}), case).await;
        }

        let before = ok(&router, "DescribeSecret", json!({"SecretId": name}), case).await;
        for repeat in 0..repeats {
            let restored = ok(&router, "RestoreSecret", json!({"SecretId": name}), case).await;
            assert_eq!(
                text(&restored, "ARN", case),
                arn,
                "case {case:?}: restore {repeat} reported another ARN: {restored}"
            );
            assert_eq!(
                text(&restored, "Name", case),
                name,
                "case {case:?}: restore {repeat} reported another name: {restored}"
            );
        }
        let after = ok(&router, "DescribeSecret", json!({"SecretId": name}), case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a restore with no scheduled deletion changed the secret: expected \
             {before}, got {after}"
        );
    }
}

#[tokio::test]
async fn restore_secret_refuses_a_secret_id_it_holds_no_restorable_secret_for() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateSecret",
        json!({"Name": "restore/known", "SecretString": "v"}),
        "create a secret to build unknown identifiers from",
    )
    .await;
    let known_arn = text(
        &created,
        "ARN",
        "create a secret to build unknown identifiers from",
    );
    let missing_arn = known_arn.replace("restore/known", "restore/missing");
    let missing_partial_arn = missing_arn[..missing_arn.len() - ARN_SUFFIX_LEN].to_string();

    // No secret of any form: a name, a full ARN and a partial ARN that name
    // nothing this service holds.
    for (case, secret_id) in [
        ("an unknown name", "restore/missing".to_string()),
        ("an unknown full ARN", missing_arn.clone()),
        ("an unknown partial ARN", missing_partial_arn),
    ] {
        assert_not_found(&router, "RestoreSecret", json!({"SecretId": secret_id}), case).await;
    }
}

#[tokio::test]
async fn a_deletion_with_no_recovery_window_is_past_restoring() {
    // A delete that skips the recovery window stamps the secret's deletion date
    // at the call itself, and that date is "the date and time after which this
    // secret Secrets Manager can permanently delete this secret, and it can no
    // longer be restored" (DeleteSecretResponse$DeletionDate, this service's
    // Smithy model) — so there is no span in which such a deletion can be
    // cancelled, inside the asynchronous removal delay or after it.
    let router = emulator();
    let name = "restore/forced";
    ok(
        &router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "v"}),
        "create for the forced delete",
    )
    .await;
    let started = Instant::now();
    ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": name, "ForceDeleteWithoutRecovery": true}),
        "force delete",
    )
    .await;

    // Straight after the call the secret is still there, marked for deletion —
    // and a restore neither resurrects it nor clears that mark. Only asserted
    // while the removal span cannot yet have elapsed, so a loaded machine never
    // turns this into a false failure.
    if started.elapsed() < FORCE_DELETE_PURGE_DELAY {
        let case = "inside the asynchronous removal delay";
        assert_not_found(&router, "RestoreSecret", json!({"SecretId": name}), case).await;
        let described = ok(&router, "DescribeSecret", json!({"SecretId": name}), case).await;
        assert!(
            described.get("DeletedDate").is_some(),
            "case {case:?}: expected the refused restore to leave the deletion stamp in place, \
             got {described}"
        );
    }

    // Once the removal the clock owed has happened there is no secret left at
    // all, and the refusal reads the same.
    tokio::time::sleep(FORCE_DELETE_PURGE_DELAY + Duration::from_millis(200)).await;
    let case = "after the asynchronous removal delay";
    assert_not_found(&router, "RestoreSecret", json!({"SecretId": name}), case).await;
    assert_not_found(&router, "DescribeSecret", json!({"SecretId": name}), case).await;
    assert!(
        listed_names(
            &router,
            json!({"IncludePlannedDeletion": true}),
            "list after the removal"
        )
        .await
        .is_empty(),
        "case {case:?}: expected the store to hold no secret after the removal"
    );
}
