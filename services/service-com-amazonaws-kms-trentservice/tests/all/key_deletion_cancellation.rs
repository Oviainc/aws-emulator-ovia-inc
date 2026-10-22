// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CancelKeyDeletion` and `UpdateKeyDescription`, driven over
//! the wire through the same router the emulator boots.
//!
//! The rules under test:
//!
//! * `CancelKeyDeletion` is accepted only on a key whose key state is
//!   `PendingDeletion`, and when it succeeds it writes that key back to the
//!   `Disabled` key state and drops the pending deletion bookkeeping (the deletion
//!   date and the waiting period) — so afterwards the key is indistinguishable from
//!   any other disabled key, for every operation a disabled key accepts.
//! * `UpdateKeyDescription` persists the description it is given, which `DescribeKey`
//!   then reports, and is refused while the key's key state is not compatible with a
//!   metadata update.
//!
//! Each assertion is about the rule over a TABLE of inputs: the rows differ in how
//! the deletion was scheduled (the waiting period, and the key state it was
//! scheduled from), which identifier form names the key, what kind of key it is, and
//! which description is written.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire plumbing lives in `tests/all/harness.rs`, shared with every other suite
// in this crate.
use super::harness::{call, create_key, describe, error_code, failure, ok, router, REGION};

/// How a request may name a key: `("<case>", <identifier>)`.
fn identifier_forms(metadata: &Value) -> Vec<(&'static str, String)> {
    vec![
        (
            "key id",
            metadata["KeyId"].as_str().expect("a key id").to_string(),
        ),
        (
            "key ARN",
            metadata["Arn"].as_str().expect("an ARN").to_string(),
        ),
    ]
}

/// The rows every cancellation case runs over: a key to create, the waiting period
/// the deletion names (`None` for "let it default"), and whether the key is disabled
/// before it is scheduled.
///
/// The waiting periods are the documented bounds of
/// `smithy_query(query="doc",shape="ScheduleKeyDeletionRequest",member="PendingWindowInDays")`
/// ("it must be between 7 and 30, inclusive") plus the default, so a cancellation is
/// exercised at each end of the window and without one.
fn scheduling_rows() -> Vec<(&'static str, Value, Option<i64>, bool)> {
    vec![
        ("default window, from Enabled", json!({}), None, false),
        (
            "minimum window, from Enabled",
            json!({"Description": "seven days"}),
            Some(7),
            false,
        ),
        (
            "maximum window, from Disabled",
            json!({"Description": "thirty days"}),
            Some(30),
            true,
        ),
        (
            "mid window, from Disabled, tagged",
            json!({"Tags": [{"TagKey": "app", "TagValue": "demo"}]}),
            Some(14),
            true,
        ),
        (
            "multi-Region primary",
            json!({"Description": "primary", "MultiRegion": true}),
            Some(7),
            false,
        ),
        (
            "non-ASCII description",
            json!({"Description": "clé ✓ 鍵"}),
            Some(30),
            false,
        ),
    ]
}

/// Schedule a key's deletion, requiring the answer AWS documents for it, and report
/// the key's metadata as `DescribeKey` then sees it.
async fn schedule(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    window: Option<i64>,
) -> Value {
    let mut request = json!({"KeyId": identifier});
    if let Some(days) = window {
        request["PendingWindowInDays"] = json!(days);
    }
    let scheduled = ok(router, "ScheduleKeyDeletion", &request).await;
    assert_eq!(
        scheduled["KeyState"],
        json!("PendingDeletion"),
        "case {case}: ScheduleKeyDeletion must report the pending state, got {scheduled:?}"
    );
    assert_eq!(
        scheduled["PendingWindowInDays"],
        json!(window.unwrap_or(30)),
        "case {case}: ScheduleKeyDeletion must echo the waiting period, got {scheduled:?}"
    );
    assert!(
        scheduled["DeletionDate"].is_number(),
        "case {case}: ScheduleKeyDeletion must report a deletion date, got {scheduled:?}"
    );
    describe(router, identifier).await
}

/// A cancellation returns the key to the `Disabled` state it would be in had the
/// deletion never been scheduled: the metadata after the cancel is byte-for-byte the
/// metadata of the same key disabled, so the deletion date and the waiting period are
/// gone too.
#[tokio::test]
async fn cancel_key_deletion_restores_a_pending_key_to_disabled() {
    let router = router();

    for (case, create_request, window, disable_first) in scheduling_rows() {
        for (form, identifier) in identifier_forms(&create_key(&router, &create_request).await) {
            let case = format!("{case} named by {form}");

            // The baseline: the same key, merely disabled. `CancelKeyDeletion` must
            // land exactly here.
            ok(&router, "DisableKey", &json!({"KeyId": identifier})).await;
            let disabled = describe(&router, &identifier).await;

            if !disable_first {
                ok(&router, "EnableKey", &json!({"KeyId": identifier})).await;
            }

            let pending = schedule(&router, &case, &identifier, window).await;
            assert_eq!(
                pending["KeyState"],
                json!("PendingDeletion"),
                "case {case}: the scheduled key must report PendingDeletion, got {pending:?}"
            );

            let cancelled = ok(
                &router,
                "CancelKeyDeletion",
                &json!({"KeyId": identifier}),
            )
            .await;
            assert_eq!(
                cancelled["KeyId"], disabled["Arn"],
                "case {case}: CancelKeyDeletion must answer the key ARN, got {cancelled:?}"
            );

            let restored = describe(&router, &identifier).await;
            assert_eq!(
                restored, disabled,
                "case {case}: the cancelled key must be exactly the disabled key, got {restored:?}"
            );
            assert_eq!(
                restored["KeyState"],
                json!("Disabled"),
                "case {case}: expected KeyState Disabled, got {restored:?}"
            );
            assert_eq!(
                restored["Enabled"],
                json!(false),
                "case {case}: a cancelled key is never re-enabled, got {restored:?}"
            );
            assert!(
                restored["DeletionDate"].is_null(),
                "case {case}: the cancelled deletion date must be gone, got {restored:?}"
            );
        }
    }
}

/// Once the deletion is cancelled the key accepts everything a disabled key accepts:
/// it can be enabled, its description can be updated, and it can be scheduled for
/// deletion again — and the whole cycle can be repeated.
#[tokio::test]
async fn a_cancelled_key_accepts_every_operation_a_disabled_key_accepts() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "recoverable"})).await;
    let key_id = metadata["KeyId"].as_str().expect("a key id").to_string();
    let arn = metadata["Arn"].as_str().expect("an ARN").to_string();

    // The rows are successive rounds of the schedule/cancel cycle, differing in the
    // waiting period and in the identifier form each round uses.
    let rounds: Vec<(&str, Option<i64>, &str)> = vec![
        ("first round", Some(7), "id"),
        ("second round", None, "arn"),
        ("third round", Some(30), "id"),
    ];

    for (case, window, form) in rounds {
        let identifier = if form == "id" { &key_id } else { &arn };

        schedule(&router, case, identifier, window).await;
        let cancelled = ok(
            &router,
            "CancelKeyDeletion",
            &json!({"KeyId": identifier}),
        )
        .await;
        assert_eq!(
            cancelled["KeyId"],
            json!(arn),
            "case {case}: CancelKeyDeletion must answer the key ARN, got {cancelled:?}"
        );

        // EnableKey: a disabled key accepts it, so a cancelled key must too.
        let (status, payload) =
            call(&router, REGION, "EnableKey", &json!({"KeyId": identifier})).await;
        assert_eq!(
            status, 200,
            "case {case}: EnableKey on a cancelled key must succeed, got {status} with {payload:?}"
        );
        let enabled = describe(&router, identifier).await;
        assert_eq!(
            enabled["KeyState"],
            json!("Enabled"),
            "case {case}: expected KeyState Enabled after EnableKey, got {enabled:?}"
        );
        assert_eq!(
            enabled["Enabled"],
            json!(true),
            "case {case}: expected Enabled true after EnableKey, got {enabled:?}"
        );

        // UpdateKeyDescription: accepted, and the new description is what DescribeKey
        // reports.
        let description = format!("after {case}");
        ok(
            &router,
            "UpdateKeyDescription",
            &json!({"KeyId": identifier, "Description": description}),
        )
        .await;
        let described = describe(&router, identifier).await;
        assert_eq!(
            described["Description"],
            json!(description),
            "case {case}: DescribeKey must report the updated description, got {described:?}"
        );
    }
}

/// One row of the "`CancelKeyDeletion` is refused here" table: the case name, the
/// calls that put the key into the state, and the key state that leaves behind.
type IncompatibleStateRow = (&'static str, Vec<(&'static str, Value)>, &'static str);

/// `CancelKeyDeletion` is refused on a key that is not pending deletion, and on an
/// identifier that names no key or is not a key ARN at all — and a refusal changes
/// nothing.
#[tokio::test]
async fn cancel_key_deletion_is_refused_unless_the_key_is_pending_deletion() {
    let router = router();
    let account = engine::emulator_account_id();

    // Rows: a key state `CancelKeyDeletion` is not compatible with, and how to get a
    // key into it.
    let states: Vec<IncompatibleStateRow> = vec![
        ("freshly created", vec![], "Enabled"),
        (
            "disabled",
            vec![("DisableKey", json!({}))],
            "Disabled",
        ),
        (
            "already cancelled",
            vec![
                ("ScheduleKeyDeletion", json!({"PendingWindowInDays": 7})),
                ("CancelKeyDeletion", json!({})),
            ],
            "Disabled",
        ),
        (
            "re-enabled after a cancellation",
            vec![
                ("ScheduleKeyDeletion", json!({"PendingWindowInDays": 7})),
                ("CancelKeyDeletion", json!({})),
                ("EnableKey", json!({})),
            ],
            "Enabled",
        ),
    ];

    for (case, setup, expected_state) in states {
        let metadata = create_key(&router, &json!({"Description": case})).await;
        let key_id = metadata["KeyId"].as_str().expect("a key id").to_string();
        for (operation, extra) in setup {
            let mut request = json!({"KeyId": key_id});
            for (member, value) in extra.as_object().expect("an object") {
                request[member] = value.clone();
            }
            ok(&router, operation, &request).await;
        }
        let before = describe(&router, &key_id).await;
        assert_eq!(
            before["KeyState"],
            json!(expected_state),
            "case {case}: the setup must leave KeyState {expected_state}, got {before:?}"
        );

        let (status, code) = failure(&router, "CancelKeyDeletion", &json!({"KeyId": key_id})).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "KMSInvalidStateException",
            "case {case}: expected KMSInvalidStateException, got {code}"
        );
        assert_eq!(
            describe(&router, &key_id).await,
            before,
            "case {case}: a refused CancelKeyDeletion must change nothing"
        );
    }

    // An identifier that names no key, and one that is not a key ARN at all.
    let absent = [
        ("unknown key id", "11111111-2222-3333-4444-555555555555".to_string()),
        (
            "unknown key ARN",
            format!("arn:aws:kms:{REGION}:{account}:key/11111111-2222-3333-4444-555555555555"),
        ),
    ];
    for (case, identifier) in absent {
        let (status, code) =
            failure(&router, "CancelKeyDeletion", &json!({"KeyId": identifier})).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case}: expected NotFoundException, got {code}"
        );
    }
    let malformed = [
        ("another service's ARN", format!("arn:aws:s3:::bucket/{account}")),
        ("too few segments", "arn:aws:kms".to_string()),
    ];
    for (case, identifier) in malformed {
        let (status, code) =
            failure(&router, "CancelKeyDeletion", &json!({"KeyId": identifier})).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "InvalidArnException",
            "case {case}: expected InvalidArnException, got {code}"
        );
    }
}

/// `UpdateKeyDescription` persists whatever description it is given — the rows span
/// the modelled `DescriptionType` length bound 0..8192 and the character classes a
/// description may carry — and is refused while the key is pending deletion, leaving
/// the stored description untouched.
#[tokio::test]
async fn update_key_description_persists_every_description_and_respects_the_key_state() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "original"})).await;
    let key_id = metadata["KeyId"].as_str().expect("a key id").to_string();
    let arn = metadata["Arn"].as_str().expect("an ARN").to_string();

    // Rows: the description written, and the identifier form that names the key.
    // The bounds are `DescriptionType`'s modelled `len 0..8192`
    // (`smithy_query(query="operation",operation="UpdateKeyDescription")`).
    let long = "d".repeat(8192);
    let descriptions: Vec<(&str, String, &str)> = vec![
        ("ASCII", "a plain description".to_string(), "id"),
        ("empty, at the lower bound", String::new(), "arn"),
        ("non-ASCII", "clé ✓ 鍵".to_string(), "id"),
        ("at the modelled maximum", long.clone(), "arn"),
        ("single character", "x".to_string(), "id"),
        (
            "back to something short",
            "short again".to_string(),
            "arn",
        ),
    ];

    for (case, description, form) in descriptions {
        let identifier = if form == "id" { &key_id } else { &arn };
        ok(
            &router,
            "UpdateKeyDescription",
            &json!({"KeyId": identifier, "Description": description}),
        )
        .await;
        let described = describe(&router, identifier).await;
        assert_eq!(
            described["Description"],
            json!(description),
            "case {case}: DescribeKey must report the written description, got {described:?}"
        );
    }

    // Past the modelled maximum the request is refused before anything is written.
    let (status, payload) = call(
        &router,
        REGION,
        "UpdateKeyDescription",
        &json!({"KeyId": key_id, "Description": format!("{long}x")}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the modelled maximum: expected HTTP 400, got {status} with {payload:?} \
         ({})",
        error_code(&payload)
    );

    // While the key is pending deletion the update is refused, and the stored
    // description is unchanged.
    let before = describe(&router, &key_id).await;
    ok(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": key_id, "PendingWindowInDays": 7}),
    )
    .await;
    let (status, code) = failure(
        &router,
        "UpdateKeyDescription",
        &json!({"KeyId": key_id, "Description": "while pending deletion"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case pending deletion: expected HTTP 400, got {status} ({code})"
    );
    assert_eq!(
        code, "KMSInvalidStateException",
        "case pending deletion: expected KMSInvalidStateException, got {code}"
    );
    ok(&router, "CancelKeyDeletion", &json!({"KeyId": key_id})).await;
    let after = describe(&router, &key_id).await;
    assert_eq!(
        after["Description"], before["Description"],
        "case pending deletion: a refused update must leave the description alone, got {after:?}"
    );

    // And the same refusals on an identifier that names no key or is not a key ARN.
    for (case, identifier, expected_status, expected_code) in [
        (
            "unknown key id",
            "11111111-2222-3333-4444-555555555555".to_string(),
            400,
            "NotFoundException",
        ),
        ("too few segments", "arn:aws:kms".to_string(), 400, "InvalidArnException"),
    ] {
        let (status, code) = failure(
            &router,
            "UpdateKeyDescription",
            &json!({"KeyId": identifier, "Description": "nowhere"}),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} ({code})"
        );
        assert_eq!(
            code, expected_code,
            "case {case}: expected {expected_code}, got {code}"
        );
    }
}
