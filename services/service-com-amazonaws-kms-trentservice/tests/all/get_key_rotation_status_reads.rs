// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the READER of a KMS key's automatic rotation schedule,
//! `GetKeyRotationStatus`, driven over the wire through the shared [`super::harness`].
//!
//! The sibling [`super::key_rotation_schedule`] suite drives this reader from the
//! WRITERS of the schedule (`EnableKeyRotation`, `DisableKeyRotation`); this one drives
//! it from the IDENTIFIER the request names the key by and from the endpoint the
//! request reached, so the rules under test are the reader's own:
//!
//! 1. The report belongs to the key the `KeyId` member RESOLVES to, whatever form that
//!    identifier takes (key id, key ARN, `alias/<name>`, alias ARN), and the key it
//!    resolved to is reported back as that key's ARN rather than as the identifier the
//!    request used.
//! 2. A KMS key is a regional resource, so the reader serves only the keys of the
//!    endpoint's Region and account: an identifier that names no key there is
//!    `NotFoundException`, and an ARN that is no KMS key or alias ARN at all is
//!    `InvalidArnException`.
//! 3. The report is a REPORT: it is derived from the schedule the key carries, so it
//!    does not move when the key is written for any other reason, and reading it
//!    changes nothing a later caller sees.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::harness::{
    alias_arn, assert_empty, b64, create_alias, create_alias_in, create_key, describe, failure,
    failure_in, ok, ok_in, router, text, REGION,
};

/// Seconds in a day: the unit `RotationPeriodInDays` counts in
/// (`smithy_query(query="operation",operation="GetKeyRotationStatus")`).
const SECONDS_PER_DAY: f64 = 86_400.0;

/// How far either side of the window that served the configuring call a reported
/// timestamp may fall: a `DateType` goes on the wire with millisecond resolution, so
/// the instant is rendered rounded rather than to the nanosecond the test read the
/// clock at.
const TIMESTAMP_TOLERANCE: f64 = 1.0;

/// A second Region, for the rows about which endpoint serves a key.
const OTHER_REGION: &str = "us-west-2";

/// Rotation periods inside the modelled bound of `RotationPeriodInDaysType` (90..2560,
/// `smithy_query(query="operation",operation="GetKeyRotationStatus")`) together with
/// the number of days each is expected to report. `None` names no period at all, whose
/// documented default is 365 days
/// (`smithy_query(query="doc",shape="EnableKeyRotationRequest",member="RotationPeriodInDays")`).
const PERIOD_ROWS: [(Option<i64>, i64); 4] = [
    (None, 365),
    (Some(90), 90),
    (Some(400), 400),
    (Some(2_560), 2_560),
];

/// One Region's key, for the rows about which endpoint serves which key.
struct RegionalKey {
    /// The Region whose endpoint created the key.
    region: &'static str,
    /// The key id `CreateKey` minted there.
    key_id: String,
    /// That key's ARN, which is what the reader reports.
    key_arn: String,
    /// The number of days its schedule is expected to report.
    period_in_days: i64,
    /// The epoch-second window that served the call configuring that schedule.
    window: (f64, f64),
}

/// Now, as the `awsJson1_1` wire carries a timestamp: epoch seconds.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// Read the rotation status at `region`'s endpoint, requiring success.
async fn status_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> Value {
    ok_in(
        router,
        region,
        "GetKeyRotationStatus",
        &json!({"KeyId": identifier}),
    )
    .await
}

/// [`status_in`], at the [`REGION`] endpoint.
async fn status(router: &smithy_server_runtime::MultiServiceRouter, identifier: &str) -> Value {
    status_in(router, REGION, identifier).await
}

/// Configure automatic rotation at `region`'s endpoint and report the epoch-second
/// window the call was served in, which is what the next rotation date is measured
/// from.
async fn enable_rotation_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    region: &str,
    identifier: &str,
    period: Option<i64>,
) -> (f64, f64) {
    let mut request = json!({"KeyId": identifier});
    if let Some(days) = period {
        request["RotationPeriodInDays"] = json!(days);
    }
    let before = epoch_seconds();
    let answer = ok_in(router, region, "EnableKeyRotation", &request).await;
    let after = epoch_seconds();
    assert_empty(case, "EnableKeyRotation", &answer);
    (before, after)
}

/// Assert a report says the key is not rotating, so neither the period nor the next
/// rotation date is on the wire, and that it names the key by its ARN.
fn assert_not_rotating(case: &str, report: &Value, arn: &str) {
    assert_eq!(
        report["KeyRotationEnabled"],
        json!(false),
        "case {case:?}: expected KeyRotationEnabled false, got {report}"
    );
    assert_eq!(
        report["KeyId"],
        json!(arn),
        "case {case:?}: expected the key ARN {arn}, got {report}"
    );
    assert!(
        report["RotationPeriodInDays"].is_null(),
        "case {case:?}: a key that is not rotating reports no period, got {report}"
    );
    assert!(
        report["NextRotationDate"].is_null(),
        "case {case:?}: a key that is not rotating reports no next rotation date, got {report}"
    );
    assert!(
        report["OnDemandRotationStartDate"].is_null(),
        "case {case:?}: no on-demand rotation is in progress, got {report}"
    );
}

/// Assert a report says the key rotates every `period` days, with a next rotation date
/// one period after a configuring call served between `before` and `after`, and that it
/// names the key by its ARN.
fn assert_rotating(case: &str, report: &Value, arn: &str, period: i64, before: f64, after: f64) {
    assert_eq!(
        report["KeyRotationEnabled"],
        json!(true),
        "case {case:?}: expected KeyRotationEnabled true, got {report}"
    );
    assert_eq!(
        report["KeyId"],
        json!(arn),
        "case {case:?}: expected the key ARN {arn}, got {report}"
    );
    assert_eq!(
        report["RotationPeriodInDays"],
        json!(period),
        "case {case:?}: expected RotationPeriodInDays {period}, got {report}"
    );
    let next = report["NextRotationDate"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a NextRotationDate, got {report}"));
    let span = period as f64 * SECONDS_PER_DAY;
    assert!(
        next >= before + span - TIMESTAMP_TOLERANCE && next <= after + span + TIMESTAMP_TOLERANCE,
        "case {case:?}: expected a next rotation date {period} day(s) after the configuring \
         call (between {} and {}), got {next}",
        before + span - TIMESTAMP_TOLERANCE,
        after + span + TIMESTAMP_TOLERANCE
    );
    assert!(
        report["OnDemandRotationStartDate"].is_null(),
        "case {case:?}: an automatic schedule starts no on-demand rotation, got {report}"
    );
}

/// Every identifier form that resolves to a key reports that key's schedule, and
/// reports the key as its ARN — so the four forms answer one and the same report.
///
/// The rows differ in the kind of key (the `KeySpec`/`KeyUsage` combinations this
/// service mints, inside and outside the symmetric encryption keys whose rotation is
/// configurable, plus a key created with no key material of its own), in whether a
/// schedule is configured, and in the period it was configured with ([`PERIOD_ROWS`]).
#[tokio::test]
async fn every_identifier_form_reports_the_resolved_key_by_its_arn() {
    let router = router();

    // Rows: a key kind, the rest of its `CreateKey` request, and whether automatic
    // rotation can be configured on it at all — "automatic key rotation is supported
    // only on symmetric encryption KMS keys ... you cannot enable automatic rotation of
    // asymmetric KMS keys, HMAC KMS keys, KMS keys with imported key material"
    // (`smithy_query(query="doc",shape="GetKeyRotationStatus")`).
    let kinds: [(&str, Value, bool); 6] = [
        ("the default symmetric encryption key", json!({}), true),
        (
            "a multi-Region primary key",
            json!({"MultiRegion": true}),
            true,
        ),
        (
            "an HMAC_384 key",
            json!({"KeySpec": "HMAC_384", "KeyUsage": "GENERATE_VERIFY_MAC"}),
            false,
        ),
        (
            "an RSA_3072 encryption key",
            json!({"KeySpec": "RSA_3072", "KeyUsage": "ENCRYPT_DECRYPT"}),
            false,
        ),
        (
            "an ECC_NIST_P256 signing key",
            json!({"KeySpec": "ECC_NIST_P256", "KeyUsage": "SIGN_VERIFY"}),
            false,
        ),
        (
            "a key created with no key material",
            json!({"Origin": "EXTERNAL"}),
            false,
        ),
    ];

    for (index, (described, request, rotatable)) in kinds.into_iter().enumerate() {
        let metadata = create_key(&router, &request).await;
        let key_id = text(described, &metadata, "KeyId");
        let key_arn = text(described, &metadata, "Arn");
        let alias = format!("alias/rotation-reader-{index}");
        create_alias(&router, described, &alias, &key_id).await;
        let forms = [
            ("key id", key_id.clone()),
            ("key ARN", key_arn.clone()),
            ("alias name", alias.clone()),
            ("alias ARN", alias_arn(REGION, &alias)),
        ];

        // A key nothing configured is not rotating, by every form that names it.
        let mut reports = Vec::new();
        for (form, identifier) in &forms {
            let case = format!("{described} read by {form}, nothing configured");
            let report = status(&router, identifier).await;
            assert_not_rotating(&case, &report, &key_arn);
            reports.push((case, report));
        }
        for (case, report) in &reports[1..] {
            assert_eq!(
                report, &reports[0].1,
                "case {case:?}: every identifier form must answer the same report, got \
                 {report} and {}",
                reports[0].1
            );
        }

        if !rotatable {
            continue;
        }

        for (period, expected) in PERIOD_ROWS {
            let case = format!("{described}, period {period:?}");
            let (before, after) =
                enable_rotation_in(&router, &case, REGION, &key_id, period).await;

            let mut reports = Vec::new();
            for (form, identifier) in &forms {
                let case = format!("{case} read by {form}");
                let report = status(&router, identifier).await;
                assert_rotating(&case, &report, &key_arn, expected, before, after);
                reports.push((case, report));
            }
            for (case, report) in &reports[1..] {
                assert_eq!(
                    report, &reports[0].1,
                    "case {case:?}: every identifier form must answer the same report, got \
                     {report} and {}",
                    reports[0].1
                );
            }

            let answer = ok(&router, "DisableKeyRotation", &json!({"KeyId": key_id})).await;
            assert_empty(&case, "DisableKeyRotation", &answer);
            for (form, identifier) in &forms {
                let case = format!("{case} read by {form}, after the disable");
                assert_not_rotating(&case, &status(&router, identifier).await, &key_arn);
            }
        }
    }
}

/// The reader serves the keys of the endpoint it was called at: each Region's key
/// reports its own schedule, and the same key id at another Region's endpoint — like
/// every other identifier that resolves to no key of this account and Region — is
/// refused rather than answered with a default report.
#[tokio::test]
async fn the_reader_serves_only_the_keys_of_its_own_region_and_account() {
    let router = router();
    let account = engine::emulator_account_id();

    // Rows: a Region, the period its key's schedule is configured with, and the alias
    // the key carries there. The same alias name in two Regions names two keys.
    let regions: [(&'static str, Option<i64>, i64); 2] =
        [(REGION, None, 365), (OTHER_REGION, Some(90), 90)];
    let mut keys: Vec<RegionalKey> = Vec::new();
    for (region, period, expected) in regions {
        let case = format!("the key of {region}");
        let metadata = ok_in(
            &router,
            region,
            "CreateKey",
            &json!({"Description": case.clone()}),
        )
        .await["KeyMetadata"]
            .clone();
        let key_id = text(&case, &metadata, "KeyId");
        let key_arn = text(&case, &metadata, "Arn");
        create_alias_in(&router, region, &case, "alias/regional-rotation", &key_id).await;
        let window = enable_rotation_in(&router, &case, region, &key_id, period).await;
        keys.push(RegionalKey {
            region,
            key_id,
            key_arn,
            period_in_days: expected,
            window,
        });
    }

    for key in &keys {
        let (before, after) = key.window;
        // The schedule of the key this endpoint serves, by key id and by the alias the
        // two Regions share.
        for identifier in [key.key_id.as_str(), "alias/regional-rotation"] {
            let case = format!("{} serves its own key, read by {identifier}", key.region);
            let report = status_in(&router, key.region, identifier).await;
            assert_rotating(&case, &report, &key.key_arn, key.period_in_days, before, after);
        }

        // And no other endpoint serves it.
        for other in &keys {
            if other.region == key.region {
                continue;
            }
            let case = format!("the key of {} read at {}", key.region, other.region);
            let (http_status, code) = failure_in(
                &router,
                other.region,
                "GetKeyRotationStatus",
                &json!({"KeyId": key.key_id}),
            )
            .await;
            assert_eq!(
                http_status, 400,
                "case {case:?}: expected HTTP 400, got {http_status} ({code})"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {case:?}: expected NotFoundException, got {code}"
            );
        }
    }

    let local_key_id = &keys[0].key_id;

    // Rows: a well-formed identifier that names no key of this account and Region.
    let not_found: [(&str, String); 6] = [
        (
            "an unknown key id",
            "11111111-1111-4111-1111-111111111111".to_string(),
        ),
        (
            "an unknown key ARN",
            format!("arn:aws:kms:{REGION}:{account}:key/mrk-{}", "1".repeat(32)),
        ),
        ("an unknown alias", "alias/no-such-rotation".to_string()),
        (
            "an unknown alias ARN",
            alias_arn(REGION, "alias/no-such-rotation"),
        ),
        (
            "this key's ARN in another Region",
            format!("arn:aws:kms:{OTHER_REGION}:{account}:key/{local_key_id}"),
        ),
        (
            "this key's ARN in another account",
            format!("arn:aws:kms:{REGION}:999999999999:key/{local_key_id}"),
        ),
    ];
    for (case, identifier) in not_found {
        let (http_status, code) =
            failure(&router, "GetKeyRotationStatus", &json!({"KeyId": identifier})).await;
        assert_eq!(
            http_status, 400,
            "case {case:?}: expected HTTP 400, got {http_status} ({code})"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case:?}: expected NotFoundException, got {code}"
        );
    }

    // Rows: an ARN that is no KMS key or alias ARN at all, which this operation models
    // its own error for.
    let malformed: [(&str, String); 5] = [
        (
            "an unknown resource type",
            format!("arn:aws:kms:{REGION}:{account}:thing/{local_key_id}"),
        ),
        ("another service's ARN", "arn:aws:s3:::my-bucket".to_string()),
        (
            "an empty key id",
            format!("arn:aws:kms:{REGION}:{account}:key/"),
        ),
        (
            "no Region segment",
            format!("arn:aws:kms::{account}:key/{local_key_id}"),
        ),
        ("nothing but the prefix", "arn:".to_string()),
    ];
    for (case, identifier) in malformed {
        let (http_status, code) =
            failure(&router, "GetKeyRotationStatus", &json!({"KeyId": identifier})).await;
        assert_eq!(
            http_status, 400,
            "case {case:?}: expected HTTP 400, got {http_status} ({code})"
        );
        assert_eq!(
            code, "InvalidArnException",
            "case {case:?}: expected InvalidArnException, got {code}"
        );
    }
}

/// The report is derived from the schedule the key carries, so writing the key for any
/// other reason — and using its key material — leaves the report byte for byte as it
/// was, including the next rotation date, which is measured from the call that
/// configured the schedule and not from the clock the read happened at. Reading it
/// persists nothing.
#[tokio::test]
async fn the_report_does_not_move_when_the_key_is_written_for_another_reason() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "unrelated writes"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let key_arn = text("setup", &metadata, "Arn");
    let (before, after) = enable_rotation_in(&router, "setup", REGION, &key_id, Some(120)).await;
    let configured = status(&router, &key_id).await;
    assert_rotating("setup", &configured, &key_arn, 120, before, after);

    // Rows: a call that writes or uses the key without touching its rotation schedule.
    let writes: [(&str, &str, Value); 6] = [
        (
            "a new description",
            "UpdateKeyDescription",
            json!({"KeyId": key_id, "Description": "renamed"}),
        ),
        (
            "a new tag",
            "TagResource",
            json!({"KeyId": key_id, "Tags": [{"TagKey": "env", "TagValue": "prod"}]}),
        ),
        (
            "a tag removed",
            "UntagResource",
            json!({"KeyId": key_id, "TagKeys": ["env"]}),
        ),
        (
            "an alias added",
            "CreateAlias",
            json!({"AliasName": "alias/unrelated-write", "TargetKeyId": key_id}),
        ),
        (
            "the key material used to encrypt",
            "Encrypt",
            json!({"KeyId": key_id, "Plaintext": b64(b"rotation status")}),
        ),
        (
            "a data key generated under the key",
            "GenerateDataKeyWithoutPlaintext",
            json!({"KeyId": key_id, "KeySpec": "AES_256"}),
        ),
    ];
    for (case, operation, request) in writes {
        ok(&router, operation, &request).await;
        let report = status(&router, &key_id).await;
        assert_eq!(
            report, configured,
            "case {case:?}: {operation} must leave the rotation report exactly as it was, \
             got {report} (was {configured})"
        );
    }

    // Reading is not writing: repeated reads answer the same report and leave the key
    // exactly as the last write left it.
    let described = describe(&router, &key_id).await;
    for round in 1..=3 {
        let case = format!("read {round}");
        let report = status(&router, &key_id).await;
        assert_eq!(
            report, configured,
            "case {case:?}: reading the status must not change what the next read sees, \
             got {report} (was {configured})"
        );
    }
    let after_reads = describe(&router, &key_id).await;
    assert_eq!(
        after_reads, described,
        "case reads: reading the status must persist nothing, got {after_reads} (was {described})"
    );
}
