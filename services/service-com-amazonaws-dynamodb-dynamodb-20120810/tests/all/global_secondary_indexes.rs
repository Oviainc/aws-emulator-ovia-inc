// SPDX-License-Identifier: CC0-1.0

//! What a global secondary index LOOKS LIKE to a reader, which is what another service
//! resolves an `index` scalable target against.
//!
//! `regions_indexes_and_capacity.rs` covers `CreateTable`'s index rules and `UpdateTable`'s
//! capacity moves. This suite pins the narrow contract a CROSS-SERVICE reader depends on —
//! Application Auto Scaling resolves a `dynamodb:index:*` target by looking the named index
//! up on the table and reading its provisioned capacity — over a table of index shapes:
//!
//! * every index the table was created with is reported by `DescribeTable`, by name, with
//!   the key schema and the provisioned capacity it was created with;
//! * an index name the table does not carry is absent from that list, so a reader cannot
//!   resolve one that does not exist.

use serde_json::{json, Value};

use crate::emulator::{call, emulator};

/// One row: the index to create, and the capacity a reader must find on it.
struct IndexRow {
    case: &'static str,
    index: &'static str,
    key: &'static str,
    read: i64,
    write: i64,
}

/// The rows differ in the attribute the index is keyed on, in its name, and in both
/// capacity dimensions — the values a `dynamodb:index:ReadCapacityUnits` or
/// `…:WriteCapacityUnits` target scales.
const INDEX_TABLE: &[IndexRow] = &[
    IndexRow {
        case: "an index keyed on a string attribute",
        index: "by-customer",
        key: "customer",
        read: 3,
        write: 4,
    },
    IndexRow {
        case: "an index keyed on a number attribute, with equal capacities",
        index: "by-placed",
        key: "placed",
        read: 1,
        write: 1,
    },
];

#[tokio::test]
async fn every_created_index_is_reported_with_the_capacity_it_was_created_with() {
    let router = emulator();
    let indexes: Vec<Value> = INDEX_TABLE
        .iter()
        .map(|row| {
            json!({
                "IndexName": row.index,
                "KeySchema": [{"AttributeName": row.key, "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {
                    "ReadCapacityUnits": row.read, "WriteCapacityUnits": row.write,
                },
            })
        })
        .collect();
    let (status, created) = call(
        &router,
        "CreateTable",
        json!({
            "TableName": "orders",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "customer", "AttributeType": "S"},
                {"AttributeName": "placed", "AttributeType": "N"},
            ],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 2},
            "GlobalSecondaryIndexes": indexes,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create\": expected CreateTable to answer 200, got {status} {created}"
    );

    let (status, described) = call(&router, "DescribeTable", json!({"TableName": "orders"})).await;
    assert_eq!(
        status, 200,
        "case \"describe\": expected DescribeTable to answer 200, got {status} {described}"
    );
    let reported = described["Table"]["GlobalSecondaryIndexes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        reported.len(),
        INDEX_TABLE.len(),
        "case \"describe\": expected every index the table was created with, got {described}"
    );

    for row in INDEX_TABLE {
        let case = row.case;
        let index = reported
            .iter()
            .find(|index| index["IndexName"] == json!(row.index))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an index named {:?}, got {reported:?}", row.index)
            });
        assert_eq!(
            index["KeySchema"],
            json!([{"AttributeName": row.key, "KeyType": "HASH"}]),
            "case {case:?}: expected the key schema it was created with, got {index}"
        );
        assert_eq!(
            index["ProvisionedThroughput"]["ReadCapacityUnits"],
            json!(row.read),
            "case {case:?}: expected the read capacity a reader scales, got {index}"
        );
        assert_eq!(
            index["ProvisionedThroughput"]["WriteCapacityUnits"],
            json!(row.write),
            "case {case:?}: expected the write capacity a reader scales, got {index}"
        );
    }

    // An index the table does not carry is absent, so a reader resolving one by name
    // finds nothing rather than a default.
    assert!(
        !reported
            .iter()
            .any(|index| index["IndexName"] == json!("by-nothing")),
        "case \"an index the table does not carry\": expected it to be absent, got {reported:?}"
    );
}

#[tokio::test]
async fn a_table_created_without_an_index_reports_none() {
    let router = emulator();
    let (status, created) = call(
        &router,
        "CreateTable",
        json!({
            "TableName": "plain",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create\": expected CreateTable to answer 200, got {status} {created}"
    );
    let (status, described) = call(&router, "DescribeTable", json!({"TableName": "plain"})).await;
    assert_eq!(
        status, 200,
        "case \"describe\": expected DescribeTable to answer 200, got {status} {described}"
    );
    assert!(
        described["Table"]
            .get("GlobalSecondaryIndexes")
            .is_none_or(|indexes| indexes.as_array().is_some_and(Vec::is_empty)),
        "case \"describe\": expected a table with no index to report none, got {described}"
    );
}
