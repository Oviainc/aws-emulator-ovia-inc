// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the three table rules this suite pins:
//!
//! 1. a table name identifies a table WITHIN ONE REGION ("In an Amazon Web Services
//!    account, table names must be unique within each Region. That is, you can have two
//!    tables with same name if you create the tables in different Regions" — the
//!    `CreateTable` doc), so the same name in another region is a second, independent
//!    table;
//! 2. `CreateTable` stores the `GlobalSecondaryIndexes` it is given and every table
//!    operation reports them, each with its own ARN, key schema, projection, status and
//!    provisioned throughput, and with the item figures computed from the table's items;
//! 3. `UpdateTable` moves the provisioned throughput of the table and of one index
//!    independently, and a later read reports what it moved.
//!
//! Each test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and spans the classes the rules
//! generalise over — the region, the projection type, the key-schema shape, the
//! direction of a capacity change — rather than one recorded example.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    MAX_ATTRIBUTE_DEFINITIONS, MAX_GLOBAL_SECONDARY_INDEXES, MAX_REGIONS, STATUS_TRANSITION,
};
use service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The refusal-code reader is the harness's, so there is one implementation of it.
use crate::emulator::error_code;

/// The regions these tests span. `CreateTable`'s uniqueness scope is the region, so the
/// rows differ in nothing but the region they are signed for.
const REGIONS: [&str; 4] = ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"];

/// One emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4-shaped `Authorization` header for `region`: its credential scope is what
/// routes the request to this service (signing name `dynamodb`) and names the region
/// the service answers for. Request authentication is off in these tests, so the
/// signature itself is never checked.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/dynamodb/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0000"
    )
}

/// Send one operation signed for `region` and return its HTTP status and parsed body.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header("authorization", authorization(region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Send one operation that must succeed.
async fn ok(router: &MultiServiceRouter, region: &str, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, region, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {region} {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Wait out a transient status and report the table, which must now be `ACTIVE`.
async fn wait_until_active(router: &MultiServiceRouter, region: &str, name: &str) -> Value {
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
    let described = ok(router, region, "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        described["Table"]["TableStatus"],
        json!("ACTIVE"),
        "case wait {region}/{name}: the table becomes ACTIVE once the transition is due, \
         got {described}"
    );
    described
}

/// The `GlobalSecondaryIndexes` of a description, keyed by index name.
fn indexes(description: &Value) -> Vec<Value> {
    description["GlobalSecondaryIndexes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One index of a description, by name.
fn index_named(description: &Value, name: &str) -> Value {
    indexes(description)
        .into_iter()
        .find(|index| index["IndexName"] == json!(name))
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn a_table_name_is_unique_only_within_one_region() {
    let router = emulator();
    // One name, created once per region. Each row differs ONLY in the region, plus a
    // per-region capacity so a read proves which region's table it reports.
    let name = "SharedName";

    for (ordinal, region) in REGIONS.iter().enumerate() {
        let read_capacity = i64::try_from(ordinal).expect("a small ordinal") + 1;
        let created = ok(
            &router,
            region,
            "CreateTable",
            json!({
                "TableName": name,
                "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {
                    "ReadCapacityUnits": read_capacity,
                    "WriteCapacityUnits": 7,
                },
            }),
        )
        .await;
        assert_eq!(
            created["TableDescription"]["TableArn"],
            json!(format!(
                "arn:aws:dynamodb:{region}:000000000000:table/{name}"
            )),
            "case create in {region}: the ARN carries the region the request was signed \
             for, got {created}"
        );
    }

    // Each region reports ITS table: the capacity it was created with, and a name list
    // holding exactly that region's tables.
    for (ordinal, region) in REGIONS.iter().enumerate() {
        let expected = i64::try_from(ordinal).expect("a small ordinal") + 1;
        let described = wait_until_active(&router, region, name).await;
        assert_eq!(
            described["Table"]["ProvisionedThroughput"]["ReadCapacityUnits"],
            json!(expected),
            "case read in {region}: a region reports its own table, got {described}"
        );
        let listed = ok(&router, region, "ListTables", json!({})).await;
        assert_eq!(
            listed["TableNames"],
            json!([name]),
            "case list in {region}: ListTables reports only this region's tables, got {listed}"
        );
    }

    // A region-local table of its own is invisible to every other region.
    ok(
        &router,
        REGIONS[1],
        "CreateTable",
        json!({
            "TableName": "OnlyInOneRegion",
            "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    for region in REGIONS {
        let listed = ok(&router, region, "ListTables", json!({})).await;
        let expected = if region == REGIONS[1] {
            json!(["OnlyInOneRegion", name])
        } else {
            json!([name])
        };
        assert_eq!(
            listed["TableNames"], expected,
            "case list after a region-local create in {region}: got {listed}"
        );
    }

    // Deleting in one region leaves every other region's table alone.
    ok(
        &router,
        REGIONS[0],
        "DeleteTable",
        json!({"TableName": name}),
    )
    .await;
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
    let (status, body) = call(&router, REGIONS[0], "DescribeTable", &json!({"TableName": name})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case describe the deleted region's table: got {status} with {body}"
    );
    for region in &REGIONS[1..] {
        let described = ok(
            &router,
            region,
            "DescribeTable",
            json!({"TableName": name}),
        )
        .await;
        assert_eq!(
            described["Table"]["TableStatus"],
            json!("ACTIVE"),
            "case describe {region} after deleting in {}: the other regions' tables are \
             untouched, got {described}",
            REGIONS[0]
        );
    }
}

#[tokio::test]
async fn the_store_holds_tables_for_a_bounded_number_of_regions() {
    let router = emulator();
    // The region a table is stored under comes from the request's own credential
    // scope, so the per-region table quota bounds the store only together with a cap
    // on the number of regions: `MAX_REGIONS` buckets, and so at most
    // `MAX_REGIONS * MAX_TABLES` tables.
    let table = json!({
        "TableName": "Bounded",
        "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    });
    for ordinal in 0..MAX_REGIONS {
        let region = format!("emulated-region-{ordinal}");
        ok(&router, &region, "CreateTable", table.clone()).await;
    }
    // One region past the cap is refused with the modelled quota error...
    let beyond = format!("emulated-region-{MAX_REGIONS}");
    let (status, body) = call(&router, &beyond, "CreateTable", &table).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "LimitExceededException"),
        "case a region past the cap of {MAX_REGIONS}: got {status} with {body}"
    );
    // ...while a region the store already holds tables for keeps working, so the cap
    // bounds the buckets rather than the traffic.
    ok(
        &router,
        "emulated-region-0",
        "CreateTable",
        json!({
            "TableName": "AlsoBounded",
            "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
}

#[tokio::test]
async fn create_table_stores_the_global_secondary_indexes_it_is_given() {
    let router = emulator();
    let region = REGIONS[2];

    // Each row is one index of the SAME table, differing along what the rule
    // generalises over: the projection type (the `ProjectionType` enum's three values,
    // from smithy_query shape=Projection), the key-schema shape (hash only vs hash and
    // range) and the capacity it asks for.
    let created = ok(
        &router,
        region,
        "CreateTable",
        json!({
            "TableName": "Indexed",
            "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "Id", "AttributeType": "S"},
                {"AttributeName": "Owner", "AttributeType": "S"},
                {"AttributeName": "Rank", "AttributeType": "N"},
                {"AttributeName": "Kind", "AttributeType": "S"},
            ],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
            "GlobalSecondaryIndexes": [
                {
                    "IndexName": "ByOwnerAll",
                    "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": 11,
                        "WriteCapacityUnits": 12,
                    },
                },
                {
                    "IndexName": "ByOwnerRankKeysOnly",
                    "KeySchema": [
                        {"AttributeName": "Owner", "KeyType": "HASH"},
                        {"AttributeName": "Rank", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": 3,
                        "WriteCapacityUnits": 4,
                    },
                },
                {
                    "IndexName": "ByKindInclude",
                    "KeySchema": [{"AttributeName": "Kind", "KeyType": "HASH"}],
                    "Projection": {
                        "ProjectionType": "INCLUDE",
                        "NonKeyAttributes": ["Owner"],
                    },
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": 1,
                        "WriteCapacityUnits": 1,
                    },
                },
            ],
        }),
    )
    .await;

    let on_create = &created["TableDescription"];
    assert_eq!(
        indexes(on_create).len(),
        3,
        "case create: every index given is reported back, got {created}"
    );
    for index in indexes(on_create) {
        assert_eq!(
            index["IndexStatus"],
            json!("CREATING"),
            "case create {index}: an index created with the table is reported CREATING"
        );
    }

    let described = wait_until_active(&router, region, "Indexed").await;
    let table = &described["Table"];

    // Each row's expectations: name, the key attributes in order, the projection, and
    // the capacity it was created with.
    let cases: [(&str, Vec<&str>, Value, i64, i64); 3] = [
        (
            "ByOwnerAll",
            vec!["Owner"],
            json!({"ProjectionType": "ALL"}),
            11,
            12,
        ),
        (
            "ByOwnerRankKeysOnly",
            vec!["Owner", "Rank"],
            json!({"ProjectionType": "KEYS_ONLY"}),
            3,
            4,
        ),
        (
            "ByKindInclude",
            vec!["Kind"],
            json!({"ProjectionType": "INCLUDE", "NonKeyAttributes": ["Owner"]}),
            1,
            1,
        ),
    ];
    for (name, key_attributes, projection, read, write) in cases {
        let index = index_named(table, name);
        assert_eq!(
            index["IndexArn"],
            json!(format!(
                "arn:aws:dynamodb:{region}:000000000000:table/Indexed/index/{name}"
            )),
            "case index {name}: its ARN names the table and the index, got {index}"
        );
        assert_eq!(
            index["IndexStatus"],
            json!("ACTIVE"),
            "case index {name}: it becomes ACTIVE with the table, got {index}"
        );
        let actual_keys: Vec<String> = index["KeySchema"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|element| element["AttributeName"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(
            actual_keys, key_attributes,
            "case index {name}: its key schema is the one it was created with, got {index}"
        );
        assert_eq!(
            index["Projection"], projection,
            "case index {name}: its projection is the one it was created with, got {index}"
        );
        assert_eq!(
            (
                index["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
                index["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
            ),
            (json!(read), json!(write)),
            "case index {name}: its capacity is the one it was created with, got {index}"
        );
        assert_eq!(
            index["ItemCount"],
            json!(0),
            "case index {name}: an empty table projects no items into any index, got {index}"
        );
    }

    // The item figures are COMPUTED from the table's items: an item reaches an index
    // only when it carries that index's key attributes.
    ok(
        &router,
        region,
        "PutItem",
        json!({
            "TableName": "Indexed",
            "Item": {
                "Id": {"S": "one"},
                "Owner": {"S": "ada"},
                "Rank": {"N": "3"},
                "Payload": {"S": "a longer value than the keys"},
            },
        }),
    )
    .await;
    ok(
        &router,
        region,
        "PutItem",
        json!({
            "TableName": "Indexed",
            "Item": {"Id": {"S": "two"}, "Kind": {"S": "book"}},
        }),
    )
    .await;

    let described = ok(
        &router,
        region,
        "DescribeTable",
        json!({"TableName": "Indexed"}),
    )
    .await;
    let table = &described["Table"];
    let expected_counts = [
        ("ByOwnerAll", 1),
        ("ByOwnerRankKeysOnly", 1),
        ("ByKindInclude", 1),
    ];
    for (name, expected) in expected_counts {
        let index = index_named(table, name);
        assert_eq!(
            index["ItemCount"],
            json!(expected),
            "case index {name} after two writes: only the items carrying its key \
             attributes are projected, got {index}"
        );
    }
    let all = index_named(table, "ByOwnerAll");
    let keys_only = index_named(table, "ByOwnerRankKeysOnly");
    assert!(
        all["IndexSizeBytes"].as_i64().unwrap_or(0)
            > keys_only["IndexSizeBytes"].as_i64().unwrap_or(0),
        "case index sizes: an ALL projection stores more than a KEYS_ONLY one for the \
         same item, got {all} and {keys_only}"
    );

    // An index is refused for the same reasons a table's own key schema is.
    let oversized_index_list: Vec<Value> = (0..=MAX_GLOBAL_SECONDARY_INDEXES)
        .map(|ordinal| {
            json!({
                "IndexName": format!("Index{ordinal}"),
                "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            })
        })
        .collect();
    let refusals: [(&str, Value); 4] = [
        (
            "more indexes than a table may carry",
            json!(oversized_index_list),
        ),
        (
            "an index key attribute with no AttributeDefinition",
            json!([{
                "IndexName": "Undeclared",
                "KeySchema": [{"AttributeName": "Nowhere", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            }]),
        ),
        (
            "NonKeyAttributes outside an INCLUDE projection",
            json!([{
                "IndexName": "BadProjection",
                "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL", "NonKeyAttributes": ["Id"]},
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            }]),
        ),
        (
            "two indexes with one name",
            json!([
                {
                    "IndexName": "Twice",
                    "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                    "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
                },
                {
                    "IndexName": "Twice",
                    "KeySchema": [{"AttributeName": "Kind", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                    "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
                },
            ]),
        ),
    ];
    for (case, index_list) in refusals {
        let (status, body) = call(
            &router,
            region,
            "CreateTable",
            &json!({
                "TableName": "Refused",
                "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
                "AttributeDefinitions": [
                    {"AttributeName": "Id", "AttributeType": "S"},
                    {"AttributeName": "Owner", "AttributeType": "S"},
                    {"AttributeName": "Kind", "AttributeType": "S"},
                ],
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
                "GlobalSecondaryIndexes": index_list,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: a bad index is refused, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn update_table_moves_the_capacity_it_is_given() {
    let router = emulator();
    let region = REGIONS[3];

    ok(
        &router,
        region,
        "CreateTable",
        json!({
            "TableName": "Capacity",
            "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "Id", "AttributeType": "S"},
                {"AttributeName": "Owner", "AttributeType": "S"},
            ],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 10, "WriteCapacityUnits": 10},
            "GlobalSecondaryIndexes": [{
                "IndexName": "ByOwner",
                "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 10, "WriteCapacityUnits": 10},
            }],
        }),
    )
    .await;
    wait_until_active(&router, region, "Capacity").await;

    // Each row moves the table's capacity along a different direction: both up, both
    // down, one of each, and back to where it started.
    let cases: [(&str, i64, i64, i64); 4] = [
        ("both dimensions up", 25, 30, 0),
        ("both dimensions down", 4, 6, 1),
        ("read up and write down", 40, 2, 2),
        ("back to the starting capacity", 10, 10, 3),
    ];
    for (case, read, write, decreases_today) in cases {
        let updated = ok(
            &router,
            region,
            "UpdateTable",
            json!({
                "TableName": "Capacity",
                "ProvisionedThroughput": {
                    "ReadCapacityUnits": read,
                    "WriteCapacityUnits": write,
                },
            }),
        )
        .await;
        let description = &updated["TableDescription"];
        assert_eq!(
            description["TableStatus"],
            json!("UPDATING"),
            "case {case}: UpdateTable is asynchronous, so the table is reported UPDATING, \
             got {updated}"
        );
        assert_eq!(
            (
                description["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
                description["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
            ),
            (json!(read), json!(write)),
            "case {case}: the new capacity is in effect from the call, got {updated}"
        );

        // A second UpdateTable inside the span is refused: "While it's UPDATING, you
        // can't issue another UpdateTable request."
        let (status, body) = call(
            &router,
            region,
            "UpdateTable",
            &json!({
                "TableName": "Capacity",
                "ProvisionedThroughput": {"ReadCapacityUnits": 9, "WriteCapacityUnits": 9},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceInUseException"),
            "case {case}: a second UpdateTable while UPDATING is refused, got {status} \
             with {body}"
        );

        let described = wait_until_active(&router, region, "Capacity").await;
        let throughput = &described["Table"]["ProvisionedThroughput"];
        assert_eq!(
            (
                throughput["ReadCapacityUnits"].clone(),
                throughput["WriteCapacityUnits"].clone(),
            ),
            (json!(read), json!(write)),
            "case {case}: a later read reports the moved capacity, got {described}"
        );
        let reported_decreases = throughput["NumberOfDecreasesToday"].as_i64().unwrap_or(0);
        assert_eq!(
            reported_decreases, decreases_today,
            "case {case}: NumberOfDecreasesToday counts the decreases applied today, got \
             {described}"
        );
    }

    // An index's capacity moves on its own: "New provisioned throughput parameters for
    // an existing global secondary index" leaves the table's own capacity alone.
    let updated = ok(
        &router,
        region,
        "UpdateTable",
        json!({
            "TableName": "Capacity",
            "GlobalSecondaryIndexUpdates": [{
                "Update": {
                    "IndexName": "ByOwner",
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": 77,
                        "WriteCapacityUnits": 88,
                    },
                },
            }],
        }),
    )
    .await;
    let description = &updated["TableDescription"];
    assert_eq!(
        (
            description["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            description["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
        ),
        (json!(10), json!(10)),
        "case index-only update: the table's own capacity is untouched, got {updated}"
    );
    let index = index_named(description, "ByOwner");
    assert_eq!(
        (
            index["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            index["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
            index["IndexStatus"].clone(),
        ),
        (json!(77), json!(88), json!("UPDATING")),
        "case index-only update: the index moves and is reported UPDATING, got {updated}"
    );
    let described = wait_until_active(&router, region, "Capacity").await;
    let index = index_named(&described["Table"], "ByOwner");
    assert_eq!(
        (
            index["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            index["IndexStatus"].clone(),
        ),
        (json!(77), json!("ACTIVE")),
        "case index-only update: a later read reports the moved index capacity, got \
         {described}"
    );

    // An index can be added to, and removed from, a table that already exists.
    ok(
        &router,
        region,
        "UpdateTable",
        json!({
            "TableName": "Capacity",
            "AttributeDefinitions": [{"AttributeName": "Kind", "AttributeType": "S"}],
            "GlobalSecondaryIndexUpdates": [{
                "Create": {
                    "IndexName": "ByKind",
                    "KeySchema": [{"AttributeName": "Kind", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": 2,
                        "WriteCapacityUnits": 2,
                    },
                },
            }],
        }),
    )
    .await;
    let described = wait_until_active(&router, region, "Capacity").await;
    let names: Vec<Value> = indexes(&described["Table"])
        .into_iter()
        .map(|index| index["IndexName"].clone())
        .collect();
    assert_eq!(
        names,
        vec![json!("ByOwner"), json!("ByKind")],
        "case index create: the new index joins the stored ones, got {described}"
    );

    ok(
        &router,
        region,
        "UpdateTable",
        json!({
            "TableName": "Capacity",
            "GlobalSecondaryIndexUpdates": [{"Delete": {"IndexName": "ByOwner"}}],
        }),
    )
    .await;
    let described = wait_until_active(&router, region, "Capacity").await;
    let names: Vec<Value> = indexes(&described["Table"])
        .into_iter()
        .map(|index| index["IndexName"].clone())
        .collect();
    assert_eq!(
        names,
        vec![json!("ByKind")],
        "case index delete: the removed index is gone, got {described}"
    );

    // The refusals: a table in another region does not exist, an absent index cannot be
    // updated, a capacity below the documented floor is not a capacity, and a batch
    // longer than the most indexes a table can carry is refused before it is walked.
    let oversized_batch: Vec<Value> = (0..=MAX_GLOBAL_SECONDARY_INDEXES)
        .map(|ordinal| json!({"Delete": {"IndexName": format!("Index{ordinal}")}}))
        .collect();
    let oversized_definitions: Vec<Value> = (0..=MAX_ATTRIBUTE_DEFINITIONS)
        .map(|ordinal| json!({"AttributeName": format!("Attr{ordinal}"), "AttributeType": "S"}))
        .collect();
    let refusals: [(&str, &str, Value, &str); 7] = [
        (
            "more attribute definitions than any table could keep",
            region,
            json!({
                "TableName": "Capacity",
                "AttributeDefinitions": oversized_definitions,
            }),
            "ValidationException",
        ),
        (
            "more index updates than a table can carry indexes",
            region,
            json!({
                "TableName": "Capacity",
                "GlobalSecondaryIndexUpdates": oversized_batch,
            }),
            "ValidationException",
        ),
        (
            "a table of this name in another region",
            REGIONS[0],
            json!({
                "TableName": "Capacity",
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
            }),
            "ResourceNotFoundException",
        ),
        (
            "an index the table does not carry",
            region,
            json!({
                "TableName": "Capacity",
                "GlobalSecondaryIndexUpdates": [{
                    "Update": {
                        "IndexName": "Absent",
                        "ProvisionedThroughput": {
                            "ReadCapacityUnits": 5,
                            "WriteCapacityUnits": 5,
                        },
                    },
                }],
            }),
            "ResourceNotFoundException",
        ),
        (
            "a capacity below the floor of 1",
            region,
            json!({
                "TableName": "Capacity",
                "ProvisionedThroughput": {"ReadCapacityUnits": 0, "WriteCapacityUnits": 5},
            }),
            "ValidationException",
        ),
        (
            "an attribute definition no key schema uses",
            region,
            json!({
                "TableName": "Capacity",
                "AttributeDefinitions": [{"AttributeName": "Unused", "AttributeType": "S"}],
            }),
            "ValidationException",
        ),
        (
            "two creates in one request",
            region,
            json!({
                "TableName": "Capacity",
                "AttributeDefinitions": [
                    {"AttributeName": "Owner", "AttributeType": "S"},
                    {"AttributeName": "Kind", "AttributeType": "S"},
                ],
                "GlobalSecondaryIndexUpdates": [
                    {
                        "Create": {
                            "IndexName": "First",
                            "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"}],
                            "Projection": {"ProjectionType": "ALL"},
                            "ProvisionedThroughput": {
                                "ReadCapacityUnits": 1,
                                "WriteCapacityUnits": 1,
                            },
                        },
                    },
                    {
                        "Create": {
                            "IndexName": "Second",
                            "KeySchema": [{"AttributeName": "Kind", "KeyType": "HASH"}],
                            "Projection": {"ProjectionType": "ALL"},
                            "ProvisionedThroughput": {
                                "ReadCapacityUnits": 1,
                                "WriteCapacityUnits": 1,
                            },
                        },
                    },
                ],
            }),
            "ValidationException",
        ),
    ];
    for (case, request_region, body, expected_code) in refusals {
        let (status, parsed) = call(&router, request_region, "UpdateTable", &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, expected_code),
            "case {case}: got {status} with {parsed}"
        );
    }

    // Switching to on-demand clears the provisioned capacity of the table AND of its
    // indexes, because an index's capacity mode is the table's.
    let updated = ok(
        &router,
        region,
        "UpdateTable",
        json!({"TableName": "Capacity", "BillingMode": "PAY_PER_REQUEST"}),
    )
    .await;
    let description = &updated["TableDescription"];
    assert_eq!(
        description["BillingModeSummary"]["BillingMode"],
        json!("PAY_PER_REQUEST"),
        "case billing-mode switch: the new mode is reported, got {updated}"
    );
    assert_eq!(
        description["ProvisionedThroughput"]["ReadCapacityUnits"],
        json!(0),
        "case billing-mode switch: an on-demand table reports zero provisioned read \
         capacity, got {updated}"
    );
    assert_eq!(
        index_named(description, "ByKind")["ProvisionedThroughput"]["WriteCapacityUnits"],
        json!(0),
        "case billing-mode switch: its indexes report zero too, got {updated}"
    );

    // Switching BACK needs initial capacity for the table AND for every index — "When
    // switching from pay-per-request to provisioned capacity, initial provisioned
    // capacity values must be set" — so a switch that names only the table's is refused
    // naming the index that has none.
    wait_until_active(&router, region, "Capacity").await;
    let (status, parsed) = call(
        &router,
        region,
        "UpdateTable",
        &json!({
            "TableName": "Capacity",
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 3},
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "case switch back without index capacity: got {status} with {parsed}"
    );
    let updated = ok(
        &router,
        region,
        "UpdateTable",
        json!({
            "TableName": "Capacity",
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 3},
            "GlobalSecondaryIndexUpdates": [{
                "Update": {
                    "IndexName": "ByKind",
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": 6,
                        "WriteCapacityUnits": 6,
                    },
                },
            }],
        }),
    )
    .await;
    let description = &updated["TableDescription"];
    assert_eq!(
        (
            description["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            index_named(description, "ByKind")["ProvisionedThroughput"]["ReadCapacityUnits"]
                .clone(),
        ),
        (json!(3), json!(6)),
        "case switch back with index capacity: the table and the index report the \
         capacity the switch set, got {updated}"
    );
}

#[tokio::test]
async fn a_numeric_key_value_is_held_to_the_documented_number_form() {
    let router = emulator();
    let region = REGIONS[0];
    ok(
        &router,
        region,
        "CreateTable",
        json!({
            "TableName": "Numbers",
            "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "N"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    wait_until_active(&router, region, "Numbers").await;

    // The rule generalises over the TEXT of an `N` value: a value is a number when it
    // denotes one within the documented range ("Number: ... can have up to 38 digits of
    // precision. Exponent range is 10^-130 to 10^125" — Service, Account and Table
    // Quotas), whatever spelling it arrives in. A text that denotes none is refused, as
    // is one past this EMULATOR's own numeric-text limit (the last row: AWS normalises
    // such padding away, this emulator refuses it so what it retains stays accounted).
    let thirty_eight = "1".repeat(38);
    let thirty_nine = "1".repeat(39);
    let padded_zero = format!("0.{}", "0".repeat(400));
    let cases: [(&str, &str, bool); 7] = [
        ("a plain integer", "42", true),
        ("zero", "0", true),
        ("zero with a fraction", "0.000", true),
        ("exponent notation", "1e5", true),
        ("the 38-digit precision bound", thirty_eight.as_str(), true),
        ("one digit past the precision bound", thirty_nine.as_str(), false),
        (
            "a zero padded past this emulator's numeric-text limit",
            padded_zero.as_str(),
            false,
        ),
    ];
    for (case, text, accepted) in cases {
        let (status, parsed) = call(
            &router,
            region,
            "PutItem",
            &json!({"TableName": "Numbers", "Item": {"Id": {"N": text}}}),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: a documented number is accepted, got {status} with {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "ValidationException"),
                "case {case}: a text outside the documented form, or past this emulator's \
                 numeric-text limit, is refused, got {status} with {parsed}"
            );
        }
    }
}
