// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHERE a table's Kinesis streaming destination settles: the
//! `StreamArn` a command names is resolved against the Kinesis service's own stream
//! registry, and a destination whose stream is not there never replicates.
//!
//! The rules pinned here:
//!
//! 1. `DestinationStatus` has `ENABLE_FAILED` for an enable that cannot be completed, so a
//!    destination registered for a stream the Kinesis service does not hold is `ENABLING`
//!    for the transient span and settles `ENABLE_FAILED` — never `ACTIVE`, whatever the
//!    request's configuration or the table it names;
//! 2. the stream is resolved at the ENDPOINT the command was signed for, so a stream that
//!    exists in another region resolves no destination in this one;
//! 3. a failed enable is a status an enable may start from: once the stream exists, the
//!    same command registers the destination again in place and it settles `ACTIVE` —
//!    whereas replication that IS running cannot be started twice;
//! 4. the destination a stream resolves for still replicates: an enable naming a stream
//!    the Kinesis service holds settles `ACTIVE`, which is the control the rows above are
//!    read against.
//!
//! Every command and every read is driven over the wire (`POST /` with
//! `X-Amz-Target: <Service>.<OperationName>`) through the same `MultiServiceRouter` the
//! emulator's front doors use, with the DynamoDB and the Kinesis service mounted on ONE
//! shared engine — which is how the router mounts them — and nothing seeded through either
//! store.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::contributor_insights::{create_table, settle};
use crate::emulator::{call, emulator, ensure_kinesis_stream, error_code, message, ok, REGION};

/// The ARN of a Kinesis data stream in the region the suite drives, built through the same
/// ARN helper the emulator mints every other ARN with — for a stream nothing created.
fn stream_arn(name: &str) -> String {
    engine::emulator_arn("kinesis", REGION, &format!("stream/{name}"))
}

/// Start replication of `table` to `arn` and answer the status the command reports.
async fn enable(router: &MultiServiceRouter, table: &str, arn: &str) -> String {
    ok(
        router,
        "EnableKinesisStreamingDestination",
        json!({"TableName": table, "StreamArn": arn}),
    )
    .await["DestinationStatus"]
        .as_str()
        .unwrap_or("<no status>")
        .to_string()
}

/// The status the reader reports for `table`'s destination for `arn`, or `"<absent>"` when
/// the table holds no record for that stream.
async fn reported(router: &MultiServiceRouter, table: &str, arn: &str) -> String {
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
        .find(|destination| destination["StreamArn"] == json!(arn))
        .map_or("<absent>".to_string(), |destination| {
            destination["DestinationStatus"]
                .as_str()
                .unwrap_or("<no status>")
                .to_string()
        })
}

/// Rules 1, 2 and 4: where a destination settles is decided by the stream its `StreamArn`
/// resolves to, and the transient status it is reported in first is the same either way.
#[tokio::test]
async fn a_destination_settles_on_whether_its_stream_resolves() {
    let router = emulator();
    create_table(&router, "Resolving", &[]).await;
    // One stream the Kinesis service holds in THIS region, and one it holds in another:
    // the second is the row that shows the resolution is per endpoint.
    let live = ensure_kinesis_stream(&router, "live").await;

    // Rows differ along what the `StreamArn` names: a stream the Kinesis service holds, a
    // name nothing created, a name that reads like one of the suite's but is not, and a
    // stream of another region's endpoint.
    let elsewhere = engine::emulator_arn("kinesis", "eu-west-3", "stream/live");
    let cases: [(&str, String, &str); 4] = [
        ("a stream the registry holds", live.clone(), "ACTIVE"),
        (
            "a stream nothing created",
            stream_arn("no-such-stream"),
            "ENABLE_FAILED",
        ),
        (
            "a near miss of a stream that exists",
            stream_arn("live-2"),
            "ENABLE_FAILED",
        ),
        (
            "a stream of another region's endpoint",
            elsewhere,
            "ENABLE_FAILED",
        ),
    ];

    for (case, arn, settled) in cases {
        // Registering is asynchronous either way, so the command reports the transient
        // status whichever end it is heading for.
        let answered = enable(&router, "Resolving", &arn).await;
        assert_eq!(
            answered, "ENABLING",
            "case {case:?}: expected the transient status, got {answered}"
        );
        let registering = reported(&router, "Resolving", &arn).await;
        assert_eq!(
            registering, "ENABLING",
            "case {case:?}: the reader reports the transient status too, got {registering}"
        );

        settle().await;
        let reached = reported(&router, "Resolving", &arn).await;
        assert_eq!(
            reached, settled,
            "case {case:?}: expected the destination to settle on {settled}, got {reached}"
        );
    }
}

/// Rule 3: `ENABLE_FAILED` is a status an enable may start from, and `ACTIVE` is not.
#[tokio::test]
async fn an_enable_may_start_again_from_a_failed_destination_but_not_from_a_live_one() {
    let router = emulator();
    create_table(&router, "Retried", &[]).await;
    let arn = stream_arn("created-later");

    // The stream does not exist yet, so the first enable fails.
    enable(&router, "Retried", &arn).await;
    settle().await;
    let failed = reported(&router, "Retried", &arn).await;
    assert_eq!(
        failed, "ENABLE_FAILED",
        "case \"before the stream exists\": expected ENABLE_FAILED, got {failed}"
    );

    // Create the stream and start replication again: the record is re-registered in place
    // and settles ACTIVE, so the failure is not terminal for the table.
    ensure_kinesis_stream(&router, "created-later").await;
    let answered = enable(&router, "Retried", &arn).await;
    assert_eq!(
        answered, "ENABLING",
        "case \"after the stream exists\": expected the transient status, got {answered}"
    );
    settle().await;
    let replicating = reported(&router, "Retried", &arn).await;
    assert_eq!(
        replicating, "ACTIVE",
        "case \"after the stream exists\": expected ACTIVE, got {replicating}"
    );

    // Replication that is already running cannot be started again, which is the half of
    // the rule the failed status is the exception to.
    let (status, body) = call(
        &router,
        "EnableKinesisStreamingDestination",
        json!({"TableName": "Retried", "StreamArn": arn}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body), message(&body)),
        (
            400,
            "ValidationException".to_string(),
            "Table is not in a valid state to enable Kinesis Streaming Destination: \
             EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform \
             ENABLE operation."
                .to_string()
        ),
        "case \"a third enable\": expected DynamoDB's enable-state sentence, got {status} with \
         {body}"
    );
}

/// A destination that never replicated is still the table's own record: the reader reports
/// it, and it is told apart from a stream the table has no record of at all.
#[tokio::test]
async fn a_failed_destination_is_reported_as_the_table_s_own_record() {
    let router = emulator();
    create_table(&router, "Recorded", &[]).await;
    let failed_arn = stream_arn("absent-stream");
    enable(&router, "Recorded", &failed_arn).await;
    settle().await;

    let reported_failed = reported(&router, "Recorded", &failed_arn).await;
    let unknown = reported(&router, "Recorded", &stream_arn("never-named")).await;
    assert_eq!(
        (reported_failed.as_str(), unknown.as_str()),
        ("ENABLE_FAILED", "<absent>"),
        "case \"the table's records\": a failed destination is reported and a stream the table \
         never named is not, got {reported_failed} and {unknown}"
    );

    // A stop addresses live replication only, so there is none to stop here.
    let (status, body) = call(
        &router,
        "DisableKinesisStreamingDestination",
        json!({"TableName": "Recorded", "StreamArn": failed_arn}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body), message(&body)),
        (
            400,
            "ValidationException".to_string(),
            "Table is not in a valid state to enable Kinesis Streaming Destination: \
             KinesisStreamingDestination must be ACTIVE to perform DISABLE operation."
                .to_string()
        ),
        "case \"a stop of a failed destination\": expected the required-status sentence, got \
         {status} with {body}"
    );
    let unchanged: Value = json!(reported(&router, "Recorded", &failed_arn).await);
    assert_eq!(
        unchanged,
        json!("ENABLE_FAILED"),
        "case \"a stop of a failed destination\": the refusal changed nothing, got {unchanged}"
    );
}
