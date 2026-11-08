// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DisableKinesisStreamingDestination`, the command that STOPS a
//! table's replication to a Kinesis data stream, together with the registration it
//! addresses (`EnableKinesisStreamingDestination`) and the reader it round-trips through
//! (`DescribeKinesisStreamingDestination`).
//!
//! The rules pinned here:
//!
//! 1. the command addresses ONE destination of the table it names — the destination
//!    `ACTIVE`ly replicating to the stream its `StreamArn` names — and stops that
//!    replication: the destination is `DISABLING` for the transient span and `DISABLED`
//!    after it, both as the command answers and as the reader reports;
//! 2. stopping replication changes no configuration, so the precision the destination
//!    replicated under survives the stop, and the answer carries no
//!    `EnableKinesisStreamingConfiguration` — the member reports "the destination for
//!    the Kinesis streaming information that is being enabled", and this command enables
//!    nothing — whatever configuration the request itself carried;
//! 3. a table with no `ACTIVE` destination for that stream is refused with HTTP 400
//!    `ValidationException` carrying DynamoDB's required-status sentence, whether it
//!    never registered the stream, registered a different one, or has already stopped
//!    (or is stopping) replicating to it — so a second stop of the same stream is
//!    refused, not idempotent. Every status that is not `ACTIVE`, against this command
//!    and against the configuration change alike, has its own suite
//!    (`kinesis_destination_state_refusals`);
//! 4. the table is resolved like every other table-addressed operation's: by bare name
//!    or by the table's ARN, with a name no table answers for refused as
//!    `ResourceNotFoundException` and a name outside the documented form refused by the
//!    validator's own clause; a `StreamArn` is bounded in LENGTH alone, so a value that
//!    clears that bound is read, and one that names no Kinesis data stream addresses no
//!    `ACTIVE` destination of this table — the required-status refusal, like every other
//!    stream the table does not replicate to;
//! 5. a stopped destination stays in the reader's report — it is the table's replication
//!    history — and the stop leaves every other destination, of this table and of
//!    another, exactly as it was.
//!
//! Each rule is asserted over a SET of inputs — the precision in force before the stop,
//! the configuration the stopping request carries, why the table has no `ACTIVE`
//! destination for the stream, and how the table is addressed — rather than one example.
//! Every command and every read is driven
//! over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<OperationName>`)
//! through the same `MultiServiceRouter` the emulator's front doors use, with nothing
//! seeded through the store.

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

/// Register one destination on `table` over the wire, leaving its registration span
/// owed: a caller that needs it replicating waits that span out with [`settle`].
async fn register(
    router: &MultiServiceRouter,
    table: &str,
    stream: &str,
    precision: Option<&str>,
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
}

/// DynamoDB's refusal of a stop against a destination that is not `ACTIVE`: the status
/// the command requires, stated over nothing of the request.
fn not_active_message() -> String {
    "Table is not in a valid state to enable Kinesis Streaming Destination: \
     KinesisStreamingDestination must be ACTIVE to perform DISABLE operation."
        .to_string()
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

/// One row of the table below: the case, the precision in force before the stop, and the
/// configuration member the stopping request carries.
type StopCase = (&'static str, Option<&'static str>, Option<Value>);

/// Rules 1 and 2: an accepted stop moves the destination it addresses through
/// `DISABLING` to `DISABLED` and changes no configuration, whatever precision was in
/// force and whatever the request itself carried.
#[tokio::test]
async fn a_disable_stops_replication_to_the_stream_it_names() {
    let cases: Vec<StopCase> = vec![
        ("no precision in force", None, None),
        ("millisecond in force", Some("MILLISECOND"), None),
        ("microsecond in force", Some("MICROSECOND"), None),
        (
            "millisecond in force, the stopping request names another precision",
            Some("MILLISECOND"),
            Some(json!({"ApproximateCreationDateTimePrecision": "MICROSECOND"})),
        ),
        (
            "no precision in force, the stopping request carries an empty configuration",
            None,
            Some(json!({})),
        ),
    ];

    for (index, (case, precision, configuration)) in cases.into_iter().enumerate() {
        let router = emulator();
        let table = format!("Stopped{index}");
        create_table(&router, &table, &[]).await;
        register(&router, &table, "cdc", precision).await;
        settle().await;
        let arn = stream_arn("cdc");

        let mut request = json!({"TableName": table.clone(), "StreamArn": arn.clone()});
        if let Some(configuration) = configuration {
            request["EnableKinesisStreamingConfiguration"] = configuration;
        }
        let answered = ok(&router, "DisableKinesisStreamingDestination", request).await;
        assert_eq!(
            answered["TableName"],
            json!(table),
            "case {case:?}: the answer names the table it changed, got {answered}"
        );
        assert_eq!(
            answered["StreamArn"],
            json!(arn),
            "case {case:?}: the answer names the stream it stopped replicating to, got {answered}"
        );
        // Stopping replication is asynchronous, so the destination is reported in the
        // transient status until the span has elapsed.
        assert_eq!(
            answered["DestinationStatus"],
            json!("DISABLING"),
            "case {case:?}: expected the transient status, got {answered}"
        );
        assert_eq!(
            answered["EnableKinesisStreamingConfiguration"],
            Value::Null,
            "case {case:?}: a stop enables nothing, so it reports no configuration, got \
             {answered}"
        );

        let stopping = described(&router, &table, &arn).await;
        assert_eq!(
            stopping["DestinationStatus"],
            json!("DISABLING"),
            "case {case:?}: the reader reports the transient status too, got {stopping}"
        );
        assert_eq!(
            stopping["ApproximateCreationDateTimePrecision"],
            precision.map_or(Value::Null, |precision| json!(precision)),
            "case {case:?}: expected the precision in force to be {precision:?}, got {stopping}"
        );

        settle().await;
        let stopped = described(&router, &table, &arn).await;
        assert_eq!(
            stopped["DestinationStatus"],
            json!("DISABLED"),
            "case {case:?}: once the span has elapsed replication has stopped, got {stopped}"
        );
        assert_eq!(
            stopped["ApproximateCreationDateTimePrecision"],
            precision.map_or(Value::Null, |precision| json!(precision)),
            "case {case:?}: the precision replicated under survives the stop, got {stopped}"
        );
    }
}

/// Rule 5: the stop changes the ONE destination it addresses, leaves every other
/// destination — of this table and of another — where it was, and the stopped record
/// stays in the report as this table's replication history, in registration order.
#[tokio::test]
async fn a_disable_stops_only_the_destination_it_addresses() {
    let router = emulator();
    create_table(&router, "Addressed", &[]).await;
    create_table(&router, "Untouched", &[]).await;
    // A table replicates to ONE stream at a time, so the history below is built the only
    // way a client can build one: replication to `cdc` is started and stopped, and `audit`
    // is registered after it — which is the destination the stop under test addresses.
    register(&router, "Addressed", "cdc", Some("MICROSECOND")).await;
    settle().await;
    ok(
        &router,
        "DisableKinesisStreamingDestination",
        json!({"TableName": "Addressed", "StreamArn": stream_arn("cdc")}),
    )
    .await;
    settle().await;
    register(&router, "Addressed", "audit", Some("MILLISECOND")).await;
    register(&router, "Untouched", "cdc", Some("MICROSECOND")).await;
    settle().await;

    let addressed = stream_arn("audit");
    ok(
        &router,
        "DisableKinesisStreamingDestination",
        json!({"TableName": "Addressed", "StreamArn": addressed.clone()}),
    )
    .await;

    // (case, the table, the stream, the expected status, the expected precision)
    let expected: Vec<(&str, &str, &str, &str, Option<&str>)> = vec![
        (
            "the destination addressed",
            "Addressed",
            "audit",
            "DISABLING",
            Some("MILLISECOND"),
        ),
        (
            "a destination registered before it",
            "Addressed",
            "cdc",
            "DISABLED",
            Some("MICROSECOND"),
        ),
        (
            "the same stream of another table",
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
            "case {case:?}: expected {status}, got {reported}"
        );
        assert_eq!(
            reported["ApproximateCreationDateTimePrecision"],
            precision.map_or(Value::Null, |precision| json!(precision)),
            "case {case:?}: expected the precision {precision:?}, got {reported}"
        );
    }

    // The stopped record is still reported, in the position it was registered in: the
    // report is the table's replication history and not only its live replication.
    settle().await;
    let report = ok(
        &router,
        "DescribeKinesisStreamingDestination",
        json!({"TableName": "Addressed"}),
    )
    .await;
    let order: Vec<(Value, Value)> = report["KinesisDataStreamDestinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|destination| {
            (
                destination["StreamArn"].clone(),
                destination["DestinationStatus"].clone(),
            )
        })
        .collect();
    let wanted: Vec<(Value, Value)> = vec![
        (json!(stream_arn("cdc")), json!("DISABLED")),
        (json!(stream_arn("audit")), json!("DISABLED")),
    ];
    assert_eq!(
        order, wanted,
        "case \"the replication history\": expected {wanted:?}, got {order:?}"
    );
}

/// One row of the refusal table below: the case, the stream the table registers before
/// the stop under test — with the precision it registers it under, if any — and whether
/// that registration is stopped first.
type NotActiveCase = (&'static str, Option<(&'static str, Option<&'static str>)>, bool);

/// Rule 3: a table with no `ACTIVE` destination for the stream is refused, whichever way
/// it came to have none — including a stream whose replication this very suite already
/// stopped, so a second stop is refused rather than silently accepted.
#[tokio::test]
async fn a_disable_without_an_active_destination_is_refused() {
    // (case, the stream the table registers — if any — and where its lifecycle is left)
    let cases: Vec<NotActiveCase> = vec![
        ("never registered one", None, false),
        ("registered another stream", Some(("other", None)), false),
        (
            "already stopped replicating to it",
            Some(("cdc", Some("MICROSECOND"))),
            true,
        ),
        (
            "is still stopping replication to it",
            Some(("cdc", None)),
            true,
        ),
    ];

    for (index, (case, registered, stop_first)) in cases.into_iter().enumerate() {
        let router = emulator();
        let table = format!("NotActive{index}");
        create_table(&router, &table, &[]).await;
        let arn = stream_arn("cdc");
        if let Some((stream, precision)) = registered {
            register(&router, &table, stream, precision).await;
            settle().await;
        }
        if stop_first {
            ok(
                &router,
                "DisableKinesisStreamingDestination",
                json!({"TableName": table.clone(), "StreamArn": arn.clone()}),
            )
            .await;
            // "already stopped" waits the stop span out; "still stopping" does not, so
            // the two rows address a `DISABLED` and a `DISABLING` record respectively.
            if case == "already stopped replicating to it" {
                settle().await;
            }
        }

        let (status, body) = call(
            &router,
            "DisableKinesisStreamingDestination",
            json!({"TableName": table.clone(), "StreamArn": arn.clone()}),
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
            not_active_message(),
            "case {case:?}: expected DynamoDB's required-status sentence, got {}",
            message(&body)
        );

        // A refused stop changes nothing: no destination of the table is left mid-stop
        // that was not already stopping before the refused command.
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
            reported
                .iter()
                .all(|destination| destination["StreamArn"] != json!(arn)
                    || destination["DestinationStatus"] == json!("DISABLED")
                    || destination["DestinationStatus"] == json!("DISABLING")),
            "case {case:?}: the refused command must start no stop of its own, got {report}"
        );
        assert!(
            reported.iter().all(|destination| destination["StreamArn"]
                == json!(arn)
                || destination["DestinationStatus"] == json!("ACTIVE")),
            "case {case:?}: the refused command must leave the other destinations \
             replicating, got {report}"
        );
    }
}

/// Rule 4: the table is resolved by name or by ARN, a table no name answers for is
/// refused as a missing resource, and a `StreamArn` that names no Kinesis data stream is
/// refused as a member the service cannot read.
#[tokio::test]
async fn a_disable_resolves_the_table_and_the_stream_the_request_names() {
    let router = emulator();
    create_table(&router, "Resolved", &[]).await;
    register(&router, "Resolved", "cdc", None).await;
    settle().await;
    let arn = stream_arn("cdc");

    // (case, the TableName member, the expected error code, the expected message)
    let refusals: Vec<(&str, String, &str, String)> = vec![
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

    for (case, table_name, expected_code, expected_message) in refusals {
        let (status, body) = call(
            &router,
            "DisableKinesisStreamingDestination",
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
            "case {case:?}: expected {expected_message:?}, got {}",
            message(&body)
        );
    }

    // Every value here satisfies the `StreamArn` member's documented 37..=1024 length
    // bound (`smithy_query(query="operation",operation="DisableKinesisStreamingDestination")`),
    // which is the only constraint the member carries: each is READ, and names no stream
    // this table replicates to, so each earns the required-status refusal.
    for value in [
        "arn:aws:dynamodb:us-east-1:000000000000:table/Other",
        "arn:aws:kinesis:us-east-1:000000000000:table/Other",
        "arn:aws:kinesis::000000000000:stream/no-region-at-all",
        "arn:aws:kinesis:us-east-1:000000000000:stream/",
        "not-an-arn-but-long-enough-to-clear-the-length-bound",
    ] {
        let (status, body) = call(
            &router,
            "DisableKinesisStreamingDestination",
            json!({"TableName": "Resolved", "StreamArn": value}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {value:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            message(&body),
            not_active_message(),
            "case {value:?}: expected the required-status sentence, got {}",
            message(&body)
        );
    }

    // A `StreamArn` shorter than the member's documented minimum is refused by the
    // boundary validator's own length clause, before the handler reads it.
    let short = "arn:aws:kinesis:x";
    let (status, body) = call(
        &router,
        "DisableKinesisStreamingDestination",
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

    // Nothing above was accepted, so the destination still replicates and the same
    // command addressed through the table's own ARN — the documented alternative
    // spelling of the member — resolves to the same table and stops it.
    let answered = ok(
        &router,
        "DisableKinesisStreamingDestination",
        json!({
            "TableName": state::table_arn(REGION, "Resolved"),
            "StreamArn": arn.clone(),
        }),
    )
    .await;
    assert_eq!(
        answered["TableName"],
        json!("Resolved"),
        "case \"addressed by ARN\": the answer names the resolved table, got {answered}"
    );
    assert_eq!(
        answered["DestinationStatus"],
        json!("DISABLING"),
        "case \"addressed by ARN\": expected the transient status, got {answered}"
    );
}
