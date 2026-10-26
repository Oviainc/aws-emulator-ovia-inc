// SPDX-License-Identifier: CC0-1.0

//! `DeleteTable` ACCEPTS a table the registry holds, and reports the whole table.
//!
//! The rule pinned here: `DeleteTable` resolves the table by name in the request's
//! region exactly as `DescribeTable` does — "If the table is in the ACTIVE state, you
//! can delete it", and `ResourceNotFoundException` is modelled only for a table that
//! does not exist — so a table any accepted `CreateTable` registered is deletable, and
//! the accepted delete answers 200 with the SAME complete `TableDescription` projection
//! `DescribeTable` reports for that table, differing only in `TableStatus`, which is
//! `DELETING` ("After a DeleteTable request, the specified table is in the DELETING
//! state"). Which optional members the create configured — a `StreamSpecification`, a
//! billing mode, a sort key, secondary indexes, items already written — changes what
//! that description SAYS, never whether the table is found or whether the description
//! is there at all.
//!
//! The rows below are a TABLE spanning what the rule generalises over: a create with no
//! `StreamSpecification`, one that asks for a disabled stream, one per `StreamViewType`
//! the model declares, both billing modes, keys with and without a sort key, tables with
//! and without secondary indexes, and tables holding items. The recorded case — a
//! stream-enabled `PAY_PER_REQUEST` table torn down at the end of a streams workflow —
//! is ONE of them.
//!
//! Everything is driven over the wire against DynamoDB and DynamoDB Streams on one
//! engine, so nothing is asserted that a client could not observe.

use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::ops::types::STREAM_VIEW_TYPE_VALUES;

use crate::emulator::{
    call, call_in, describe_when_active, emulator_with_streams, error_code, ok, ACTIVE_DEADLINE,
    POLL_INTERVAL,
};

/// Every value the model's `StreamViewType` enum declares, taken from the list the
/// streams crate derives from that model, so these rows cannot drift from the enum.
const VIEW_TYPES: &[&str] = STREAM_VIEW_TYPE_VALUES;

/// One row of the table: a table to create, and what the create configured.
struct Row {
    case: &'static str,
    /// Whether the key schema carries a `RANGE` key beside the `HASH` key.
    with_sort_key: bool,
    /// `PAY_PER_REQUEST` or `PROVISIONED`.
    billing: &'static str,
    /// The `StreamSpecification` the create carries, if any.
    stream: Option<Value>,
    /// Whether the create also declares a global and a local secondary index.
    with_indexes: bool,
    /// How many items are written to the table before it is deleted.
    items: usize,
}

/// The rows this suite runs: the configurations a delete must be indifferent to.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "no StreamSpecification, on demand, hash key only",
            with_sort_key: false,
            billing: "PAY_PER_REQUEST",
            stream: None,
            with_indexes: false,
            items: 0,
        },
        Row {
            case: "no StreamSpecification, provisioned, sort key, items written",
            with_sort_key: true,
            billing: "PROVISIONED",
            stream: None,
            with_indexes: false,
            items: 3,
        },
        Row {
            case: "StreamEnabled false",
            with_sort_key: false,
            billing: "PROVISIONED",
            stream: Some(json!({"StreamEnabled": false})),
            with_indexes: false,
            items: 1,
        },
        Row {
            case: "a stream beside secondary indexes",
            with_sort_key: true,
            billing: "PROVISIONED",
            stream: Some(json!({"StreamEnabled": true, "StreamViewType": "NEW_AND_OLD_IMAGES"})),
            with_indexes: true,
            items: 2,
        },
    ];
    // One row per view type the model declares, alternating the members the rule is
    // indifferent to so no view type is only ever seen in one table shape.
    for (position, view_type) in VIEW_TYPES.iter().enumerate() {
        rows.push(Row {
            case: match *view_type {
                "KEYS_ONLY" => "StreamViewType KEYS_ONLY",
                "NEW_AND_OLD_IMAGES" => "StreamViewType NEW_AND_OLD_IMAGES",
                "NEW_IMAGE" => "StreamViewType NEW_IMAGE",
                "OLD_IMAGE" => "StreamViewType OLD_IMAGE",
                other => panic!("the model declares a StreamViewType this suite has no row for: {other:?}"),
            },
            with_sort_key: position % 2 == 1,
            billing: if position % 2 == 0 {
                "PAY_PER_REQUEST"
            } else {
                "PROVISIONED"
            },
            stream: Some(json!({"StreamEnabled": true, "StreamViewType": view_type})),
            with_indexes: position % 3 == 0,
            items: position,
        });
    }
    rows
}

/// Whether this row's create asks for a stream that is ON.
fn stream_is_on(row: &Row) -> bool {
    row.stream
        .as_ref()
        .is_some_and(|specification| specification["StreamEnabled"] == json!(true))
}

/// The `CreateTable` body for one row.
fn create_body(row: &Row, name: &str) -> Value {
    let mut key_schema = vec![json!({"AttributeName": "Pk", "KeyType": "HASH"})];
    let mut definitions = vec![json!({"AttributeName": "Pk", "AttributeType": "S"})];
    if row.with_sort_key {
        key_schema.push(json!({"AttributeName": "Sk", "KeyType": "RANGE"}));
        definitions.push(json!({"AttributeName": "Sk", "AttributeType": "N"}));
    }
    let mut body = json!({
        "TableName": name,
        "KeySchema": key_schema,
        "BillingMode": row.billing,
    });
    if row.billing == "PROVISIONED" {
        body["ProvisionedThroughput"] = json!({"ReadCapacityUnits": 4, "WriteCapacityUnits": 2});
    }
    if row.with_indexes {
        definitions.push(json!({"AttributeName": "Gsi", "AttributeType": "S"}));
        body["GlobalSecondaryIndexes"] = json!([{
            "IndexName": "ByGsi",
            "KeySchema": [{"AttributeName": "Gsi", "KeyType": "HASH"}],
            "Projection": {"ProjectionType": "ALL"},
        }]);
        if row.billing == "PROVISIONED" {
            body["GlobalSecondaryIndexes"][0]["ProvisionedThroughput"] =
                json!({"ReadCapacityUnits": 1, "WriteCapacityUnits": 1});
        }
        // A local secondary index needs the table's own sort key to exist.
        if row.with_sort_key {
            definitions.push(json!({"AttributeName": "Lsi", "AttributeType": "S"}));
            body["LocalSecondaryIndexes"] = json!([{
                "IndexName": "ByLsi",
                "KeySchema": [
                    {"AttributeName": "Pk", "KeyType": "HASH"},
                    {"AttributeName": "Lsi", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "KEYS_ONLY"},
            }]);
        }
    }
    body["AttributeDefinitions"] = json!(definitions);
    if let Some(specification) = row.stream.clone() {
        body["StreamSpecification"] = specification;
    }
    body
}

/// The item this row writes at index `which`.
fn item(row: &Row, which: usize) -> Value {
    let mut stored = json!({"Pk": {"S": format!("key-{which}")}, "Payload": {"S": "payload"}});
    if row.with_sort_key {
        stored["Sk"] = json!({"N": which.to_string()});
    }
    if row.with_indexes {
        stored["Gsi"] = json!({"S": format!("gsi-{which}")});
        if row.with_sort_key {
            stored["Lsi"] = json!({"S": format!("lsi-{which}")});
        }
    }
    stored
}

#[tokio::test]
async fn an_accepted_delete_reports_the_same_description_describe_table_does() {
    for row in rows() {
        let case = row.case;
        let router = emulator_with_streams();
        let name = "teardown-subject";

        let created = ok(&router, "CreateTable", create_body(&row, name)).await;
        assert_eq!(
            created["TableDescription"]["TableStatus"],
            json!("CREATING"),
            "case {case:?}: the create reports the table CREATING, got {created}"
        );
        let active = describe_when_active(&router, case, name).await;
        for which in 0..row.items {
            ok(
                &router,
                "PutItem",
                json!({"TableName": name, "Item": item(&row, which)}),
            )
            .await;
        }

        let (status, deleted) = call(&router, "DeleteTable", json!({"TableName": name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a table the create registered must be deletable, got {status} with \
             {deleted}"
        );
        let description = &deleted["TableDescription"];
        assert!(
            description.is_object(),
            "case {case:?}: the delete must report a TableDescription, got {deleted}"
        );

        // The whole projection, not a reduced one: the delete reports the table exactly
        // as DescribeTable did, with only the status moved on.
        let mut expected = active.clone();
        expected["TableStatus"] = json!("DELETING");
        assert_eq!(
            description, &expected,
            "case {case:?}: expected the delete to report the table DescribeTable reported with \
             TableStatus DELETING, got {description}"
        );
    }
}

#[tokio::test]
async fn the_reported_description_carries_every_member_the_table_was_created_with() {
    for row in rows() {
        let case = row.case;
        let router = emulator_with_streams();
        let name = "teardown-members";

        ok(&router, "CreateTable", create_body(&row, name)).await;
        describe_when_active(&router, case, name).await;
        for which in 0..row.items {
            ok(
                &router,
                "PutItem",
                json!({"TableName": name, "Item": item(&row, which)}),
            )
            .await;
        }

        let deleted = ok(&router, "DeleteTable", json!({"TableName": name})).await;
        let description = &deleted["TableDescription"];

        assert_eq!(
            description["TableStatus"], json!("DELETING"),
            "case {case:?}: expected TableStatus DELETING, got {description}"
        );
        assert_eq!(
            description["TableName"], json!(name),
            "case {case:?}: the description must name the table it is deleting, got {description}"
        );
        let arn = description["TableArn"].as_str().unwrap_or_default();
        assert!(
            arn.ends_with(&format!("table/{name}")),
            "case {case:?}: expected a TableArn naming the table, got {arn:?}"
        );
        assert!(
            !description["TableId"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: the description must carry the table's id, got {description}"
        );
        assert!(
            description["CreationDateTime"].is_number(),
            "case {case:?}: the description must carry the table's creation instant, got \
             {description}"
        );
        assert_eq!(
            description["KeySchema"], create_body(&row, name)["KeySchema"],
            "case {case:?}: the description must report the key schema the create declared, got \
             {description}"
        );
        // "DynamoDB updates this value approximately every six hours": a table created
        // moments ago has not been metered since, so both counters are still the figures
        // it started life with, whatever was written to it.
        assert_eq!(
            description["ItemCount"], json!(0),
            "case {case:?}: expected the metered ItemCount, got {description}"
        );
        assert_eq!(
            description["TableSizeBytes"], json!(0),
            "case {case:?}: expected the metered TableSizeBytes, got {description}"
        );
        assert_eq!(
            description["DeletionProtectionEnabled"], json!(false),
            "case {case:?}: a table created without deletion protection reports it off, got \
             {description}"
        );
        assert_eq!(
            description["BillingModeSummary"]["BillingMode"], json!(row.billing),
            "case {case:?}: expected BillingModeSummary to report {:?}, got {description}",
            row.billing
        );
        assert_eq!(
            description["TableThroughputModeSummary"]["TableThroughputMode"], json!(row.billing),
            "case {case:?}: expected TableThroughputModeSummary to report {:?}, got {description}",
            row.billing
        );
        let throughput = &description["ProvisionedThroughput"];
        let (read, write) = if row.billing == "PROVISIONED" {
            (4, 2)
        } else {
            // On-demand mode provisions nothing, so the reported pair is zeroes.
            (0, 0)
        };
        assert_eq!(
            throughput["ReadCapacityUnits"], json!(read),
            "case {case:?}: expected ReadCapacityUnits {read}, got {throughput}"
        );
        assert_eq!(
            throughput["WriteCapacityUnits"], json!(write),
            "case {case:?}: expected WriteCapacityUnits {write}, got {throughput}"
        );
        assert_eq!(
            throughput["NumberOfDecreasesToday"], json!(0),
            "case {case:?}: a table never decreased reports no decreases today, got {throughput}"
        );

        // The stream members are reported for a table whose stream is ON, and for no
        // other: "the ARN that uniquely identifies the latest stream for this table".
        if stream_is_on(&row) {
            let requested = row
                .stream
                .as_ref()
                .expect("a stream that is ON was asked for by a specification");
            assert_eq!(
                &description["StreamSpecification"], requested,
                "case {case:?}: the delete must echo the table's stream configuration, got \
                 {description}"
            );
            let stream_arn = description["LatestStreamArn"].as_str().unwrap_or_default();
            assert!(
                stream_arn.contains(&format!("table/{name}/stream/")),
                "case {case:?}: expected a LatestStreamArn naming the table's stream, got \
                 {stream_arn:?}"
            );
            assert!(
                !description["LatestStreamLabel"]
                    .as_str()
                    .unwrap_or_default()
                    .is_empty(),
                "case {case:?}: an enabled stream must be reported with its label, got \
                 {description}"
            );
        } else {
            for member in ["StreamSpecification", "LatestStreamArn", "LatestStreamLabel"] {
                assert!(
                    description.get(member).is_none(),
                    "case {case:?}: a table with no stream ON reports no {member}, got \
                     {description}"
                );
            }
        }

        // The secondary indexes the create declared are reported, and a table that
        // declared none reports no list at all.
        assert_eq!(
            description.get("GlobalSecondaryIndexes").is_some(),
            row.with_indexes,
            "case {case:?}: GlobalSecondaryIndexes is reported exactly for a table that has \
             one, got {description}"
        );
        assert_eq!(
            description.get("LocalSecondaryIndexes").is_some(),
            row.with_indexes && row.with_sort_key,
            "case {case:?}: LocalSecondaryIndexes is reported exactly for a table that has \
             one, got {description}"
        );
    }
}

#[tokio::test]
async fn only_a_table_the_registry_does_not_hold_is_answered_not_found() {
    // The counterpart of the rule: `ResourceNotFoundException` is modelled for a table
    // that does not exist, so it is answered for THAT and nothing else. Each case below
    // is a different way for a name not to resolve — never created, created in another
    // region, and created but already gone — and every one of them, unlike a
    // stream-enabled table standing in the registry, is a refusal with no description.
    let row = Row {
        case: "the stream-enabled table under test",
        with_sort_key: false,
        billing: "PAY_PER_REQUEST",
        stream: Some(json!({"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"})),
        with_indexes: false,
        items: 0,
    };
    let router = emulator_with_streams();

    // Created, then torn down, and the teardown allowed to complete.
    ok(&router, "CreateTable", create_body(&row, "recycled")).await;
    describe_when_active(&router, row.case, "recycled").await;
    ok(&router, "DeleteTable", json!({"TableName": "recycled"})).await;
    let removed = Instant::now();
    let mut last = Value::Null;
    while removed.elapsed() < ACTIVE_DEADLINE {
        let (_, body) = call(&router, "DescribeTable", json!({"TableName": "recycled"})).await;
        if error_code(&body) == "ResourceNotFoundException" {
            last = Value::Null;
            break;
        }
        last = body;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert_eq!(
        last, Value::Null,
        "the accepted delete must complete within {ACTIVE_DEADLINE:?}, last {last}"
    );

    // Created, but in another region, where this request does not look.
    let other = "eu-west-1";
    let (status, elsewhere) =
        call_in(&router, other, "CreateTable", create_body(&row, "regional")).await;
    assert_eq!(
        status, 200,
        "the table in {other} must be created, got {status} with {elsewhere}"
    );

    for (case, name) in [
        ("a name no create ever carried", "never-created"),
        ("a name created in another region", "regional"),
        ("a table whose deletion completed", "recycled"),
    ] {
        let (status, refused) = call(&router, "DeleteTable", json!({"TableName": name})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting a table that does not exist is refused, got {status} with \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused}"
        );
        assert!(
            refused.get("TableDescription").is_none(),
            "case {case:?}: a refusal carries no TableDescription, got {refused}"
        );
    }
}
