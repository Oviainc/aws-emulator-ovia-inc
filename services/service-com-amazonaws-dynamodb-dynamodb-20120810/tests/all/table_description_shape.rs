// SPDX-License-Identifier: CC0-1.0

//! In what ORDER a `TableDescription` reports a table's `AttributeDefinitions`, and what
//! warm throughput it reports for the table and for each of its indexes.
//!
//! `CreateTable`, `DescribeTable`, `UpdateTable` and `DeleteTable` all project the same
//! stored table, so these are rules of that projection rather than of one operation:
//!
//! 1. `AttributeDefinitions` is reported ordered by `AttributeName` whatever order the
//!    request declared the definitions in, so a client that compares its desired table
//!    shape against the echoed one (an idempotent create, drift detection, infrastructure
//!    diffing) sees no difference where there is none;
//! 2. warm throughput is "the minimum provisioned capacity maintained for immediate data
//!    access" (the `TableWarmThroughputDescription` doc), so every table AND every global
//!    secondary index reports one: the pair a request named for it, otherwise the capacity
//!    it is provisioned for, and the documented default pair when it has no provisioned
//!    capacity at all.
//!
//! Each rule is asserted over a TABLE of inputs spanning what it generalises over: how
//! many definitions are declared, whether the submission order already matches name
//! order, which scalar type each name carries, names that share a prefix; and the billing
//! mode, equal and unequal capacities per level, and whether a request names the pair.
//! The other members of a description (`TableId`, the throughput summaries, the metered
//! item figures) are pinned by `table_description_members`.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    DEFAULT_WARM_READ_UNITS_PER_SECOND, DEFAULT_WARM_WRITE_UNITS_PER_SECOND, STATUS_TRANSITION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{emulator, index_named, ok};

/// Wait out the `CREATING` -> `ACTIVE` transition the clock owes a new table.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) {
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
    let described = ok(router, "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        described["Table"]["TableStatus"],
        json!("ACTIVE"),
        "case wait {name}: expected the table to be ACTIVE once its transition is due, \
         got {described}"
    );
}

/// One row: the attribute definitions a `CreateTable` declares, in the order it declares
/// them.
struct DefinitionRow {
    case: &'static str,
    table: &'static str,
    /// `(AttributeName, AttributeType)` pairs, in submission order. The first is the
    /// table's `HASH` key; each of the rest keys a global secondary index, so every
    /// definition is used by some key schema, as `CreateTable` requires.
    submitted: &'static [(&'static str, &'static str)],
}

/// The rows differ in how many definitions are declared, in whether the submission order
/// already matches name order, in which of the three scalar types each name carries, and
/// in whether two names share a prefix. The names are lowercase, so the expected order is
/// the same under any sensible collation of them.
const DEFINITION_TABLE: &[DefinitionRow] = &[
    DefinitionRow {
        case: "a single definition",
        table: "single",
        submitted: &[("pk", "S")],
    },
    DefinitionRow {
        case: "the table key declared before the index key",
        table: "key_first",
        submitted: &[("pk", "S"), ("gsi_pk", "S")],
    },
    DefinitionRow {
        case: "the same two declared the other way round",
        table: "key_second",
        submitted: &[("gsi_pk", "S"), ("pk", "S")],
    },
    DefinitionRow {
        case: "three definitions in reverse name order, one of each scalar type",
        table: "reversed",
        submitted: &[("c", "B"), ("b", "N"), ("a", "S")],
    },
    DefinitionRow {
        case: "four definitions, two of them sharing a prefix",
        table: "prefixed",
        submitted: &[
            ("id", "S"),
            ("id_2", "N"),
            ("created_at", "S"),
            ("owner", "B"),
        ],
    },
];

/// The `CreateTable` body for one row: a `PROVISIONED` table keyed on the first
/// definition, with one global secondary index per remaining definition.
fn create_body(row: &DefinitionRow) -> Value {
    let indexes: Vec<Value> = row.submitted[1..]
        .iter()
        .map(|(name, _)| {
            json!({
                "IndexName": format!("by-{name}"),
                "KeySchema": [{"AttributeName": name, "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "KEYS_ONLY"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            })
        })
        .collect();
    let definitions: Vec<Value> = row
        .submitted
        .iter()
        .map(|(name, kind)| json!({"AttributeName": name, "AttributeType": kind}))
        .collect();
    let mut body = json!({
        "TableName": row.table,
        "KeySchema": [{"AttributeName": row.submitted[0].0, "KeyType": "HASH"}],
        "AttributeDefinitions": definitions,
        "BillingMode": "PROVISIONED",
        "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 2},
    });
    if !indexes.is_empty() {
        body["GlobalSecondaryIndexes"] = Value::Array(indexes);
    }
    body
}

/// The definitions in the order a description must report them: by attribute name, each
/// name still carrying its own type.
fn expected_definitions(row: &DefinitionRow) -> Value {
    let mut ordered: Vec<&(&str, &str)> = row.submitted.iter().collect();
    ordered.sort_by(|left, right| left.0.cmp(right.0));
    Value::Array(
        ordered
            .into_iter()
            .map(|(name, kind)| json!({"AttributeName": name, "AttributeType": kind}))
            .collect(),
    )
}

#[tokio::test]
async fn attribute_definitions_are_reported_ordered_by_name_whatever_order_was_submitted() {
    let router = emulator();
    for row in DEFINITION_TABLE {
        let case = row.case;
        let expected = expected_definitions(row);

        let created = ok(&router, "CreateTable", create_body(row)).await;
        assert_eq!(
            created["TableDescription"]["AttributeDefinitions"], expected,
            "case {case:?}: expected CreateTable to echo the definitions ordered by name, \
             got {created}"
        );

        wait_until_active(&router, row.table).await;
        let described = ok(&router, "DescribeTable", json!({"TableName": row.table})).await;
        assert_eq!(
            described["Table"]["AttributeDefinitions"], expected,
            "case {case:?}: expected DescribeTable to report the same ordering, got {described}"
        );

        // An UpdateTable that changes nothing about the definitions reports the same
        // list: the ordering is the projection's, not something a write establishes.
        let updated = ok(
            &router,
            "UpdateTable",
            json!({
                "TableName": row.table,
                "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 3},
            }),
        )
        .await;
        assert_eq!(
            updated["TableDescription"]["AttributeDefinitions"], expected,
            "case {case:?}: expected UpdateTable's echo to report the same ordering, \
             got {updated}"
        );

        std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
        let deleted = ok(&router, "DeleteTable", json!({"TableName": row.table})).await;
        assert_eq!(
            deleted["TableDescription"]["AttributeDefinitions"], expected,
            "case {case:?}: expected DeleteTable's echo to report the same ordering, \
             got {deleted}"
        );
    }
}

#[tokio::test]
async fn two_creates_differing_only_in_submission_order_echo_the_same_shape() {
    let router = emulator();
    // The two rows declare the SAME definitions in opposite orders — the comparison a
    // client making an idempotent create, or diffing its desired shape against the
    // reported one, performs.
    let first = &DEFINITION_TABLE[1];
    let second = &DEFINITION_TABLE[2];
    let one = ok(&router, "CreateTable", create_body(first)).await;
    let other = ok(&router, "CreateTable", create_body(second)).await;
    assert_eq!(
        one["TableDescription"]["AttributeDefinitions"],
        other["TableDescription"]["AttributeDefinitions"],
        "case \"same definitions, opposite submission order\": expected one echoed shape, \
         got {one} and {other}"
    );
}

/// One row of the warm-throughput table: the capacity a table and its index are created
/// with, optionally a `WarmThroughput` the request names, and the pair each level must
/// then report.
struct WarmRow {
    case: &'static str,
    table: &'static str,
    /// The table's and the index's provisioned capacity; `None` on both for an on-demand
    /// table, which has no provisioned capacity at all.
    table_capacity: Option<(i64, i64)>,
    index_capacity: Option<(i64, i64)>,
    /// The `WarmThroughput` the create names for the TABLE, member by member; `None`
    /// leaves the member out, and `None` for the row names no pair at all. No request may
    /// name one per index — `CreateTable` refuses that member with the scaffold's 501.
    named: Option<(Option<i64>, Option<i64>)>,
    table_warm: (i64, i64),
    index_warm: (i64, i64),
}

/// The rows span what the rule generalises over: whether the resource has provisioned
/// capacity at all, equal and unequal capacities set independently on the table and on
/// its index, and whether a request named the table's pair.
const WARM_TABLE: &[WarmRow] = &[
    WarmRow {
        case: "a provisioned table and index with equal capacities",
        table: "warm_equal",
        table_capacity: Some((5, 5)),
        index_capacity: Some((5, 5)),
        named: None,
        table_warm: (5, 5),
        index_warm: (5, 5),
    },
    WarmRow {
        case: "a provisioned table and index with different capacities per dimension",
        table: "warm_uneven",
        table_capacity: Some((7, 2)),
        index_capacity: Some((1, 9)),
        named: None,
        table_warm: (7, 2),
        index_warm: (1, 9),
    },
    WarmRow {
        case: "an on-demand table and index, which carry no provisioned capacity",
        table: "warm_on_demand",
        table_capacity: None,
        index_capacity: None,
        named: None,
        table_warm: (
            DEFAULT_WARM_READ_UNITS_PER_SECOND,
            DEFAULT_WARM_WRITE_UNITS_PER_SECOND,
        ),
        index_warm: (
            DEFAULT_WARM_READ_UNITS_PER_SECOND,
            DEFAULT_WARM_WRITE_UNITS_PER_SECOND,
        ),
    },
    WarmRow {
        case: "a provisioned table whose request names a warm throughput of its own",
        table: "warm_named",
        table_capacity: Some((2, 2)),
        index_capacity: Some((3, 4)),
        named: Some((Some(24_000), Some(8_000))),
        // The named pair is what the table keeps warm; the index, which no request can
        // name one for, still keeps its own provisioned capacity warm.
        table_warm: (24_000, 8_000),
        index_warm: (3, 4),
    },
    WarmRow {
        case: "a request naming exactly the pair already in force",
        table: "warm_named_same",
        table_capacity: Some((5, 5)),
        index_capacity: Some((5, 5)),
        named: Some((Some(5), Some(5))),
        // Asking for what is already in force asks for no change, and the pair reported
        // is still the one the request named.
        table_warm: (5, 5),
        index_warm: (5, 5),
    },
    WarmRow {
        case: "a request naming only the read half of the pair",
        table: "warm_named_read",
        table_capacity: Some((6, 3)),
        index_capacity: Some((2, 2)),
        named: Some((Some(30_000), None)),
        // The member the request leaves out keeps the value in force, which for a table
        // nobody had named a pair for is its own write capacity.
        table_warm: (30_000, 3),
        index_warm: (2, 2),
    },
];

/// The warm-throughput structure a description must carry for `(read, write)` units.
fn expected_warm((read, write): (i64, i64)) -> Value {
    json!({
        "ReadUnitsPerSecond": read,
        "WriteUnitsPerSecond": write,
        "Status": "ACTIVE",
    })
}

#[tokio::test]
async fn warm_throughput_is_reported_for_the_table_and_for_every_index() {
    let router = emulator();
    for row in WARM_TABLE {
        let case = row.case;
        let mut index = json!({
            "IndexName": "by-gsi_pk",
            "KeySchema": [{"AttributeName": "gsi_pk", "KeyType": "HASH"}],
            "Projection": {"ProjectionType": "ALL"},
        });
        if let Some((read, write)) = row.index_capacity {
            index["ProvisionedThroughput"] =
                json!({"ReadCapacityUnits": read, "WriteCapacityUnits": write});
        }
        let mut body = json!({
            "TableName": row.table,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "gsi_pk", "AttributeType": "S"},
            ],
            "BillingMode": if row.table_capacity.is_some() {
                "PROVISIONED"
            } else {
                "PAY_PER_REQUEST"
            },
            "GlobalSecondaryIndexes": [index],
        });
        if let Some((read, write)) = row.table_capacity {
            body["ProvisionedThroughput"] =
                json!({"ReadCapacityUnits": read, "WriteCapacityUnits": write});
        }
        if let Some((read, write)) = row.named {
            let mut warm = json!({});
            if let Some(read) = read {
                warm["ReadUnitsPerSecond"] = json!(read);
            }
            if let Some(write) = write {
                warm["WriteUnitsPerSecond"] = json!(write);
            }
            body["WarmThroughput"] = warm;
        }

        let created = ok(&router, "CreateTable", body).await;
        let description = &created["TableDescription"];
        assert_eq!(
            description["WarmThroughput"],
            expected_warm(row.table_warm),
            "case {case:?}: expected the table's warm throughput in the create echo, \
             got {description}"
        );
        assert_eq!(
            index_named(description, "by-gsi_pk")["WarmThroughput"],
            expected_warm(row.index_warm),
            "case {case:?}: expected the index's warm throughput in the create echo, \
             got {description}"
        );

        wait_until_active(&router, row.table).await;
        let described = ok(&router, "DescribeTable", json!({"TableName": row.table})).await;
        let table = &described["Table"];
        assert_eq!(
            table["WarmThroughput"],
            expected_warm(row.table_warm),
            "case {case:?}: expected DescribeTable to report the same table pair, \
             got {described}"
        );
        assert_eq!(
            index_named(table, "by-gsi_pk")["WarmThroughput"],
            expected_warm(row.index_warm),
            "case {case:?}: expected DescribeTable to report the same index pair, \
             got {described}"
        );
    }
}

#[tokio::test]
async fn a_capacity_change_moves_the_warm_throughput_nobody_named() {
    // The rule's consequence: what a resource keeps warm is the capacity it is
    // provisioned for, so raising that capacity raises both — at the table and at the
    // index whose capacity the same call moves.
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "warm_moving",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "gsi_pk", "AttributeType": "S"},
            ],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 2},
            "GlobalSecondaryIndexes": [{
                "IndexName": "by-gsi_pk",
                "KeySchema": [{"AttributeName": "gsi_pk", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 2},
            }],
        }),
    )
    .await;
    wait_until_active(&router, "warm_moving").await;

    let updated = ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": "warm_moving",
            "ProvisionedThroughput": {"ReadCapacityUnits": 9, "WriteCapacityUnits": 6},
            "GlobalSecondaryIndexUpdates": [{
                "Update": {
                    "IndexName": "by-gsi_pk",
                    "ProvisionedThroughput": {"ReadCapacityUnits": 8, "WriteCapacityUnits": 4},
                },
            }],
        }),
    )
    .await;
    let description = &updated["TableDescription"];
    assert_eq!(
        description["WarmThroughput"],
        expected_warm((9, 6)),
        "case \"capacity raised\": expected the table to keep its new capacity warm, \
         got {description}"
    );
    assert_eq!(
        index_named(description, "by-gsi_pk")["WarmThroughput"],
        expected_warm((8, 4)),
        "case \"capacity raised\": expected the index to keep its new capacity warm, \
         got {description}"
    );
}
