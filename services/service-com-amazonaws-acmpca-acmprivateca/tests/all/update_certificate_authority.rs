// SPDX-License-Identifier: CC0-1.0

//! `UpdateCertificateAuthority`: what a client may change about a private CA, and from
//! which statuses, driven over the wire through the same router the emulator boots.
//!
//! The rules under test:
//!
//! * a CA's status or configuration is updatable only while the CA is in one of the two
//!   statuses the operation's documented transitions run between (ACTIVE and DISABLED);
//!   from every other status the call is refused with `InvalidStateException` and the one
//!   sentence that class carries, and the CA is left exactly as it was;
//! * from an allowed status the requested status is persisted at once, a request for the
//!   status the CA already holds moves nothing, and a supplied `RevocationConfiguration`
//!   REPLACES the one the CA held while an absent one leaves it unchanged;
//! * the statuses a caller may request are those same two, since disabling an ACTIVE CA
//!   and activating a DISABLED one are the whole of what this call performs; every other
//!   requested status, a request naming nothing to update, and a revocation configuration
//!   breaking the rules that member documents are all refused with
//!   `InvalidArgsException`;
//! * an ARN is judged before any CA is read, so an ARN this service resolves no CA from is
//!   `InvalidArnException` while a well-formed ARN of a CA that is not there is
//!   `ResourceNotFoundException`;
//! * a success answers the modelled `Unit` output: HTTP 200 carrying no members.
//!
//! The harness (the engine, the router, the awsJson1_1 call) is
//! [`crate::private_ca_lifecycle::Emulator`].

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::ops::types::CERTIFICATE_AUTHORITY_STATUS_VALUES;
use service_com_amazonaws_acmpca_acmprivateca::state::{
    DELETED_INVALID_STATE_MESSAGE, DELETE_INVALID_STATE_MESSAGE, STATUS_ACTIVE, STATUS_DELETED,
    STATUS_DISABLED, UPDATABLE_STATUSES, UPDATE_INVALID_STATE_MESSAGE,
};

use crate::delete_certificate_authority::force_status;
use crate::private_ca_lifecycle::{active_ca, root_ca_request, string, Emulator};
use crate::resource_policy::error_message;

/// The `RevocationConfiguration` a CA created without one reports: both mechanisms
/// disabled.
fn no_revocation() -> Value {
    json!({"CrlConfiguration": {"Enabled": false}, "OcspConfiguration": {"Enabled": false}})
}

/// A revocation configuration that satisfies every rule the member documents, so a refusal
/// of a request carrying it is about something other than the configuration.
fn disabled_revocation() -> Value {
    json!({"CrlConfiguration": {"Enabled": false}})
}

/// The request body an update carries.
fn update_request(arn: &str, status: Option<&str>, revocation: Option<Value>) -> Value {
    let mut body = json!({"CertificateAuthorityArn": arn});
    if let Some(status) = status {
        body["Status"] = json!(status);
    }
    if let Some(revocation) = revocation {
        body["RevocationConfiguration"] = revocation;
    }
    body
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

/// What the store holds for `arn`: its status and the instant it last changed status.
///
/// `LastStateChangeAt` is reported in whole seconds, so the instant is read off the record
/// the handlers share to tell "the status moved" from "nothing moved" inside one second.
fn stored(emulator: &Emulator, arn: &str, case: &str) -> (String, SystemTime) {
    let state = emulator.state();
    let state = state.lock();
    let record = state
        .certificate_authorities
        .get(arn)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no CA at {arn}"));
    (record.status.clone(), record.last_state_change_at)
}

/// Call the operation, requiring the modelled empty success.
async fn updated(emulator: &Emulator, case: &str, body: &Value) {
    let (status, answer) = emulator.call("UpdateCertificateAuthority", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the update to answer 200, got {status} {answer:?}"
    );
    assert!(
        answer.is_null() || answer.as_object().is_some_and(|members| members.is_empty()),
        "case {case:?}: the modelled output is Unit, so a success carries no members, got \
         {answer:?}"
    );
}

/// Create a CA and bring it to `status` through the implemented path where there is one:
/// a CA reaches ACTIVE by importing its certificate and DISABLED by this very operation;
/// the statuses no implemented operation moves a CA into are seeded into the store the
/// handlers read.
async fn ca_in_status(emulator: &Emulator, label: &str, status: &str) -> String {
    let (arn, _) = active_ca(emulator, label).await;
    match status {
        STATUS_ACTIVE => {}
        STATUS_DISABLED => {
            updated(
                emulator,
                &format!("{label}: disabled"),
                &update_request(&arn, Some(STATUS_DISABLED), None),
            )
            .await;
        }
        other => force_status(emulator, &arn, other, label),
    }
    arn
}

/// The statuses this operation is NOT allowed from: every `CertificateAuthorityStatus` the
/// model declares (`ops::types::CERTIFICATE_AUTHORITY_STATUS_VALUES`) outside the two the
/// operation documents, so a member added to the model is covered without editing a list.
fn statuses_outside_the_gate() -> Vec<&'static str> {
    CERTIFICATE_AUTHORITY_STATUS_VALUES
        .iter()
        .copied()
        .filter(|status| !UPDATABLE_STATUSES.contains(status))
        .collect()
}

/// "Your private CA must be in the ACTIVE or DISABLED state before you can update it."
///
/// The rows cross every status outside that pair with each kind of change a request can
/// ask for — a status, a revocation configuration, and both at once — so the gate is shown
/// to cover the whole call rather than the status member alone.
#[tokio::test]
async fn an_update_is_refused_from_every_status_outside_active_or_disabled() {
    let emulator = Emulator::new();

    let outside = statuses_outside_the_gate();
    assert_eq!(
        outside.len(),
        CERTIFICATE_AUTHORITY_STATUS_VALUES.len() - UPDATABLE_STATUSES.len(),
        "the rows must span every declared status outside the gate, got {outside:?} against \
         {CERTIFICATE_AUTHORITY_STATUS_VALUES:?}"
    );

    // (what the request asks to change)
    let changes: [(&str, Option<&str>, Option<Value>); 4] = [
        ("a status of DISABLED", Some(STATUS_DISABLED), None),
        ("a status of ACTIVE", Some(STATUS_ACTIVE), None),
        (
            "a revocation configuration alone",
            None,
            Some(disabled_revocation()),
        ),
        (
            "both a status and a revocation configuration",
            Some(STATUS_DISABLED),
            Some(disabled_revocation()),
        ),
    ];

    for (index, status) in outside.iter().enumerate() {
        for (change, requested, revocation) in &changes {
            let case = format!("{change} against a CA in {status}");
            let created = emulator
                .ok(
                    &case,
                    "CreateCertificateAuthority",
                    &root_ca_request(&format!("gate-{index}-{change}")),
                )
                .await;
            let arn = string(&created, "CertificateAuthorityArn", &case);
            force_status(&emulator, &arn, status, &case);

            let answer = emulator
                .refused(
                    &case,
                    "UpdateCertificateAuthority",
                    &update_request(&arn, *requested, revocation.clone()),
                    "InvalidStateException",
                )
                .await;
            let message = error_message(&answer, &case);
            assert_eq!(
                message, UPDATE_INVALID_STATE_MESSAGE,
                "case {case:?}: expected {UPDATE_INVALID_STATE_MESSAGE:?}, got {message:?}"
            );
            // The sibling refusal under the same error code keeps its own sentence: a
            // caller tells the two apart by the text.
            assert_ne!(
                message, DELETED_INVALID_STATE_MESSAGE,
                "case {case:?}: the update refusal must not borrow the restore sentence"
            );

            // The refused call wrote nothing: neither the status nor the configuration
            // moved.
            let described = describe(&emulator, &arn, &case).await;
            assert_eq!(
                &string(&described, "Status", &case),
                status,
                "case {case:?}: a refused update leaves the status as it was, got {described:?}"
            );
            assert_eq!(
                described.get("RevocationConfiguration"),
                Some(&no_revocation()),
                "case {case:?}: a refused update leaves the revocation configuration as it \
                 was, got {described:?}"
            );
        }
    }
}

/// From a status the operation is allowed in, the requested status is persisted at once —
/// and a request for the status the CA already holds moves nothing at all.
///
/// The rows are every ordered pair over the two statuses an update runs between, so both
/// documented transitions and both no-ops are covered.
#[tokio::test]
async fn a_ca_in_an_updatable_status_takes_the_requested_status() {
    let emulator = Emulator::new();

    for (index, from) in UPDATABLE_STATUSES.iter().enumerate() {
        for (offset, to) in UPDATABLE_STATUSES.iter().enumerate() {
            let case = format!("{from} -> {to}");
            let arn = ca_in_status(&emulator, &format!("move-{index}-{offset}"), from).await;
            let (before_status, before_change) = stored(&emulator, &arn, &case);
            assert_eq!(
                &before_status, from,
                "case {case:?}: the row needs a CA in {from}, got {before_status:?}"
            );

            updated(&emulator, &case, &update_request(&arn, Some(to), None)).await;

            let described = describe(&emulator, &arn, &case).await;
            assert_eq!(
                &string(&described, "Status", &case),
                to,
                "case {case:?}: expected the CA to read {to}, got {described:?}"
            );
            let (after_status, after_change) = stored(&emulator, &arn, &case);
            assert_eq!(
                &after_status, to,
                "case {case:?}: expected the persisted status to be {to}, got {after_status:?}"
            );
            if from == to {
                assert_eq!(
                    after_change, before_change,
                    "case {case:?}: nothing changed state, so the instant it last changed \
                     stands still, got {after_change:?} against {before_change:?}"
                );
            } else {
                assert!(
                    after_change > before_change,
                    "case {case:?}: the status moved, so the instant it last changed moves \
                     with it, got {after_change:?} against {before_change:?}"
                );
            }
        }
    }
}

/// "You can disable a private CA that is in the ACTIVE state": disabling is the documented
/// precondition for deleting an ACTIVE CA, so the delete that was refused before the update
/// succeeds after it.
#[tokio::test]
async fn disabling_an_active_ca_is_what_lets_it_be_deleted() {
    let emulator = Emulator::new();
    let case = "an active CA is disabled and then deleted";
    let (arn, _) = active_ca(&emulator, "disable-then-delete").await;

    let refused = emulator
        .refused(
            case,
            "DeleteCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
            "InvalidStateException",
        )
        .await;
    assert_eq!(
        error_message(&refused, case),
        DELETE_INVALID_STATE_MESSAGE,
        "case {case:?}: an ACTIVE CA is refused a delete about the statuses a delete is \
         allowed from"
    );

    updated(
        &emulator,
        case,
        &update_request(&arn, Some(STATUS_DISABLED), None),
    )
    .await;
    let disabled = describe(&emulator, &arn, case).await;
    assert_eq!(
        string(&disabled, "Status", case),
        STATUS_DISABLED,
        "case {case:?}: expected the disabled CA to read DISABLED, got {disabled:?}"
    );

    emulator
        .ok(
            case,
            "DeleteCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn, "PermanentDeletionTimeInDays": 7}),
        )
        .await;
    let deleted = describe(&emulator, &arn, case).await;
    assert_eq!(
        string(&deleted, "Status", case),
        STATUS_DELETED,
        "case {case:?}: expected the deleted CA to read DELETED, got {deleted:?}"
    );
}

/// A supplied `RevocationConfiguration` states which mechanisms the CA supports from now
/// on, so it REPLACES the one the CA held; an absent one leaves "existing capibilites
/// unchanged".
///
/// The rows are applied to one CA in sequence, so each configuration is also read back
/// against the one before it: the mechanisms a row does not name come back disabled rather
/// than carried over. They differ along which mechanism is enabled, how many are, and
/// which optional members the enabled one carries.
#[tokio::test]
async fn a_supplied_revocation_configuration_replaces_the_one_the_ca_held() {
    let emulator = Emulator::new();
    let (arn, _) = active_ca(&emulator, "revocation-configuration").await;

    // (case, what the request supplies, what a read reports afterwards)
    let rows: &[(&str, Value, Value)] = &[
        (
            "both mechanisms named and both disabled",
            json!({"CrlConfiguration": {"Enabled": false}, "OcspConfiguration": {"Enabled": false}}),
            no_revocation(),
        ),
        (
            "CRLs enabled, OCSP not named",
            json!({"CrlConfiguration": {
                "Enabled": true,
                "S3BucketName": "example-crl-bucket",
                "ExpirationInDays": 7,
            }}),
            json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "S3BucketName": "example-crl-bucket",
                    "ExpirationInDays": 7,
                },
                "OcspConfiguration": {"Enabled": false},
            }),
        ),
        (
            "OCSP enabled with a custom responder, CRLs not named",
            json!({"OcspConfiguration": {
                "Enabled": true,
                "OcspCustomCname": "ocsp.example.com",
            }}),
            json!({
                "CrlConfiguration": {"Enabled": false},
                "OcspConfiguration": {"Enabled": true, "OcspCustomCname": "ocsp.example.com"},
            }),
        ),
        (
            "both mechanisms enabled, the CRL carrying every optional member it may",
            json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "S3BucketName": "other-crl-bucket",
                    "ExpirationInDays": 30,
                    "CustomCname": "crl.example.com",
                    "S3ObjectAcl": "BUCKET_OWNER_FULL_CONTROL",
                },
                "OcspConfiguration": {"Enabled": true},
            }),
            json!({
                "CrlConfiguration": {
                    "Enabled": true,
                    "S3BucketName": "other-crl-bucket",
                    "ExpirationInDays": 30,
                    "CustomCname": "crl.example.com",
                    "S3ObjectAcl": "BUCKET_OWNER_FULL_CONTROL",
                },
                "OcspConfiguration": {"Enabled": true},
            }),
        ),
        (
            "CRLs disabled again, which drops the OCSP support the row before enabled",
            json!({"CrlConfiguration": {"Enabled": false}}),
            no_revocation(),
        ),
    ];

    for (case, supplied, expected) in rows {
        updated(
            &emulator,
            case,
            &update_request(&arn, None, Some(supplied.clone())),
        )
        .await;
        let described = describe(&emulator, &arn, case).await;
        assert_eq!(
            described.get("RevocationConfiguration"),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {described:?}"
        );
    }

    // An update that supplies no configuration at all leaves the CA's capabilities where
    // the last one left them.
    let case = "a status-only update leaves the configuration unchanged";
    let supplied = json!({"OcspConfiguration": {"Enabled": true}});
    updated(
        &emulator,
        case,
        &update_request(&arn, None, Some(supplied.clone())),
    )
    .await;
    updated(
        &emulator,
        case,
        &update_request(&arn, Some(STATUS_DISABLED), None),
    )
    .await;
    let described = describe(&emulator, &arn, case).await;
    assert_eq!(
        described.get("RevocationConfiguration"),
        Some(&json!({
            "CrlConfiguration": {"Enabled": false},
            "OcspConfiguration": {"Enabled": true},
        })),
        "case {case:?}: an absent RevocationConfiguration changes nothing, got {described:?}"
    );
}

/// What this operation cannot be asked for: a status it does not transition a CA into, no
/// change at all, and a revocation configuration breaking the rules its member documents.
///
/// The rows run against a CA the gate admits, so each refusal is about the request and
/// nothing else, and every one of them leaves the CA exactly as it was.
#[tokio::test]
async fn an_update_this_operation_cannot_carry_out_is_refused_with_invalid_args() {
    let emulator = Emulator::new();
    let (arn, _) = active_ca(&emulator, "invalid-args").await;

    let mut rows: Vec<(String, Value)> = Vec::new();

    // A status outside the two this call transitions between is not one a caller assigns:
    // the service itself moves a CA into the rest (a creation settling, a certificate
    // expiring, a deletion, a failed create).
    for status in statuses_outside_the_gate() {
        rows.push((
            format!("a requested status of {status}"),
            update_request(&arn, Some(status), None),
        ));
    }

    // A request naming neither member asks for no update at all.
    rows.push((
        "neither a status nor a revocation configuration".to_string(),
        update_request(&arn, None, None),
    ));

    // The rules `RevocationConfiguration` documents, in the ones the SEMANTIC argument
    // check owns: an enabled CRL needs the period it expires after, and a disabled
    // mechanism may carry nothing but `Enabled=false`. The bucket an enabled CRL publishes
    // to is a member the request-validation stage requires instead, so its absence is
    // refused as `ValidationException` naming the member — `crate::validation_stage_refusals`
    // drives that rule on this operation and on the create.
    let bad_configurations: [(&str, Value); 4] = [
        (
            "CRLs enabled with no expiry",
            json!({"CrlConfiguration": {"Enabled": true, "S3BucketName": "example-crl-bucket"}}),
        ),
        (
            "CRLs disabled but carrying an expiry",
            json!({"CrlConfiguration": {"Enabled": false, "ExpirationInDays": 7}}),
        ),
        (
            "CRLs disabled but carrying a custom name",
            json!({"CrlConfiguration": {"Enabled": false, "CustomCname": "crl.example.com"}}),
        ),
        (
            "OCSP disabled but carrying a custom responder",
            json!({"OcspConfiguration": {"Enabled": false, "OcspCustomCname": "ocsp.example.com"}}),
        ),
    ];
    for (case, configuration) in bad_configurations {
        rows.push((
            case.to_string(),
            update_request(&arn, None, Some(configuration)),
        ));
    }

    for (case, body) in &rows {
        emulator
            .refused(case, "UpdateCertificateAuthority", body, "InvalidArgsException")
            .await;

        let described = describe(&emulator, &arn, case).await;
        assert_eq!(
            string(&described, "Status", case),
            STATUS_ACTIVE,
            "case {case:?}: a refused update leaves the status as it was, got {described:?}"
        );
        assert_eq!(
            described.get("RevocationConfiguration"),
            Some(&no_revocation()),
            "case {case:?}: a refused update writes no configuration, got {described:?}"
        );
    }
}

/// The ARN is judged before any CA is read: an ARN this service resolves no CA from is
/// `InvalidArnException`, and a well-formed ARN of a CA that is not there is
/// `ResourceNotFoundException`.
///
/// Every ARN below satisfies the member's modelled pattern and length, so each reaches the
/// handler rather than the boundary validator. The rows differ along WHICH part of the ARN
/// names no CA of this service.
#[tokio::test]
async fn an_arn_no_ca_resolves_from_is_refused_before_the_ca_is_read() {
    let emulator = Emulator::new();

    let malformed: [(&str, &str); 4] = [
        (
            "no resource id",
            "arn:aws:acm-pca:eu-west-1:000000000000:certificate-authority",
        ),
        (
            "another resource type of this service",
            "arn:aws:acm-pca:eu-west-1:000000000000:certificate/0123456789",
        ),
        (
            "an account that is not twelve digits",
            "arn:aws:acm-pca:eu-west-1:1:certificate-authority/11111111-1111-1111-1111-111111111111",
        ),
        (
            "an empty account",
            "arn:aws:acm-pca:eu-west-1::certificate-authority/11111111-1111-1111-1111-111111111111",
        ),
    ];

    for (case, arn) in malformed {
        emulator
            .refused(
                case,
                "UpdateCertificateAuthority",
                &update_request(arn, Some(STATUS_DISABLED), None),
                "InvalidArnException",
            )
            .await;
    }

    // Well formed, and no CA of this emulator: the ARN is readable, the resource is not
    // there.
    let case = "a well-formed ARN of a CA that was never created";
    emulator
        .refused(
            case,
            "UpdateCertificateAuthority",
            &update_request(
                "arn:aws:acm-pca:eu-west-1:000000000000:certificate-authority/\
                 22222222-2222-2222-2222-222222222222",
                Some(STATUS_DISABLED),
                None,
            ),
            "ResourceNotFoundException",
        )
        .await;
}
