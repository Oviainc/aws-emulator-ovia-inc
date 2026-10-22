// SPDX-License-Identifier: CC0-1.0

//! Deleting a private CA and restoring it, driven over the wire through the same router
//! the emulator boots.
//!
//! The rules under test:
//!
//! * `DeleteCertificateAuthority` moves a CA that is in a deletable status to DELETED and
//!   assigns it the restoration period the request asked for (30 days by default), except
//!   for a CA deleted in CREATING or FAILED, which gets none.
//! * `RestoreCertificateAuthority` undoes a pending deletion: a CA still inside its
//!   restoration period goes back to the status it held before the delete and stops
//!   reporting a restoration window; a CA with no pending deletion is left exactly as it
//!   is and the call still succeeds; a CA deleted with no assigned restoration period
//!   cannot be restored; a CA whose restoration period has run out is permanently deleted
//!   and so is found by nobody.
//!
//! `UpdateCertificateAuthority` is still a scaffold stub, so the statuses no implemented
//! operation can reach (DISABLED, EXPIRED, FAILED, and a CREATING CA that has not settled
//! yet) are seeded straight into the store the handlers share; every assertion then goes
//! over the wire.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::ops::types::CERTIFICATE_AUTHORITY_STATUS_VALUES;
use service_com_amazonaws_acmpca_acmprivateca::state::{
    CertificateAuthorityRecord, DEFAULT_PERMANENT_DELETION_TIME_IN_DAYS, ONE_DAY, STATUS_ACTIVE,
    STATUS_CREATING, STATUS_DELETED, STATUS_PENDING_CERTIFICATE,
};

use crate::private_ca_lifecycle::{active_ca, root_ca_request, string, wait_for_the_clock, Emulator};

/// How far a reported `RestorableUntil` may sit from the instant this test computes, in
/// seconds: the window is assigned from the server's clock, not the test's.
const CLOCK_SLACK_SECONDS: f64 = 300.0;

/// Create a CA and let its creation settle, so it sits in `PENDING_CERTIFICATE`.
async fn pending_ca(emulator: &Emulator, common_name: &str) -> String {
    let created = emulator
        .ok(
            common_name,
            "CreateCertificateAuthority",
            &root_ca_request(common_name),
        )
        .await;
    let arn = string(&created, "CertificateAuthorityArn", common_name);
    wait_for_the_clock();
    arn
}

/// Edit the stored record of `arn` the way an operation's handler would.
fn with_ca<R>(
    emulator: &Emulator,
    arn: &str,
    edit: impl FnOnce(&mut CertificateAuthorityRecord) -> R,
) -> R {
    let state = emulator.state();
    let mut state = state.lock();
    let record = state
        .certificate_authorities
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the certificate authority {arn:?} is in the store"));
    edit(record)
}

/// Put `arn` into `status` with no pending deletion, pushing the CREATING ->
/// PENDING_CERTIFICATE transition the clock owes out of reach so the seeded status is the
/// one a reader observes.
fn set_status(emulator: &Emulator, arn: &str, status: &str) {
    with_ca(emulator, arn, |record| {
        record.status = status.to_string();
        record.settles_at = SystemTime::now() + ONE_DAY;
        record.status_before_deletion = None;
        record.restorable_until = None;
    });
}

/// Move the end of `arn`'s restoration period to `ago` before now, which is how a window
/// that has run out is reached without waiting days for it.
fn expire_restoration_period(emulator: &Emulator, arn: &str, ago: Duration) {
    with_ca(emulator, arn, |record| {
        record.restorable_until = Some(SystemTime::now() - ago);
    });
}

/// The `CertificateAuthority` structure `DescribeCertificateAuthority` reports.
async fn described(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let answer = emulator
        .ok(
            case,
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    answer["CertificateAuthority"].clone()
}

/// A numeric member (an awsJson1_1 timestamp is epoch seconds), or a panic naming the
/// case.
fn number(value: &Value, member: &str, case: &str) -> f64 {
    value
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: no numeric {member:?} in {value:?}"))
}

/// Now, in epoch seconds.
fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A delete, with the restoration period the request names (none means the default).
async fn delete(emulator: &Emulator, case: &str, arn: &str, days: Option<i32>) -> Value {
    let mut body = json!({"CertificateAuthorityArn": arn});
    if let Some(days) = days {
        body["PermanentDeletionTimeInDays"] = json!(days);
    }
    emulator
        .ok(case, "DeleteCertificateAuthority", &body)
        .await
}

/// A deleted CA inside its restoration period goes back to the status it held before the
/// delete, and stops reporting a restoration window.
///
/// The rows differ along the status the CA was deleted from and along the length of the
/// restoration period, including both published bounds of
/// `PermanentDeletionTimeInDays` ("anywhere from 7 to 30 days, with 30 being the
/// default") and a request that names none at all.
#[tokio::test]
async fn a_deleted_ca_is_restored_to_the_status_it_held_before_the_delete() {
    let emulator = Emulator::new();

    let rows: &[(&str, &str, Option<i32>, i32)] = &[
        (
            "waiting for its certificate, no period named",
            STATUS_PENDING_CERTIFICATE,
            None,
            DEFAULT_PERMANENT_DELETION_TIME_IN_DAYS,
        ),
        (
            "waiting for its certificate, the shortest period",
            STATUS_PENDING_CERTIFICATE,
            Some(7),
            7,
        ),
        (
            "waiting for its certificate, the longest period",
            STATUS_PENDING_CERTIFICATE,
            Some(30),
            30,
        ),
        ("disabled, a mid-range period", "DISABLED", Some(14), 14),
        ("expired, the shortest period", "EXPIRED", Some(7), 7),
    ];

    for (case, deleted_from, days, window_days) in rows {
        let arn = pending_ca(&emulator, &format!("restore-{case}")).await;
        if *deleted_from != STATUS_PENDING_CERTIFICATE {
            set_status(&emulator, &arn, deleted_from);
        }

        delete(&emulator, case, &arn, *days).await;

        let deleted = described(&emulator, case, &arn).await;
        assert_eq!(
            string(&deleted, "Status", case),
            STATUS_DELETED,
            "case {case:?}: a deleted CA reads DELETED, got {deleted:?}"
        );
        let restorable_until = number(&deleted, "RestorableUntil", case);
        let expected = epoch_now() + f64::from(window_days * 86_400);
        assert!(
            (restorable_until - expected).abs() < CLOCK_SLACK_SECONDS,
            "case {case:?}: expected a restoration window ending near {expected} \
             ({window_days} days out), got {restorable_until}"
        );

        emulator
            .ok(
                case,
                "RestoreCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;

        let restored = described(&emulator, case, &arn).await;
        assert_eq!(
            &string(&restored, "Status", case),
            deleted_from,
            "case {case:?}: expected the restored CA to read {deleted_from}, got {restored:?}"
        );
        assert!(
            restored.get("RestorableUntil").is_none(),
            "case {case:?}: a restored CA is no longer deleted, so it reports no \
             RestorableUntil, got {restored:?}"
        );

        // And it is deletable again, from the status it was restored to.
        delete(&emulator, &format!("{case}: deleted again"), &arn, *days).await;
        let redeleted = described(&emulator, case, &arn).await;
        assert_eq!(
            string(&redeleted, "Status", case),
            STATUS_DELETED,
            "case {case:?}: a restored CA can be deleted again, got {redeleted:?}"
        );
    }
}

/// A restore of a CA with no pending deletion succeeds and changes nothing: there is no
/// deletion to undo, so neither the status nor the instant it last changed moves.
///
/// The rows are `CertificateAuthorityStatus`'s whole catalog
/// (`ops::types::CERTIFICATE_AUTHORITY_STATUS_VALUES`, the model's own member list) minus
/// DELETED, so a status added to the model cannot slip past this table.
#[tokio::test]
async fn a_restore_of_a_ca_with_no_pending_deletion_changes_nothing() {
    let emulator = Emulator::new();

    for status in CERTIFICATE_AUTHORITY_STATUS_VALUES
        .iter()
        .filter(|status| **status != STATUS_DELETED)
    {
        let case = format!("a CA in {status}");
        let arn = pending_ca(&emulator, &format!("no-op-{status}")).await;
        set_status(&emulator, &arn, status);

        let before = described(&emulator, &case, &arn).await;
        emulator
            .ok(
                &case,
                "RestoreCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;
        let after = described(&emulator, &case, &arn).await;

        assert_eq!(
            &string(&after, "Status", &case),
            status,
            "case {case:?}: the status is untouched, got {after:?}"
        );
        assert_eq!(
            number(&after, "LastStateChangeAt", &case),
            number(&before, "LastStateChangeAt", &case),
            "case {case:?}: nothing changed state, so LastStateChangeAt stands still, got \
             {after:?}"
        );
        assert!(
            after.get("RestorableUntil").is_none(),
            "case {case:?}: a CA that was never deleted reports no RestorableUntil, got \
             {after:?}"
        );
    }

    // The same, for the two statuses an implemented operation reaches on its own: a CA
    // whose creation has not settled yet, and one that has taken its certificate.
    let created = emulator
        .ok(
            "a freshly created CA",
            "CreateCertificateAuthority",
            &root_ca_request("no-op-fresh"),
        )
        .await;
    let creating = string(&created, "CertificateAuthorityArn", "a freshly created CA");
    emulator
        .ok(
            "a freshly created CA",
            "RestoreCertificateAuthority",
            &json!({"CertificateAuthorityArn": creating}),
        )
        .await;
    let fresh = described(&emulator, "a freshly created CA", &creating).await;
    let fresh_status = string(&fresh, "Status", "a freshly created CA");
    assert!(
        fresh_status == STATUS_CREATING || fresh_status == STATUS_PENDING_CERTIFICATE,
        "case {:?}: a created CA is CREATING until the clock settles it, got {fresh_status:?}",
        "a freshly created CA"
    );

    let (active, _) = active_ca(&emulator, "no-op-active").await;
    emulator
        .ok(
            "an active CA",
            "RestoreCertificateAuthority",
            &json!({"CertificateAuthorityArn": active}),
        )
        .await;
    assert_eq!(
        string(
            &described(&emulator, "an active CA", &active).await,
            "Status",
            "an active CA"
        ),
        STATUS_ACTIVE,
        "case {:?}: an active CA stays active across a restore",
        "an active CA"
    );
}

/// "A private CA deleted in the CREATING or FAILED state has no assigned restoration
/// period and cannot be restored." (`DeleteCertificateAuthority` docs): the delete
/// succeeds, the CA reports no restoration window, and the restore is refused.
#[tokio::test]
async fn a_ca_deleted_with_no_assigned_restoration_period_cannot_be_restored() {
    let emulator = Emulator::new();

    for status in [STATUS_CREATING, "FAILED"] {
        let case = format!("deleted in {status}");
        let arn = pending_ca(&emulator, &format!("no-period-{status}")).await;
        set_status(&emulator, &arn, status);

        // The period the request names makes no difference: this CA is assigned none.
        delete(&emulator, &case, &arn, Some(7)).await;

        let deleted = described(&emulator, &case, &arn).await;
        assert_eq!(
            string(&deleted, "Status", &case),
            STATUS_DELETED,
            "case {case:?}: the CA is deleted, got {deleted:?}"
        );
        assert!(
            deleted.get("RestorableUntil").is_none(),
            "case {case:?}: no restoration period is assigned, so none is reported, got \
             {deleted:?}"
        );

        emulator
            .refused(
                &case,
                "RestoreCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
                "InvalidStateException",
            )
            .await;

        let after = described(&emulator, &case, &arn).await;
        assert_eq!(
            string(&after, "Status", &case),
            STATUS_DELETED,
            "case {case:?}: the refused restore leaves the CA deleted, got {after:?}"
        );
    }
}

/// "You cannot restore a CA after the restoration period has ended"
/// (`RestoreCertificateAuthority` docs): past that instant the CA is permanently deleted,
/// so it is found by nobody — not by a restore and not by a read.
///
/// The rows walk the far side of the window: just past it, and well past it.
#[tokio::test]
async fn a_deleted_ca_past_its_restoration_period_is_gone_for_good() {
    let emulator = Emulator::new();

    let rows: &[(&str, Duration)] = &[
        ("the window ended a second ago", Duration::from_secs(1)),
        ("the window ended a day ago", ONE_DAY),
        (
            "the whole default window has run out",
            ONE_DAY * DEFAULT_PERMANENT_DELETION_TIME_IN_DAYS.unsigned_abs(),
        ),
    ];

    for (case, ago) in rows {
        let arn = pending_ca(&emulator, &format!("expired-{}", ago.as_secs())).await;
        delete(&emulator, case, &arn, Some(7)).await;
        expire_restoration_period(&emulator, &arn, *ago);

        emulator
            .refused(
                case,
                "RestoreCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
                "ResourceNotFoundException",
            )
            .await;

        // Permanently deleted, not merely unrestorable: the CA is gone for every reader.
        emulator
            .refused(
                &format!("{case}: and then described"),
                "DescribeCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
                "ResourceNotFoundException",
            )
            .await;
    }
}

// Deletion's own rules — which statuses it is allowed from, the window it opens and the
// refusal outside the modelled 7..30 range — are `DeleteCertificateAuthority`'s, and are
// covered by `tests/all/delete_certificate_authority.rs`. This file keeps to what a
// RESTORE does with the state a deletion left behind.
