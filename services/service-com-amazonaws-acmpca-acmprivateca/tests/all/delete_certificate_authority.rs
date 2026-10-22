// SPDX-License-Identifier: CC0-1.0

//! `DeleteCertificateAuthority`: which statuses a private CA can be deleted from, the
//! restoration window the deletion opens, and what the end of that window means.
//!
//! The rules under test are the ones the operation documents: "A private CA can be
//! deleted if it is in the PENDING_CERTIFICATE, CREATING, EXPIRED, DISABLED, or FAILED
//! state"; "By default, if you do not set the PermanentDeletionTimeInDays parameter, the
//! CA remains restorable for 30 days. You can set the parameter from 7 to 30 days"; and
//! "A private CA deleted in the CREATING or FAILED state has no assigned restoration
//! period and cannot be restored."
//!
//! The wire harness is the one `private_ca_lifecycle` builds: the same router, the same
//! awsJson1_1 calls.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::ops::types::CERTIFICATE_AUTHORITY_STATUS_VALUES;
use service_com_amazonaws_acmpca_acmprivateca::state::{
    ONE_DAY, STATUS_ACTIVE, STATUS_CREATING, STATUS_DELETED, STATUS_DISABLED, STATUS_EXPIRED,
    STATUS_FAILED, STATUS_PENDING_CERTIFICATE,
};

use crate::private_ca_lifecycle::{
    root_ca_request, string, wait_for_the_clock, Emulator,
};

/// Seconds in a day, the unit `PermanentDeletionTimeInDays` counts in, as the crate
/// publishes it.
const DAY_SECONDS: f64 = ONE_DAY.as_secs() as f64;

/// A CA created through the wire, named after `label`.
async fn create_ca(emulator: &Emulator, case: &str, label: &str) -> String {
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &root_ca_request(label))
        .await;
    string(&created, "CertificateAuthorityArn", case)
}

/// The `CertificateAuthority` structure `DescribeCertificateAuthority` reports for `arn`.
async fn describe(emulator: &Emulator, arn: &str, case: &str) -> Value {
    let described = emulator
        .ok(
            case,
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    described
        .get("CertificateAuthority")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no CertificateAuthority in {described:?}"))
}

/// A modelled timestamp member as the epoch seconds awsJson1_1 carries it as.
fn seconds(value: &Value, member: &str, case: &str) -> f64 {
    value
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: no numeric {member:?} in {value:?}"))
}

/// What the store holds for `arn`: its status, the status a deletion recorded, and the
/// end of its restoration window.
fn stored(
    emulator: &Emulator,
    arn: &str,
    case: &str,
) -> (String, Option<String>, Option<SystemTime>) {
    let state = emulator.state();
    let state = state.lock();
    let record = state
        .certificate_authorities
        .get(arn)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no CA at {arn}"));
    (
        record.status.clone(),
        record.status_before_deletion.clone(),
        record.restorable_until,
    )
}

/// Put the stored CA into `status`.
///
/// Four of the seven declared statuses are reachable over no implemented operation yet
/// (`UpdateCertificateAuthority` is still scaffolded under its own issue, and this
/// emulator never fails or expires a CA of its own accord), so the table below reaches
/// them through the store the handlers read. The CA's due instant is pushed ahead at the
/// same time, so a `CREATING` row is not settled out from under the test by the
/// transition the clock would otherwise owe.
fn force_status(emulator: &Emulator, arn: &str, status: &str, case: &str) {
    let now = SystemTime::now();
    let state = emulator.state();
    let mut state = state.lock();
    let record = state
        .certificate_authorities
        .get_mut(arn)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no CA at {arn}"));
    record.status = status.to_string();
    record.settles_at = now + ONE_DAY;
    record.restorable_until = (status == STATUS_DELETED).then(|| now + ONE_DAY);
}

/// Every `CertificateAuthorityStatus` the model declares, against whether a CA in it can
/// be deleted and whether that deletion opens a restoration window.
#[tokio::test]
async fn delete_is_allowed_only_from_the_statuses_the_operation_documents() {
    let emulator = Emulator::new();

    // (status, deletable, opens a restoration window)
    let rows: &[(&str, bool, bool)] = &[
        (STATUS_CREATING, true, false),
        (STATUS_PENDING_CERTIFICATE, true, true),
        (STATUS_ACTIVE, false, false),
        (STATUS_DISABLED, true, true),
        (STATUS_EXPIRED, true, true),
        (STATUS_FAILED, true, false),
        (STATUS_DELETED, false, false),
    ];

    // The table spans the whole enum, so a member the model gains cannot go untested.
    let mut covered: Vec<&str> = rows.iter().map(|(status, _, _)| *status).collect();
    covered.sort_unstable();
    let mut declared: Vec<&str> = CERTIFICATE_AUTHORITY_STATUS_VALUES.to_vec();
    declared.sort_unstable();
    assert_eq!(
        covered, declared,
        "the table must span every CertificateAuthorityStatus the model declares, got \
         {covered:?} against {declared:?}"
    );

    for (index, (status, deletable, opens_window)) in rows.iter().enumerate() {
        let case = format!("delete from {status}");
        let arn = create_ca(&emulator, &case, &format!("delete-status-{index}")).await;
        force_status(&emulator, &arn, status, &case);

        if *deletable {
            emulator
                .ok(
                    &case,
                    "DeleteCertificateAuthority",
                    &json!({"CertificateAuthorityArn": arn}),
                )
                .await;
            let (stored_status, deleted_from, restorable_until) = stored(&emulator, &arn, &case);
            assert_eq!(
                stored_status, STATUS_DELETED,
                "case {case:?}: expected the CA to be left in DELETED, got {stored_status:?}"
            );
            assert_eq!(
                deleted_from.as_deref(),
                Some(*status),
                "case {case:?}: expected the deletion to record {status:?} as the \
                 pre-deletion status, got {deleted_from:?}"
            );
            assert_eq!(
                restorable_until.is_some(),
                *opens_window,
                "case {case:?}: expected a restoration window present={opens_window}, got \
                 {restorable_until:?}"
            );

            // The same thing the wire reports.
            let described = describe(&emulator, &arn, &case).await;
            assert_eq!(
                string(&described, "Status", &case),
                STATUS_DELETED,
                "case {case:?}: expected a described status of DELETED, got {described:?}"
            );
            assert_eq!(
                described.get("RestorableUntil").is_some(),
                *opens_window,
                "case {case:?}: expected RestorableUntil present={opens_window}, got \
                 {described:?}"
            );
        } else {
            emulator
                .refused(
                    &case,
                    "DeleteCertificateAuthority",
                    &json!({"CertificateAuthorityArn": arn}),
                    "InvalidStateException",
                )
                .await;
            let (stored_status, deleted_from, _) = stored(&emulator, &arn, &case);
            assert_eq!(
                stored_status, *status,
                "case {case:?}: a refused deletion must leave the status alone, got \
                 {stored_status:?}"
            );
            assert_eq!(
                deleted_from, None,
                "case {case:?}: a refused deletion must record no pre-deletion status, got \
                 {deleted_from:?}"
            );
        }
    }
}

/// The restoration window a deletion opens is the `PermanentDeletionTimeInDays` the
/// request asked for, counted in days from the deletion, and 30 days when the request
/// names no value at all.
///
/// The rows are both bounds the member publishes (`range 7..30`), a day inside the range,
/// and the absent member the default covers.
#[tokio::test]
async fn a_deletion_opens_the_restoration_window_the_request_asked_for() {
    let emulator = Emulator::new();

    let rows: &[(&str, Option<i32>, f64)] = &[
        ("no PermanentDeletionTimeInDays", None, 30.0),
        ("the lower bound", Some(7), 7.0),
        ("a day inside the range", Some(14), 14.0),
        ("the upper bound", Some(30), 30.0),
    ];

    // One CA per row, all created before the single wait for the clock that settles them
    // into PENDING_CERTIFICATE.
    let mut arns = Vec::with_capacity(rows.len());
    for (index, (case, _, _)) in rows.iter().enumerate() {
        arns.push(create_ca(&emulator, case, &format!("delete-window-{index}")).await);
    }
    wait_for_the_clock();

    for ((case, days, expected_days), arn) in rows.iter().zip(&arns) {
        let described = describe(&emulator, arn, case).await;
        assert_eq!(
            string(&described, "Status", case),
            STATUS_PENDING_CERTIFICATE,
            "case {case:?}: the CA must be PENDING_CERTIFICATE before it is deleted, got \
             {described:?}"
        );

        let mut body = json!({"CertificateAuthorityArn": arn});
        if let Some(days) = days {
            body["PermanentDeletionTimeInDays"] = json!(days);
        }
        emulator.ok(case, "DeleteCertificateAuthority", &body).await;

        let described = describe(&emulator, arn, case).await;
        assert_eq!(
            string(&described, "Status", case),
            STATUS_DELETED,
            "case {case:?}: expected a described status of DELETED, got {described:?}"
        );
        // The deletion is the CA's last state change, so the window is the span between
        // the two reported instants — no wall-clock reading of the test's own.
        let deleted_at = seconds(&described, "LastStateChangeAt", case);
        let restorable_until = seconds(&described, "RestorableUntil", case);
        let window_days = (restorable_until - deleted_at) / DAY_SECONDS;
        assert!(
            (window_days - expected_days).abs() < 0.001,
            "case {case:?}: expected a {expected_days}-day restoration window, got \
             {window_days} days (RestorableUntil {restorable_until}, LastStateChangeAt \
             {deleted_at})"
        );
    }
}

/// `PermanentDeletionTimeInDays` is modelled `range 7..30`, so a day count outside it is
/// refused before the handler runs and deletes nothing.
#[tokio::test]
async fn a_restoration_period_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();

    for (case, days) in [("one day below the range", 6), ("one day above it", 31)] {
        let arn = create_ca(&emulator, case, &format!("delete-range-{days}")).await;
        emulator
            .refused(
                case,
                "DeleteCertificateAuthority",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "PermanentDeletionTimeInDays": days,
                }),
                "ValidationException",
            )
            .await;
        let (status, deleted_from, _) = stored(&emulator, &arn, case);
        assert_ne!(
            status, STATUS_DELETED,
            "case {case:?}: a refused request must delete nothing, got {status:?}"
        );
        assert_eq!(
            deleted_from, None,
            "case {case:?}: a refused request must record no pre-deletion status, got \
             {deleted_from:?}"
        );

        // And it was the day count that was refused, not the CA: the same CA deletes once
        // the request asks for a period inside the range.
        emulator
            .ok(
                case,
                "DeleteCertificateAuthority",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "PermanentDeletionTimeInDays": 7,
                }),
            )
            .await;
        let (status, deleted_from, _) = stored(&emulator, &arn, case);
        assert_eq!(
            status, STATUS_DELETED,
            "case {case:?}: a day count inside the range deletes the CA, got {status:?}"
        );
        assert!(
            deleted_from.is_some(),
            "case {case:?}: the accepted deletion must record the pre-deletion status"
        );
    }
}

/// "the CA won't be permanently deleted until the restoration period has passed": once it
/// has, the CA is a resource no longer, and every call that names it says so.
#[tokio::test]
async fn a_deleted_ca_is_gone_once_its_restoration_window_has_closed() {
    let emulator = Emulator::new();
    let case = "a closed restoration window";

    let arn = create_ca(&emulator, case, "delete-window-closes").await;
    wait_for_the_clock();
    emulator
        .ok(
            case,
            "DeleteCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn, "PermanentDeletionTimeInDays": 7}),
        )
        .await;

    // While the window is open the CA still answers, as DELETED.
    let described = describe(&emulator, &arn, case).await;
    assert_eq!(
        string(&described, "Status", case),
        STATUS_DELETED,
        "case {case:?}: a CA inside its restoration window is DELETED, got {described:?}"
    );

    // Move the end of the window into the past: the permanent deletion was due the
    // instant the window closed, and waiting seven days for it is the one thing a test
    // cannot do.
    {
        let state = emulator.state();
        let mut state = state.lock();
        let record = state
            .certificate_authorities
            .get_mut(&arn)
            .unwrap_or_else(|| panic!("case {case:?}: the store holds no CA at {arn}"));
        record.restorable_until = Some(SystemTime::now() - Duration::from_secs(1));
    }

    emulator
        .refused(
            case,
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
            "ResourceNotFoundException",
        )
        .await;
    emulator
        .refused(
            case,
            "DeleteCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
            "ResourceNotFoundException",
        )
        .await;

    // Every reader agrees: the listing does not report it either.
    let listed = emulator
        .ok(case, "ListCertificateAuthorities", &json!({}))
        .await;
    let reported: Vec<String> = listed
        .get("CertificateAuthorities")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|authority| authority.get("Arn").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    assert!(
        !reported.contains(&arn),
        "case {case:?}: a permanently deleted CA must not be listed, got {reported:?}"
    );
    assert!(
        !emulator
            .state()
            .lock()
            .certificate_authorities
            .contains_key(&arn),
        "case {case:?}: a permanently deleted CA must be gone from the store"
    );
}
