// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a table's Kinesis streaming destination DOES: where an enable
//! settles, what it reports when it could not be completed, how many streams one table
//! replicates to, and the change records a write delivers into the Kinesis data stream.
//!
//! The rules pinned here:
//!
//! 1. `StreamArn` carries a length bound and no other constraint
//!    (`smithy_query(query="operation",operation="EnableKinesisStreamingDestination")`:
//!    `StreamArn`, len 37..1024), so EVERY value that clears it is ACCEPTED by the call,
//!    which answers `ENABLING`; whether it names a usable Kinesis data stream of the
//!    table's own Region is then resolved asynchronously, and a destination that resolves
//!    to none settles `ENABLE_FAILED` carrying the human-readable
//!    `DestinationStatusDescription` of why ("the human-readable string that corresponds to
//!    the replica status");
//! 2. a table replicates to AT MOST ONE Kinesis data stream: while it has a live
//!    destination — `ENABLING`, `ACTIVE` or `UPDATING` — every enable is refused with HTTP
//!    400 `ValidationException` and DynamoDB's enable-state sentence, whichever stream it
//!    names, and an enable is served again once that replication has stopped or failed;
//! 3. an item change made on a table whose destination is replicating is DELIVERED to that
//!    destination's stream as one Kinesis data record per change, carrying the
//!    modification, the item's key and the images the change left;
//! 4. `ApproximateCreationDateTime` is stated in the unit the destination's
//!    `ApproximateCreationDateTimePrecision` names — MILLISECOND or MICROSECOND, and
//!    MILLISECOND for a destination no command named one for — and a configuration change
//!    switches the unit of the records delivered AFTER it;
//! 5. the records already delivered, and the stream itself, are the Kinesis service's: they
//!    outlive the DynamoDB table that delivered them;
//! 6. only a destination that is replicating delivers: a registration still in flight, one
//!    that failed and one that has been stopped deliver nothing.
//!
//! Each rule is asserted over a SET of inputs — the precision in force, the modification
//! made, why a `StreamArn` resolves to no stream, and the status the live destination is in
//! — rather than one example. Every command, every read and every record is driven over the
//! wire (`POST /` with `X-Amz-Target: <Service>.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, with the DynamoDB and the Kinesis
//! service mounted on ONE shared engine and nothing seeded through either store.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::contributor_insights::{create_table, settle};
use crate::emulator::{
    call, emulator, ensure_kinesis_stream, error_code, message, ok, ok_kinesis, REGION,
};

/// Both values `ApproximateCreationDateTimePrecision` declares
/// (`smithy_query(query="shape",shape="ApproximateCreationDateTimePrecision")`:
/// `MILLISECOND | MICROSECOND`).
const MILLISECOND: &str = "MILLISECOND";
const MICROSECOND: &str = "MICROSECOND";

/// DynamoDB's refusal of an enable the table's streaming state cannot serve.
fn not_enableable_message() -> String {
    "Table is not in a valid state to enable Kinesis Streaming Destination: \
     EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform ENABLE \
     operation."
        .to_string()
}

/// Start replication of `table` to `stream_arn`, under `precision` where a row names one,
/// and answer the body the command reported.
async fn enable(
    router: &MultiServiceRouter,
    table: &str,
    stream_arn: &str,
    precision: Option<&str>,
) -> Value {
    let mut request = json!({"TableName": table, "StreamArn": stream_arn});
    if let Some(precision) = precision {
        request["EnableKinesisStreamingConfiguration"] =
            json!({"ApproximateCreationDateTimePrecision": precision});
    }
    ok(router, "EnableKinesisStreamingDestination", request).await
}

/// The destination `table` reports for `stream_arn`, or `Value::Null` where it reports
/// none.
async fn described(router: &MultiServiceRouter, table: &str, stream_arn: &str) -> Value {
    ok(
        router,
        "DescribeKinesisStreamingDestination",
        json!({"TableName": table}),
    )
    .await["KinesisDataStreamDestinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|destination| destination["StreamArn"] == json!(stream_arn))
        .unwrap_or(Value::Null)
}

/// Store one item of `table` under the key `id`, carrying `value`.
async fn put_item(router: &MultiServiceRouter, table: &str, id: &str, value: &str) {
    ok(
        router,
        "PutItem",
        json!({
            "TableName": table,
            "Item": {"Part": {"S": id}, "Value": {"S": value}},
        }),
    )
    .await;
}

/// Remove the item of `table` under the key `id`.
async fn delete_item(router: &MultiServiceRouter, table: &str, id: &str) {
    ok(
        router,
        "DeleteItem",
        json!({"TableName": table, "Key": {"Part": {"S": id}}}),
    )
    .await;
}

/// Every record the Kinesis data stream `stream` holds, oldest first, each parsed from the
/// JSON document its `Data` blob carries — what a change-data-capture consumer of a table's
/// streaming destination reads.
///
/// The drain itself is the harness's ([`crate::emulator::drain_kinesis_stream`]); this
/// suite is about the records rather than where in the shard map they landed, so the shard
/// each came from is dropped here.
async fn delivered(router: &MultiServiceRouter, case: &str, stream: &str) -> Vec<Value> {
    crate::emulator::drain_kinesis_stream(router, case, stream)
        .await
        .into_iter()
        .map(|(_, document)| document)
        .collect()
}

/// Epoch milliseconds now — what a record's creation time is compared against.
fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_millis()
}

/// Assert `record`'s creation time states the present in the unit `precision` names.
///
/// The two units are a factor of a thousand apart, so a generous window around the test's
/// own clock pins WHICH unit the number is in: microseconds read as milliseconds would be
/// tens of thousands of years from now.
fn assert_creation_time(case: &str, record: &Value, precision: &str) {
    let modification = &record["dynamodb"];
    assert_eq!(
        modification["ApproximateCreationDateTimePrecision"],
        json!(precision),
        "case {case:?}: the record names the unit its creation time is in, got {record}"
    );
    let stated = modification["ApproximateCreationDateTime"]
        .as_u64()
        .unwrap_or_else(|| {
            panic!("case {case:?}: every record carries ApproximateCreationDateTime, got {record}")
        }) as i128;
    let (expected, tolerance) = if precision == MICROSECOND {
        (now_millis() as i128 * 1_000, 120_000_000i128)
    } else {
        (now_millis() as i128, 120_000i128)
    };
    assert!(
        (stated - expected).abs() < tolerance,
        "case {case:?}: expected a creation time near {expected} in {precision}, got {stated}"
    );
}

/// Rules 3 and 4: every modification a write makes is delivered as one record, in the unit
/// the precision in force names — including the default unit for a destination no command
/// named a precision for.
#[tokio::test]
async fn every_item_change_is_delivered_in_the_precision_in_force() {
    // (case, the precision the enable names, the unit the records are stated in)
    let cases: [(&str, Option<&str>, &str); 3] = [
        ("no precision named", None, MILLISECOND),
        ("millisecond named", Some(MILLISECOND), MILLISECOND),
        ("microsecond named", Some(MICROSECOND), MICROSECOND),
    ];

    for (case, named, unit) in cases {
        let router = emulator();
        let stream = "cdc";
        let stream_arn = ensure_kinesis_stream(&router, stream).await;
        create_table(&router, "Delivering", &[]).await;
        enable(&router, "Delivering", &stream_arn, named).await;
        settle().await;

        // One of each modification the change feed describes, in order.
        put_item(&router, "Delivering", "row", "first").await;
        put_item(&router, "Delivering", "row", "second").await;
        delete_item(&router, "Delivering", "row").await;

        let records = delivered(&router, case, stream).await;
        let names: Vec<&str> = records
            .iter()
            .map(|record| record["eventName"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            vec!["INSERT", "MODIFY", "REMOVE"],
            "case {case:?}: expected one record per modification, got {records:?}"
        );

        for record in &records {
            assert_eq!(
                (
                    record["awsRegion"].clone(),
                    record["tableName"].clone(),
                    record["eventSource"].clone(),
                    record["recordFormat"].clone(),
                    record["userIdentity"].clone(),
                ),
                (
                    json!(REGION),
                    json!("Delivering"),
                    json!("aws:dynamodb"),
                    json!("application/json"),
                    Value::Null,
                ),
                "case {case:?}: a record names where it came from, got {record}"
            );
            assert!(
                record["eventID"].as_str().is_some_and(|id| !id.is_empty()),
                "case {case:?}: every record identifies itself, got {record}"
            );
            assert_eq!(
                record["dynamodb"]["Keys"],
                json!({"Part": {"S": "row"}}),
                "case {case:?}: a record carries the key of the item it is about, got {record}"
            );
            assert!(
                record["dynamodb"]["SizeBytes"].as_i64().unwrap_or_default() > 0,
                "case {case:?}: a record reports the size of the item, got {record}"
            );
            assert_creation_time(case, record, unit);
        }

        // The images each modification left: an insert has only the new item, a
        // modification both, and a removal only the one that is gone.
        assert_eq!(
            (
                records[0]["dynamodb"]["NewImage"].clone(),
                records[0]["dynamodb"]["OldImage"].clone(),
            ),
            (
                json!({"Part": {"S": "row"}, "Value": {"S": "first"}}),
                Value::Null,
            ),
            "case {case:?}: an insert carries the item it stored, got {:?}",
            records[0]
        );
        assert_eq!(
            (
                records[1]["dynamodb"]["NewImage"].clone(),
                records[1]["dynamodb"]["OldImage"].clone(),
            ),
            (
                json!({"Part": {"S": "row"}, "Value": {"S": "second"}}),
                json!({"Part": {"S": "row"}, "Value": {"S": "first"}}),
            ),
            "case {case:?}: a modification carries both images, got {:?}",
            records[1]
        );
        assert_eq!(
            (
                records[2]["dynamodb"]["NewImage"].clone(),
                records[2]["dynamodb"]["OldImage"].clone(),
            ),
            (
                Value::Null,
                json!({"Part": {"S": "row"}, "Value": {"S": "second"}}),
            ),
            "case {case:?}: a removal carries the item that is gone, got {:?}",
            records[2]
        );
    }
}

/// Rule 4, the other half: a configuration change switches the unit of the records
/// delivered after it and leaves the ones delivered before it stated as they were.
#[tokio::test]
async fn a_precision_change_switches_the_unit_of_later_records() {
    let router = emulator();
    let stream = "cdc";
    let stream_arn = ensure_kinesis_stream(&router, stream).await;
    create_table(&router, "Switching", &[]).await;
    enable(&router, "Switching", &stream_arn, Some(MILLISECOND)).await;
    settle().await;
    put_item(&router, "Switching", "before", "x").await;

    ok(
        &router,
        "UpdateKinesisStreamingDestination",
        json!({
            "TableName": "Switching",
            "StreamArn": stream_arn.clone(),
            "UpdateKinesisStreamingConfiguration": {
                "ApproximateCreationDateTimePrecision": MICROSECOND,
            },
        }),
    )
    .await;
    settle().await;
    put_item(&router, "Switching", "after", "y").await;

    let records = delivered(&router, "a precision change", stream).await;
    assert_eq!(
        records.len(),
        2,
        "case \"a precision change\": both writes are delivered, got {records:?}"
    );
    assert_creation_time("the record before the change", &records[0], MILLISECOND);
    assert_creation_time("the record after the change", &records[1], MICROSECOND);
}

/// Rule 5: the stream and the records already delivered belong to Kinesis, so they are
/// still there once the table that delivered them is gone.
#[tokio::test]
async fn the_stream_and_its_records_outlive_the_table() {
    let router = emulator();
    let stream = "cdc";
    let stream_arn = ensure_kinesis_stream(&router, stream).await;
    create_table(&router, "Transient", &[]).await;
    enable(&router, "Transient", &stream_arn, Some(MICROSECOND)).await;
    settle().await;
    for (id, value) in [("a", "1"), ("b", "2")] {
        put_item(&router, "Transient", id, value).await;
    }
    let before = delivered(&router, "before the delete", stream).await;
    assert_eq!(
        before.len(),
        2,
        "case \"before the delete\": both writes are delivered, got {before:?}"
    );

    ok(&router, "DeleteTable", json!({"TableName": "Transient"})).await;
    settle().await;
    let (status, body) = call(
        &router,
        "DescribeKinesisStreamingDestination",
        json!({"TableName": "Transient"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "ResourceNotFoundException".to_string()),
        "case \"after the delete\": the table is gone, got {status} with {body}"
    );

    let summary = ok_kinesis(
        &router,
        "DescribeStreamSummary",
        json!({"StreamName": stream}),
    )
    .await;
    assert_eq!(
        summary["StreamDescriptionSummary"]["StreamStatus"],
        json!("ACTIVE"),
        "case \"after the delete\": the stream survives its table, got {summary}"
    );
    let after = delivered(&router, "after the delete", stream).await;
    assert_eq!(
        after, before,
        "case \"after the delete\": the records already delivered survive, got {after:?}"
    );
}

/// Rule 6: only a destination that is replicating delivers. The rows differ along where the
/// destination's own lifecycle is when the write lands.
#[tokio::test]
async fn only_a_replicating_destination_delivers() {
    // (case, whether a destination is registered at all, whether its registration is
    //  waited out, whether replication is then stopped, how many records the write delivers)
    let cases: [(&str, bool, bool, bool, usize); 4] = [
        ("no destination at all", false, false, false, 0),
        ("the registration still in flight", true, false, false, 0),
        ("replicating", true, true, false, 1),
        ("replication stopped", true, true, true, 0),
    ];

    for (case, registered, waited, stopped, expected) in cases {
        let router = emulator();
        let stream = "cdc";
        let stream_arn = ensure_kinesis_stream(&router, stream).await;
        create_table(&router, "Lifecycle", &[]).await;
        // The write below is served by the table itself, so the create's own span is waited
        // out before the row's destination is arranged: the row is about where the
        // DESTINATION's lifecycle is, not about the table's.
        settle().await;
        if registered {
            enable(&router, "Lifecycle", &stream_arn, Some(MILLISECOND)).await;
        }
        if waited {
            settle().await;
        }
        if stopped {
            ok(
                &router,
                "DisableKinesisStreamingDestination",
                json!({"TableName": "Lifecycle", "StreamArn": stream_arn.clone()}),
            )
            .await;
            settle().await;
        }

        put_item(&router, "Lifecycle", "row", "value").await;
        let records = delivered(&router, case, stream).await;
        assert_eq!(
            records.len(),
            expected,
            "case {case:?}: expected {expected} delivered record(s), got {records:?}"
        );
    }
}

/// Rule 1: every `StreamArn` that clears the member's length bound is ACCEPTED, and the
/// destination settles on whether it resolves to a usable stream — reporting why it could
/// not where it did not.
#[tokio::test]
async fn an_unresolvable_stream_arn_settles_enable_failed() {
    let router = emulator();
    // The one stream that exists, in the region the suite drives: the control row.
    let live = ensure_kinesis_stream(&router, "live").await;
    // Rows differ along WHY the value resolves to no stream of this table's Region: the
    // value is no ARN at all, it names another service's resource, another resource type of
    // this one, a stream of another Region, and a stream of this Region that nothing
    // created. Every value clears the modelled 37..=1024 length bound.
    let elsewhere = engine::emulator_arn("kinesis", "eu-west-3", "stream/live");
    let absent = engine::emulator_arn("kinesis", REGION, "stream/never-created");
    let cases: Vec<(&str, String, &str)> = vec![
        ("the stream that exists", live.clone(), "ACTIVE"),
        (
            "a value that is no ARN",
            "this-is-not-an-arn-but-clears-the-length-floor".to_string(),
            "ENABLE_FAILED",
        ),
        (
            "another service's resource",
            engine::emulator_arn("dynamodb", REGION, "table/Other"),
            "ENABLE_FAILED",
        ),
        (
            "another resource type of Kinesis",
            engine::emulator_arn("kinesis", REGION, "channel/live"),
            "ENABLE_FAILED",
        ),
        ("a stream of another Region", elsewhere, "ENABLE_FAILED"),
        ("a stream nothing created", absent, "ENABLE_FAILED"),
    ];

    for (index, (case, value, settled)) in cases.into_iter().enumerate() {
        let table = format!("Resolving{index}");
        create_table(&router, &table, &[]).await;
        // The call ACCEPTS the value and reports the transient status: whether it resolves
        // is answered asynchronously.
        let answered = enable(&router, &table, &value, None).await;
        assert_eq!(
            answered["DestinationStatus"],
            json!("ENABLING"),
            "case {case:?}: expected the transient status, got {answered}"
        );

        settle().await;
        let reported = described(&router, &table, &value).await;
        assert_eq!(
            reported["DestinationStatus"],
            json!(settled),
            "case {case:?}: expected the destination to settle on {settled}, got {reported}"
        );
        let description = reported["DestinationStatusDescription"].clone();
        if settled == "ENABLE_FAILED" {
            assert!(
                description
                    .as_str()
                    .is_some_and(|text| text.contains(&value)),
                "case {case:?}: a failed destination describes why over the value it named, got \
                 {reported}"
            );
        } else {
            assert_eq!(
                description,
                Value::Null,
                "case {case:?}: a destination that replicates describes no failure, got {reported}"
            );
        }
    }

    // A `StreamArn` SHORTER than the member's documented minimum is still refused, by the
    // boundary validator's own length clause: the bound is the one constraint the member
    // carries, so it is also the only thing this call refuses the value for.
    let short = "arn:aws:kinesis:x";
    let (status, body) = call(
        &router,
        "EnableKinesisStreamingDestination",
        json!({"TableName": "Resolving0", "StreamArn": short}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "ValidationException".to_string()),
        "case {short:?}: expected the length refusal, got {status} with {body}"
    );
}

/// Rule 2: one live destination per table. Every enable is refused while replication is
/// live — whichever stream it names and whatever transient status that replication is in —
/// and served again once the replication has stopped or failed.
#[tokio::test]
async fn a_table_replicates_to_at_most_one_stream() {
    // (case, how the live destination is left before the second enable)
    let live_states = ["still registering", "replicating", "mid configuration change"];

    for (index, case) in live_states.into_iter().enumerate() {
        let router = emulator();
        let first = ensure_kinesis_stream(&router, "first").await;
        let second = ensure_kinesis_stream(&router, "second").await;
        let table = format!("Single{index}");
        create_table(&router, &table, &[]).await;
        enable(&router, &table, &first, Some(MILLISECOND)).await;
        if case != "still registering" {
            settle().await;
        }
        if case == "mid configuration change" {
            ok(
                &router,
                "UpdateKinesisStreamingDestination",
                json!({"TableName": table.clone(), "StreamArn": first.clone()}),
            )
            .await;
        }

        // A SECOND stream, and the stream already live: the table's streaming state refuses
        // both the same way.
        for (addressed, arn) in [("another stream", &second), ("the same stream", &first)] {
            let (status, body) = call(
                &router,
                "EnableKinesisStreamingDestination",
                json!({"TableName": table.clone(), "StreamArn": arn.clone()}),
            )
            .await;
            let wanted = not_enableable_message();
            assert_eq!(
                (status, error_code(&body), message(&body)),
                (400, "ValidationException".to_string(), wanted.clone()),
                "case {case:?} with {addressed:?}: expected {wanted:?}, got {status} with {body}"
            );
        }

        // The refusals registered nothing: the table still holds its one destination.
        settle().await;
        let report = ok(
            &router,
            "DescribeKinesisStreamingDestination",
            json!({"TableName": table.clone()}),
        )
        .await;
        assert_eq!(
            report["KinesisDataStreamDestinations"]
                .as_array()
                .map(Vec::len),
            Some(1),
            "case {case:?}: a refused enable registers no destination, got {report}"
        );
        assert_eq!(
            described(&router, &table, &first).await["DestinationStatus"],
            json!("ACTIVE"),
            "case {case:?}: the live destination is untouched, got {report}"
        );

        // Once replication has STOPPED the table takes an enable again — for the other
        // stream, which is what the refusals above were about.
        ok(
            &router,
            "DisableKinesisStreamingDestination",
            json!({"TableName": table.clone(), "StreamArn": first.clone()}),
        )
        .await;
        settle().await;
        let answered = enable(&router, &table, &second, Some(MICROSECOND)).await;
        assert_eq!(
            answered["DestinationStatus"],
            json!("ENABLING"),
            "case {case:?}: an enable is served once replication has stopped, got {answered}"
        );
        settle().await;
        assert_eq!(
            (
                described(&router, &table, &first).await["DestinationStatus"].clone(),
                described(&router, &table, &second).await["DestinationStatus"].clone(),
            ),
            (json!("DISABLED"), json!("ACTIVE")),
            "case {case:?}: the stopped record is history and the new stream replicates, got \
             {report}"
        );
    }
}

/// Rule 2, the other end: an enable may start again from a destination whose own enable
/// FAILED, and the retry delivers the table's changes once the stream is there.
#[tokio::test]
async fn an_enable_starts_again_from_a_failed_destination() {
    let router = emulator();
    let stream = "created-later";
    let arn = engine::emulator_arn("kinesis", REGION, &format!("stream/{stream}"));
    create_table(&router, "Retried", &[]).await;
    enable(&router, "Retried", &arn, Some(MILLISECOND)).await;
    settle().await;
    let failed = described(&router, "Retried", &arn).await;
    assert_eq!(
        failed["DestinationStatus"],
        json!("ENABLE_FAILED"),
        "case \"before the stream exists\": expected ENABLE_FAILED, got {failed}"
    );

    // The stream exists now, so the same command registers the destination again in place
    // and it replicates — which the delivered record is the proof of.
    let created = ensure_kinesis_stream(&router, stream).await;
    assert_eq!(
        created, arn,
        "case \"the created stream\": the suite named the ARN the Kinesis service mints"
    );
    enable(&router, "Retried", &arn, Some(MILLISECOND)).await;
    settle().await;
    put_item(&router, "Retried", "row", "value").await;

    let records = delivered(&router, "after the retry", stream).await;
    assert_eq!(
        records.len(),
        1,
        "case \"after the retry\": the retried destination delivers, got {records:?}"
    );
    assert_eq!(
        records[0]["eventName"],
        json!("INSERT"),
        "case \"after the retry\": expected the insert, got {:?}",
        records[0]
    );
    let reported = described(&router, "Retried", &arn).await;
    assert_eq!(
        (
            reported["DestinationStatus"].clone(),
            reported["DestinationStatusDescription"].clone(),
        ),
        (json!("ACTIVE"), Value::Null),
        "case \"after the retry\": the record carries no failure any more, got {reported}"
    );
}
