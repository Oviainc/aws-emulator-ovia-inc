// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two instants a scheduled deletion of a secret
//! carries, driven over the wire through the multi-service router.
//!
//! The rule under test: a delete stamps the secret with the instant the deletion
//! was REQUESTED — "The date and time the deletion of the secret occurred. Not
//! present on active secrets. The secret can be recovered until the number of
//! days in the recovery window has passed" (`SecretListEntry$DeletedDate`, this
//! service's Smithy model) — and reports the END of the recovery window
//! separately: "This value is the date and time of the delete request plus the
//! number of days in RecoveryWindowInDays"
//! (`DeleteSecretResponse$DeletionDate`). So the recovery window a caller asked
//! for is the span between the two, and the window is measured from each delete
//! request: a later one re-states the schedule.
//!
//! Every test is a TABLE over the class the rule generalises: the window asked
//! for (each published bound of the documented 7..30 range, one inside it, and
//! the window left out so the documented default applies), the form of the
//! `SecretId`, and the reporting operation the stamp is read back through
//! (`DescribeSecret` and `ListSecrets`).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    ARN_SUFFIX_LEN, FORCE_DELETE_PURGE_DELAY,
};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager`; the router routes on the credential
/// scope and the emulator verifies signatures only when an authenticator is
/// registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// Seconds in a day, for reading a window back as the number of days it covers.
const SECONDS_PER_DAY: f64 = 86_400.0;

/// How far either stamp may sit from the instant this test read the clock, in
/// seconds: enough slack for a loaded machine, far below the shortest window the
/// service accepts.
const SLACK_SECONDS: f64 = 300.0;

/// The documented recovery-window range, as the rows name it: "The number of
/// days from 7 to 30 that Secrets Manager waits before permanently deleting the
/// secret. ... If you don't use either, then by default Secrets Manager uses a 30
/// day recovery window." (`DeleteSecretRequest$RecoveryWindowInDays`, this
/// service's Smithy model.) Each row is (case, the window asked for, the window
/// that applies).
const WINDOW_ROWS: [(&str, Option<i64>, i64); 4] = [
    ("the published minimum", Some(7), 7),
    ("a window inside the range", Some(19), 19),
    ("the published maximum", Some(30), 30),
    ("no window, so the documented default", None, 30),
];

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

/// A required timestamp member of a response body, as seconds since the epoch.
fn instant(parsed: &Value, member: &str, case: &str) -> f64 {
    parsed
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
}

/// The clock, as the wire reports an instant.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs_f64()
}

/// `DescribeSecret` for one secret.
async fn describe(router: &MultiServiceRouter, secret_id: &str, case: &str) -> Value {
    ok(
        router,
        "DescribeSecret",
        json!({"SecretId": secret_id}),
        case,
    )
    .await
}

/// The entry `ListSecrets` reports for one secret, scheduled deletions included.
async fn listed(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let page = ok(
        router,
        "ListSecrets",
        json!({"IncludePlannedDeletion": true}),
        case,
    )
    .await;
    page.get("SecretList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no SecretList in {page}"))
        .iter()
        .find(|entry| entry.get("Name").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("case {case:?}: {name} is not listed in {page}"))
        .clone()
}

/// Create a secret and return its ARN.
async fn create(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "stored"}),
        case,
    )
    .await;
    text(&created, "ARN", case)
}

#[tokio::test]
async fn a_scheduled_deletion_reports_the_request_instant_and_the_window_separately() {
    // Every documented window, crossed with the form of the `SecretId` the
    // delete addresses the secret by and the reporting operation the stamp is
    // read back through: in each the DeletedDate is the request and the
    // DeletionDate is that request plus the window.
    for (window_case, window, days) in WINDOW_ROWS {
        for id_case in ["by name", "by full ARN", "by partial ARN"] {
            let router = emulator();
            let name = "window/subject";
            let case = format!("{window_case}, addressed {id_case}");
            let arn = create(&router, name, &case).await;
            let secret_id = match id_case {
                "by name" => name.to_string(),
                "by full ARN" => arn.clone(),
                _ => arn[..arn.len() - ARN_SUFFIX_LEN - 1].to_string(),
            };

            let mut body = json!({"SecretId": secret_id});
            if let Some(window) = window {
                body["RecoveryWindowInDays"] = json!(window);
            }
            let requested_at = now_seconds();
            let deleted = ok(&router, "DeleteSecret", body, &case).await;
            let deletion_date = instant(&deleted, "DeletionDate", &case);
            assert!(
                (deletion_date - (requested_at + days as f64 * SECONDS_PER_DAY)).abs()
                    < SLACK_SECONDS,
                "case {case:?}: expected a DeletionDate {days} days past the request \
                 {requested_at}, got {deletion_date}"
            );

            // Both reporting operations carry the same stamp, and it is the
            // request instant — so the window is measurable as the span between
            // the two instants.
            for (reporter, reported) in [
                ("DescribeSecret", describe(&router, name, &case).await),
                ("ListSecrets", listed(&router, name, &case).await),
            ] {
                let marked = instant(&reported, "DeletedDate", &case);
                assert!(
                    (marked - requested_at).abs() < SLACK_SECONDS,
                    "case {case:?}: expected {reporter} to report DeletedDate at the request \
                     instant {requested_at}, got {marked}"
                );
                let measured = ((deletion_date - marked) / SECONDS_PER_DAY).round() as i64;
                assert_eq!(
                    measured, days,
                    "case {case:?}: expected DeletionDate - {reporter}.DeletedDate to measure \
                     {days} days, got {measured} ({reported})"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_live_secret_carries_neither_stamp_and_a_restore_clears_both() {
    // A secret no deletion is scheduled for reports no DeletedDate at all, and
    // cancelling a scheduled deletion puts it back in that state whatever window
    // scheduled it: RestoreSecret "cancels the scheduled deletion of a secret by
    // removing the DeletedDate time stamp" (this service's Smithy model).
    for (window_case, window, _days) in WINDOW_ROWS {
        let router = emulator();
        let name = "restore/subject";
        create(&router, name, window_case).await;
        let live = describe(&router, name, window_case).await;
        assert!(
            live.get("DeletedDate").is_none(),
            "case {window_case:?}: a live secret must carry no DeletedDate, got {live}"
        );

        let mut body = json!({"SecretId": name});
        if let Some(window) = window {
            body["RecoveryWindowInDays"] = json!(window);
        }
        ok(&router, "DeleteSecret", body, window_case).await;
        let scheduled = describe(&router, name, window_case).await;
        assert!(
            scheduled.get("DeletedDate").is_some(),
            "case {window_case:?}: the scheduled secret must carry a DeletedDate, got {scheduled}"
        );

        ok(
            &router,
            "RestoreSecret",
            json!({"SecretId": name}),
            window_case,
        )
        .await;
        let restored = describe(&router, name, window_case).await;
        assert!(
            restored.get("DeletedDate").is_none(),
            "case {window_case:?}: the restored secret must carry no DeletedDate, got {restored}"
        );
        // And the value is served again, so the restore cleared the schedule
        // rather than only the stamp it reports.
        let fetched = ok(
            &router,
            "GetSecretValue",
            json!({"SecretId": name}),
            window_case,
        )
        .await;
        assert_eq!(
            text(&fetched, "SecretString", window_case),
            "stored",
            "case {window_case:?}: expected the restored secret's value back, got {fetched}"
        );
    }
}

#[tokio::test]
async fn a_second_delete_re_states_the_schedule_from_its_own_request() {
    // The window is measured from the delete request that asks for it, so a
    // delete of a secret a deletion is already scheduled for re-states both
    // stamps. The rows pair each documented window with a DIFFERENT one, so the
    // second schedule is read apart from the first in both directions.
    for (first_case, first_window, first_days) in WINDOW_ROWS {
        for (second_case, second_window, second_days) in WINDOW_ROWS {
            if first_days == second_days {
                continue;
            }
            let router = emulator();
            let name = "reschedule/subject";
            let case = format!("{first_case}, then {second_case}");
            create(&router, name, &case).await;

            let mut first_body = json!({"SecretId": name});
            if let Some(window) = first_window {
                first_body["RecoveryWindowInDays"] = json!(window);
            }
            let first = ok(&router, "DeleteSecret", first_body, &case).await;
            let first_scheduled = instant(&first, "DeletionDate", &case);
            let first_marked = instant(&describe(&router, name, &case).await, "DeletedDate", &case);
            assert_eq!(
                ((first_scheduled - first_marked) / SECONDS_PER_DAY).round() as i64,
                first_days,
                "case {case:?}: expected the first schedule to measure {first_days} days, got \
                 {first_scheduled} against {first_marked}"
            );

            let mut second_body = json!({"SecretId": name});
            if let Some(window) = second_window {
                second_body["RecoveryWindowInDays"] = json!(window);
            }
            let requested_at = now_seconds();
            let second = ok(&router, "DeleteSecret", second_body, &case).await;
            let second_scheduled = instant(&second, "DeletionDate", &case);
            let second_marked = instant(&describe(&router, name, &case).await, "DeletedDate", &case);
            assert!(
                (second_marked - requested_at).abs() < SLACK_SECONDS,
                "case {case:?}: expected the second delete to re-stamp DeletedDate at its own \
                 request {requested_at}, got {second_marked}"
            );
            assert_eq!(
                ((second_scheduled - second_marked) / SECONDS_PER_DAY).round() as i64,
                second_days,
                "case {case:?}: expected the second schedule to measure {second_days} days, got \
                 {second_scheduled} against {second_marked}"
            );
            assert!(
                (second_scheduled - first_scheduled).abs() > SLACK_SECONDS,
                "case {case:?}: expected the DeletionDate to move off the first schedule \
                 {first_scheduled}, got {second_scheduled}"
            );
            // The re-scheduled secret is still restorable, so the second delete
            // replaced the schedule rather than finishing it.
            ok(&router, "RestoreSecret", json!({"SecretId": name}), &case).await;
            let restored = describe(&router, name, &case).await;
            assert!(
                restored.get("DeletedDate").is_none(),
                "case {case:?}: expected the re-scheduled deletion to be cancellable, got \
                 {restored}"
            );
        }
    }
}

#[tokio::test]
async fn a_forced_delete_overrides_a_pending_window() {
    // "If you forcibly delete an already deleted or nonexistent secret, the
    // operation does not return ResourceNotFoundException."
    // (`DeleteSecretRequest$ForceDeleteWithoutRecovery`, this service's Smithy
    // model.) A force delete carries no window, so the deletion it reports falls
    // due as of its own call — past the instant after which the secret "can no
    // longer be restored" (`DeleteSecretResponse$DeletionDate`) — whatever window
    // was pending on the secret.
    for (window_case, window, _days) in WINDOW_ROWS {
        let router = emulator();
        let name = "forced/subject";
        create(&router, name, window_case).await;
        let mut body = json!({"SecretId": name});
        if let Some(window) = window {
            body["RecoveryWindowInDays"] = json!(window);
        }
        ok(&router, "DeleteSecret", body, window_case).await;

        let started = Instant::now();
        let requested_at = now_seconds();
        let forced = ok(
            &router,
            "DeleteSecret",
            json!({"SecretId": name, "ForceDeleteWithoutRecovery": true}),
            window_case,
        )
        .await;
        let forced_date = instant(&forced, "DeletionDate", window_case);
        assert!(
            (forced_date - requested_at).abs() < SLACK_SECONDS,
            "case {window_case:?}: expected the forced delete to fall due at its own call \
             {requested_at}, got {forced_date}"
        );

        // The pending window is gone: the secret is past restoring from this
        // call. Only asserted while the asynchronous removal span cannot yet have
        // elapsed, so a loaded machine never turns this into a false failure.
        if started.elapsed() < FORCE_DELETE_PURGE_DELAY {
            let (status, answer) =
                call(&router, "RestoreSecret", &json!({"SecretId": name})).await;
            assert_eq!(
                status, 400,
                "case {window_case:?}: expected the forced delete to be past restoring, got \
                 {status} with body {answer}"
            );
        }

        // And once the span the forced delete owes has elapsed the secret is
        // gone, rather than waiting out the window it replaced.
        tokio::time::sleep(FORCE_DELETE_PURGE_DELAY + Duration::from_millis(200)).await;
        let (status, answer) = call(&router, "DescribeSecret", &json!({"SecretId": name})).await;
        assert_eq!(
            status, 400,
            "case {window_case:?}: expected the secret to be gone once the forced delete was \
             materialised, got {status} with body {answer}"
        );
    }
}
