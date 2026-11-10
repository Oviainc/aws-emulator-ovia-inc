// SPDX-License-Identifier: CC0-1.0

//! The release of the SQL engine this data plane is BUILT on, and that statements are
//! still planned and evaluated by that engine once it is raised.
//!
//! Every statement here goes through the router, so routing, restJson1 deserialization,
//! the handler and serialization are all exercised; the client harness is the one
//! `sql_data_api` builds.

use serde_json::json;

use crate::sql_data_api::DataApi;

/// The lowest engine release a SQL data plane of this emulator may be built on, written
/// the way the engine writes its own version number — `major * 1_000_000`, plus
/// `minor * 1_000`, plus `patch` — which is the encoding `rusqlite::version_number`
/// reports. Below 3.54.0 a query over a database whose write-ahead log declares an
/// undersized page can be answered with page-cache bytes the database never wrote, so no
/// build of this workspace links one.
const ENGINE_RELEASE_FLOOR: i32 = 3_054_000;

/// One dotted engine release, encoded the way the engine encodes its own.
///
/// A release carries three components; a shorter one counts its missing components as
/// zero, so the floor's own components can be written as published.
fn encoded(release: &str) -> i32 {
    let mut number = 0;
    for (place, component) in [1_000_000, 1_000, 1].into_iter().zip(
        release
            .split('.')
            .map(|component| component.parse::<i32>().unwrap_or(0))
            .chain(std::iter::repeat(0)),
    ) {
        number += place * component;
    }
    number
}

/// RULE: a SQL data plane of this emulator is built on the bundled engine release the
/// workspace locks, and that release is at or past the first one whose page cache cannot
/// disclose bytes the database never wrote — so a build on any earlier release is
/// refused, whichever way the release is read.
///
/// The rows span the published bound both ways: the floor itself, one step past it in
/// each of the three components it is written in, one step below it, a release older
/// than the issue cited, and the release this workspace actually linked. The encoding is
/// the engine's own documented `SQLITE_VERSION_NUMBER` form, which is also what
/// `rusqlite::version_number` reports.
#[test]
fn the_linked_sql_engine_is_at_or_past_the_release_floor() {
    // (case, the release, whether a data plane may be built on it)
    let cases = [
        ("the floor itself", "3.54.0".to_string(), true),
        ("one patch past the floor", "3.54.1".to_string(), true),
        ("one minor past the floor", "3.55.0".to_string(), true),
        ("one major past the floor", "4.0.0".to_string(), true),
        ("one patch below the floor", "3.53.9".to_string(), false),
        ("one minor below the floor", "3.53.0".to_string(), false),
        (
            "the release the previous lock file held",
            "3.49.1".to_string(),
            false,
        ),
        (
            "the release this workspace linked",
            rusqlite::version().to_string(),
            true,
        ),
    ];

    for (case, release, allowed) in cases {
        let number = encoded(&release);
        assert_eq!(
            number >= ENGINE_RELEASE_FLOOR,
            allowed,
            "case {case:?}: release {release:?} encodes to {number}, so \
             \"may be built on\" is {}, expected {allowed}",
            number >= ENGINE_RELEASE_FLOOR
        );
    }

    // The floor is written in the engine's own encoding, and the engine the data plane
    // links reports the same release both ways: the dotted string and the number.
    assert_eq!(
        encoded("3.54.0"),
        ENGINE_RELEASE_FLOOR,
        "case \"the floor's own encoding\": expected {ENGINE_RELEASE_FLOOR}, got {}",
        encoded("3.54.0")
    );
    let linked = rusqlite::version_number();
    assert_eq!(
        linked,
        encoded(rusqlite::version()),
        "case \"the linked engine reports one release\": the number says {linked} and \
         the string says {:?}",
        rusqlite::version()
    );
}

/// RULE: a statement a Data API call carries is PLANNED AND EVALUATED by that engine, so
/// the value a response reports for an expression is the one the engine computed, not one
/// the service read off the request.
///
/// The rows span the kinds of work a plan does: arithmetic, a scalar function, a cast, a
/// conditional and an aggregate over stored rows — each one a value no part of the
/// request spells.
#[tokio::test]
async fn an_expression_is_evaluated_by_that_engine() {
    // (case, the selected expression, the records the engine must compute — one row per
    //  row the plan produces, so a row-wise expression answers once per stored row and
    //  an aggregate answers once)
    let cases = [
        (
            "integer arithmetic",
            "2 + 3 * 4",
            json!([
                [{"longValue": 14}],
                [{"longValue": 14}],
                [{"longValue": 14}],
            ]),
        ),
        (
            "an aggregate over the stored rows",
            "sum(amount)",
            json!([[{"longValue": 60}]]),
        ),
        (
            "a count of the stored rows",
            "count(*)",
            json!([[{"longValue": 3}]]),
        ),
        (
            "a scalar string function",
            "upper(max(label))",
            json!([[{"stringValue": "THIRD"}]]),
        ),
        (
            "a cast of text to a number",
            "CAST('7' AS INTEGER) + min(amount)",
            json!([[{"longValue": 17}]]),
        ),
        (
            "a conditional",
            "CASE WHEN sum(amount) > 50 THEN 'over' ELSE 'under' END",
            json!([[{"stringValue": "over"}]]),
        ),
    ];

    for (case, expression, expected) in cases {
        let api = DataApi::new();
        api.run(json!({"sql": "CREATE TABLE ledger (amount INT, label VARCHAR(16))"}))
            .await;
        api.run(json!({
            "sql": "INSERT INTO ledger (amount, label) VALUES \
                    (10, 'first'), (20, 'second'), (30, 'third')",
        }))
        .await;

        let answered = api
            .run(json!({"sql": format!("SELECT {expression} AS value FROM ledger")}))
            .await;
        assert_eq!(
            answered["records"], expected,
            "case {case:?}: expected the engine to compute {expected}, got {answered}"
        );
    }
}
