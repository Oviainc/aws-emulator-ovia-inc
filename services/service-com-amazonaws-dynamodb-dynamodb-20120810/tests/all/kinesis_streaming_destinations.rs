// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateKinesisStreamingDestination`, the command that registers
//! the destination it addresses (`EnableKinesisStreamingDestination`) and the reader both
//! round-trip through (`DescribeKinesisStreamingDestination`).
//!
//! The rules pinned here:
//!
//! 1. the command addresses ONE destination of the table it names — the destination
//!    record its `StreamArn` names, which has to be `ACTIVE` — so a command the table
//!    cannot serve is refused with HTTP 400 `ValidationException`, carrying DynamoDB's
//!    no-such-destination sentence over the stream ARN and the table name the request
//!    carried when the table holds no record of that stream at all, and DynamoDB's
//!    required-status sentence when the record exists but is not `ACTIVE`;
//! 2. the table is resolved like every other table-addressed operation's: by bare name
//!    or by the table's ARN, with a name no table answers for refused as
//!    `ResourceNotFoundException` and a name outside the documented form refused by the
//!    validator's own clause; `StreamArn` is bounded in LENGTH alone, so a value that
//!    clears that bound is read, and one that names no Kinesis data stream addresses no
//!    destination record of this table — the no-such-destination refusal;
//! 3. an accepted command applies the configuration it names to that one destination and
//!    to nothing else, and the destination is `UPDATING` for the transient span and
//!    `ACTIVE` after it — both as the command answers and as the reader reports;
//! 4. a command that names no precision changes none: the precision in force stays what
//!    it was, and a destination carrying none reports none.
//!
//! Each rule is asserted over a SET of inputs — why the table cannot serve the command,
//! how the table is addressed, the precision in force before the command, the
//! precision the command names, and whether it names one at all — rather than one
//! example. Every command and every read is driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.
//!
//! The rows about a destination whose replication has STOPPED place the stop's span in
//! the past through the same per-service store the handlers read (the shared engine
//! `emulator_and_engine` hands back), exactly as the `ListImports` suite seeds the
//! import records its reader reports, so this suite settles no stop span of its own.
//! `DisableKinesisStreamingDestination` over the wire has its own suite
//! (`disable_kinesis_streaming_destination`); everything else here is driven over the
//! wire.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{self, STATUS_TRANSITION};

use crate::contributor_insights::{create_table, settle};
use crate::emulator::{
    call, emulator_and_engine, ensure_kinesis_stream, error_code, message, ok, REGION,
};

/// The ARN of a Kinesis data stream in the region the suite drives, built through the
/// same ARN helper the emulator mints every other ARN with.
fn stream_arn(name: &str) -> String {
    engine::emulator_arn("kinesis", REGION, &format!("stream/{name}"))
}

/// Where a row wants one of the table's destinations left before the command under test.
#[derive(Clone, Copy, Debug)]
enum Replication {
    /// Registered and replicating: `ACTIVE`.
    Live,
    /// Stopped, with the stop span elapsed: `DISABLED`.
    Stopped,
    /// Stopping, with the stop span still owed: `DISABLING`.
    Stopping,
}

/// One destination a row wants on the table under test: the stream, the precision it
/// replicates under, and where its replication is left.
type Destination = (&'static str, Option<&'static str>, Replication);

/// Leave `destinations` on `table`: each is REGISTERED over the wire, and the ones a row
/// wants stopped are stopped in the store the handlers read.
///
/// A table replicates to AT MOST ONE Kinesis data stream, so the destinations a row wants
/// STOPPED are registered first and stopped one by one; the row's live one — a row carries
/// at most one — is registered last, which is the only order a client can build such a
/// history in.
///
/// The stop is the one step applied through the store rather than over the wire, because
/// a row wants its span already elapsed: that is a stop placed in the past, which only
/// the store can state — the same per-service store the handlers read (the shared engine
/// `emulator_and_engine` hands back), reached the way the `ListImports` suite reaches it.
async fn set_up(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    table: &str,
    destinations: &[Destination],
) {
    let ordered = destinations.iter().filter(|(_, _, replication)| {
        !matches!(replication, Replication::Live)
    });
    for (stream, precision, replication) in ordered.chain(
        destinations
            .iter()
            .filter(|(_, _, replication)| matches!(replication, Replication::Live)),
    ) {
        // The stream has to EXIST for the registration to replicate: a destination whose
        // `StreamArn` resolves to no stream settles `ENABLE_FAILED` rather than `ACTIVE`.
        let arn = ensure_kinesis_stream(router, stream).await;
        let mut request = json!({"TableName": table, "StreamArn": arn});
        if let Some(precision) = precision {
            request["EnableKinesisStreamingConfiguration"] =
                json!({"ApproximateCreationDateTimePrecision": precision});
        }
        ok(router, "EnableKinesisStreamingDestination", request).await;
        // Every registration is asynchronous, so wait the span out: the destination is
        // replicating before the row's own command runs, and before the next one is
        // registered.
        settle().await;
        let stopped_at = match replication {
            Replication::Live => continue,
            // Far enough in the past that the stop span has already elapsed.
            Replication::Stopped => {
                SystemTime::now() - (STATUS_TRANSITION + Duration::from_millis(150))
            }
            Replication::Stopping => SystemTime::now(),
        };
        let store =
            state::service_state(&**engine).expect("the shared engine holds this service's store");
        let mut store = store.lock();
        store
            .table_mut(REGION, table)
            .expect("the suite created this table")
            .replicating_kinesis_destination_mut(&stream_arn(stream), SystemTime::now())
            .expect("the suite registered this destination")
            .apply_disable(stopped_at);
    }
}

/// DynamoDB's refusal of a command against a stream the table holds no destination
/// record for: the invalid-state sentence, the stream ARN the request named and the
/// table it named it against.
fn no_destination_message(table: &str, stream_arn: &str) -> String {
    format!(
        "Table is not in a valid state to enable Kinesis Streaming Destination: No streaming \
         destination with streamArn: {stream_arn} found for table with tableName: {table}"
    )
}

/// DynamoDB's refusal of a configuration change against a destination record that EXISTS
/// but is not `ACTIVE`: the status changes are allowed in, over the table name and the
/// stream ARN the request addressed the destination by.
fn not_active_message(table: &str, stream_arn: &str) -> String {
    format!(
        "Table is not in a valid state to enable Kinesis Streaming Destination: Kinesis \
         streaming is not in ACTIVE state. Updates are only allowed in ACTIVE state. TableName: \
         {table}, kdsArn: {stream_arn}"
    )
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
    assert_eq!(
        report["TableName"],
        json!(table),
        "case {table:?}: the report names the table it describes, got {report}"
    );
    report["KinesisDataStreamDestinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|destination| destination["StreamArn"] == json!(stream_arn))
        .unwrap_or(Value::Null)
}

/// Rule 1: a table with no `ACTIVE` destination for the stream the command addresses is
/// refused — as a lookup miss where it holds no record of that stream, and on the
/// record's status where it holds one that is not `ACTIVE`.
#[tokio::test]
async fn an_update_without_an_active_destination_is_refused() {
    // (case, the destinations the table carries, the stream the command addresses,
    //  whether the table holds a record of that stream)
    let cases: Vec<(&str, Vec<Destination>, &str, bool)> = vec![
        ("never registered one", Vec::new(), "cdc", false),
        (
            "registered another stream",
            vec![("other", Some("MICROSECOND"), Replication::Live)],
            "cdc",
            false,
        ),
        (
            "stopped replicating to it",
            vec![("cdc", Some("MICROSECOND"), Replication::Stopped)],
            "cdc",
            true,
        ),
        (
            "is still stopping replication to it",
            vec![("cdc", None, Replication::Stopping)],
            "cdc",
            true,
        ),
        (
            "several destinations, none of them this stream",
            vec![
                ("one", None, Replication::Live),
                ("two", Some("MILLISECOND"), Replication::Stopped),
            ],
            "cdc",
            false,
        ),
    ];

    for (index, (case, destinations, addressed, holds_a_record)) in cases.into_iter().enumerate() {
        let (router, engine) = emulator_and_engine();
        let table = format!("Replication{index}");
        create_table(&router, &table, &[]).await;
        set_up(&router, &engine, &table, &destinations).await;

        let arn = stream_arn(addressed);
        let (status, body) = call(
            &router,
            "UpdateKinesisStreamingDestination",
            json!({
                "TableName": table.clone(),
                "StreamArn": arn.clone(),
                "UpdateKinesisStreamingConfiguration": {
                    "ApproximateCreationDateTimePrecision": "MILLISECOND",
                },
            }),
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
        let wanted = if holds_a_record {
            not_active_message(&table, &arn)
        } else {
            no_destination_message(&table, &arn)
        };
        assert_eq!(
            message(&body),
            wanted,
            "case {case:?}: expected {wanted:?}, got {}",
            message(&body)
        );

        // A refused command changes nothing: no destination of the table replicates to
        // the stream it addressed, and none of them is reported mid-update. A stopped
        // destination stays in the report, which is the table's replication history.
        let report = ok(
            &router,
            "DescribeKinesisStreamingDestination",
            json!({"TableName": table.clone()}),
        )
        .await;
        let reported = report["KinesisDataStreamDestinations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            reported.iter().all(|destination| destination
                ["DestinationStatus"]
                != json!("UPDATING")),
            "case {case:?}: the refused command must change no destination, got {report}"
        );
        assert!(
            reported.iter().all(|destination| destination["StreamArn"] != json!(arn)
                || destination["DestinationStatus"] == json!("DISABLED")
                || destination["DestinationStatus"] == json!("DISABLING")),
            "case {case:?}: the refused command must start no replication to the stream it \
             addressed, got {report}"
        );
    }
}

/// Rule 2: the table is resolved by name or by ARN, and a table no name answers for is
/// refused as a missing resource.
#[tokio::test]
async fn an_update_resolves_the_table_the_request_names() {
    let (router, engine) = emulator_and_engine();
    create_table(&router, "Addressed", &[]).await;
    set_up(
        &router,
        &engine,
        "Addressed",
        &[("cdc", None, Replication::Live)],
    )
    .await;
    let arn = stream_arn("cdc");

    // (case, the TableName member, the expected HTTP status, the expected error code,
    //  the expected message)
    let refusals: Vec<(&str, String, u16, &str, String)> = vec![
        (
            "a table no name answers for",
            "NeverCreated".to_string(),
            400,
            "ResourceNotFoundException",
            "Requested resource not found: Table: NeverCreated not found".to_string(),
        ),
        (
            "a name shorter than the documented form",
            "ab".to_string(),
            400,
            "ValidationException",
            "1 validation error detected: Value 'ab' at 'tableName' failed to satisfy \
             constraint: Member must have length greater than or equal to 3"
                .to_string(),
        ),
        (
            "an ARN of another service's resource",
            arn.clone(),
            400,
            "ValidationException",
            format!(
                "1 validation error detected: Value '{arn}' at 'tableName' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
            ),
        ),
    ];

    for (case, table_name, expected_status, expected_code, expected_message) in refusals {
        let (status, body) = call(
            &router,
            "UpdateKinesisStreamingDestination",
            json!({"TableName": table_name, "StreamArn": arn.clone()}),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert_eq!(
            message(&body),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {}",
            message(&body)
        );
    }

    // The same destination, addressed through the table's own ARN: the documented
    // alternative spelling of the member resolves to the same table.
    let answered = ok(
        &router,
        "UpdateKinesisStreamingDestination",
        json!({
            "TableName": state::table_arn(REGION, "Addressed"),
            "StreamArn": arn.clone(),
            "UpdateKinesisStreamingConfiguration": {
                "ApproximateCreationDateTimePrecision": "MILLISECOND",
            },
        }),
    )
    .await;
    assert_eq!(
        answered["TableName"],
        json!("Addressed"),
        "case \"addressed by ARN\": the answer names the resolved table, got {answered}"
    );
    assert_eq!(
        answered["StreamArn"],
        json!(arn),
        "case \"addressed by ARN\": the answer names the stream the command addressed, got \
         {answered}"
    );
}

/// One row of the configuration table below: the case, the precision in force before
/// the command, the configuration member the command carries, and the precision in force
/// after it.
type PrecisionCase = (
    &'static str,
    Option<&'static str>,
    Option<Value>,
    Option<&'static str>,
);

/// Rules 3 and 4: the configuration the command names is what the destination replicates
/// under afterwards, through the transient span and after it, and a command that names
/// none changes none.
#[tokio::test]
async fn an_update_applies_the_configuration_it_names() {
    // (case, the precision in force before, the configuration member the command
    //  carries, the precision in force after)
    let cases: Vec<PrecisionCase> = vec![
        (
            "none in force, microsecond asked for",
            None,
            Some(json!({"ApproximateCreationDateTimePrecision": "MICROSECOND"})),
            Some("MICROSECOND"),
        ),
        (
            "microsecond in force, millisecond asked for",
            Some("MICROSECOND"),
            Some(json!({"ApproximateCreationDateTimePrecision": "MILLISECOND"})),
            Some("MILLISECOND"),
        ),
        (
            "millisecond in force, microsecond asked for",
            Some("MILLISECOND"),
            Some(json!({"ApproximateCreationDateTimePrecision": "MICROSECOND"})),
            Some("MICROSECOND"),
        ),
        // A command that RE-STATES the precision already in force asks for no change at
        // all and is refused rather than applied, so it is not a row of this table: it
        // is asserted in `crate::request_validation_refusals`.
        (
            "microsecond in force, an empty configuration",
            Some("MICROSECOND"),
            Some(json!({})),
            Some("MICROSECOND"),
        ),
        (
            "microsecond in force, no configuration at all",
            Some("MICROSECOND"),
            None,
            Some("MICROSECOND"),
        ),
        ("none in force, no configuration at all", None, None, None),
    ];

    for (index, (case, before, configuration, after)) in cases.into_iter().enumerate() {
        let (router, engine) = emulator_and_engine();
        let table = format!("Precision{index}");
        create_table(&router, &table, &[]).await;
        set_up(
            &router,
            &engine,
            &table,
            &[("cdc", before, Replication::Live)],
        )
        .await;
        let arn = stream_arn("cdc");

        let mut request = json!({"TableName": table.clone(), "StreamArn": arn.clone()});
        if let Some(configuration) = configuration {
            request["UpdateKinesisStreamingConfiguration"] = configuration;
        }
        let answered = ok(&router, "UpdateKinesisStreamingDestination", request).await;
        assert_eq!(
            answered["TableName"],
            json!(table),
            "case {case:?}: the answer names the table, got {answered}"
        );
        assert_eq!(
            answered["StreamArn"],
            json!(arn),
            "case {case:?}: the answer names the stream, got {answered}"
        );
        // Applying a configuration is asynchronous, so the destination is reported in
        // the transient status until the span has elapsed.
        assert_eq!(
            answered["DestinationStatus"],
            json!("UPDATING"),
            "case {case:?}: expected the transient status, got {answered}"
        );
        assert_eq!(
            answered["UpdateKinesisStreamingConfiguration"]
                ["ApproximateCreationDateTimePrecision"],
            after.map_or(Value::Null, |precision| json!(precision)),
            "case {case:?}: expected the precision in force to be {after:?}, got {answered}"
        );

        let reported = described(&router, &table, &arn).await;
        assert_eq!(
            reported["DestinationStatus"],
            json!("UPDATING"),
            "case {case:?}: the reader reports the transient status too, got {reported}"
        );
        assert_eq!(
            reported["ApproximateCreationDateTimePrecision"],
            after.map_or(Value::Null, |precision| json!(precision)),
            "case {case:?}: the reader reports the precision in force, got {reported}"
        );

        settle().await;
        let settled = described(&router, &table, &arn).await;
        assert_eq!(
            settled["DestinationStatus"],
            json!("ACTIVE"),
            "case {case:?}: once the span has elapsed the destination replicates again, got \
             {settled}"
        );
        assert_eq!(
            settled["ApproximateCreationDateTimePrecision"],
            after.map_or(Value::Null, |precision| json!(precision)),
            "case {case:?}: the precision in force survives the span, got {settled}"
        );
    }
}

/// Rule 3, the other half: the command changes the ONE destination it addresses, and
/// leaves every other destination — of this table and of another — exactly as it was.
#[tokio::test]
async fn an_update_changes_only_the_destination_it_addresses() {
    let (router, engine) = emulator_and_engine();
    create_table(&router, "Addressed", &[]).await;
    create_table(&router, "Untouched", &[]).await;
    set_up(
        &router,
        &engine,
        "Addressed",
        &[
            ("history", Some("MILLISECOND"), Replication::Stopped),
            ("cdc", Some("MICROSECOND"), Replication::Live),
        ],
    )
    .await;
    set_up(
        &router,
        &engine,
        "Untouched",
        &[("cdc", Some("MICROSECOND"), Replication::Live)],
    )
    .await;

    let addressed = stream_arn("cdc");
    ok(
        &router,
        "UpdateKinesisStreamingDestination",
        json!({
            "TableName": "Addressed",
            "StreamArn": addressed.clone(),
            "UpdateKinesisStreamingConfiguration": {
                "ApproximateCreationDateTimePrecision": "MILLISECOND",
            },
        }),
    )
    .await;

    // (case, the table, the stream, the expected status, the expected precision)
    let expected: Vec<(&str, &str, &str, &str, Option<&str>)> = vec![
        (
            "the destination addressed",
            "Addressed",
            "cdc",
            "UPDATING",
            Some("MILLISECOND"),
        ),
        (
            "a stopped destination of the same table",
            "Addressed",
            "history",
            "DISABLED",
            Some("MILLISECOND"),
        ),
        (
            "the same stream on another table",
            "Untouched",
            "cdc",
            "ACTIVE",
            Some("MICROSECOND"),
        ),
    ];

    for (case, table, stream, status, precision) in expected {
        let reported = described(&router, table, &stream_arn(stream)).await;
        assert_eq!(
            reported["DestinationStatus"],
            json!(status),
            "case {case:?}: expected status {status}, got {reported}"
        );
        assert_eq!(
            reported["ApproximateCreationDateTimePrecision"],
            precision.map_or(Value::Null, |precision| json!(precision)),
            "case {case:?}: expected precision {precision:?}, got {reported}"
        );
    }
}

/// Rule 2, the stream half: a `StreamArn` that names no Kinesis data stream names no
/// destination record of the table either, so the command that addresses an EXISTING
/// destination answers its own lookup miss — the value is read, not refused as unreadable.
/// What the command that REGISTERS a destination does with such a value (accept it, and
/// settle `ENABLE_FAILED`) is its own suite's
/// (`kinesis_destination_record_delivery`).
#[tokio::test]
async fn a_stream_arn_that_names_no_stream_is_refused() {
    let (router, _engine) = emulator_and_engine();
    create_table(&router, "Addressed", &[]).await;

    // Every value here satisfies the member's documented 37..=1024 length bound, which is
    // the only constraint it carries, so each is READ and resolves to no destination.
    let values = [
        "arn:aws:dynamodb:us-east-1:000000000000:table/Other",
        "arn:aws:kinesis:us-east-1:000000000000:table/Other",
        "arn:aws:kinesis::000000000000:stream/no-region-at-all",
        "arn:aws:kinesis:us-east-1:000000000000:stream/",
        "not-an-arn-but-long-enough-to-clear-the-length-bound",
    ];

    for value in values {
        let case = format!("UpdateKinesisStreamingDestination with {value}");
        let (status, body) = call(
            &router,
            "UpdateKinesisStreamingDestination",
            json!({"TableName": "Addressed", "StreamArn": value}),
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
        assert_eq!(
            message(&body),
            no_destination_message("Addressed", value),
            "case {case:?}: expected DynamoDB's no-such-destination sentence, got {}",
            message(&body)
        );
    }
}

/// The whole surface over the wire, with nothing seeded: registering replication,
/// reading it back through the transient span and after it, refusing a second
/// registration of the same stream, and updating the configuration of the registered
/// destination.
#[tokio::test]
async fn the_destination_lifecycle_is_driven_over_the_wire() {
    let (router, _engine) = emulator_and_engine();
    create_table(&router, "Wired", &[]).await;
    let arn = ensure_kinesis_stream(&router, "cdc").await;

    // A table no command has named replicates to nothing at all.
    let empty = ok(
        &router,
        "DescribeKinesisStreamingDestination",
        json!({"TableName": "Wired"}),
    )
    .await;
    assert_eq!(
        empty["KinesisDataStreamDestinations"],
        json!([]),
        "case \"before any command\": a table replicates to nothing, got {empty}"
    );

    // A registration that names no precision registers none: the member stays absent
    // until a command names one.
    let enabled = ok(
        &router,
        "EnableKinesisStreamingDestination",
        json!({"TableName": "Wired", "StreamArn": arn.clone()}),
    )
    .await;
    assert_eq!(
        enabled["DestinationStatus"],
        json!("ENABLING"),
        "case \"registered\": expected the transient status, got {enabled}"
    );
    assert_eq!(
        enabled["EnableKinesisStreamingConfiguration"],
        Value::Null,
        "case \"registered\": a registration that named no precision reports none, got {enabled}"
    );

    let registering = described(&router, "Wired", &arn).await;
    assert_eq!(
        registering["DestinationStatus"],
        json!("ENABLING"),
        "case \"registering\": the reader reports the transient status, got {registering}"
    );
    assert_eq!(
        registering["ApproximateCreationDateTimePrecision"],
        Value::Null,
        "case \"registering\": the reader reports no precision, got {registering}"
    );

    // Replication that is already on cannot be started again.
    let (status, body) = call(
        &router,
        "EnableKinesisStreamingDestination",
        json!({"TableName": "Wired", "StreamArn": arn.clone()}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"registered twice\": expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        message(&body),
        "Table is not in a valid state to enable Kinesis Streaming Destination: \
         EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform ENABLE \
         operation.",
        "case \"registered twice\": expected DynamoDB's enable-state sentence, got {}",
        message(&body)
    );

    settle().await;
    let replicating = described(&router, "Wired", &arn).await;
    assert_eq!(
        replicating["DestinationStatus"],
        json!("ACTIVE"),
        "case \"registered\": once the span has elapsed the destination replicates, got \
         {replicating}"
    );

    let updated = ok(
        &router,
        "UpdateKinesisStreamingDestination",
        json!({
            "TableName": "Wired",
            "StreamArn": arn.clone(),
            "UpdateKinesisStreamingConfiguration": {
                "ApproximateCreationDateTimePrecision": "MILLISECOND",
            },
        }),
    )
    .await;
    assert_eq!(
        updated["UpdateKinesisStreamingConfiguration"]["ApproximateCreationDateTimePrecision"],
        json!("MILLISECOND"),
        "case \"updated\": the answer reports the precision now in force, got {updated}"
    );

    settle().await;
    let settled = described(&router, "Wired", &arn).await;
    assert_eq!(
        settled["DestinationStatus"],
        json!("ACTIVE"),
        "case \"updated\": the destination replicates again after the span, got {settled}"
    );
    assert_eq!(
        settled["ApproximateCreationDateTimePrecision"],
        json!("MILLISECOND"),
        "case \"updated\": the reader reports the precision the command named, got {settled}"
    );
}

/// A precision outside the two values the model declares is refused as invalid input,
/// whichever layer refuses it — the boundary validator or the handler's own check.
#[tokio::test]
async fn a_precision_outside_the_declared_set_is_refused() {
    let (router, engine) = emulator_and_engine();
    create_table(&router, "Declared", &[]).await;
    set_up(
        &router,
        &engine,
        "Declared",
        &[("cdc", Some("MICROSECOND"), Replication::Live)],
    )
    .await;
    let arn = stream_arn("cdc");

    // Neither value is in `ApproximateCreationDateTimePrecision`, which declares
    // MILLISECOND and MICROSECOND only (the shape's own value list).
    for case in ["NANOSECOND", "millisecond"] {
        let (status, body) = call(
            &router,
            "UpdateKinesisStreamingDestination",
            json!({
                "TableName": "Declared",
                "StreamArn": arn.clone(),
                "UpdateKinesisStreamingConfiguration": {
                    "ApproximateCreationDateTimePrecision": case,
                },
            }),
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
    }

    // The refused command left the precision in force where it was.
    let reported = described(&router, "Declared", &arn).await;
    assert_eq!(
        reported["ApproximateCreationDateTimePrecision"],
        json!("MICROSECOND"),
        "case \"a refused precision\": the destination keeps the precision in force, got \
         {reported}"
    );
    assert_eq!(
        reported["DestinationStatus"],
        json!("ACTIVE"),
        "case \"a refused precision\": the destination keeps replicating, got {reported}"
    );
}
