// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AUTOMATIC key rotation schedule of a KMS key —
//! `EnableKeyRotation`, `DisableKeyRotation` and the `GetKeyRotationStatus` that
//! reports them — driven over the wire through the shared [`super::harness`].
//!
//! Four rules, each asserted over a TABLE rather than one call:
//!
//! 1. Enabling automatic rotation records a schedule computed from the period the
//!    request named (365 days when it named none): the status read back reports
//!    rotation enabled, that period, and a next rotation date one period after the
//!    call that configured it. Configuring it again replaces the period, and an
//!    on-demand rotation leaves the schedule alone.
//! 2. Disabling automatic rotation removes the schedule: the status read back
//!    reports rotation disabled and no period and no next rotation date, whether or
//!    not the key had a schedule, and the key material is untouched.
//! 3. The status is a REPORT: every key has one, reading it changes nothing, a
//!    disabled KMS key keeps the status it had, and a key awaiting deletion reports
//!    none until the deletion is cancelled.
//! 4. Automatic rotation is configurable only on an enabled, customer managed,
//!    symmetric encryption KMS key whose material KMS generated, and only on the
//!    primary key of a multi-Region set; every other key, and an identifier that
//!    names no key, is refused by BOTH operations with nothing persisted — and a key
//!    whose kind cannot rotate is refused by every rotation operation alike, with the
//!    error code alone and no message.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::state::service_state;

use super::harness::{
    assert_empty, call, create_alias, create_key, describe, error_code, ok, router,
    router_with_engine, text, REGION,
};

/// Seconds in a day: the unit `RotationPeriodInDays` counts in
/// (`smithy_query(query="operation",operation="EnableKeyRotation")`).
const SECONDS_PER_DAY: f64 = 86_400.0;

/// How far either side of the window the call was served in a reported timestamp may
/// fall: a `DateType` goes on the wire with millisecond resolution, so the instant the
/// schedule was configured is rendered rounded, not to the nanosecond the test read
/// the clock at.
const TIMESTAMP_TOLERANCE: f64 = 1.0;

/// Rotation periods inside the modelled bound of `RotationPeriodInDaysType`
/// (90..2560, `smithy_query(query="operation",operation="EnableKeyRotation")`),
/// including both published bounds, together with the days each is expected to
/// schedule. `None` is a request that names no period at all, whose documented
/// default is 365 days
/// (`smithy_query(query="doc",shape="EnableKeyRotationRequest",member="RotationPeriodInDays")`).
const PERIOD_ROWS: [(Option<i64>, i64); 8] = [
    (None, 365),
    (Some(90), 90),
    (Some(91), 91),
    (Some(120), 120),
    (Some(365), 365),
    (Some(1_000), 1_000),
    (Some(2_559), 2_559),
    (Some(2_560), 2_560),
];

/// Now, as the `awsJson1_1` wire carries a timestamp: epoch seconds.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// Assert an error body carries no `message` at all: KMS refuses a key the rotation
/// operations cannot rotate with the error code alone, so no explanatory text is
/// manufactured for it.
fn assert_no_message(case: &str, payload: &Value) {
    let message = payload["message"]
        .as_str()
        .or_else(|| payload["Message"].as_str());
    assert!(
        message.is_none_or(str::is_empty),
        "case {case:?}: expected no message on the refusal, got {message:?} in {payload}"
    );
}

/// `GetKeyRotationStatus` for an identifier, requiring success.
async fn rotation_status(
    router: &smithy_server_runtime::MultiServiceRouter,
    identifier: &str,
) -> Value {
    ok(router, "GetKeyRotationStatus", &json!({"KeyId": identifier})).await
}

/// Enable automatic rotation, requiring the empty 200 AWS documents for it, and
/// report the epoch-second window the call was served in.
async fn enable_rotation(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    identifier: &str,
    period: Option<i64>,
) -> (f64, f64) {
    let mut request = json!({"KeyId": identifier});
    if let Some(days) = period {
        request["RotationPeriodInDays"] = json!(days);
    }
    let before = epoch_seconds();
    let answer = ok(router, "EnableKeyRotation", &request).await;
    let after = epoch_seconds();
    assert_empty(case, "EnableKeyRotation", &answer);
    (before, after)
}

/// Assert the status reports a schedule of `period` days whose next rotation date
/// falls one period after a call served between `before` and `after`.
fn assert_scheduled(case: &str, status: &Value, arn: &str, period: i64, before: f64, after: f64) {
    assert_eq!(
        status["KeyRotationEnabled"],
        json!(true),
        "case {case:?}: expected KeyRotationEnabled true, got {status}"
    );
    assert_eq!(
        status["KeyId"],
        json!(arn),
        "case {case:?}: expected the key ARN {arn}, got {status}"
    );
    assert_eq!(
        status["RotationPeriodInDays"],
        json!(period),
        "case {case:?}: expected RotationPeriodInDays {period}, got {status}"
    );
    let next = status["NextRotationDate"].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected a NextRotationDate timestamp, got {status}")
    });
    let span = period as f64 * SECONDS_PER_DAY;
    assert!(
        next >= before + span - TIMESTAMP_TOLERANCE
            && next <= after + span + TIMESTAMP_TOLERANCE,
        "case {case:?}: expected a next rotation date {period} day(s) after the call \
         (between {} and {}), got {next}",
        before + span - TIMESTAMP_TOLERANCE,
        after + span + TIMESTAMP_TOLERANCE
    );
}

/// Assert the status reports no automatic rotation at all, so neither the period nor
/// the next rotation date is on the wire.
fn assert_unscheduled(case: &str, status: &Value, arn: &str) {
    assert_eq!(
        status["KeyRotationEnabled"],
        json!(false),
        "case {case:?}: expected KeyRotationEnabled false, got {status}"
    );
    assert_eq!(
        status["KeyId"],
        json!(arn),
        "case {case:?}: expected the key ARN {arn}, got {status}"
    );
    assert!(
        status["RotationPeriodInDays"].is_null(),
        "case {case:?}: a key that is not rotating has no rotation period, got {status}"
    );
    assert!(
        status["NextRotationDate"].is_null(),
        "case {case:?}: a key that is not rotating has no next rotation date, got {status}"
    );
    assert!(
        status["OnDemandRotationStartDate"].is_null(),
        "case {case:?}: no on-demand rotation is in progress, got {status}"
    );
}

/// Enabling automatic rotation schedules the key material rotation the request asked
/// for: the status read back reports the period, and a next rotation date that is one
/// period after the call — computed from the schedule, not echoed from the request.
///
/// The rows differ in the rotation period ([`PERIOD_ROWS`], spanning both published
/// bounds and a request that names none), in the form of the identifier the request
/// names the key by (key id, key ARN, alias — the forms
/// `smithy_query(query="doc",shape="EnableKeyRotationRequest",member="KeyId")`
/// accepts), and in how the key was created.
#[tokio::test]
async fn enabling_rotation_schedules_one_period_after_the_call() {
    let router = router();

    let creates: [(&str, Value); 3] = [
        ("the default symmetric encryption key", json!({})),
        (
            "an explicit SYMMETRIC_DEFAULT ENCRYPT_DECRYPT key",
            json!({"KeySpec": "SYMMETRIC_DEFAULT", "KeyUsage": "ENCRYPT_DECRYPT"}),
        ),
        ("a multi-Region primary key", json!({"MultiRegion": true})),
    ];

    for (described, request) in creates {
        for identifier_form in ["key id", "key ARN", "alias"] {
            for (period, expected) in PERIOD_ROWS {
                let case =
                    format!("{described} named by {identifier_form}, period {period:?} days");
                let created = create_key(&router, &request).await;
                let key_id = text(&case, &created, "KeyId");
                let arn = text(&case, &created, "Arn");
                let identifier = match identifier_form {
                    "key id" => key_id.clone(),
                    "key ARN" => arn.clone(),
                    _ => {
                        let alias = format!("alias/schedule-{key_id}");
                        create_alias(&router, &case, &alias, &key_id).await;
                        alias
                    }
                };

                // A key nothing has configured is not rotating.
                let before_enable = rotation_status(&router, &identifier).await;
                assert_unscheduled(&case, &before_enable, &arn);

                let (before, after) =
                    enable_rotation(&router, &case, &identifier, period).await;
                let status = rotation_status(&router, &identifier).await;
                assert_scheduled(&case, &status, &arn, expected, before, after);
                assert!(
                    status["OnDemandRotationStartDate"].is_null(),
                    "case {case:?}: enabling automatic rotation starts no on-demand \
                     rotation, got {status}"
                );

                // A schedule is not a rotation: the key material the key encrypts
                // under, and the key itself, are exactly as they were.
                let after_enable = describe(&router, &key_id).await;
                assert_eq!(
                    after_enable, created,
                    "case {case:?}: enabling automatic rotation must not change the key, \
                     got {after_enable}"
                );
            }
        }
    }
}

/// Configuring automatic rotation again on a key that is already rotating replaces the
/// period and re-measures the schedule from the later call, and an on-demand rotation
/// in between leaves the schedule exactly as it was.
///
/// The rows are successive periods applied to one key, drawn from [`PERIOD_ROWS`] so
/// the sequence both raises and lowers the period and crosses both published bounds.
#[tokio::test]
async fn re_enabling_rotation_replaces_the_period() {
    let router = router();
    let created = create_key(&router, &json!({"Description": "re-configured"})).await;
    let key_id = text("setup", &created, "KeyId");
    let arn = text("setup", &created, "Arn");

    for (period, expected) in PERIOD_ROWS {
        let case = format!("re-configured to {period:?} days");
        let (before, after) = enable_rotation(&router, &case, &key_id, period).await;
        let status = rotation_status(&router, &key_id).await;
        assert_scheduled(&case, &status, &arn, expected, before, after);

        // "On-demand rotations do not change existing automatic rotation schedules"
        // (`smithy_query(query="doc",shape="RotateKeyOnDemand")`).
        ok(&router, "RotateKeyOnDemand", &json!({"KeyId": key_id})).await;
        let after_rotation = rotation_status(&router, &key_id).await;
        assert_eq!(
            after_rotation, status,
            "case {case:?}: an on-demand rotation must leave the automatic schedule \
             alone, got {after_rotation} (was {status})"
        );
    }
}

/// Disabling automatic rotation removes the schedule, whether or not the key had one,
/// and leaves everything else about the key alone. Re-enabling it afterwards schedules
/// it again.
///
/// The rows differ in the period the schedule had before it was disabled
/// ([`PERIOD_ROWS`]), in the identifier form, and in how many times in a row the
/// disable is repeated — disabling is a request for rotation to be OFF, so repeating
/// it reaches the same state.
#[tokio::test]
async fn disabling_rotation_removes_the_schedule() {
    let router = router();

    for (period, expected) in PERIOD_ROWS {
        for identifier_form in ["key id", "key ARN"] {
            for repeats in [1usize, 2, 3] {
                let case = format!(
                    "period {period:?} days, named by {identifier_form}, disabled {repeats} \
                     time(s)"
                );
                let created = create_key(&router, &json!({"Description": "disable rotation"})).await;
                let key_id = text(&case, &created, "KeyId");
                let arn = text(&case, &created, "Arn");
                let identifier = if identifier_form == "key id" {
                    key_id.clone()
                } else {
                    arn.clone()
                };

                let (before, after) = enable_rotation(&router, &case, &identifier, period).await;
                assert_scheduled(
                    &case,
                    &rotation_status(&router, &identifier).await,
                    &arn,
                    expected,
                    before,
                    after,
                );

                for round in 1..=repeats {
                    let case = format!("{case}, round {round}");
                    let answer =
                        ok(&router, "DisableKeyRotation", &json!({"KeyId": identifier})).await;
                    assert_empty(&case, "DisableKeyRotation", &answer);
                    assert_unscheduled(&case, &rotation_status(&router, &identifier).await, &arn);
                }

                // Disabling rotation is not a rotation either: the key is untouched.
                let after_disable = describe(&router, &key_id).await;
                assert_eq!(
                    after_disable, created,
                    "case {case:?}: disabling automatic rotation must not change the key, \
                     got {after_disable}"
                );

                // And the schedule can be put back.
                let (before, after) = enable_rotation(&router, &case, &identifier, period).await;
                assert_scheduled(
                    &case,
                    &rotation_status(&router, &identifier).await,
                    &arn,
                    expected,
                    before,
                    after,
                );
            }
        }
    }

    // A key that was never configured accepts the disable too, and stays not rotating.
    let never = create_key(&router, &json!({"Description": "never enabled"})).await;
    let never_id = text("setup", &never, "KeyId");
    let never_arn = text("setup", &never, "Arn");
    let answer = ok(&router, "DisableKeyRotation", &json!({"KeyId": never_id})).await;
    assert_empty("a key that was never rotating", "DisableKeyRotation", &answer);
    assert_unscheduled(
        "a key that was never rotating",
        &rotation_status(&router, &never_id).await,
        &never_arn,
    );
}

/// The rotation status is a REPORT: reading it twice answers the same, every key has
/// one — including keys whose automatic rotation could never be enabled — a disabled
/// KMS key keeps the status it had, and a key awaiting deletion reports none until the
/// deletion is cancelled.
///
/// The rows for "every key has a status" span the key kinds
/// `smithy_query(query="shape",shape="KeySpec")` names outside symmetric encryption,
/// under each usage that spec accepts, plus a key created with no key material.
#[tokio::test]
async fn the_rotation_status_is_a_report_that_changes_nothing() {
    let router = router();

    // Rows: a key spec and the usage to create it with, for kinds of key that cannot
    // rotate automatically, plus a key whose material does not come from KMS.
    let unrotatable: [(&str, Value); 11] = [
        ("HMAC_224", json!({"KeySpec": "HMAC_224", "KeyUsage": "GENERATE_VERIFY_MAC"})),
        ("HMAC_256", json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"})),
        ("HMAC_384", json!({"KeySpec": "HMAC_384", "KeyUsage": "GENERATE_VERIFY_MAC"})),
        ("HMAC_512", json!({"KeySpec": "HMAC_512", "KeyUsage": "GENERATE_VERIFY_MAC"})),
        ("RSA_2048 encrypt", json!({"KeySpec": "RSA_2048", "KeyUsage": "ENCRYPT_DECRYPT"})),
        ("RSA_3072 sign", json!({"KeySpec": "RSA_3072", "KeyUsage": "SIGN_VERIFY"})),
        ("RSA_4096 sign", json!({"KeySpec": "RSA_4096", "KeyUsage": "SIGN_VERIFY"})),
        ("ECC_NIST_P521 sign", json!({"KeySpec": "ECC_NIST_P521", "KeyUsage": "SIGN_VERIFY"})),
        ("ECC_NIST_P384 agree", json!({"KeySpec": "ECC_NIST_P384", "KeyUsage": "KEY_AGREEMENT"})),
        ("SM2 encrypt", json!({"KeySpec": "SM2", "KeyUsage": "ENCRYPT_DECRYPT"})),
        ("an EXTERNAL-origin key", json!({"Origin": "EXTERNAL"})),
    ];
    for (described, request) in unrotatable {
        let case = format!("the rotation status of {described}");
        let created = create_key(&router, &request).await;
        let key_id = text(&case, &created, "KeyId");
        let arn = text(&case, &created, "Arn");
        let first = rotation_status(&router, &key_id).await;
        assert_unscheduled(&case, &first, &arn);
        let second = rotation_status(&router, &key_id).await;
        assert_eq!(
            second, first,
            "case {case:?}: reading the status must not change what the next read sees, \
             got {second} (was {first})"
        );
        let after = describe(&router, &key_id).await;
        assert_eq!(
            after, created,
            "case {case:?}: reading the status must persist nothing, got {after}"
        );
    }

    // "The key rotation status does not change when you disable a KMS key" — and it
    // does not change when the key is enabled again either.
    for (case, period, expected) in [
        ("a disabled key with the default period", None, 365i64),
        ("a disabled key with a custom period", Some(90), 90),
    ] {
        let created = create_key(&router, &json!({"Description": "disabled key"})).await;
        let key_id = text(case, &created, "KeyId");
        let arn = text(case, &created, "Arn");
        let (before, after) = enable_rotation(&router, case, &key_id, period).await;
        let scheduled = rotation_status(&router, &key_id).await;
        assert_scheduled(case, &scheduled, &arn, expected, before, after);

        ok(&router, "DisableKey", &json!({"KeyId": key_id})).await;
        let while_disabled = rotation_status(&router, &key_id).await;
        assert_eq!(
            while_disabled, scheduled,
            "case {case:?}: a disabled KMS key keeps the rotation status it had, \
             got {while_disabled} (was {scheduled})"
        );

        ok(&router, "EnableKey", &json!({"KeyId": key_id})).await;
        let re_enabled = rotation_status(&router, &key_id).await;
        assert_eq!(
            re_enabled, scheduled,
            "case {case:?}: re-enabling the KMS key keeps the rotation status it had, \
             got {re_enabled} (was {scheduled})"
        );
    }

    // "While a KMS key is pending deletion, its key rotation status is false ... if you
    // cancel the deletion, the original key rotation status returns."
    for (case, period, expected) in [
        ("a key pending deletion, default period", None, 365i64),
        ("a key pending deletion, custom period", Some(2_560), 2_560),
    ] {
        let created = create_key(&router, &json!({"Description": "pending deletion"})).await;
        let key_id = text(case, &created, "KeyId");
        let arn = text(case, &created, "Arn");
        let (before, after) = enable_rotation(&router, case, &key_id, period).await;
        let scheduled = rotation_status(&router, &key_id).await;
        assert_scheduled(case, &scheduled, &arn, expected, before, after);

        ok(&router, "ScheduleKeyDeletion", &json!({"KeyId": key_id})).await;
        assert_unscheduled(case, &rotation_status(&router, &key_id).await, &arn);

        ok(&router, "CancelKeyDeletion", &json!({"KeyId": key_id})).await;
        let restored = rotation_status(&router, &key_id).await;
        assert_eq!(
            restored, scheduled,
            "case {case:?}: cancelling the deletion returns the original rotation status, \
             got {restored} (was {scheduled})"
        );
    }
}

/// Both operations that configure automatic rotation accept exactly the keys whose
/// schedule is configurable, and refuse every other key — and an identifier that names
/// no key — with nothing persisted.
///
/// The rows span the key kinds outside symmetric encryption, a key whose material did
/// not come from KMS, the key states outside `Enabled`
/// (`smithy_query(query="shape",shape="KeyState")`), the identifier forms that resolve
/// to nothing, and the rotation periods outside the modelled bound — each asserted for
/// `EnableKeyRotation` and for `DisableKeyRotation`.
#[tokio::test]
async fn configuring_rotation_is_refused_outside_an_enabled_symmetric_kms_key() {
    let (router, engine) = router_with_engine();
    let account = engine::emulator_account_id();

    // Rows: a key this operation is not valid for, and the error that reports it.
    let kinds: [(&str, Value); 7] = [
        ("an HMAC_256 key", json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"})),
        ("an RSA_2048 encryption key", json!({"KeySpec": "RSA_2048", "KeyUsage": "ENCRYPT_DECRYPT"})),
        ("an RSA_4096 signing key", json!({"KeySpec": "RSA_4096", "KeyUsage": "SIGN_VERIFY"})),
        ("an ECC_NIST_P256 signing key", json!({"KeySpec": "ECC_NIST_P256", "KeyUsage": "SIGN_VERIFY"})),
        ("an ECC_NIST_P384 key-agreement key", json!({"KeySpec": "ECC_NIST_P384", "KeyUsage": "KEY_AGREEMENT"})),
        ("an SM2 signing key", json!({"KeySpec": "SM2", "KeyUsage": "SIGN_VERIFY"})),
        ("a key with no key material of its own", json!({"Origin": "EXTERNAL"})),
    ];
    for (described, request) in kinds {
        let created = create_key(&router, &request).await;
        let key_id = text(described, &created, "KeyId");
        let arn = text(described, &created, "Arn");
        for operation in ["EnableKeyRotation", "DisableKeyRotation"] {
            let case = format!("{operation} on {described}");
            let (status, payload) =
                call(&router, REGION, operation, &json!({"KeyId": key_id})).await;
            let code = error_code(&payload);
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                code, "UnsupportedOperationException",
                "case {case:?}: expected UnsupportedOperationException, got {code:?} in {payload}"
            );
            assert_no_message(&case, &payload);
            assert_unscheduled(&case, &rotation_status(&router, &key_id).await, &arn);
        }
        let after = describe(&router, &key_id).await;
        assert_eq!(
            after, created,
            "case {described:?}: a refused request must persist nothing, got {after}"
        );
    }

    // Every operation that rotates key material refuses a key whose kind cannot
    // rotate the same way: the error code alone, with no message.
    let asymmetric = create_key(
        &router,
        &json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
    )
    .await;
    let asymmetric_id = text("setup", &asymmetric, "KeyId");
    for operation in [
        "EnableKeyRotation",
        "DisableKeyRotation",
        "RotateKeyOnDemand",
    ] {
        let case = format!("{operation} on an asymmetric signing key");
        let (status, payload) =
            call(&router, REGION, operation, &json!({"KeyId": asymmetric_id})).await;
        let code = error_code(&payload);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            code, "UnsupportedOperationException",
            "case {case:?}: expected UnsupportedOperationException, got {code:?} in {payload}"
        );
        assert_no_message(&case, &payload);
    }

    // Rows: a key whose key state refuses the request, and the error that reports it.
    // A disabled key has its own modelled refusal; any other key state outside
    // `Enabled` is reported as an incompatible key state.
    let disabled = create_key(&router, &json!({"Description": "disabled"})).await;
    let disabled_id = text("setup", &disabled, "KeyId");
    ok(&router, "DisableKey", &json!({"KeyId": disabled_id})).await;
    let pending = create_key(&router, &json!({"Description": "pending deletion"})).await;
    let pending_id = text("setup", &pending, "KeyId");
    ok(&router, "ScheduleKeyDeletion", &json!({"KeyId": pending_id})).await;

    let states: [(&str, &str, &str); 2] = [
        ("a disabled key", &disabled_id, "DisabledException"),
        ("a key pending deletion", &pending_id, "KMSInvalidStateException"),
    ];
    for (described, identifier, expected) in states {
        for operation in ["EnableKeyRotation", "DisableKeyRotation"] {
            let case = format!("{operation} on {described}");
            let before = describe(&router, identifier).await;
            let (status, payload) =
                call(&router, REGION, operation, &json!({"KeyId": identifier})).await;
            let code = error_code(&payload);
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                code, expected,
                "case {case:?}: expected {expected}, got {code:?} in {payload}"
            );
            let after = describe(&router, identifier).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused request must persist nothing, got {after}"
            );
        }
    }

    // Rows: an identifier that resolves to no key of this account and Region, by each
    // form it can take, and the error that reports it — for the reader too, which
    // models the same two.
    let lookups: [(&str, String, &str); 4] = [
        (
            "an unknown key id",
            "00000000-0000-0000-0000-000000000000".to_string(),
            "NotFoundException",
        ),
        (
            "an unknown key ARN",
            format!("arn:aws:kms:{REGION}:{account}:key/mrk-{}", "0".repeat(32)),
            "NotFoundException",
        ),
        (
            "an unknown alias",
            "alias/no-such-key".to_string(),
            "NotFoundException",
        ),
        (
            "an ARN that is not a KMS key or alias ARN",
            format!("arn:aws:kms:{REGION}:{account}:thing/abc"),
            "InvalidArnException",
        ),
    ];
    for (described, identifier, expected) in lookups {
        for operation in [
            "EnableKeyRotation",
            "DisableKeyRotation",
            "GetKeyRotationStatus",
        ] {
            let case = format!("{operation} on {described}");
            let (status, payload) =
                call(&router, REGION, operation, &json!({"KeyId": identifier})).await;
            let code = error_code(&payload);
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                code, expected,
                "case {case:?}: expected {expected}, got {code:?} in {payload}"
            );
        }
    }

    // Rows: a rotation period one past each published bound of
    // `RotationPeriodInDaysType` (90..2560), which the modelled constraint refuses
    // before the key is even looked at, so no schedule is recorded.
    let rotatable = create_key(&router, &json!({"Description": "bounds"})).await;
    let rotatable_id = text("setup", &rotatable, "KeyId");
    let rotatable_arn = text("setup", &rotatable, "Arn");
    for days in [89i64, 2_561, 0, -1] {
        let case = format!("a rotation period of {days} days");
        let (status, payload) = call(
            &router,
            REGION,
            "EnableKeyRotation",
            &json!({"KeyId": rotatable_id, "RotationPeriodInDays": days}),
        )
        .await;
        let code = error_code(&payload);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code:?} in {payload}"
        );
        assert_unscheduled(
            &case,
            &rotation_status(&router, &rotatable_id).await,
            &rotatable_arn,
        );
    }

    // A replica key of a multi-Region set carries no schedule of its own: the
    // property is set on the primary key of the set. The set is seeded with a replica
    // Region and then promoted there, which is what leaves this endpoint serving a
    // replica of the set rather than its primary key.
    let replica = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    seed_replica_region(&engine, &replica, "eu-west-2");
    ok(
        &router,
        "UpdatePrimaryRegion",
        &json!({"KeyId": replica, "PrimaryRegion": "eu-west-2"}),
    )
    .await;
    for operation in ["EnableKeyRotation", "DisableKeyRotation"] {
        let case = format!("{operation} on a replica key of a multi-Region set");
        let (status, payload) = call(&router, REGION, operation, &json!({"KeyId": replica})).await;
        let code = error_code(&payload);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            code, "UnsupportedOperationException",
            "case {case:?}: expected UnsupportedOperationException, got {code:?} in {payload}"
        );
        assert_no_message(&case, &payload);
    }
}

/// Register `region` as a Region holding a replica key of the multi-Region key
/// `key_id`, which is the record `ReplicateKey` leaves behind once it is implemented
/// (its own issue) — the same seam `multi_region_primary` seeds through. Written
/// through the engine the router was built over, so every later request observes it.
fn seed_replica_region(engine: &Arc<engine::InMemoryEngine>, key_id: &str, region: &str) {
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    let mut state = state.lock();
    let key = state
        .keys
        .get_mut(key_id)
        .expect("the key the test just created is in the store");
    let topology = key
        .multi_region_topology
        .as_mut()
        .expect("a multi-Region key carries a multi-Region topology");
    topology.replica_regions.insert(region.to_string());
}
