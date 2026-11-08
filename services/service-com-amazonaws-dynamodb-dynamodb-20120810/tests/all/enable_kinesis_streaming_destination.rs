// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `EnableKinesisStreamingDestination`, the command that STARTS a
//! table's replication to a Kinesis data stream, round-tripped through the reader that
//! reports a table's destinations (`DescribeKinesisStreamingDestination`).
//!
//! The rules pinned here:
//!
//! 1. an accepted command registers ONE destination record on the table it names, for the
//!    stream its `StreamArn` names, and answers the resolved table name, that stream and
//!    the transient status: the destination is `ENABLING` for the transient span and
//!    `ACTIVE` after it, both as the command answers and as the reader reports;
//! 2. the destination replicates under the configuration the request carried, and the
//!    answer reports that configuration back from the record: a request that names no
//!    precision — by carrying no configuration at all, or one with the member absent —
//!    registers none, and the member stays absent in the answer and in the reader's
//!    report until a command names one;
//! 3. a table replicates to AT MOST ONE Kinesis data stream, and holds ONE record per
//!    stream: an enable against a table whose replication is live is refused with HTTP 400
//!    `ValidationException` carrying DynamoDB's enable-state sentence — whichever stream it
//!    names and whatever transient status that replication is in (the whole set of live
//!    statuses, and the second stream, have their own suite
//!    `kinesis_destination_record_delivery`) — while a stream whose replication has STOPPED
//!    is re-registered in place, so the reader's report stays the table's replication
//!    history, in registration order, and a replacement stream is appended beside the
//!    stopped record it replaces;
//! 4. the table is resolved like every other table-addressed operation's: by bare name or
//!    by the table's ARN, with a name no table answers for refused as
//!    `ResourceNotFoundException` and a name outside the documented form refused by the
//!    validator's own clause; `StreamArn` is bounded in LENGTH alone, so a value shorter
//!    than the bound is refused by the validator and every value that clears it is accepted
//!    — what it resolves to is settled asynchronously
//!    (`kinesis_destination_record_delivery`);
//! 5. the registration changes the ONE table it names: every destination of another
//!    table, and every other destination of this one, is left exactly as it was.
//!
//! Each rule is asserted over a SET of inputs — the precision the request names (the
//! whole set the model declares for
//! `EnableKinesisStreamingConfiguration$ApproximateCreationDateTimePrecision`, plus both
//! ways of naming none), how the table is addressed, the status the replication under
//! test is in, and why a request cannot be read — rather than one example. Every command
//! and every read is driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, with nothing seeded through the
//! store.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_dynamodb_dynamodb_20120810::state;

use crate::contributor_insights::{create_table, settle};
use crate::emulator::{call, emulator, ensure_kinesis_stream, error_code, message, ok, REGION};

/// The ARN of a Kinesis data stream in the region the suite drives, built through the
/// same ARN helper the emulator mints every other ARN with.
fn stream_arn(name: &str) -> String {
    engine::emulator_arn("kinesis", REGION, &format!("stream/{name}"))
}

/// Every precision the model declares for
/// `EnableKinesisStreamingConfiguration$ApproximateCreationDateTimePrecision`
/// (`smithy_query(query="shape",shape="ApproximateCreationDateTimePrecision")`:
/// `MILLISECOND | MICROSECOND`).
const PRECISIONS: [&str; 2] = ["MILLISECOND", "MICROSECOND"];

/// Every Kinesis data stream the destinations below name, created on the emulator's
/// Kinesis service so an enable can RESOLVE them: a destination whose `StreamArn` names no
/// stream never replicates, so a suite about registration creates its streams first.
async fn declared_streams(router: &MultiServiceRouter) {
    for name in ["cdc", "audit", "history", "replacement"] {
        ensure_kinesis_stream(router, name).await;
    }
}

/// Start replication of `table` to `stream` over the wire, leaving the registration span
/// owed: a caller that needs it replicating waits that span out with [`settle`].
async fn register(
    router: &MultiServiceRouter,
    table: &str,
    stream: &str,
    precision: Option<&str>,
) -> Value {
    let mut request = json!({"TableName": table, "StreamArn": stream_arn(stream)});
    if let Some(precision) = precision {
        request["EnableKinesisStreamingConfiguration"] =
            json!({"ApproximateCreationDateTimePrecision": precision});
    }
    ok(router, "EnableKinesisStreamingDestination", request).await
}

/// The destination the reader reports for `stream_arn`, or `Value::Null` when the table
/// reports none for that stream.
async fn described(router: &MultiServiceRouter, table: &str, stream_arn: &str) -> Value {
    report(router, table)
        .await
        .into_iter()
        .find(|destination| destination["StreamArn"] == json!(stream_arn))
        .unwrap_or(Value::Null)
}

/// Every destination the reader reports for `table`, in the order it reports them.
async fn report(router: &MultiServiceRouter, table: &str) -> Vec<Value> {
    ok(
        router,
        "DescribeKinesisStreamingDestination",
        json!({"TableName": table}),
    )
    .await["KinesisDataStreamDestinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The (stream ARN, status) pair of every destination a table reports, in order — the
/// table's replication history as a client reads it.
async fn history(router: &MultiServiceRouter, table: &str) -> Vec<(String, String)> {
    report(router, table)
        .await
        .into_iter()
        .map(|destination| {
            (
                destination["StreamArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                destination["DestinationStatus"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// DynamoDB's refusal of a command that starts replication the table already runs: the
/// statuses an enable may start from, under the invalid-state sentence, naming neither
/// the table nor the stream.
fn already_enabled_message() -> String {
    "Table is not in a valid state to enable Kinesis Streaming Destination: \
     EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform ENABLE \
     operation."
        .to_string()
}

/// How a row of the registration table below names the precision: not at all, through an
/// empty configuration, or through one carrying one of the model's values.
#[derive(Clone, Copy, Debug)]
enum Named {
    /// The request carries no `EnableKinesisStreamingConfiguration` member.
    NoConfiguration,
    /// It carries one, with the precision member absent.
    EmptyConfiguration,
    /// It carries one naming this precision.
    Precision(&'static str),
}

impl Named {
    /// The configuration member the request carries, if any.
    fn member(self) -> Option<Value> {
        match self {
            Named::NoConfiguration => None,
            Named::EmptyConfiguration => Some(json!({})),
            Named::Precision(precision) => {
                Some(json!({"ApproximateCreationDateTimePrecision": precision}))
            }
        }
    }

    /// The precision in force once the command is accepted: only a named one registers.
    fn in_force(self) -> Option<&'static str> {
        match self {
            Named::NoConfiguration | Named::EmptyConfiguration => None,
            Named::Precision(precision) => Some(precision),
        }
    }
}

/// Rules 1, 2 and 4 (the ARN spelling): an accepted command registers the destination the
/// request names under the configuration it carries, whichever way the request names the
/// precision and however it addresses the table, and the reader round-trips it through
/// the transient span to `ACTIVE`.
#[tokio::test]
async fn an_enable_registers_the_destination_it_names() {
    let mut cases: Vec<(String, Named, bool)> = Vec::new();
    for named in [
        Named::NoConfiguration,
        Named::EmptyConfiguration,
        Named::Precision(PRECISIONS[0]),
        Named::Precision(PRECISIONS[1]),
    ] {
        for by_arn in [false, true] {
            let addressed = if by_arn { "by ARN" } else { "by name" };
            cases.push((format!("{named:?} addressed {addressed}"), named, by_arn));
        }
    }

    for (index, (case, named, by_arn)) in cases.into_iter().enumerate() {
        let router = emulator();
        declared_streams(&router).await;
        let table = format!("Registered{index}");
        create_table(&router, &table, &[]).await;
        let arn = stream_arn("cdc");
        let addressed = if by_arn {
            state::table_arn(REGION, &table)
        } else {
            table.clone()
        };

        let mut request = json!({"TableName": addressed, "StreamArn": arn.clone()});
        if let Some(configuration) = named.member() {
            request["EnableKinesisStreamingConfiguration"] = configuration;
        }
        let answered = ok(&router, "EnableKinesisStreamingDestination", request).await;

        assert_eq!(
            answered["TableName"],
            json!(table),
            "case {case:?}: the answer names the table it changed, got {answered}"
        );
        assert_eq!(
            answered["StreamArn"],
            json!(arn),
            "case {case:?}: the answer names the stream it registered, got {answered}"
        );
        // Starting replication is asynchronous, so the destination is reported in the
        // transient status until the span has elapsed.
        assert_eq!(
            answered["DestinationStatus"],
            json!("ENABLING"),
            "case {case:?}: expected the transient status, got {answered}"
        );
        let expected_configuration = named.in_force().map_or(Value::Null, |precision| {
            json!({"ApproximateCreationDateTimePrecision": precision})
        });
        assert_eq!(
            answered["EnableKinesisStreamingConfiguration"],
            expected_configuration,
            "case {case:?}: expected the configuration registered to be \
             {expected_configuration}, got {answered}"
        );

        let registering = described(&router, &table, &arn).await;
        assert_eq!(
            registering["DestinationStatus"],
            json!("ENABLING"),
            "case {case:?}: the reader reports the transient status too, got {registering}"
        );
        assert_eq!(
            registering["ApproximateCreationDateTimePrecision"],
            named
                .in_force()
                .map_or(Value::Null, |precision| json!(precision)),
            "case {case:?}: expected the precision {:?}, got {registering}",
            named.in_force()
        );

        settle().await;
        let replicating = described(&router, &table, &arn).await;
        assert_eq!(
            replicating["DestinationStatus"],
            json!("ACTIVE"),
            "case {case:?}: once the span has elapsed the destination replicates, got \
             {replicating}"
        );
        assert_eq!(
            replicating["ApproximateCreationDateTimePrecision"],
            named
                .in_force()
                .map_or(Value::Null, |precision| json!(precision)),
            "case {case:?}: the precision registered survives the span, got {replicating}"
        );
    }
}

/// Rule 3, the refusal half: replication the table already runs cannot be started again,
/// whatever transient status that replication is in, and the refused command changes
/// nothing — the destination settles where its OWN lifecycle was heading, under the
/// precision it was registered with.
#[tokio::test]
async fn an_enable_of_a_stream_already_replicating_is_refused() {
    // (case, whether the registration span is waited out, whether a configuration change
    //  is accepted first — which leaves the destination `UPDATING`)
    let cases: [(&str, bool, bool); 3] = [
        ("still registering", false, false),
        ("replicating", true, false),
        ("mid configuration change", true, true),
    ];

    for (index, (case, settled, updating)) in cases.into_iter().enumerate() {
        let router = emulator();
        declared_streams(&router).await;
        let table = format!("Running{index}");
        create_table(&router, &table, &[]).await;
        let arn = stream_arn("cdc");
        register(&router, &table, "cdc", Some(PRECISIONS[0])).await;
        if settled {
            settle().await;
        }
        if updating {
            ok(
                &router,
                "UpdateKinesisStreamingDestination",
                json!({"TableName": table.clone(), "StreamArn": arn.clone()}),
            )
            .await;
        }

        let (status, body) = call(
            &router,
            "EnableKinesisStreamingDestination",
            json!({
                "TableName": table.clone(),
                "StreamArn": arn.clone(),
                "EnableKinesisStreamingConfiguration": {
                    "ApproximateCreationDateTimePrecision": PRECISIONS[1],
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
        let wanted = already_enabled_message();
        assert_eq!(
            message(&body),
            wanted,
            "case {case:?}: expected {wanted:?}, got {:?}",
            message(&body)
        );

        settle().await;
        let reported = described(&router, &table, &arn).await;
        assert_eq!(
            reported["DestinationStatus"],
            json!("ACTIVE"),
            "case {case:?}: the refused command leaves the destination replicating, got \
             {reported}"
        );
        assert_eq!(
            reported["ApproximateCreationDateTimePrecision"],
            json!(PRECISIONS[0]),
            "case {case:?}: the refused command registers no configuration of its own, got \
             {reported}"
        );
        assert_eq!(
            report(&router, &table).await.len(),
            1,
            "case {case:?}: a table holds one record per stream, got {:?}",
            history(&router, &table).await
        );
    }
}

/// Rules 3 and 5: a stream whose replication has stopped is re-registered IN PLACE — one
/// record per stream, keeping its position in the table's replication history — a
/// replacement stream is appended beside the stopped record it replaces, and neither
/// touches another destination of this table or of another table.
#[tokio::test]
async fn an_enable_re_registers_a_stopped_stream_in_place() {
    let router = emulator();
    declared_streams(&router).await;
    create_table(&router, "Replaced", &[]).await;
    create_table(&router, "Untouched", &[]).await;
    register(&router, "Untouched", "audit", Some(PRECISIONS[1])).await;

    /// Stop `table`'s replication to `stream` and wait the stop out, which is what frees
    /// the table to replicate to the next one.
    async fn stop(router: &MultiServiceRouter, table: &str, stream: &str) {
        ok(
            router,
            "DisableKinesisStreamingDestination",
            json!({"TableName": table, "StreamArn": stream_arn(stream)}),
        )
        .await;
        settle().await;
    }

    // A table replicates to ONE stream at a time, so each stream below is registered,
    // waited out and stopped before the next one is: `cdc` first, then `audit`.
    register(&router, "Replaced", "cdc", Some(PRECISIONS[0])).await;
    settle().await;
    stop(&router, "Replaced", "cdc").await;
    register(&router, "Replaced", "audit", None).await;
    settle().await;
    stop(&router, "Replaced", "audit").await;

    // Start replication to `audit` again under the other precision: the record is
    // re-registered where it already was, rather than appended as a second record.
    let restarted = register(&router, "Replaced", "audit", Some(PRECISIONS[1])).await;
    assert_eq!(
        restarted["DestinationStatus"],
        json!("ENABLING"),
        "case \"re-registered\": expected the transient status, got {restarted}"
    );
    settle().await;

    // Replace it with a DIFFERENT stream: the stopped record stays in the report and the
    // new stream is appended after it.
    stop(&router, "Replaced", "audit").await;
    register(&router, "Replaced", "replacement", Some(PRECISIONS[0])).await;
    settle().await;

    let wanted: Vec<(String, String)> = vec![
        (stream_arn("cdc"), "DISABLED".to_string()),
        (stream_arn("audit"), "DISABLED".to_string()),
        (stream_arn("replacement"), "ACTIVE".to_string()),
    ];
    let reported = history(&router, "Replaced").await;
    assert_eq!(
        reported, wanted,
        "case \"the replication history\": expected {wanted:?}, got {reported:?}"
    );

    // (case, the table, the stream, the precision the destination replicates under)
    let precisions: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "the re-registered record",
            "Replaced",
            "audit",
            Some(PRECISIONS[1]),
        ),
        (
            "a record registered before it",
            "Replaced",
            "cdc",
            Some(PRECISIONS[0]),
        ),
        (
            "the replacement stream",
            "Replaced",
            "replacement",
            Some(PRECISIONS[0]),
        ),
        (
            "the same stream of another table",
            "Untouched",
            "audit",
            Some(PRECISIONS[1]),
        ),
    ];
    for (case, table, stream, precision) in precisions {
        let destination = described(&router, table, &stream_arn(stream)).await;
        assert_eq!(
            destination["ApproximateCreationDateTimePrecision"],
            precision.map_or(Value::Null, |precision| json!(precision)),
            "case {case:?}: expected the precision {precision:?}, got {destination}"
        );
    }
    let untouched = history(&router, "Untouched").await;
    assert_eq!(
        untouched,
        vec![(stream_arn("audit"), "ACTIVE".to_string())],
        "case \"another table\": a registration changes one table, got {untouched:?}"
    );
}

/// Rule 4: the table and the stream are resolved from the request, and a request naming
/// either one the service cannot read is refused without registering anything.
#[tokio::test]
async fn an_enable_resolves_the_table_and_the_stream_it_names() {
    let router = emulator();
    declared_streams(&router).await;
    create_table(&router, "Resolved", &[]).await;
    let arn = stream_arn("cdc");

    // (case, the TableName member, the expected error code, the expected message)
    let tables: Vec<(&str, String, &str, String)> = vec![
        (
            "a table no name answers for",
            "NeverCreated".to_string(),
            "ResourceNotFoundException",
            "Requested resource not found: Table: NeverCreated not found".to_string(),
        ),
        (
            "a name shorter than the documented form",
            "ab".to_string(),
            "ValidationException",
            "1 validation error detected: Value 'ab' at 'tableName' failed to satisfy \
             constraint: Member must have length greater than or equal to 3"
                .to_string(),
        ),
        (
            "an ARN of another service's resource",
            arn.clone(),
            "ValidationException",
            format!(
                "1 validation error detected: Value '{arn}' at 'tableName' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
            ),
        ),
    ];

    for (case, table_name, expected_code, expected_message) in tables {
        let (status, body) = call(
            &router,
            "EnableKinesisStreamingDestination",
            json!({"TableName": table_name, "StreamArn": arn.clone()}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert_eq!(
            message(&body),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            message(&body)
        );
    }

    // A `StreamArn` SHORTER than the member's documented minimum is refused by the
    // boundary validator's own length clause, before the handler reads it — the one
    // constraint the member carries, so every value that clears it is accepted and settled
    // asynchronously (`kinesis_destination_record_delivery`).
    let short = "arn:aws:kinesis:x";
    let (status, body) = call(
        &router,
        "EnableKinesisStreamingDestination",
        json!({"TableName": "Resolved", "StreamArn": short}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {short:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {short:?}: expected ValidationException, got {body}"
    );

    // Nothing above was accepted, so the table replicates to nothing at all — and the
    // same command with both members readable registers the destination.
    let before = history(&router, "Resolved").await;
    assert!(
        before.is_empty(),
        "case \"after the refusals\": a refused command registers nothing, got {before:?}"
    );
    let answered = ok(
        &router,
        "EnableKinesisStreamingDestination",
        json!({"TableName": "Resolved", "StreamArn": arn.clone()}),
    )
    .await;
    assert_eq!(
        answered["DestinationStatus"],
        json!("ENABLING"),
        "case \"both members readable\": expected the transient status, got {answered}"
    );
}
