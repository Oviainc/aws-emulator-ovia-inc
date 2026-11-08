// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the STATE contract the two commands that change a table's
//! Kinesis streaming destination share: `DisableKinesisStreamingDestination` and
//! `UpdateKinesisStreamingDestination`.
//!
//! The rules pinned here:
//!
//! 1. both commands act only on a destination whose `DestinationStatus` is `ACTIVE`:
//!    `ENABLING`, `UPDATING`, `DISABLING` and `DISABLED` are each refused with HTTP 400
//!    `ValidationException`, and so is a stream the table holds no destination record
//!    for at all;
//! 2. each command words that refusal the way DynamoDB words it for ITS state check —
//!    the stop states the status it requires and interpolates nothing, while the
//!    configuration change states the status updates are allowed in and labels the
//!    table name and the Kinesis stream ARN the request addressed the destination by;
//! 3. only the configuration change distinguishes a destination record that EXISTS in a
//!    non-`ACTIVE` status from a stream the table has no record of: the latter is
//!    DynamoDB's no-such-destination refusal, so a caller can tell "never registered"
//!    (terminal) from "not ACTIVE yet" (waited out, or re-enabled);
//! 4. a refused command changes nothing: once every span the clock already owed has
//!    elapsed, the destination reports the status its own lifecycle was heading for and
//!    the precision it already replicated under.
//!
//! Each rule is asserted over a SET of inputs — every status of the model's
//! `DestinationStatus` lifecycle this store reaches, plus the absent record, against both
//! commands — rather than one example. Every command and every read is driven over the
//! wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the
//! same `MultiServiceRouter` the emulator's front doors use, with nothing seeded through
//! the store.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::contributor_insights::{create_table, settle};
use crate::emulator::{call, emulator, ensure_kinesis_stream, error_code, message, ok, REGION};

/// The ARN of a Kinesis data stream in the region the suite drives, built through the
/// same ARN helper the emulator mints every other ARN with.
fn stream_arn(name: &str) -> String {
    engine::emulator_arn("kinesis", REGION, &format!("stream/{name}"))
}

/// The precision every destination here is registered under, so a refused configuration
/// change is visible as a precision that did NOT move.
const REGISTERED_PRECISION: &str = "MILLISECOND";

/// The precision the configuration change under test asks for — the other value the
/// model's `ApproximateCreationDateTimePrecision` declares.
const ASKED_PRECISION: &str = "MICROSECOND";

/// Where a row leaves the destination's lifecycle before the command under test arrives:
/// the absent record, and every status this store's destination lifecycle reaches.
#[derive(Clone, Copy, Debug)]
enum Reached {
    /// The table holds no destination record for the stream at all.
    Absent,
    /// Registered, with the registration span still owed: `ENABLING`.
    Enabling,
    /// Registered and settled: `ACTIVE`.
    Active,
    /// A configuration change accepted, with its span still owed: `UPDATING`.
    Updating,
    /// A stop accepted, with its span still owed: `DISABLING`.
    Disabling,
    /// A stop accepted and settled: `DISABLED`.
    Disabled,
}

impl Reached {
    /// The status the destination reports once every span the clock owed it has elapsed,
    /// or `None` where there is no record to report — what rule 4 reads back.
    fn settled_status(self) -> Option<&'static str> {
        match self {
            Reached::Absent => None,
            Reached::Enabling | Reached::Active | Reached::Updating => Some("ACTIVE"),
            Reached::Disabling | Reached::Disabled => Some("DISABLED"),
        }
    }
}

/// Leave `table`'s replication to `arn` at `reached`, over the wire.
async fn arrange(router: &MultiServiceRouter, table: &str, arn: &str, reached: Reached) {
    if matches!(reached, Reached::Absent) {
        return;
    }
    // The destination's stream has to EXIST for the registration to replicate at all: one
    // whose `StreamArn` resolves to no stream settles `ENABLE_FAILED`.
    ensure_kinesis_stream(router, "cdc").await;
    ok(
        router,
        "EnableKinesisStreamingDestination",
        json!({
            "TableName": table,
            "StreamArn": arn,
            "EnableKinesisStreamingConfiguration": {
                "ApproximateCreationDateTimePrecision": REGISTERED_PRECISION,
            },
        }),
    )
    .await;
    let addressed = json!({"TableName": table, "StreamArn": arn});
    match reached {
        // The registration span is deliberately left owed, so the row addresses a
        // destination that is still `ENABLING`.
        Reached::Absent | Reached::Enabling => {}
        Reached::Active => settle().await,
        Reached::Updating => {
            settle().await;
            // A change that names no precision changes none, so this row differs from
            // the `ACTIVE` one only in the span it leaves owed.
            ok(
                router,
                "UpdateKinesisStreamingDestination",
                addressed.clone(),
            )
            .await;
        }
        Reached::Disabling => {
            settle().await;
            ok(
                router,
                "DisableKinesisStreamingDestination",
                addressed.clone(),
            )
            .await;
        }
        Reached::Disabled => {
            settle().await;
            ok(
                router,
                "DisableKinesisStreamingDestination",
                addressed.clone(),
            )
            .await;
            settle().await;
        }
    }
}

/// The request body of the command under test, addressing `table`'s replication to
/// `arn`. The configuration change asks for a precision the destination is NOT already
/// replicating under, so an accepted one is visible and a refused one leaves a trace.
fn request_for(operation: &str, table: &str, arn: &str) -> Value {
    let mut request = json!({"TableName": table, "StreamArn": arn});
    if operation == "UpdateKinesisStreamingDestination" {
        request["UpdateKinesisStreamingConfiguration"] =
            json!({"ApproximateCreationDateTimePrecision": ASKED_PRECISION});
    }
    request
}

/// DynamoDB's refusal of a STOP against a destination that is not `ACTIVE`: the status
/// the command requires, stated over nothing of the request.
fn not_active_to_disable_message() -> String {
    "Table is not in a valid state to enable Kinesis Streaming Destination: \
     KinesisStreamingDestination must be ACTIVE to perform DISABLE operation."
        .to_string()
}

/// DynamoDB's refusal of a CONFIGURATION CHANGE against a destination record that exists
/// but is not `ACTIVE`: the status changes are allowed in, over the table name and the
/// stream ARN the request addressed the destination by.
fn not_active_to_update_message(table: &str, stream_arn: &str) -> String {
    format!(
        "Table is not in a valid state to enable Kinesis Streaming Destination: Kinesis \
         streaming is not in ACTIVE state. Updates are only allowed in ACTIVE state. TableName: \
         {table}, kdsArn: {stream_arn}"
    )
}

/// DynamoDB's refusal of a command against a stream the table holds no destination
/// record for.
fn no_destination_message(table: &str, stream_arn: &str) -> String {
    format!(
        "Table is not in a valid state to enable Kinesis Streaming Destination: No streaming \
         destination with streamArn: {stream_arn} found for table with tableName: {table}"
    )
}

/// The message each command is refused with for a destination left at `reached` — rules
/// 2 and 3.
fn expected_message(operation: &str, reached: Reached, table: &str, stream_arn: &str) -> String {
    match (operation, reached) {
        ("DisableKinesisStreamingDestination", _) => not_active_to_disable_message(),
        (_, Reached::Absent) => no_destination_message(table, stream_arn),
        _ => not_active_to_update_message(table, stream_arn),
    }
}

/// The destination the reader reports for `stream_arn`, or `Value::Null` when the table
/// reports none for that stream.
async fn described(router: &MultiServiceRouter, table: &str, stream_arn: &str) -> Value {
    let report = ok(
        router,
        "DescribeKinesisStreamingDestination",
        json!({"TableName": table}),
    )
    .await;
    report["KinesisDataStreamDestinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|destination| destination["StreamArn"] == json!(stream_arn))
        .unwrap_or(Value::Null)
}

/// Rules 1 to 4: every status that is not `ACTIVE`, and the absent record, refuse both
/// commands — each with the wording DynamoDB gives its own state check — and leave the
/// destination where its own lifecycle was heading.
#[tokio::test]
async fn a_destination_that_is_not_active_refuses_both_commands() {
    let states = [
        ("no destination record at all", Reached::Absent),
        ("still registering", Reached::Enabling),
        ("mid configuration change", Reached::Updating),
        ("still stopping", Reached::Disabling),
        ("stopped", Reached::Disabled),
    ];
    let operations = [
        "DisableKinesisStreamingDestination",
        "UpdateKinesisStreamingDestination",
    ];

    for (index, (state, reached)) in states.into_iter().enumerate() {
        for (offset, operation) in operations.into_iter().enumerate() {
            let case = format!("{operation} against a destination {state}");
            let router = emulator();
            let table = format!("State{index}{offset}");
            create_table(&router, &table, &[]).await;
            let arn = stream_arn("cdc");
            arrange(&router, &table, &arn, reached).await;

            let (status, body) = call(
                &router,
                operation,
                request_for(operation, &table, &arn),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            let wanted = expected_message(operation, reached, &table, &arn);
            assert_eq!(
                message(&body),
                wanted,
                "case {case:?}: expected {wanted:?}, got {:?}",
                message(&body)
            );

            // Rule 4: the refusal started nothing of its own. Once every span the clock
            // already owed has elapsed, the destination is where its OWN lifecycle was
            // heading, under the precision it was registered with.
            settle().await;
            let reported = described(&router, &table, &arn).await;
            match reached.settled_status() {
                None => assert_eq!(
                    reported,
                    Value::Null,
                    "case {case:?}: a refused command registers no destination, got {reported}"
                ),
                Some(settled) => {
                    assert_eq!(
                        reported["DestinationStatus"],
                        json!(settled),
                        "case {case:?}: expected the destination to settle on {settled}, got \
                         {reported}"
                    );
                    assert_eq!(
                        reported["ApproximateCreationDateTimePrecision"],
                        json!(REGISTERED_PRECISION),
                        "case {case:?}: a refused command changes no configuration, got {reported}"
                    );
                }
            }
        }
    }
}

/// The positive control of the same rule: a destination that IS `ACTIVE` serves both
/// commands, so the refusals above are about the status and not about the commands.
#[tokio::test]
async fn an_active_destination_serves_both_commands() {
    // (the operation, the transient status it answers with, the status it settles on,
    //  the precision in force after it)
    let accepted = [
        (
            "DisableKinesisStreamingDestination",
            "DISABLING",
            "DISABLED",
            REGISTERED_PRECISION,
        ),
        (
            "UpdateKinesisStreamingDestination",
            "UPDATING",
            "ACTIVE",
            ASKED_PRECISION,
        ),
    ];

    for (operation, transient, settled, precision) in accepted {
        let router = emulator();
        let table = format!("Active{operation}");
        create_table(&router, &table, &[]).await;
        let arn = stream_arn("cdc");
        arrange(&router, &table, &arn, Reached::Active).await;

        let answered = ok(&router, operation, request_for(operation, &table, &arn)).await;
        assert_eq!(
            answered["DestinationStatus"],
            json!(transient),
            "case {operation:?}: expected the transient status {transient}, got {answered}"
        );

        settle().await;
        let reported = described(&router, &table, &arn).await;
        assert_eq!(
            reported["DestinationStatus"],
            json!(settled),
            "case {operation:?}: expected the destination to settle on {settled}, got {reported}"
        );
        assert_eq!(
            reported["ApproximateCreationDateTimePrecision"],
            json!(precision),
            "case {operation:?}: expected the precision {precision}, got {reported}"
        );
    }
}
