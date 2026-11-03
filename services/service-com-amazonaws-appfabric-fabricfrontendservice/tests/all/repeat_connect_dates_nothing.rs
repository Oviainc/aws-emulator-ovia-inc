// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `ConnectAppAuthorization` does to the record it
//! names when there is nothing left to connect, driven over the wire (restJson1)
//! through the same router the emulator boots.
//!
//! RULE: a call dates an app authorization — the `updatedAt` its summary, its
//! `GetAppAuthorization` record and its listing all report — only when it
//! actually changes the stored record. The connect handshake of an `apiKey`
//! authorization changes it with the call (it moves to `Connected`), so a connect
//! of one that already holds that status has nothing to apply: it is served with
//! the record exactly as it stands, `updatedAt` included, and nothing records a
//! change either. The handshake of an `oauth2` authorization leaves its outcome
//! to the clock, so the call itself dates nothing — see
//! `app_authorization_connect_validation` for what that outcome is.
//!
//! Each test states that rule over a TABLE of inputs: every value of the model's
//! `AppAuthorizationStatus` enum as the starting state, both `AuthType` values,
//! both modelled identifier forms, and repeat counts beyond the single retry.
//!
//! The harness (the router, `Answer`, `encode`, `uuid_of`, the app-bundle calls)
//! is the one `app_bundle_ingestion_lifecycle` already builds, and the
//! app-authorization calls are `app_authorization_deletion`'s.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state::service_state;

use crate::app_authorization_deletion::{
    as_form, authorization_arn, connect_authorization, create_authorization, get_authorization,
    Form,
};
use crate::app_bundle_ingestion_lifecycle::{
    arn_of, audit_capture, encode, uuid_of, Answer, Emulator,
};

/// Long enough that a write would land on a different instant than the one
/// before it, so a re-stamp cannot hide inside the clock's resolution.
const APART: Duration = Duration::from_millis(20);

/// Every value of the model's `AppAuthorizationStatus` enum
/// (`smithy_query(query="shape", shape="AppAuthorizationStatus")`): the state a
/// create leaves a record in, the one the connect handshake reaches, and the two
/// post-handshake failures the model documents as reconnectable.
const APP_AUTHORIZATION_STATUSES: &[&str] = &[
    "PendingConnect",
    "Connected",
    "ConnectionValidationFailed",
    "TokenAutoRotationFailed",
];

/// The two values the model's `AuthType` enum carries, which decide whether a
/// connect has to carry an `authRequest`.
const AUTH_TYPES: &[&str] = &["apiKey", "oauth2"];

/// What a connect of an authorization of this `authType` leaves the record
/// reading, given the `held` status it found: the handshake of an `apiKey`
/// authorization completes with the call, while an `oauth2` one has its outcome
/// owed to the clock, so the record still reads what it held when the call
/// arrived.
fn status_after_connect<'a>(auth_type: &str, held: &'a str) -> &'a str {
    if auth_type == "apiKey" {
        "Connected"
    } else {
        held
    }
}

/// Applications from the list the model's own documentation of `app` gives
/// (`smithy_query(query="doc", shape="CreateAppAuthorizationRequest",
/// member="app")`: "SLACK ASANA JIRA M365 M365AUDITLOGS ZOOM ZENDESK OKTA GOOGLE
/// DROPBOX SMARTSHEET CISCO"), walked by the tables below so no row depends on
/// which application an authorization carries.
const APPS: &[&str] = &["M365", "SMARTSHEET", "ASANA", "JIRA", "DROPBOX", "ZENDESK"];

/// An app bundle to hang authorizations off, as its ARN.
async fn app_bundle(emulator: &Emulator) -> String {
    let created = emulator.create_app_bundle(json!({})).await;
    assert_eq!(
        created.status, 201,
        "the set-up app bundle must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    arn_of(&created.body, "appBundle")
}

/// One app authorization under `bundle`, as its ARN.
async fn authorization(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    auth_type: &str,
    tenant: &str,
    case: &str,
) -> String {
    let created = create_authorization(emulator, bundle, app, auth_type, tenant).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: the set-up app authorization must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    authorization_arn(&created, case)
}

/// Write one `AppAuthorizationStatus` straight into the SAME store the handlers
/// read, because two of the four values are states no implemented operation
/// reaches — so the set-up is the real record rather than a double of one. Only
/// the status is written: dating the record is what is under test.
fn set_status(
    engine: &engine::InMemoryEngine,
    bundle: &str,
    arn: &str,
    status: &str,
    case: &str,
) {
    let store = service_state(engine).unwrap_or_else(|| panic!("case {case:?}: no appfabric store"));
    let mut store = store.lock();
    let record = store
        .resolve_bundle_mut(bundle)
        .unwrap_or_else(|| panic!("case {case:?}: bundle {bundle} is not stored"))
        .resolve_app_authorization_mut(arn)
        .unwrap_or_else(|| panic!("case {case:?}: authorization {arn} is not stored"));
    record.status = status.to_string();
}

/// The instant the STORED record carries as its last change, read out of the
/// store the handlers write. Read from the record rather than from the wire so
/// the comparison is on the instant itself, whichever way the timestamp is
/// rendered.
fn stored_updated_at(
    engine: &engine::InMemoryEngine,
    bundle: &str,
    arn: &str,
    case: &str,
) -> SystemTime {
    let store = service_state(engine).unwrap_or_else(|| panic!("case {case:?}: no appfabric store"));
    let mut store = store.lock();
    store
        .resolve_bundle_mut(bundle)
        .unwrap_or_else(|| panic!("case {case:?}: bundle {bundle} is not stored"))
        .resolve_app_authorization_mut(arn)
        .unwrap_or_else(|| panic!("case {case:?}: authorization {arn} is not stored"))
        .updated_at
}

/// The `appAuthorizationSummary` a connect answer reports.
fn summary(answer: &Answer, case: &str) -> Value {
    answer
        .body
        .get("appAuthorizationSummary")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected appAuthorizationSummary in {}",
                answer.body
            )
        })
        .clone()
}

/// The `appAuthorization` a `GetAppAuthorization` answer reports.
async fn stored_record(emulator: &Emulator, bundle: &str, arn: &str, case: &str) -> Value {
    let answer = get_authorization(emulator, bundle, arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetAppAuthorization must answer HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("appAuthorization")
        .unwrap_or_else(|| panic!("case {case:?}: expected appAuthorization in {}", answer.body))
        .clone()
}

/// The summary `ListAppAuthorizations` reports for one authorization of a bundle.
async fn listed_summary(emulator: &Emulator, bundle: &str, arn: &str, case: &str) -> Value {
    let answer = emulator
        .call(
            "GET",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: ListAppAuthorizations must answer HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("appAuthorizationSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected appAuthorizationSummaryList in {}",
                answer.body
            )
        })
        .iter()
        .find(|candidate| {
            candidate.get("appAuthorizationArn").and_then(Value::as_str) == Some(arn)
        })
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary for {arn} in {}", answer.body))
        .clone()
}

/// A string member of a record, or `""`.
fn member(record: &Value, name: &str) -> String {
    record
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// How many audit records name this authorization as changed by a connect.
fn connect_audit_records(id: &str) -> usize {
    audit_capture()
        .audit_records()
        .iter()
        .filter(|rendered| {
            rendered.contains("operation=ConnectAppAuthorization")
                && rendered.contains(&format!("resource_id={id}"))
        })
        .count()
}

/// RULE: a connect dates the app authorization only when it actually moves its
/// status to `Connected`; one that finds it already there changes nothing, so the
/// stored record — `updatedAt` and every other member — is exactly what it was
/// before the call, and that is what the call reports.
///
/// The table is every value of the model's `AppAuthorizationStatus` enum as the
/// starting state (including the two post-handshake failures the issue did not
/// cite), crossed with both `AuthType` values and both modelled identifier forms.
#[tokio::test]
async fn a_connect_dates_the_authorization_only_when_it_moves_its_status() {
    for (index, status) in APP_AUTHORIZATION_STATUSES.iter().enumerate() {
        for auth_type in AUTH_TYPES {
            for form in [Form::Arn, Form::Uuid] {
                let app = APPS[index % APPS.len()];
                let case = format!("{app} {auth_type} at {status}, named by {form:?}");
                let engine = Arc::new(engine::InMemoryEngine::new());
                let emulator = Emulator::with_engine(&engine);
                let bundle = app_bundle(&emulator).await;
                let arn =
                    authorization(&emulator, &bundle, app, auth_type, "tenant-a", &case).await;
                set_status(&engine, &bundle, &arn, status, &case);

                let before = stored_record(&emulator, &bundle, &arn, &case).await;
                let dated_before = stored_updated_at(&engine, &bundle, &arn, &case);
                assert_eq!(
                    member(&before, "status"),
                    *status,
                    "case {case:?}: the set-up status must be stored, got {before}"
                );

                // Any write this call makes lands at a later instant than the
                // record already carries, so a re-stamp would be visible.
                tokio::time::sleep(APART).await;
                let connected = connect_authorization(
                    &emulator,
                    &as_form(&bundle, form),
                    &as_form(&arn, form),
                    auth_type,
                )
                .await;
                assert_eq!(
                    connected.status, 200,
                    "case {case:?}: expected HTTP 200 from ConnectAppAuthorization, got {} with body {}",
                    connected.status, connected.body
                );
                let reported = summary(&connected, &case);
                let expected_status = status_after_connect(auth_type, status);
                assert_eq!(
                    member(&reported, "status"),
                    expected_status,
                    "case {case:?}: the summary must report {expected_status}, got {reported}"
                );

                let after = stored_record(&emulator, &bundle, &arn, &case).await;
                let dated_after = stored_updated_at(&engine, &bundle, &arn, &case);
                let moved = expected_status != *status;
                if moved {
                    assert!(
                        dated_after > dated_before,
                        "case {case:?}: a connect that moves the status must date the record, got \
                         {dated_after:?} after {dated_before:?}"
                    );
                } else {
                    assert_eq!(
                        dated_after, dated_before,
                        "case {case:?}: a connect with nothing to apply must leave updatedAt as it \
                         stands, got {dated_after:?} after {dated_before:?}"
                    );
                    assert_eq!(
                        after, before,
                        "case {case:?}: a connect with nothing to apply must change nothing, \
                         expected {before}, got {after}"
                    );
                }

                // What the call reported is what a later read reports: the
                // summary's members are the record's.
                assert_eq!(
                    reported.get("updatedAt"),
                    after.get("updatedAt"),
                    "case {case:?}: the summary must report the stored updatedAt {after}, got \
                     {reported}"
                );
                assert_eq!(
                    listed_summary(&emulator, &bundle, &arn, &case).await,
                    reported,
                    "case {case:?}: the listing must report the summary the connect reported, got \
                     {reported}"
                );
                assert_eq!(
                    after.get("createdAt"),
                    before.get("createdAt"),
                    "case {case:?}: createdAt must not change, got {after}"
                );
            }
        }
    }
}

/// RULE (the same one, over the retries a caller actually makes): the connect
/// call carries no idempotency token, so a client may re-send it any number of
/// times; every send after the one that changed the record finds nothing to
/// apply, so each is answered with the SAME summary, the stored record keeps the
/// date of the connect that moved it, and only that one call recorded a change.
///
/// The table spans retry counts beyond the single repeat, both `AuthType` values
/// and applications the issue did not cite.
#[tokio::test]
async fn repeated_connects_are_answered_with_the_same_summary() {
    // (case, how many times the connect is sent in all)
    let repeats: &[(&str, usize)] = &[("sent twice", 2), ("sent three times", 3), ("sent five times", 5)];

    for (label, sends) in repeats {
        for auth_type in AUTH_TYPES {
            for app in [APPS[0], APPS[2]] {
                let case = format!("{app} {auth_type} {label}");
                let engine = Arc::new(engine::InMemoryEngine::new());
                let emulator = Emulator::with_engine(&engine);
                let bundle = app_bundle(&emulator).await;
                let arn =
                    authorization(&emulator, &bundle, app, auth_type, "tenant-b", &case).await;
                let id = uuid_of(&arn);
                let audited_before = connect_audit_records(&id);

                let first = connect_authorization(&emulator, &bundle, &arn, auth_type).await;
                assert_eq!(
                    first.status, 200,
                    "case {case:?}: the first connect must answer HTTP 200, got {} with body {}",
                    first.status, first.body
                );
                let expected = summary(&first, &case);
                let dated = stored_updated_at(&engine, &bundle, &arn, &case);

                for send in 2..=*sends {
                    tokio::time::sleep(APART).await;
                    let again = connect_authorization(&emulator, &bundle, &arn, auth_type).await;
                    assert_eq!(
                        again.status, 200,
                        "case {case:?}: connect {send} must answer HTTP 200, got {} with body {}",
                        again.status, again.body
                    );
                    assert_eq!(
                        summary(&again, &case),
                        expected,
                        "case {case:?}: connect {send} must report the same summary {expected}, \
                         got {}",
                        again.body
                    );
                    assert_eq!(
                        stored_updated_at(&engine, &bundle, &arn, &case),
                        dated,
                        "case {case:?}: connect {send} must leave updatedAt as the first connect \
                         wrote it, got {:?} after {dated:?}",
                        stored_updated_at(&engine, &bundle, &arn, &case)
                    );
                }

                // The record every reader sees is still the one the first
                // connect wrote.
                let record = stored_record(&emulator, &bundle, &arn, &case).await;
                let expected_status = status_after_connect(auth_type, "PendingConnect");
                assert_eq!(
                    member(&record, "status"),
                    expected_status,
                    "case {case:?}: the authorization must read {expected_status}, got {record}"
                );
                assert_eq!(
                    record.get("updatedAt"),
                    expected.get("updatedAt"),
                    "case {case:?}: GetAppAuthorization must report the connect's updatedAt \
                     {expected}, got {record}"
                );
                assert_eq!(
                    listed_summary(&emulator, &bundle, &arn, &case).await,
                    expected,
                    "case {case:?}: the listing must report the first connect's summary \
                     {expected}, got a different one"
                );
                assert_eq!(
                    connect_audit_records(&id) - audited_before,
                    1,
                    "case {case:?}: only the connect that changed the record may record a change, \
                     got {} records for {id}",
                    connect_audit_records(&id) - audited_before
                );
            }
        }
    }
}
