// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the status an accepted `UpdateTable` reports, and for what a
//! client that follows the documented sequence — submit the update, confirm the table
//! went to `UPDATING`, wait for it to come back to `ACTIVE` — observes.
//!
//! The rule under test: an `UpdateTable` whose work outlives the call is accepted with
//! the table reported `UPDATING` and the clock owes it the return to `ACTIVE`
//! ("UpdateTable is an asynchronous operation; while it's executing, the table status
//! changes from ACTIVE to UPDATING" and "UPDATING - The table/index configuration is
//! being updated. The table/index remains available for data operations when UPDATING" —
//! `TableStatus`); only a change the service applies with the call itself leaves the
//! table `ACTIVE`. Re-plumbing the table's change feed is such work: a
//! `StreamSpecification` starts or stops a DynamoDB Streams stream, a resource with its
//! own lifecycle, exactly as moving capacity provisions one.
//!
//! The rows below span the class the rule generalises over rather than one recorded
//! example: each KIND of change an `UpdateTable` can ask for (the stream on and off, the
//! provisioned throughput, the capacity mode, the storage class, the warm pair, an index
//! created and an index's throughput moved, a stream change combined with a flag) and the
//! one kind the service applies with the call. Every row also pins what the transient
//! span means: the new configuration is in force once it has settled, a second
//! `UpdateTable` inside the span conflicts, and item operations keep being served.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{
    call, describe_when_active, emulator_with_streams, error_code, index_named, REGION,
};

/// One kind of `UpdateTable`, with what it must report and what it must leave in force.
struct Row {
    /// The row this case's assertions name.
    case: &'static str,
    /// Whether the table is created with its stream already on, for a row that asks for
    /// a change to an existing stream.
    stream_on_at_create: bool,
    /// The request, minus the `TableName` every row carries.
    update: Value,
    /// The `TableStatus` the accepted update — and an immediate `DescribeTable` — must
    /// report.
    status_after_the_call: &'static str,
    /// One member of the settled description this update puts in force: a JSON pointer
    /// into the `Table`, and the value it must then carry — `None` where the update
    /// takes that member away.
    in_force: (&'static str, Option<Value>),
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a stream turned on",
            stream_on_at_create: false,
            update: json!({
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "KEYS_ONLY"},
            }),
            status_after_the_call: "UPDATING",
            in_force: (
                "/StreamSpecification",
                Some(json!({"StreamEnabled": true, "StreamViewType": "KEYS_ONLY"})),
            ),
        },
        Row {
            case: "a stream turned on capturing both images",
            stream_on_at_create: false,
            update: json!({
                "StreamSpecification": {
                    "StreamEnabled": true,
                    "StreamViewType": "NEW_AND_OLD_IMAGES",
                },
            }),
            status_after_the_call: "UPDATING",
            in_force: (
                "/StreamSpecification",
                Some(json!({"StreamEnabled": true, "StreamViewType": "NEW_AND_OLD_IMAGES"})),
            ),
        },
        Row {
            case: "a stream turned off",
            stream_on_at_create: true,
            update: json!({"StreamSpecification": {"StreamEnabled": false}}),
            status_after_the_call: "UPDATING",
            // A table whose stream is off carries no stream configuration at all.
            in_force: ("/StreamSpecification", None),
        },
        Row {
            case: "a stream turned off beside a flag the call applies",
            stream_on_at_create: true,
            update: json!({
                "StreamSpecification": {"StreamEnabled": false},
                "DeletionProtectionEnabled": true,
            }),
            status_after_the_call: "UPDATING",
            in_force: ("/DeletionProtectionEnabled", Some(json!(true))),
        },
        Row {
            case: "the provisioned throughput moved",
            stream_on_at_create: false,
            update: json!({
                "ProvisionedThroughput": {"ReadCapacityUnits": 9, "WriteCapacityUnits": 11},
            }),
            status_after_the_call: "UPDATING",
            in_force: ("/ProvisionedThroughput/ReadCapacityUnits", Some(json!(9))),
        },
        Row {
            case: "the capacity mode switched",
            stream_on_at_create: false,
            update: json!({"BillingMode": "PAY_PER_REQUEST"}),
            status_after_the_call: "UPDATING",
            in_force: (
                "/BillingModeSummary/BillingMode",
                Some(json!("PAY_PER_REQUEST")),
            ),
        },
        Row {
            case: "the storage class changed",
            stream_on_at_create: false,
            update: json!({"TableClass": "STANDARD_INFREQUENT_ACCESS"}),
            status_after_the_call: "UPDATING",
            in_force: (
                "/TableClassSummary/TableClass",
                Some(json!("STANDARD_INFREQUENT_ACCESS")),
            ),
        },
        Row {
            case: "the warm pair raised",
            stream_on_at_create: false,
            update: json!({
                "WarmThroughput": {"ReadUnitsPerSecond": 48_000, "WriteUnitsPerSecond": 16_000},
            }),
            status_after_the_call: "UPDATING",
            in_force: (
                "/WarmThroughput/ReadUnitsPerSecond",
                Some(json!(48_000_i64)),
            ),
        },
        Row {
            case: "an index created",
            stream_on_at_create: false,
            update: json!({
                "AttributeDefinitions": [{"AttributeName": "Tag", "AttributeType": "S"}],
                "GlobalSecondaryIndexUpdates": [{"Create": {
                    "IndexName": "ByTag",
                    "KeySchema": [{"AttributeName": "Tag", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                    "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 3},
                }}],
            }),
            status_after_the_call: "UPDATING",
            in_force: ("/GlobalSecondaryIndexes/1/IndexName", Some(json!("ByTag"))),
        },
        Row {
            case: "an index's throughput moved",
            stream_on_at_create: false,
            update: json!({
                "GlobalSecondaryIndexUpdates": [{"Update": {
                    "IndexName": "ByOwner",
                    "ProvisionedThroughput": {"ReadCapacityUnits": 7, "WriteCapacityUnits": 7},
                }}],
            }),
            status_after_the_call: "UPDATING",
            in_force: (
                "/GlobalSecondaryIndexes/0/ProvisionedThroughput/ReadCapacityUnits",
                Some(json!(7)),
            ),
        },
        Row {
            // The one kind of change with nothing behind it to provision: the service
            // applies the guard flag with the call, so the table is reported settled.
            case: "only the deletion-protection flag",
            stream_on_at_create: false,
            update: json!({"DeletionProtectionEnabled": true}),
            status_after_the_call: "ACTIVE",
            in_force: ("/DeletionProtectionEnabled", Some(json!(true))),
        },
    ]
}

/// A table name derived from the row, so each row's table is its own.
fn table_name(case: &str) -> String {
    let mut name = String::from("t-");
    name.extend(case.chars().map(|character| {
        if character.is_ascii_alphanumeric() {
            character
        } else {
            '-'
        }
    }));
    name
}

/// Create the `PROVISIONED` table every row updates: one global secondary index to move,
/// an attribute to add an index on, and optionally a stream already on.
async fn create_table(router: &MultiServiceRouter, case: &str, name: &str, stream_on: bool) {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
        "AttributeDefinitions": [
            {"AttributeName": "Part", "AttributeType": "S"},
            {"AttributeName": "Owner", "AttributeType": "S"},
        ],
        "BillingMode": "PROVISIONED",
        "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
        "GlobalSecondaryIndexes": [{
            "IndexName": "ByOwner",
            "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"}],
            "Projection": {"ProjectionType": "ALL"},
            "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
        }],
    });
    if stream_on {
        body["StreamSpecification"] = json!({"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"});
    }
    let (status, created) = call(router, "CreateTable", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the table the row updates must be created, got {status} with {created}"
    );
    describe_when_active(router, case, name).await;
}

#[tokio::test]
async fn an_accepted_update_whose_work_outlives_the_call_reports_updating() {
    for row in rows() {
        let router = emulator_with_streams();
        let name = table_name(row.case);
        create_table(&router, row.case, &name, row.stream_on_at_create).await;

        let mut request = row.update.clone();
        request["TableName"] = json!(name);
        let (status, updated) = call(&router, "UpdateTable", request).await;
        assert_eq!(
            (status, updated["TableDescription"]["TableStatus"].clone()),
            (200, json!(row.status_after_the_call)),
            "case {:?}: expected HTTP 200 with TableStatus {}, got {status} with {updated}",
            row.case,
            row.status_after_the_call
        );

        // The status the response reports is the STORED one, so the very next reader sees
        // it too: that is what a client's "confirm the table went to UPDATING" step reads.
        let (status, described) = call(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            (status, described["Table"]["TableStatus"].clone()),
            (200, json!(row.status_after_the_call)),
            "case {:?}: DescribeTable must report the same status as the update, got {status} \
             with {described}",
            row.case
        );

        // "The table/index remains available for data operations when UPDATING".
        let (status, written) = call(
            &router,
            "PutItem",
            json!({"TableName": name, "Item": {"Part": {"S": "one"}, "Owner": {"S": "me"}}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: an item write is served whatever the table's status, got {status} with \
             {written}",
            row.case
        );

        // "While it's UPDATING, you can't issue another UpdateTable request" — the guard a
        // client gates on to avoid a second overlapping update; a settled table serves it.
        let (status, second) = call(
            &router,
            "UpdateTable",
            json!({"TableName": name, "TableClass": "STANDARD"}),
        )
        .await;
        if row.status_after_the_call == "UPDATING" {
            assert_eq!(
                (status, error_code(&second).as_str()),
                (400, "ResourceInUseException"),
                "case {:?}: a second UpdateTable inside the span must conflict, got {status} \
                 with {second}",
                row.case
            );
        } else {
            assert_eq!(
                status, 200,
                "case {:?}: a second UpdateTable on a settled table is served, got {status} \
                 with {second}",
                row.case
            );
        }

        // The clock settles the table on its own, and the configuration the update asked
        // for is what the settled table reports.
        let settled = describe_when_active(&router, row.case, &name).await;
        let (pointer, expected) = &row.in_force;
        assert_eq!(
            settled.pointer(pointer),
            expected.as_ref(),
            "case {:?}: the settled table must report {pointer} as {expected:?}, got {settled}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_client_waiting_out_a_stream_change_sees_updating_then_active() {
    // The documented sequence, for each view type a stream can be enabled with: submit
    // the update, confirm the table went to UPDATING, poll until it is ACTIVE again. The
    // statuses observed must be some UPDATING polls followed by ACTIVE and nothing else,
    // and the stream the update asked for must be the one the settled table reports.
    for view_type in ["KEYS_ONLY", "NEW_IMAGE", "OLD_IMAGE", "NEW_AND_OLD_IMAGES"] {
        let case = format!("a stream enabled with {view_type}");
        let router = emulator_with_streams();
        let name = format!("waited-{}", view_type.to_lowercase());
        create_table(&router, &case, &name, false).await;

        let (status, updated) = call(
            &router,
            "UpdateTable",
            json!({
                "TableName": name,
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": view_type},
            }),
        )
        .await;
        assert_eq!(
            (status, updated["TableDescription"]["TableStatus"].clone()),
            (200, json!("UPDATING")),
            "case {case:?}: the accepted change must be reported UPDATING, got {status} with \
             {updated}"
        );

        let mut observed: Vec<String> = Vec::new();
        let started = std::time::Instant::now();
        let settled = loop {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "case {case:?}: the table never came back to ACTIVE, observed {observed:?}"
            );
            let (status, described) =
                call(&router, "DescribeTable", json!({"TableName": name})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: every poll must be served, got {status} with {described}"
            );
            let reported = described["Table"]["TableStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            observed.push(reported.clone());
            if reported == "ACTIVE" {
                break described["Table"].clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        };
        assert_eq!(
            observed.first().map(String::as_str),
            Some("UPDATING"),
            "case {case:?}: the first poll after the update must still see UPDATING, observed \
             {observed:?}"
        );
        assert!(
            observed
                .iter()
                .take(observed.len() - 1)
                .all(|reported| reported == "UPDATING"),
            "case {case:?}: the table goes UPDATING then ACTIVE and nowhere else, observed \
             {observed:?}"
        );
        assert_eq!(
            settled["StreamSpecification"],
            json!({"StreamEnabled": true, "StreamViewType": view_type}),
            "case {case:?}: the settled table reports the stream the update enabled, got \
             {settled}"
        );
        // The stream the change plumbed in is a real one in the streams registry: the
        // table names it, and it is named in the region the request was signed for.
        let stream_arn = settled["LatestStreamArn"].as_str().unwrap_or_default();
        assert!(
            stream_arn.contains(&format!(":dynamodb:{REGION}:")) && stream_arn.contains("/stream/"),
            "case {case:?}: the settled table must name the stream it enabled, got {settled}"
        );
        // Nothing about the index the table carries moved, so the one asynchronous change
        // the row asked for is the only one the settled table reports.
        assert_eq!(
            index_named(&settled, "ByOwner")["IndexStatus"],
            json!("ACTIVE"),
            "case {case:?}: the table's index is untouched by a stream change, got {settled}"
        );
    }
}
