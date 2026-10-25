// SPDX-License-Identifier: CC0-1.0

//! The account's data retention setting end to end: `PutAccountDataRetention` then
//! `GetAccountDataRetention` over HTTP, through the same `MultiServiceRouter` the emulator
//! boots.
//!
//! # The rule
//!
//! The data retention setting is ONE account-level record, so `PutAccountDataRetention`
//! replaces it whole with the `mode` the request carries and stamps the instant of that
//! write, `GetAccountDataRetention` reports that same mode and that same stamp back —
//! nothing merged in from an earlier put, nothing moved by a read — and while no put has
//! been made the account reports the mode under which the standard data handling applies
//! with no `updatedAt` at all.
//!
//! # What the tables below generalise over
//!
//! One row per value `DataRetentionMode` publishes (`default`, `none`, `aws_review`,
//! `provider_data_share`, `inherit`, from this service's Smithy model via
//! `smithy_query(query="shape",shape="DataRetentionMode")`), so the suite spans the whole
//! enum rather than the one mode a recording carried — including `default`, which is also
//! the unconfigured answer, so a row that sets it is distinguished from an account that set
//! nothing by the `updatedAt` it gains. The rows are driven in sequence against ONE
//! emulator as well, so each read also witnesses that the put before it REPLACED the mode
//! rather than merging with it, and in a separate pass each row is driven twice to witness
//! that an `@idempotent` put applied twice leaves one mode.
//! [`a_mode_the_model_does_not_admit_is_refused_and_changes_nothing`] drives the values
//! outside that published set: another word, the right word in the wrong case, the empty
//! string, a mode of the wrong JSON type, and no mode at all.

use serde_json::{json, Value};

use crate::support::{emulator, wall_clock_seconds, Emulator, Reply};

/// The URI both `PutAccountDataRetention` and `GetAccountDataRetention` are bound to
/// (`smithy_query(query="operation",operation="PutAccountDataRetention")`).
const RETENTION_URI: &str = "/data-retention";

/// Every value `DataRetentionMode` publishes, from this service's Smithy model
/// (`smithy_query(query="shape",shape="DataRetentionMode")`).
const MODES: &[&str] = &[
    "default",
    "none",
    "aws_review",
    "provider_data_share",
    "inherit",
];

/// The mode an account that has never put one reports: the model documents `default` as
/// "The standard data handling for the model applies".
const UNCONFIGURED_MODE: &str = "default";

/// `PUT /data-retention` with `mode`.
async fn put(emulator: &Emulator, mode: Value) -> Reply {
    emulator
        .call("PUT", RETENTION_URI, Some(json!({ "mode": mode })))
        .await
}

/// `GET /data-retention`.
async fn get(emulator: &Emulator) -> Reply {
    emulator.call("GET", RETENTION_URI, None).await
}

#[tokio::test]
async fn a_get_before_any_put_reports_the_standard_handling_and_no_update_instant() {
    let emulator = emulator();
    // Twice, because a read must not be what configures the setting: the second read sees
    // what the first one left behind.
    for attempt in 1..=2 {
        let reply = get(&emulator).await;
        assert_eq!(
            reply.status, 200,
            "case unconfigured attempt {attempt}: GetAccountDataRetention must answer 200, \
             got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("mode"),
            UNCONFIGURED_MODE,
            "case unconfigured attempt {attempt}: expected mode {UNCONFIGURED_MODE:?}, got {:?}",
            reply.get("mode")
        );
        assert_eq!(
            reply.get("updatedAt"),
            Value::Null,
            "case unconfigured attempt {attempt}: nothing has updated the setting, so there \
             is no updatedAt, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn every_mode_is_reported_back_exactly_as_it_was_put() {
    let emulator = emulator();
    for mode in MODES {
        let before = wall_clock_seconds();
        let stored = put(&emulator, json!(mode)).await;
        assert_eq!(
            stored.status, 200,
            "case {mode:?}: PutAccountDataRetention must answer 200, got {} {:?}",
            stored.status, stored.body
        );
        assert_eq!(
            stored.text("mode"),
            *mode,
            "case {mode:?}: the put must report the mode it stored, got {:?}",
            stored.get("mode")
        );
        let stamped = stored.text("updatedAt");
        let stamped_seconds = stored.instant("updatedAt");
        let after = wall_clock_seconds();
        assert!(
            stamped_seconds >= before - 1.0 && stamped_seconds <= after + 1.0,
            "case {mode:?}: updatedAt must be the instant of the put, expected within \
             [{before}, {after}], got {stamped_seconds}"
        );

        let reply = get(&emulator).await;
        assert_eq!(
            reply.status, 200,
            "case {mode:?}: GetAccountDataRetention must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("mode"),
            *mode,
            "case {mode:?}: the stored mode must be reported back, got {:?}",
            reply.get("mode")
        );
        assert_eq!(
            reply.text("updatedAt"),
            stamped,
            "case {mode:?}: the read must report the instant the put stamped, got {:?}",
            reply.get("updatedAt")
        );

        // Reading twice changes nothing: this operation is @readonly.
        let again = get(&emulator).await;
        assert_eq!(
            (again.text("mode"), again.text("updatedAt")),
            (reply.text("mode"), stamped.clone()),
            "case {mode:?}: a second read must report the same setting, got {:?}",
            again.body
        );
    }
}

#[tokio::test]
async fn putting_the_same_mode_twice_leaves_the_same_mode() {
    let emulator = emulator();
    for mode in MODES {
        let mut stamps = Vec::new();
        for attempt in 1..=2 {
            let stored = put(&emulator, json!(mode)).await;
            assert_eq!(
                stored.status, 200,
                "case {mode:?} attempt {attempt}: the put must answer 200, got {} {:?}",
                stored.status, stored.body
            );
            assert_eq!(
                stored.text("mode"),
                *mode,
                "case {mode:?} attempt {attempt}: the put must report the mode it stored, \
                 got {:?}",
                stored.get("mode")
            );
            stamps.push(stored.instant("updatedAt"));
        }
        assert!(
            stamps[1] >= stamps[0],
            "case {mode:?}: the instant of the later write cannot precede the earlier one, \
             got {stamps:?}"
        );

        let reply = get(&emulator).await;
        assert_eq!(
            reply.text("mode"),
            *mode,
            "case {mode:?}: a put applied twice must leave one mode, got {:?}",
            reply.get("mode")
        );
        assert_eq!(
            reply.instant("updatedAt"),
            stamps[1],
            "case {mode:?}: the read must report the instant of the last admitted put, got {:?}",
            reply.get("updatedAt")
        );
    }
}

#[tokio::test]
async fn each_put_replaces_the_mode_the_one_before_it_stored() {
    // Every ordered pair of distinct modes, so no row's answer can come from the row
    // before it rather than from its own put.
    let emulator = emulator();
    for first in MODES {
        for second in MODES {
            if first == second {
                continue;
            }
            let case = format!("{first} then {second}");
            let stored_first = put(&emulator, json!(first)).await;
            assert_eq!(
                stored_first.status, 200,
                "case {case}: the first put must answer 200, got {} {:?}",
                stored_first.status, stored_first.body
            );
            let first_stamp = stored_first.instant("updatedAt");

            let stored_second = put(&emulator, json!(second)).await;
            assert_eq!(
                stored_second.status, 200,
                "case {case}: the second put must answer 200, got {} {:?}",
                stored_second.status, stored_second.body
            );

            let reply = get(&emulator).await;
            assert_eq!(
                reply.text("mode"),
                *second,
                "case {case}: the later put must replace the mode whole, got {:?}",
                reply.get("mode")
            );
            assert!(
                reply.instant("updatedAt") >= first_stamp,
                "case {case}: the reported instant must be the later write's, got {:?}",
                reply.get("updatedAt")
            );
        }
    }
}

#[tokio::test]
async fn a_mode_the_model_does_not_admit_is_refused_and_changes_nothing() {
    // (case, the `mode` the request carries) — a value outside `DataRetentionMode`'s
    // published set, one of its values in the wrong case, the empty string, a value of the
    // wrong JSON type, and one past the set that only looks like a member of it.
    let refused: &[(&str, Value)] = &[
        ("a word outside DataRetentionMode", json!("retain")),
        ("a published value in the wrong case", json!("DEFAULT")),
        ("a published value in mixed case", json!("Aws_Review")),
        ("the empty string", json!("")),
        ("a published value with trailing space", json!("none ")),
        ("a number rather than a mode", json!(7)),
        ("a boolean rather than a mode", json!(true)),
    ];

    let emulator = emulator();
    // A mode that IS admitted, so each refusal is checked against a known stored setting
    // rather than against an untouched account.
    let kept = "none";
    let baseline = put(&emulator, json!(kept)).await;
    assert_eq!(
        baseline.status, 200,
        "the baseline put must answer 200, got {} {:?}",
        baseline.status, baseline.body
    );
    let kept_stamp = baseline.text("updatedAt");

    for (case, mode) in refused {
        let reply = put(&emulator, mode.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: the put must be refused 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case}: a constraint violation is a ValidationException, got {:?}",
            reply.error
        );
        let after = get(&emulator).await;
        assert_eq!(
            (after.text("mode"), after.text("updatedAt")),
            (kept.to_string(), kept_stamp.clone()),
            "case {case}: a refused put must store nothing, got {:?}",
            after.body
        );
    }

    // The member is @required: a request carrying no mode is refused the same way, and
    // stores nothing either.
    let missing = emulator.call("PUT", RETENTION_URI, Some(json!({}))).await;
    assert_eq!(
        missing.status, 400,
        "case missing mode: the put must be refused 400, got {} {:?}",
        missing.status, missing.body
    );
    let after = get(&emulator).await;
    assert_eq!(
        (after.text("mode"), after.text("updatedAt")),
        (kept.to_string(), kept_stamp.clone()),
        "case missing mode: a refused put must store nothing, got {:?}",
        after.body
    );
}
