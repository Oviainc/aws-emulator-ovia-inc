// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DynamoDB Streams configuration of a table: the
//! `StreamSpecification` on `CreateTable` and `UpdateTable`, what every
//! `TableDescription` reports about the table's latest stream, and what the STREAMS
//! service then answers for it. Both services are mounted on one engine and driven
//! over the wire, so nothing is asserted about the store that a client cannot
//! observe.
//!
//! The rule under test: a `StreamSpecification` that enables a stream REGISTERS one
//! stream for that table in the DynamoDB Streams registry — the stream every streams
//! consumer discovers through `ListStreams` and reads through
//! `DescribeStream`/`GetShardIterator`/`GetRecords` — and the table reports that
//! stream's ARN and label (`TableDescription$LatestStreamArn`: "the ARN that uniquely
//! identifies the latest stream for this table", `$LatestStreamLabel`: "a timestamp,
//! in ISO 8601 format, for this stream"). A table that has never enabled one has no
//! stream and reports none. The two contradictions the model names are
//! `ResourceInUseException`: "you try to enable a stream on a table that already has
//! a stream, or ... disable a stream on a table that doesn't have a stream"
//! (`UpdateTable$StreamSpecification`).
//!
//! Every table below spans the class the rule generalises over rather than one
//! recorded example: each `StreamViewType` the model declares, tables with and
//! without a sort key, several tables in one account, and the enable/disable/
//! re-enable sequence.

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::ops::types::STREAM_VIEW_TYPE_VALUES;
use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::state::MAX_STREAMS_PER_TABLE;

use crate::emulator::{call_in, call_streams, emulator_with_streams, error_code, REGION};

/// Every value the model's `StreamViewType` enum declares, taken from the list the
/// streams crate derives from that model
/// (`ops::types::STREAM_VIEW_TYPE_VALUES` — `KEYS_ONLY`, `NEW_AND_OLD_IMAGES`,
/// `NEW_IMAGE`, `OLD_IMAGE`), so the table below cannot drift from the enum.
const STREAM_VIEW_TYPES: &[&str] = STREAM_VIEW_TYPE_VALUES;

/// A `CreateTable` body for `name`, optionally carrying a sort key and a
/// `StreamSpecification`.
fn create_body(name: &str, with_sort_key: bool, stream: Option<Value>) -> Value {
    let mut key_schema = vec![json!({"AttributeName": "Id", "KeyType": "HASH"})];
    let mut definitions = vec![json!({"AttributeName": "Id", "AttributeType": "S"})];
    if with_sort_key {
        key_schema.push(json!({"AttributeName": "Seq", "KeyType": "RANGE"}));
        definitions.push(json!({"AttributeName": "Seq", "AttributeType": "N"}));
    }
    let mut body = json!({
        "TableName": name,
        "KeySchema": key_schema,
        "AttributeDefinitions": definitions,
        "BillingMode": "PAY_PER_REQUEST",
    });
    if let Some(specification) = stream {
        body["StreamSpecification"] = specification;
    }
    body
}

/// Send one DynamoDB operation in [`REGION`].
async fn ddb(router: &smithy_server_runtime::MultiServiceRouter, op: &str, body: Value) -> (u16, Value) {
    call_in(router, REGION, op, body).await
}

/// Send one DynamoDB operation that must answer 200.
async fn ddb_ok(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    op: &str,
    body: Value,
) -> Value {
    let (status, parsed) = ddb(router, op, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {op} to answer HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Send one DynamoDB Streams operation that must answer 200.
async fn streams_ok(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    op: &str,
    body: Value,
) -> Value {
    let (status, parsed) = call_streams(router, op, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {op} to answer HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The `Streams` entries `ListStreams` reports, optionally scoped to one table.
async fn list_streams(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    table_name: Option<&str>,
) -> Vec<Value> {
    let body = match table_name {
        Some(name) => json!({"TableName": name}),
        None => json!({}),
    };
    let listed = streams_ok(router, case, "ListStreams", body).await;
    listed["Streams"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn text(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Wait out the `CREATING` -> `ACTIVE` transition the clock owes a new table, so a
/// control-plane call that needs a settled table is not refused as in-use. The
/// margin is slack for a loaded machine, not part of the rule.
async fn settle() {
    tokio::time::sleep(STATUS_TRANSITION + std::time::Duration::from_millis(150)).await;
}

#[tokio::test]
async fn enabling_a_stream_registers_it_and_every_consumer_sees_it() {
    // One row per `StreamViewType` the model declares, over tables with and without
    // a sort key: the view type is what the stream captures, and the key schema is
    // what a record is identified by, so both travel into the registry.
    for (position, view_type) in STREAM_VIEW_TYPES.iter().enumerate() {
        let with_sort_key = position % 2 == 1;
        let case = format!("{view_type} (sort key: {with_sort_key})");
        let router = emulator_with_streams();
        let name = format!("ledger-{}", view_type.to_lowercase().replace('_', "-"));

        let created = ddb_ok(
            &router,
            &case,
            "CreateTable",
            create_body(
                &name,
                with_sort_key,
                Some(json!({"StreamEnabled": true, "StreamViewType": view_type})),
            ),
        )
        .await;
        let description = &created["TableDescription"];
        assert_eq!(
            description["StreamSpecification"],
            json!({"StreamEnabled": true, "StreamViewType": view_type}),
            "case {case:?}: the create must echo the stream specification it stored, got {description}"
        );
        let stream_arn = text(description, "/LatestStreamArn");
        let stream_label = text(description, "/LatestStreamLabel");
        assert_eq!(
            stream_arn,
            format!(
                "arn:aws:dynamodb:{REGION}:{}:table/{name}/stream/{stream_label}",
                ::engine::emulator_account_id()
            ),
            "case {case:?}: LatestStreamArn must name this table's stream by its label, got {description}"
        );
        assert!(
            !stream_label.is_empty(),
            "case {case:?}: LatestStreamLabel must be reported, got {description}"
        );

        // Read back through DescribeTable: the stream identity is stored, not a
        // value the create response made up.
        let described = ddb_ok(
            &router,
            &case,
            "DescribeTable",
            json!({"TableName": name}),
        )
        .await;
        assert_eq!(
            text(&described, "/Table/LatestStreamArn"),
            stream_arn,
            "case {case:?}: DescribeTable must report the same stream ARN, got {described}"
        );
        assert_eq!(
            text(&described, "/Table/LatestStreamLabel"),
            stream_label,
            "case {case:?}: DescribeTable must report the same stream label, got {described}"
        );
        assert_eq!(
            described["Table"]["StreamSpecification"],
            json!({"StreamEnabled": true, "StreamViewType": view_type}),
            "case {case:?}: DescribeTable must report the stored specification, got {described}"
        );

        // The streams service is where the stream itself lives: one entry for this
        // table, naming the same ARN, label and table.
        let listed = list_streams(&router, &case, None).await;
        assert_eq!(
            listed,
            vec![json!({
                "StreamArn": stream_arn,
                "StreamLabel": stream_label,
                "TableName": name,
            })],
            "case {case:?}: ListStreams must report exactly this table's stream"
        );
        let scoped = list_streams(&router, &case, Some(&name)).await;
        assert_eq!(
            scoped, listed,
            "case {case:?}: ListStreams scoped to the table must report the same entry"
        );

        let streamed = streams_ok(
            &router,
            &case,
            "DescribeStream",
            json!({"StreamArn": stream_arn}),
        )
        .await;
        assert_eq!(
            text(&streamed, "/StreamDescription/StreamViewType"),
            *view_type,
            "case {case:?}: the stream must capture the view type the table asked for, got {streamed}"
        );
        assert_eq!(
            text(&streamed, "/StreamDescription/TableName"),
            name,
            "case {case:?}: the stream must name the table it belongs to, got {streamed}"
        );
        let expected_keys = if with_sort_key { 2 } else { 1 };
        assert_eq!(
            streamed["StreamDescription"]["KeySchema"]
                .as_array()
                .map(Vec::len),
            Some(expected_keys),
            "case {case:?}: the stream must carry the table's key schema, got {streamed}"
        );
        // Element for element, the schema the CREATE request carried: the stream
        // reports the key a change is recorded by, so it cannot be a rearrangement
        // or an abbreviation of the table's own key.
        assert_eq!(
            streamed["StreamDescription"]["KeySchema"],
            create_body(&name, with_sort_key, None)["KeySchema"],
            "case {case:?}: the stream's key schema must be the table's, got {streamed}"
        );
        let status = text(&streamed, "/StreamDescription/StreamStatus");
        assert!(
            status == "ENABLING" || status == "ENABLED",
            "case {case:?}: a freshly enabled stream is ENABLING then ENABLED, got {status}"
        );
    }
}

#[tokio::test]
async fn a_table_that_enables_no_stream_has_none() {
    // A create that says nothing about streams and one that explicitly turns them
    // off are the same table: no stream, nothing reported, nothing to discover.
    for (case, specification) in [
        ("no StreamSpecification", None),
        (
            "StreamEnabled false",
            Some(json!({"StreamEnabled": false})),
        ),
        (
            "StreamEnabled false with a view type",
            Some(json!({"StreamEnabled": false, "StreamViewType": "NEW_IMAGE"})),
        ),
    ] {
        let router = emulator_with_streams();
        let created = ddb_ok(
            &router,
            case,
            "CreateTable",
            create_body("quiet-table", false, specification),
        )
        .await;
        let description = &created["TableDescription"];
        for member in ["StreamSpecification", "LatestStreamArn", "LatestStreamLabel"] {
            assert_eq!(
                description.get(member),
                None,
                "case {case:?}: a table with no stream reports no {member}, got {description}"
            );
        }
        assert_eq!(
            list_streams(&router, case, None).await,
            Vec::<Value>::new(),
            "case {case:?}: ListStreams must report no stream"
        );
    }
}

#[tokio::test]
async fn enabling_a_stream_without_a_view_type_is_refused() {
    // `StreamViewType` "determines what information is written to the stream", so a
    // specification that turns the stream on without one asks for a stream whose
    // contents are undefined: a permanently invalid request, on either operation.
    let router = emulator_with_streams();
    let (status, body) = ddb(
        &router,
        "CreateTable",
        create_body("headless", false, Some(json!({"StreamEnabled": true}))),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"CreateTable without a view type\": expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"CreateTable without a view type\": expected ValidationException, got {body}"
    );
    assert_eq!(
        list_streams(&router, "CreateTable without a view type", None).await,
        Vec::<Value>::new(),
        "case \"CreateTable without a view type\": a refused create registers no stream"
    );

    ddb_ok(
        &router,
        "UpdateTable without a view type",
        "CreateTable",
        create_body("headless", false, None),
    )
    .await;
    let (status, body) = ddb(
        &router,
        "UpdateTable",
        json!({"TableName": "headless", "StreamSpecification": {"StreamEnabled": true}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"UpdateTable without a view type\": expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"UpdateTable without a view type\": expected ValidationException, got {body}"
    );
}

#[tokio::test]
async fn update_table_turns_a_stream_on_and_off_and_refuses_the_contradictions() {
    for view_type in STREAM_VIEW_TYPES {
        let case = format!("UpdateTable {view_type}");
        let router = emulator_with_streams();
        ddb_ok(
            &router,
            &case,
            "CreateTable",
            create_body("mutable", false, None),
        )
        .await;
        settle().await;

        // Enabling on a table that has no stream registers one.
        let updated = ddb_ok(
            &router,
            &case,
            "UpdateTable",
            json!({
                "TableName": "mutable",
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": view_type},
            }),
        )
        .await;
        let first_arn = text(&updated, "/TableDescription/LatestStreamArn");
        assert_eq!(
            updated["TableDescription"]["StreamSpecification"],
            json!({"StreamEnabled": true, "StreamViewType": view_type}),
            "case {case:?}: the update must report the stream it enabled, got {updated}"
        );
        assert_eq!(
            list_streams(&router, &case, Some("mutable"))
                .await
                .iter()
                .map(|entry| text(entry, "/StreamArn"))
                .collect::<Vec<String>>(),
            vec![first_arn.clone()],
            "case {case:?}: the enabled stream must be discoverable"
        );

        // "You receive a ResourceInUseException if you try to enable a stream on a
        // table that already has a stream."
        let (status, body) = ddb(
            &router,
            "UpdateTable",
            json!({
                "TableName": "mutable",
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "KEYS_ONLY"},
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: enabling a second stream must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceInUseException",
            "case {case:?}: expected ResourceInUseException, got {body}"
        );

        // Disabling turns the stream off; the table keeps reporting its LATEST
        // stream, and the stream itself stops being enabled.
        let disabled = ddb_ok(
            &router,
            &case,
            "UpdateTable",
            json!({
                "TableName": "mutable",
                "StreamSpecification": {"StreamEnabled": false},
            }),
        )
        .await;
        assert_eq!(
            disabled["TableDescription"]["StreamSpecification"],
            json!({"StreamEnabled": false}),
            "case {case:?}: a disabled stream reports the flag alone, got {disabled}"
        );
        assert_eq!(
            text(&disabled, "/TableDescription/LatestStreamArn"),
            first_arn,
            "case {case:?}: the table still names its latest stream, got {disabled}"
        );
        let described = streams_ok(
            &router,
            &case,
            "DescribeStream",
            json!({"StreamArn": first_arn}),
        )
        .await;
        let status = text(&described, "/StreamDescription/StreamStatus");
        assert!(
            status == "DISABLING" || status == "DISABLED",
            "case {case:?}: a disabled stream is DISABLING then DISABLED, got {status}"
        );

        // "... or if you try to disable a stream on a table that doesn't have a
        // stream" — which is now the case, the stream having been turned off.
        let (status, body) = ddb(
            &router,
            "UpdateTable",
            json!({
                "TableName": "mutable",
                "StreamSpecification": {"StreamEnabled": false},
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: disabling twice must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceInUseException",
            "case {case:?}: expected ResourceInUseException, got {body}"
        );

        // Re-enabling mints a NEW stream: a stream's identity is its ARN, and the
        // disabled one keeps the records it captured.
        let re_enabled = ddb_ok(
            &router,
            &case,
            "UpdateTable",
            json!({
                "TableName": "mutable",
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "KEYS_ONLY"},
            }),
        )
        .await;
        let second_arn = text(&re_enabled, "/TableDescription/LatestStreamArn");
        assert_ne!(
            second_arn, first_arn,
            "case {case:?}: re-enabling must register a new stream, got {re_enabled}"
        );
        let arns: Vec<String> = list_streams(&router, &case, Some("mutable"))
            .await
            .iter()
            .map(|entry| text(entry, "/StreamArn"))
            .collect();
        assert!(
            arns.contains(&first_arn) && arns.contains(&second_arn),
            "case {case:?}: both of the table's streams stay discoverable, got {arns:?}"
        );
    }
}

#[tokio::test]
async fn every_stream_enabled_table_gets_its_own_entry() {
    // Several tables in one account, only some of them stream-enabled: the registry
    // holds one stream per enabled table, in stream-ARN order, and a scoped
    // `ListStreams` answers for exactly one of them.
    let case = "three tables, two with streams";
    let router = emulator_with_streams();
    let enabled = ["orders", "payments"];
    for (name, specification) in [
        ("orders", Some(json!({"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"}))),
        ("ledger", None),
        (
            "payments",
            Some(json!({"StreamEnabled": true, "StreamViewType": "NEW_AND_OLD_IMAGES"})),
        ),
    ] {
        ddb_ok(
            &router,
            case,
            "CreateTable",
            create_body(name, false, specification),
        )
        .await;
    }

    let listed = list_streams(&router, case, None).await;
    let tables: Vec<String> = listed
        .iter()
        .map(|entry| text(entry, "/TableName"))
        .collect();
    assert_eq!(
        tables.len(),
        enabled.len(),
        "case {case:?}: one entry per stream-enabled table, got {listed:?}"
    );
    for name in enabled {
        assert!(
            tables.iter().any(|listed| listed == name),
            "case {case:?}: {name} must be listed, got {tables:?}"
        );
        let scoped = list_streams(&router, case, Some(name)).await;
        assert_eq!(
            scoped.len(),
            1,
            "case {case:?}: ListStreams for {name} must report its one stream, got {scoped:?}"
        );
        assert_eq!(
            text(&scoped[0], "/TableName"),
            name,
            "case {case:?}: ListStreams for {name} must report only that table's stream"
        );
    }
    let arns: Vec<String> = listed
        .iter()
        .map(|entry| text(entry, "/StreamArn"))
        .collect();
    let mut sorted = arns.clone();
    sorted.sort();
    assert_eq!(
        arns, sorted,
        "case {case:?}: ListStreams pages in stream-ARN order, got {arns:?}"
    );
    assert_eq!(
        list_streams(&router, case, Some("ledger")).await,
        Vec::<Value>::new(),
        "case {case:?}: a table with no stream has nothing to list"
    );
}

#[tokio::test]
async fn a_table_may_not_grow_the_stream_registry_without_limit() {
    // Turning a stream off and on again mints a new stream each time and keeps the
    // disabled predecessor (AWS deletes it 24 hours later), so the registry has a
    // per-table cap: past it the control plane answers the modelled
    // LimitExceededException rather than letting the store grow.
    let case = "enable/disable past the per-table cap";
    let router = emulator_with_streams();
    ddb_ok(
        &router,
        case,
        "CreateTable",
        create_body("busy", false, None),
    )
    .await;
    settle().await;

    let enable = json!({
        "TableName": "busy",
        "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "KEYS_ONLY"},
    });
    let disable = json!({
        "TableName": "busy",
        "StreamSpecification": {"StreamEnabled": false},
    });
    for cycle in 0..MAX_STREAMS_PER_TABLE {
        let (status, body) = ddb(&router, "UpdateTable", enable.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: cycle {cycle} must be accepted, got {status} with {body}"
        );
        let (status, body) = ddb(&router, "UpdateTable", disable.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: cycle {cycle} must disable again, got {status} with {body}"
        );
    }

    let (status, body) = ddb(&router, "UpdateTable", enable).await;
    assert_eq!(
        status, 400,
        "case {case:?}: the registration past the cap must be refused, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "case {case:?}: expected LimitExceededException, got {body}"
    );
    assert_eq!(
        list_streams(&router, case, Some("busy")).await.len(),
        MAX_STREAMS_PER_TABLE,
        "case {case:?}: the registry holds exactly the streams it accepted"
    );
}

#[tokio::test]
async fn deleting_a_table_disables_its_stream() {
    // "If you have DynamoDB Streams enabled on the table, then the corresponding
    // stream on that table goes into the DISABLED state" (the `DeleteTable` doc) —
    // and the delete still reports the table's own stream configuration.
    let case = "DeleteTable on a stream-enabled table";
    let router = emulator_with_streams();
    let created = ddb_ok(
        &router,
        case,
        "CreateTable",
        create_body(
            "ephemeral",
            false,
            Some(json!({"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"})),
        ),
    )
    .await;
    let stream_arn = text(&created, "/TableDescription/LatestStreamArn");
    settle().await;

    let deleted = ddb_ok(&router, case, "DeleteTable", json!({"TableName": "ephemeral"})).await;
    assert_eq!(
        text(&deleted, "/TableDescription/TableStatus"),
        "DELETING",
        "case {case:?}: the delete reports the table DELETING, got {deleted}"
    );
    assert_eq!(
        deleted["TableDescription"]["StreamSpecification"],
        json!({"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"}),
        "case {case:?}: the delete reports the table's stream configuration, got {deleted}"
    );
    assert_eq!(
        text(&deleted, "/TableDescription/LatestStreamArn"),
        stream_arn,
        "case {case:?}: the delete reports the table's latest stream, got {deleted}"
    );

    let described = streams_ok(
        &router,
        case,
        "DescribeStream",
        json!({"StreamArn": stream_arn}),
    )
    .await;
    let status = text(&described, "/StreamDescription/StreamStatus");
    assert!(
        status == "DISABLING" || status == "DISABLED",
        "case {case:?}: the stream of a deleted table is DISABLING then DISABLED, got {status}"
    );
}
