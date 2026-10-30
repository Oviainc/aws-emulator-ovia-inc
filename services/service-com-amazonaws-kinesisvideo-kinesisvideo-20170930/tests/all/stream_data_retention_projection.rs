// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for what the stream READERS project after a stream's
//! data retention has been moved.
//!
//! The rule under test: a stream's retention is the value left by the last
//! accepted `UpdateDataRetention` — not its creation-time value — and EVERY
//! reader that projects `StreamInfo` (`DescribeStream`, `ListStreams`) reports
//! that same current value together with the `Version` the last accepted update
//! minted. Chained updates therefore accumulate, each one quoting the version its
//! predecessor minted, and either identifier form (`StreamName`, `StreamARN`)
//! addresses the one stored record.
//!
//! Every case drives the real request walk (route -> deserialize -> validate ->
//! handle -> serialize) through `MultiServiceRouter::dispatch`, so what is
//! asserted is the STORED retention as a client on the socket would read it,
//! never the update's own (empty) answer.
//!
//! The router harness and the create/describe helpers are the ones the sibling
//! `stream_data_retention` suite already owns — both suites are modules of this
//! crate's single `tests/all.rs` target, so they are shared rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::stream_data_retention::{call, create_stream, harness, retention, stream_info, version};

/// How a case addresses the stream it updates. Either identifier form names the
/// same stored record, so a chain of updates may mix them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    /// Always by `StreamName`.
    ByName,
    /// Always by `StreamARN`.
    ByArn,
    /// By `StreamName` on even steps and by `StreamARN` on odd ones.
    Alternating,
}

/// The `StreamInfo` `ListStreams` projects for `name`: the SECOND reader of the
/// same stored record, reached through the listing rather than the describe.
async fn listed_stream_info(router: &MultiServiceRouter, name: &str) -> serde_json::Value {
    let (status, body) = call(router, "/listStreams", serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "case list {name:?}: expected HTTP 200, got {status} with {body:?}"
    );
    body.get("StreamInfoList")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("case list {name:?}: no StreamInfoList in {body:?}"))
        .iter()
        .find(|info| info.get("StreamName").and_then(serde_json::Value::as_str) == Some(name))
        .cloned()
        .unwrap_or_else(|| panic!("case list {name:?}: the stream is not in {body:?}"))
}

/// A `StreamInfo` member as a string.
fn text(info: &serde_json::Value, member: &str) -> String {
    info.get(member)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("no {member} in {info:?}"))
        .to_string()
}

/// A `StreamInfo` member as a number.
fn number(info: &serde_json::Value, member: &str) -> i64 {
    info.get(member)
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_else(|| panic!("no {member} in {info:?}"))
}

/// RULE: the retention every `StreamInfo` reader projects is the stream's CURRENT
/// retention — its previous value moved by each accepted `UpdateDataRetention`
/// delta — and not the value `CreateStream` was given, so chained updates
/// accumulate and `DescribeStream` and `ListStreams` agree both on the result and
/// on the `Version` the last accepted update minted.
#[tokio::test]
async fn every_reader_projects_the_retention_the_accepted_updates_left() {
    let router = harness();

    /// One row: a stream created with `initial` hours, the chain of updates
    /// applied to it, how each one addresses it, and the retention every reader
    /// must then project.
    struct Case {
        case: &'static str,
        initial: i32,
        steps: Vec<(&'static str, i32)>,
        address: Address,
        expected: i64,
    }

    // Rows differ along the created retention, the number of chained updates,
    // their directions and magnitudes (including the smallest the model allows,
    // `@range` min 1, a chain that returns to where it started, and chains that
    // reach each published bound), and which identifier form addresses the stream.
    let cases = vec![
        Case {
            case: "one increase from zero",
            initial: 0,
            steps: vec![("INCREASE_DATA_RETENTION", 1)],
            address: Address::ByName,
            expected: 1,
        },
        Case {
            case: "one increase from zero, addressed by ARN",
            initial: 0,
            steps: vec![("INCREASE_DATA_RETENTION", 7)],
            address: Address::ByArn,
            expected: 7,
        },
        Case {
            case: "three increases accumulate",
            initial: 0,
            steps: vec![
                ("INCREASE_DATA_RETENTION", 1),
                ("INCREASE_DATA_RETENTION", 2),
                ("INCREASE_DATA_RETENTION", 21),
            ],
            address: Address::ByName,
            expected: 24,
        },
        Case {
            case: "an increase then a decrease",
            initial: 10,
            steps: vec![
                ("INCREASE_DATA_RETENTION", 14),
                ("DECREASE_DATA_RETENTION", 4),
            ],
            address: Address::Alternating,
            expected: 20,
        },
        Case {
            case: "a round trip back to the created value",
            initial: 6,
            steps: vec![
                ("INCREASE_DATA_RETENTION", 12),
                ("DECREASE_DATA_RETENTION", 12),
            ],
            address: Address::Alternating,
            expected: 6,
        },
        Case {
            case: "down to the floor and back up",
            initial: 2,
            steps: vec![
                ("DECREASE_DATA_RETENTION", 2),
                ("INCREASE_DATA_RETENTION", 5),
            ],
            address: Address::ByArn,
            expected: 5,
        },
        Case {
            case: "up to the published ceiling and down again",
            initial: 87_000,
            steps: vec![
                ("INCREASE_DATA_RETENTION", 600),
                ("DECREASE_DATA_RETENTION", 87_600),
            ],
            address: Address::ByName,
            expected: 0,
        },
    ];

    for (row, row_case) in cases.into_iter().enumerate() {
        let Case {
            case,
            initial,
            steps,
            address,
            expected,
        } = row_case;
        // `StreamName`'s modeled pattern is ^[a-zA-Z0-9_.-]+$, so the row index
        // names the stream rather than the prose label.
        let name = format!("projection-row-{row}");
        create_stream(&router, &name, initial).await;

        let created = stream_info(&router, &name).await;
        let stream_arn = text(&created, "StreamARN");
        let mut current_version = text(&created, "Version");
        assert_eq!(
            number(&created, "DataRetentionInHours"),
            i64::from(initial),
            "case {case}: a fresh stream must report its created retention, got {created:?}"
        );

        for (index, (operation, change)) in steps.iter().enumerate() {
            let by_name = match address {
                Address::ByName => true,
                Address::ByArn => false,
                Address::Alternating => index % 2 == 0,
            };
            let mut body = serde_json::Map::new();
            if by_name {
                body.insert("StreamName".to_string(), serde_json::json!(&name));
            } else {
                body.insert("StreamARN".to_string(), serde_json::json!(&stream_arn));
            }
            // Each update quotes the version the PREVIOUS accepted call minted, so
            // a chain only runs if the version a reader reports is the live one.
            body.insert(
                "CurrentVersion".to_string(),
                serde_json::json!(&current_version),
            );
            body.insert("Operation".to_string(), serde_json::json!(operation));
            body.insert(
                "DataRetentionChangeInHours".to_string(),
                serde_json::json!(change),
            );
            let (status, answered) = call(
                &router,
                "/updateDataRetention",
                serde_json::Value::Object(body),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: step {index} ({operation} {change}) expected HTTP 200, \
                 got {status} with {answered:?}"
            );

            let minted = version(&router, &name).await;
            assert_ne!(
                minted, current_version,
                "case {case}: step {index} must mint a new Version, still {minted:?}"
            );
            current_version = minted;
        }

        let described = stream_info(&router, &name).await;
        let listed = listed_stream_info(&router, &name).await;
        for (reader, info) in [("DescribeStream", &described), ("ListStreams", &listed)] {
            let stored = number(info, "DataRetentionInHours");
            assert_eq!(
                stored, expected,
                "case {case}: {reader} expected DataRetentionInHours {expected}, got {stored}"
            );
            let reported = text(info, "Version");
            assert_eq!(
                reported, current_version,
                "case {case}: {reader} expected Version {current_version:?}, got {reported:?}"
            );
            let arn = text(info, "StreamARN");
            assert_eq!(
                arn, stream_arn,
                "case {case}: {reader} expected StreamARN {stream_arn:?}, got {arn:?}"
            );
        }
    }
}

/// RULE: the retention an accepted update left is STORED, so it survives later
/// reads and later mutations of other members of the same stream — a read never
/// rolls it back and an unrelated update never overwrites it.
#[tokio::test]
async fn a_moved_retention_survives_later_reads_and_unrelated_updates() {
    let router = harness();
    let name = "retention-survivor";
    create_stream(&router, name, 0).await;

    let current = version(&router, name).await;
    let (status, answered) = call(
        &router,
        "/updateDataRetention",
        serde_json::json!({
            "StreamName": name,
            "CurrentVersion": current,
            "Operation": "INCREASE_DATA_RETENTION",
            "DataRetentionChangeInHours": 48,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case update: expected HTTP 200, got {status} with {answered:?}"
    );

    // (the case, the path, the request body) — each a later call that must leave
    // the retention where the accepted update put it.
    let later_calls: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "a repeated describe",
            "/describeStream",
            serde_json::json!({ "StreamName": name }),
        ),
        ("a listing", "/listStreams", serde_json::json!({})),
        (
            "a tag write",
            "/tagStream",
            serde_json::json!({
                "StreamName": name,
                "Tags": { "owner": "retention-suite" },
            }),
        ),
        (
            "an unrelated configuration update",
            "/updateNotificationConfiguration",
            serde_json::json!({
                "StreamName": name,
                "NotificationConfiguration": {
                    "Status": "ENABLED",
                    "DestinationConfig": {
                        "Uri": "arn:aws:sns:us-east-1:000000000000:kvs-fragment-notifications"
                    }
                },
            }),
        ),
    ];

    for (case, path, body) in later_calls {
        let (status, answered) = call(&router, path, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {answered:?}"
        );
        let stored = retention(&router, name).await;
        assert_eq!(
            stored, 48,
            "case {case}: expected DataRetentionInHours 48, got {stored}"
        );
    }
}
