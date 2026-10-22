// SPDX-License-Identifier: CC0-1.0

//! The AWS Config retention-configuration lifecycle, end to end over the wire.
//!
//! The rule pinned here: a retention configuration is a PERSISTED resource of this
//! account and Region — a write stores the retention period under the single name
//! Config assigns the object, every read reports what the store holds (and reports
//! nothing when the store holds nothing), a delete removes it, and naming an object
//! the store does not hold is refused with the modelled
//! `NoSuchRetentionConfigurationException` rather than answered with an empty page.
//!
//! Each case is a ROW of a table spanning the published `RetentionPeriodInDays` range
//! and the names a request can carry, not the one value a recording used. Every case
//! drives the emulator the way a client does, through the `MultiServiceRouter` the
//! production router builds; the harness (`Emulator`) is the one
//! `config_setup_and_rule_loop` already builds.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::Emulator;

/// The name Config gives the retention configuration object
/// (`PutRetentionConfiguration`: "The API creates the RetentionConfiguration object
/// and names the object as default").
const OBJECT_NAME: &str = "default";

/// The published `RetentionPeriodInDays` bounds, from
/// `smithy_query(query="shape", shape="RetentionPeriodInDays")` on this service's
/// model: `range 30..2557`.
const MIN_PERIOD: i64 = 30;
/// See [`MIN_PERIOD`].
const MAX_PERIOD: i64 = 2557;

/// Retention periods spanning that range: both published bounds and interior values.
const PERIODS: [i64; 6] = [MIN_PERIOD, 31, 90, 1000, 2556, MAX_PERIOD];

/// Periods one past each published bound, plus two further out, which the declared
/// range refuses.
const REFUSED_PERIODS: [i64; 4] = [MIN_PERIOD - 1, MAX_PERIOD + 1, 0, -7];

/// Write `days` as the retention period, returning the response body.
async fn put(emulator: &Emulator, days: i64) -> Value {
    emulator
        .ok(
            "PutRetentionConfiguration",
            &json!({ "RetentionPeriodInDays": days }),
        )
        .await
}

/// The `RetentionConfigurations` list a `DescribeRetentionConfigurations` response
/// carries.
fn configurations(described: &Value) -> &Vec<Value> {
    described["RetentionConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("RetentionConfigurations must be a list, got {described}"))
}

/// Describe with `body` and return the reported configurations.
async fn describe(emulator: &Emulator, body: &Value) -> Value {
    emulator
        .ok("DescribeRetentionConfigurations", body)
        .await
}

#[tokio::test]
async fn a_written_retention_period_is_what_every_read_reports() {
    let emulator = Emulator::new();

    // Nothing was written yet, so a read reports an empty list — not a refusal.
    let empty = describe(&emulator, &json!({})).await;
    assert!(
        configurations(&empty).is_empty(),
        "case {:?}: expected no retention configuration before any write, got {empty}",
        "unwritten"
    );
    assert_eq!(
        empty["NextToken"],
        Value::Null,
        "case {:?}: a page that holds everything carries no NextToken, got {empty}",
        "unwritten"
    );

    for period in PERIODS {
        let case = format!("{period} days");

        let written = put(&emulator, period).await;
        let object = &written["RetentionConfiguration"];
        assert_eq!(
            object["Name"], OBJECT_NAME,
            "case {case:?}: expected the object Config names, got {written}"
        );
        assert_eq!(
            object["RetentionPeriodInDays"],
            json!(period),
            "case {case:?}: expected the written retention period back, got {written}"
        );

        // Read back unfiltered, and filtered by the name the write reported: both
        // report the same stored object.
        for filter in [json!({}), json!({ "RetentionConfigurationNames": [OBJECT_NAME] })] {
            let described = describe(&emulator, &filter).await;
            let reported = configurations(&described);
            assert_eq!(
                reported.len(),
                1,
                "case {case:?} filter {filter}: Config holds one retention configuration \
                 per Region, got {described}"
            );
            assert_eq!(
                reported[0]["Name"], OBJECT_NAME,
                "case {case:?} filter {filter}: expected the stored name, got {described}"
            );
            assert_eq!(
                reported[0]["RetentionPeriodInDays"],
                json!(period),
                "case {case:?} filter {filter}: expected the stored retention period, \
                 got {described}"
            );
            assert_eq!(
                described["NextToken"],
                Value::Null,
                "case {case:?} filter {filter}: a page that holds everything carries no \
                 NextToken, got {described}"
            );
        }
    }

    // An empty name list filters nothing, so it reports the same single object as an
    // absent one — and the last period written is the one in state.
    let described = describe(&emulator, &json!({ "RetentionConfigurationNames": [] })).await;
    let reported = configurations(&described);
    assert_eq!(
        reported.len(),
        1,
        "case {:?}: an empty name list filters nothing, got {described}",
        "empty name list"
    );
    assert_eq!(
        reported[0]["RetentionPeriodInDays"],
        json!(*PERIODS.last().expect("the period table is not empty")),
        "case {:?}: expected the retention period the last write stored, got {described}",
        "empty name list"
    );
}

#[tokio::test]
async fn reading_a_name_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();

    // Names no write of this account can have produced, including the stored name in
    // another case — a name is matched exactly.
    let absent: [&str; 4] = [OBJECT_NAME, "Default", "other-retention", "x"];

    for name in absent {
        let case = format!("empty store, name {name:?}");
        let (status, code) = emulator
            .refused(
                "DescribeRetentionConfigurations",
                &json!({ "RetentionConfigurationNames": [name] }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchRetentionConfigurationException"),
            "case {case:?}: expected the modelled refusal for an absent retention \
             configuration, got {status} {code}"
        );
    }

    put(&emulator, MIN_PERIOD).await;

    for name in absent.iter().filter(|name| **name != OBJECT_NAME) {
        let case = format!("stored {OBJECT_NAME:?}, name {name:?}");
        let (status, code) = emulator
            .refused(
                "DescribeRetentionConfigurations",
                &json!({ "RetentionConfigurationNames": [name] }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchRetentionConfigurationException"),
            "case {case:?}: expected the modelled refusal for an absent retention \
             configuration, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_delete_removes_the_configuration_and_a_second_one_is_refused() {
    let emulator = Emulator::new();

    for period in [MIN_PERIOD, MAX_PERIOD] {
        let case = format!("{period} days");

        put(&emulator, period).await;
        emulator
            .ok(
                "DeleteRetentionConfiguration",
                &json!({ "RetentionConfigurationName": OBJECT_NAME }),
            )
            .await;

        let described = describe(&emulator, &json!({})).await;
        assert!(
            configurations(&described).is_empty(),
            "case {case:?}: a deleted retention configuration must not be reported, \
             got {described}"
        );

        // Deleting what is no longer there, and a name that never existed, are the
        // same refusal.
        for name in [OBJECT_NAME, "never-written"] {
            let (status, code) = emulator
                .refused(
                    "DeleteRetentionConfiguration",
                    &json!({ "RetentionConfigurationName": name }),
                )
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NoSuchRetentionConfigurationException"),
                "case {case:?} name {name:?}: expected the modelled refusal for an \
                 absent retention configuration, got {status} {code}"
            );
        }
    }

    // A deleted object can be written again, and the read reports the new period.
    put(&emulator, 365).await;
    let described = describe(&emulator, &json!({})).await;
    assert_eq!(
        configurations(&described)[0]["RetentionPeriodInDays"],
        json!(365),
        "case {:?}: a configuration written after a delete must be reported, got {described}",
        "rewritten after delete"
    );
}

#[tokio::test]
async fn a_next_token_this_operation_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    put(&emulator, MIN_PERIOD).await;

    // Tokens no `DescribeRetentionConfigurations` response can have returned: the
    // token is opaque, so a malformed one is refused rather than ignored.
    for token in ["not a page token", "@@@@", "}{", "a~b"] {
        let (status, code) = emulator
            .refused(
                "DescribeRetentionConfigurations",
                &json!({ "NextToken": token }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {token:?}: expected the modelled refusal for a token this operation \
             did not issue, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_retention_period_outside_the_published_range_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    put(&emulator, MIN_PERIOD).await;

    for period in REFUSED_PERIODS {
        let case = format!("{period} days");
        let (status, code) = emulator
            .refused(
                "PutRetentionConfiguration",
                &json!({ "RetentionPeriodInDays": period }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a period outside the declared range must be refused, got \
             {status} {code}"
        );
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: the modelled range is refused by request validation before \
             the handler, got {code}"
        );

        // The refused write left the stored object exactly as it was.
        let described = describe(&emulator, &json!({})).await;
        let reported = configurations(&described);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: a refused write must not change the store, got {described}"
        );
        assert_eq!(
            reported[0]["RetentionPeriodInDays"],
            json!(MIN_PERIOD),
            "case {case:?}: a refused write must leave the stored period alone, got {described}"
        );
    }
}
