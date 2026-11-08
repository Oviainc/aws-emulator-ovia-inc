// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DynamoDB table + item slice: `CreateTable`,
//! `DescribeTable`, `ListTables`, `PutItem`, `GetItem`, `Query`, `DeleteItem` and
//! `DeleteTable`, driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.
//!
//! Every assertion here is about state: what a request reports is what an earlier
//! request wrote. The tables span the classes the rules generalise over — the key
//! attribute types, the sort-key comparison operators, the `AttributeValue` types an
//! item can hold — rather than one recorded example.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{NumberKey, STATUS_TRANSITION};
use service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The refusal-code reader is the harness's, so there is one implementation of it.
use crate::emulator::error_code;

/// A SigV4-shaped `Authorization` header: its credential scope is what routes the
/// request to this service (signing name `dynamodb`) and names the region the ARNs
/// the service mints carry. Request authentication is off in these tests, so the
/// signature itself is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/dynamodb/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000";

/// The account id every generated emulator presents, and the region above.
const EXPECTED_ARN_PREFIX: &str = "arn:aws:dynamodb:us-east-1:000000000000:table/";

/// One emulator holding only this service, over one shared engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one operation and return its HTTP status and parsed body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header("authorization", AUTHORIZATION)
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
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Create a table from `(attribute, key type, attribute type)` rows and wait for the
/// `CREATING` -> `ACTIVE` transition the clock owes it.
pub(crate) async fn create_active_table(
    router: &MultiServiceRouter,
    name: &str,
    keys: &[(&str, &str, &str)],
) -> Value {
    let key_schema: Vec<Value> = keys
        .iter()
        .map(|(attribute, key_type, _)| json!({"AttributeName": attribute, "KeyType": key_type}))
        .collect();
    let definitions: Vec<Value> = keys
        .iter()
        .map(|(attribute, _, attribute_type)| {
            json!({"AttributeName": attribute, "AttributeType": attribute_type})
        })
        .collect();
    let created = ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": key_schema,
            "AttributeDefinitions": definitions,
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        created["TableDescription"]["TableStatus"],
        json!("CREATING"),
        "case create {name}: a new table is reported CREATING, got {created}"
    );
    wait_until_active(router, name).await
}

/// Wait out the transient status and report the table, which must now be `ACTIVE`.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) -> Value {
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
    let described = ok(router, "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        described["Table"]["TableStatus"],
        json!("ACTIVE"),
        "case wait {name}: the table becomes ACTIVE once the transition is due, got {described}"
    );
    described
}

/// An item carrying every `AttributeValue` type the union declares.
fn every_attribute_type(sort_key: &str) -> Value {
    json!({
        "Artist": {"S": "Acme"},
        "SongTitle": {"S": sort_key},
        "Year": {"N": "2001"},
        "Cover": {"B": "AQIDBA=="},
        "Explicit": {"BOOL": false},
        "Retired": {"NULL": true},
        "Genres": {"SS": ["rock", "pop"]},
        "Charts": {"NS": ["1", "17"]},
        "Thumbprints": {"BS": ["AQI=", "AwQ="]},
        "Credits": {"L": [{"S": "vocals"}, {"N": "3"}]},
        "Meta": {"M": {"label": {"S": "indie"}, "tracks": {"N": "12"}}}
    })
}

/// The sort key values a `Query` returned, in the order it returned them.
fn returned_sort_keys(response: &Value, attribute: &str, member: &str) -> Vec<String> {
    response["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item[attribute][member]
                .as_str()
                .unwrap_or("<missing>")
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn table_and_item_round_trip_reports_what_was_written() {
    let router = emulator();

    // CREATE: the description comes from the stored table, ARN and all.
    let created = ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Music",
            "KeySchema": [
                {"AttributeName": "Artist", "KeyType": "HASH"},
                {"AttributeName": "SongTitle", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "Artist", "AttributeType": "S"},
                {"AttributeName": "SongTitle", "AttributeType": "S"},
            ],
            "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 3},
        }),
    )
    .await;
    let description = &created["TableDescription"];
    assert_eq!(
        description["TableArn"],
        json!(format!("{EXPECTED_ARN_PREFIX}Music")),
        "case create Music: expected the table's own ARN, got {description}"
    );
    assert_eq!(
        description["ItemCount"],
        json!(0),
        "case create Music: a new table holds no items, got {description}"
    );
    assert_eq!(
        description["TableThroughputModeSummary"]["TableThroughputMode"],
        json!("PROVISIONED"),
        "case create Music: the throughput mode defaults to PROVISIONED, got {description}"
    );
    // A table that has only ever been PROVISIONED carries no BillingModeSummary: "you may
    // need to switch to on-demand mode at least once in order to return a
    // BillingModeSummary response" (that shape's own note).
    assert_eq!(
        description.get("BillingModeSummary"),
        None,
        "case create Music: a never-on-demand table carries no BillingModeSummary, got \
         {description}"
    );
    assert_eq!(
        description["ProvisionedThroughput"]["ReadCapacityUnits"],
        json!(5),
        "case create Music: the stored throughput is reported back, got {description}"
    );
    assert_eq!(
        description["KeySchema"],
        created["TableDescription"]["KeySchema"],
        "case create Music: the key schema is reported back, got {description}"
    );

    // A table that is not ACTIVE yet is not writable: "You can perform read and
    // write operations only on an ACTIVE table".
    let (status, refused) = call(
        &router,
        "PutItem",
        &json!({"TableName": "Music", "Item": every_attribute_type("A")}),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ResourceNotFoundException"),
        "case put while CREATING: expected 400 ResourceNotFoundException, got {status} {refused}"
    );

    // A second CreateTable for a name that is taken is a conflict, in any status.
    let (status, conflict) = call(
        &router,
        "CreateTable",
        &json!({
            "TableName": "Music",
            "KeySchema": [{"AttributeName": "Artist", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Artist", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&conflict).as_str()),
        (400, "ResourceInUseException"),
        "case recreate Music: expected 400 ResourceInUseException, got {status} {conflict}"
    );

    wait_until_active(&router, "Music").await;

    // PUT: three items sharing one partition key.
    for sort_key in ["A", "B", "C"] {
        let written = ok(
            &router,
            "PutItem",
            json!({"TableName": "Music", "Item": every_attribute_type(sort_key)}),
        )
        .await;
        assert_eq!(
            written.get("Attributes"),
            None,
            "case put {sort_key}: a first write replaces nothing, got {written}"
        );
    }

    // GET: exactly the AttributeValue map that was stored, every type included.
    let fetched = ok(
        &router,
        "GetItem",
        json!({
            "TableName": "Music",
            "Key": {"Artist": {"S": "Acme"}, "SongTitle": {"S": "A"}},
            "ReturnConsumedCapacity": "TOTAL",
        }),
    )
    .await;
    assert_eq!(
        fetched["Item"],
        every_attribute_type("A"),
        "case get A: expected the stored item verbatim, got {fetched}"
    );
    assert_eq!(
        fetched["ConsumedCapacity"]["TableName"],
        json!("Music"),
        "case get A: ConsumedCapacity is reported when asked for, got {fetched}"
    );

    // A key with no item is a 200 with no Item, not an error.
    let missing = ok(
        &router,
        "GetItem",
        json!({
            "TableName": "Music",
            "Key": {"Artist": {"S": "Acme"}, "SongTitle": {"S": "Z"}},
        }),
    )
    .await;
    assert_eq!(
        missing.get("Item"),
        None,
        "case get Z: a missing item is a 200 with no Item, got {missing}"
    );

    // DESCRIBE: the metered figures are what a description reports, and a write does
    // not move them — "DynamoDB updates this value approximately every six hours.
    // Recent changes might not be reflected in this value"
    // (`TableDescription$ItemCount`), so three fresh puts are not reflected yet.
    let described = ok(&router, "DescribeTable", json!({"TableName": "Music"})).await;
    assert_eq!(
        described["Table"]["ItemCount"],
        json!(0),
        "case describe after 3 puts: expected the metered ItemCount, not a live count, \
         got {described}"
    );
    assert_eq!(
        described["Table"]["TableSizeBytes"],
        json!(0),
        "case describe after 3 puts: expected the metered TableSizeBytes, not a live \
         total, got {described}"
    );

    // QUERY: one page at a time, resuming from LastEvaluatedKey.
    let page = ok(
        &router,
        "Query",
        json!({
            "TableName": "Music",
            "KeyConditionExpression": "#artist = :artist",
            "ExpressionAttributeNames": {"#artist": "Artist"},
            "ExpressionAttributeValues": {":artist": {"S": "Acme"}},
            "Limit": 2,
        }),
    )
    .await;
    assert_eq!(
        returned_sort_keys(&page, "SongTitle", "S"),
        vec!["A".to_string(), "B".to_string()],
        "case query page 1: expected the first two items in sort order, got {page}"
    );
    assert_eq!(
        (page["Count"].clone(), page["ScannedCount"].clone()),
        (json!(2), json!(2)),
        "case query page 1: Count and ScannedCount come from the result, got {page}"
    );
    assert_eq!(
        page["LastEvaluatedKey"],
        json!({"Artist": {"S": "Acme"}, "SongTitle": {"S": "B"}}),
        "case query page 1: expected the key the page stopped on, got {page}"
    );

    let next = ok(
        &router,
        "Query",
        json!({
            "TableName": "Music",
            "KeyConditionExpression": "Artist = :artist",
            "ExpressionAttributeValues": {":artist": {"S": "Acme"}},
            "Limit": 2,
            "ExclusiveStartKey": page["LastEvaluatedKey"].clone(),
        }),
    )
    .await;
    assert_eq!(
        returned_sort_keys(&next, "SongTitle", "S"),
        vec!["C".to_string()],
        "case query page 2: expected the item after the start key, got {next}"
    );
    assert_eq!(
        next.get("LastEvaluatedKey"),
        None,
        "case query page 2: the last page carries no LastEvaluatedKey, got {next}"
    );

    let descending = ok(
        &router,
        "Query",
        json!({
            "TableName": "Music",
            "KeyConditionExpression": "Artist = :artist",
            "ExpressionAttributeValues": {":artist": {"S": "Acme"}},
            "ScanIndexForward": false,
        }),
    )
    .await;
    assert_eq!(
        returned_sort_keys(&descending, "SongTitle", "S"),
        vec!["C".to_string(), "B".to_string(), "A".to_string()],
        "case query descending: ScanIndexForward=false reverses the sort order, got {descending}"
    );

    let counted = ok(
        &router,
        "Query",
        json!({
            "TableName": "Music",
            "KeyConditionExpression": "Artist = :artist",
            "ExpressionAttributeValues": {":artist": {"S": "Acme"}},
            "Select": "COUNT",
        }),
    )
    .await;
    assert_eq!(
        (counted["Count"].clone(), counted.get("Items").cloned()),
        (json!(3), None),
        "case query COUNT: expected the count without the items, got {counted}"
    );

    // PUT over an existing key REPLACES that item and can report the old one.
    let mut replacement = every_attribute_type("B");
    replacement["Year"] = json!({"N": "2002"});
    let replaced = ok(
        &router,
        "PutItem",
        json!({
            "TableName": "Music",
            "Item": replacement.clone(),
            "ReturnValues": "ALL_OLD",
        }),
    )
    .await;
    assert_eq!(
        replaced["Attributes"],
        every_attribute_type("B"),
        "case replace B: ALL_OLD reports the item that was overwritten, got {replaced}"
    );
    let reread = ok(
        &router,
        "GetItem",
        json!({
            "TableName": "Music",
            "Key": {"Artist": {"S": "Acme"}, "SongTitle": {"S": "B"}},
        }),
    )
    .await;
    assert_eq!(
        reread["Item"], replacement,
        "case replace B: the stored item is the replacement, got {reread}"
    );
    let still_three = ok(&router, "DescribeTable", json!({"TableName": "Music"})).await;
    assert_eq!(
        still_three["Table"]["ItemCount"],
        json!(0),
        "case replace B: a write of any kind leaves the metered ItemCount alone, got \
         {still_three}"
    );

    // DELETE one item: it is gone from GetItem and from Query.
    let deleted = ok(
        &router,
        "DeleteItem",
        json!({
            "TableName": "Music",
            "Key": {"Artist": {"S": "Acme"}, "SongTitle": {"S": "A"}},
            "ReturnValues": "ALL_OLD",
        }),
    )
    .await;
    assert_eq!(
        deleted["Attributes"],
        every_attribute_type("A"),
        "case delete A: ALL_OLD reports the removed attributes, got {deleted}"
    );
    let gone = ok(
        &router,
        "GetItem",
        json!({
            "TableName": "Music",
            "Key": {"Artist": {"S": "Acme"}, "SongTitle": {"S": "A"}},
        }),
    )
    .await;
    assert_eq!(
        gone.get("Item"),
        None,
        "case delete A: the item is gone from GetItem, got {gone}"
    );
    let remaining = ok(
        &router,
        "Query",
        json!({
            "TableName": "Music",
            "KeyConditionExpression": "Artist = :artist",
            "ExpressionAttributeValues": {":artist": {"S": "Acme"}},
        }),
    )
    .await;
    assert_eq!(
        returned_sort_keys(&remaining, "SongTitle", "S"),
        vec!["B".to_string(), "C".to_string()],
        "case delete A: Query no longer lists the removed item, got {remaining}"
    );

    // ListTables sees the table while it exists.
    let listed = ok(&router, "ListTables", json!({})).await;
    assert_eq!(
        listed["TableNames"],
        json!(["Music"]),
        "case list before delete: expected the stored table, got {listed}"
    );

    // DELETE the table: DELETING first, then gone.
    let dropped = ok(&router, "DeleteTable", json!({"TableName": "Music"})).await;
    assert_eq!(
        dropped["TableDescription"]["TableStatus"],
        json!("DELETING"),
        "case delete table: expected TableStatus DELETING, got {dropped}"
    );
    let deleting = ok(&router, "DescribeTable", json!({"TableName": "Music"})).await;
    assert_eq!(
        deleting["Table"]["TableStatus"],
        json!("DELETING"),
        "case describe while deleting: expected DELETING, got {deleting}"
    );
    let (status, while_deleting) = call(
        &router,
        "GetItem",
        &json!({
            "TableName": "Music",
            "Key": {"Artist": {"S": "Acme"}, "SongTitle": {"S": "B"}},
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&while_deleting).as_str()),
        (400, "ResourceNotFoundException"),
        "case get while deleting: expected 400 ResourceNotFoundException, got {status} {while_deleting}"
    );

    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
    for (operation, body) in [
        ("DescribeTable", json!({"TableName": "Music"})),
        (
            "Query",
            json!({
                "TableName": "Music",
                "KeyConditionExpression": "Artist = :artist",
                "ExpressionAttributeValues": {":artist": {"S": "Acme"}},
            }),
        ),
    ] {
        let (status, body) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {operation} after delete: expected 400 ResourceNotFoundException, got {status} {body}"
        );
    }
    let empty = ok(&router, "ListTables", json!({})).await;
    assert_eq!(
        empty["TableNames"],
        json!([]),
        "case list after delete: the deleted table is not listed, got {empty}"
    );
}

#[tokio::test]
async fn sort_key_order_is_the_order_of_the_key_attributes_type() {
    // One row per scalar key type the model's `ScalarAttributeType` enum declares
    // (S | N | B), with values written in a scrambled order and expected back in
    // that type's own order: UTF-8 byte order for S, numeric order for N, unsigned
    // byte order for B (base64 on the wire: AQ==, Ag==, /w== are 0x01, 0x02, 0xff).
    let cases: [(&str, &str, &[&str], &[&str]); 3] = [
        (
            "ByString",
            "S",
            &["b", "A", "a", "B", "aa"],
            &["A", "B", "a", "aa", "b"],
        ),
        (
            "ByNumber",
            "N",
            // '1e2' is written in exponent form and read back canonically ('100'):
            // what is stored is the NUMBER, so the order is numeric either way.
            &["10", "9", "-1", "1e2", "0.5", "0"],
            &["-1", "0", "0.5", "9", "10", "100"],
        ),
        (
            "ByBinary",
            "B",
            &["/w==", "AQ==", "Ag=="],
            &["AQ==", "Ag==", "/w=="],
        ),
    ];

    for (table, key_type, written, expected) in cases {
        let router = emulator();
        create_active_table(
            &router,
            table,
            &[("Part", "HASH", "S"), ("Sort", "RANGE", key_type)],
        )
        .await;
        for value in written {
            ok(
                &router,
                "PutItem",
                json!({
                    "TableName": table,
                    "Item": {"Part": {"S": "p"}, "Sort": {key_type: value}},
                }),
            )
            .await;
        }
        let queried = ok(
            &router,
            "Query",
            json!({
                "TableName": table,
                "KeyConditionExpression": "Part = :p",
                "ExpressionAttributeValues": {":p": {"S": "p"}},
            }),
        )
        .await;
        let got = returned_sort_keys(&queried, "Sort", key_type);
        let want: Vec<String> = expected.iter().map(ToString::to_string).collect();
        assert_eq!(
            got, want,
            "case {table} ({key_type}): expected sort-key order {want:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn sort_key_conditions_select_by_the_documented_comparison() {
    let router = emulator();
    create_active_table(
        &router,
        "Episodes",
        &[("Series", "HASH", "S"), ("Episode", "RANGE", "S")],
    )
    .await;
    for episode in ["s01e01", "s01e02", "s02e01", "s02e02"] {
        ok(
            &router,
            "PutItem",
            json!({
                "TableName": "Episodes",
                "Item": {"Series": {"S": "Acme"}, "Episode": {"S": episode}},
            }),
        )
        .await;
    }

    // One row per comparison `KeyConditionExpression` allows on a sort key
    // (`Query$KeyConditionExpression`), then the same selections through the legacy
    // `KeyConditions` map, whose supported operators are "EQ | LE | LT | GE | GT |
    // BEGINS_WITH | BETWEEN" (the `Condition` doc).
    let expression_cases: [(&str, Value, &[&str]); 7] = [
        (
            "Series = :s AND Episode = :a",
            json!({":s": {"S": "Acme"}, ":a": {"S": "s01e02"}}),
            &["s01e02"],
        ),
        (
            "Series = :s AND Episode < :a",
            json!({":s": {"S": "Acme"}, ":a": {"S": "s01e02"}}),
            &["s01e01"],
        ),
        (
            "Series = :s AND Episode <= :a",
            json!({":s": {"S": "Acme"}, ":a": {"S": "s01e02"}}),
            &["s01e01", "s01e02"],
        ),
        (
            "Series = :s AND Episode > :a",
            json!({":s": {"S": "Acme"}, ":a": {"S": "s02e01"}}),
            &["s02e02"],
        ),
        (
            "Series = :s AND Episode >= :a",
            json!({":s": {"S": "Acme"}, ":a": {"S": "s02e01"}}),
            &["s02e01", "s02e02"],
        ),
        (
            "Series = :s AND Episode BETWEEN :low AND :high",
            json!({":s": {"S": "Acme"}, ":low": {"S": "s01e02"}, ":high": {"S": "s02e01"}}),
            &["s01e02", "s02e01"],
        ),
        (
            "Series = :s AND begins_with ( Episode , :prefix )",
            json!({":s": {"S": "Acme"}, ":prefix": {"S": "s02"}}),
            &["s02e01", "s02e02"],
        ),
    ];
    for (expression, values, expected) in expression_cases {
        let queried = ok(
            &router,
            "Query",
            json!({
                "TableName": "Episodes",
                "KeyConditionExpression": expression,
                "ExpressionAttributeValues": values,
            }),
        )
        .await;
        let got = returned_sort_keys(&queried, "Episode", "S");
        let want: Vec<String> = expected.iter().map(ToString::to_string).collect();
        assert_eq!(
            got, want,
            "case {expression:?}: expected {want:?}, got {got:?} from {queried}"
        );
    }

    let legacy_cases: [(&str, Value, &[&str]); 3] = [
        (
            "EQ",
            json!({"AttributeValueList": [{"S": "s01e01"}], "ComparisonOperator": "EQ"}),
            &["s01e01"],
        ),
        (
            "BEGINS_WITH",
            json!({"AttributeValueList": [{"S": "s01"}], "ComparisonOperator": "BEGINS_WITH"}),
            &["s01e01", "s01e02"],
        ),
        (
            "BETWEEN",
            json!({
                "AttributeValueList": [{"S": "s01e02"}, {"S": "s02e02"}],
                "ComparisonOperator": "BETWEEN",
            }),
            &["s01e02", "s02e01", "s02e02"],
        ),
    ];
    for (operator, condition, expected) in legacy_cases {
        let queried = ok(
            &router,
            "Query",
            json!({
                "TableName": "Episodes",
                "KeyConditions": {
                    "Series": {"AttributeValueList": [{"S": "Acme"}], "ComparisonOperator": "EQ"},
                    "Episode": condition,
                },
            }),
        )
        .await;
        let got = returned_sort_keys(&queried, "Episode", "S");
        let want: Vec<String> = expected.iter().map(ToString::to_string).collect();
        assert_eq!(
            got, want,
            "case legacy KeyConditions {operator}: expected {want:?}, got {got:?} from {queried}"
        );
    }
}

#[tokio::test]
async fn a_table_name_may_be_given_as_its_arn() {
    let router = emulator();
    create_active_table(&router, "ByArn", &[("Part", "HASH", "S")]).await;
    let arn = format!("{EXPECTED_ARN_PREFIX}ByArn");

    ok(
        &router,
        "PutItem",
        json!({"TableName": arn, "Item": {"Part": {"S": "p"}, "Note": {"S": "written by ARN"}}}),
    )
    .await;
    let fetched = ok(
        &router,
        "GetItem",
        json!({"TableName": "ByArn", "Key": {"Part": {"S": "p"}}}),
    )
    .await;
    assert_eq!(
        fetched["Item"]["Note"],
        json!({"S": "written by ARN"}),
        "case put by ARN: the item lands in the table the ARN names, got {fetched}"
    );
}

#[tokio::test]
async fn bad_requests_are_refused_with_the_error_the_model_declares() {
    let router = emulator();
    create_active_table(
        &router,
        "Music",
        &[("Artist", "HASH", "S"), ("SongTitle", "RANGE", "S")],
    )
    .await;

    // Each row: the case, the operation, the request, and the wire error code the
    // service must answer with.
    let cases: [(&str, &str, Value, &str); 11] = [
        (
            "an unknown table",
            "DescribeTable",
            json!({"TableName": "Absent"}),
            "ResourceNotFoundException",
        ),
        (
            "a put into an unknown table",
            "PutItem",
            json!({"TableName": "Absent", "Item": {"Artist": {"S": "a"}}}),
            "ResourceNotFoundException",
        ),
        (
            "a table name that is too short",
            "DescribeTable",
            json!({"TableName": "ab"}),
            "ValidationException",
        ),
        (
            "a key schema with no HASH attribute",
            "CreateTable",
            json!({
                "TableName": "NoHash",
                "KeySchema": [{"AttributeName": "Sort", "KeyType": "RANGE"}],
                "AttributeDefinitions": [{"AttributeName": "Sort", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
            "ValidationException",
        ),
        (
            "a key attribute with no AttributeDefinition",
            "CreateTable",
            json!({
                "TableName": "NoDefinition",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
            "ValidationException",
        ),
        (
            "an AttributeDefinition no key uses",
            "CreateTable",
            json!({
                "TableName": "SpareDefinition",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Spare", "AttributeType": "N"},
                ],
                "BillingMode": "PAY_PER_REQUEST",
            }),
            "ValidationException",
        ),
        (
            "PROVISIONED with no ProvisionedThroughput",
            "CreateTable",
            json!({
                "TableName": "NoThroughput",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PROVISIONED",
            }),
            "ValidationException",
        ),
        (
            "PAY_PER_REQUEST with a ProvisionedThroughput",
            "CreateTable",
            json!({
                "TableName": "BothModes",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            }),
            "ValidationException",
        ),
        (
            "an item missing a key attribute",
            "PutItem",
            json!({"TableName": "Music", "Item": {"Artist": {"S": "Acme"}}}),
            "ValidationException",
        ),
        (
            "a key attribute of the wrong type",
            "GetItem",
            json!({
                "TableName": "Music",
                "Key": {"Artist": {"N": "1"}, "SongTitle": {"S": "A"}},
            }),
            "ValidationException",
        ),
        (
            "a query with no key condition",
            "Query",
            json!({"TableName": "Music"}),
            "ValidationException",
        ),
    ];

    for (case, operation, request, expected) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected),
            "case {case} ({operation}): expected 400 {expected}, got {status} {body}"
        );
    }

    // A query whose key condition constrains a non-key attribute, or compares the
    // partition key with anything but `=`, is refused the same way.
    for expression in [
        "Artist > :a",
        "Year = :a",
        "Artist = :a AND Year = :a",
        "Artist = :a AND",
    ] {
        let (status, body) = call(
            &router,
            "Query",
            &json!({
                "TableName": "Music",
                "KeyConditionExpression": expression,
                "ExpressionAttributeValues": {":a": {"S": "Acme"}},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case key condition {expression:?}: expected 400 ValidationException, got {status} {body}"
        );
    }

    // A placeholder the request never bound is a bad request, not a panic.
    let (status, unbound) = call(
        &router,
        "Query",
        &json!({
            "TableName": "Music",
            "KeyConditionExpression": "#artist = :artist",
            "ExpressionAttributeValues": {":artist": {"S": "Acme"}},
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&unbound).as_str()),
        (400, "ValidationException"),
        "case unbound #artist: expected 400 ValidationException, got {status} {unbound}"
    );
}

#[tokio::test]
async fn an_expression_or_item_beyond_the_documented_bounds_is_refused_not_a_crash() {
    let router = emulator();
    create_active_table(
        &router,
        "Music",
        &[("Artist", "HASH", "S"), ("SongTitle", "RANGE", "S")],
    )
    .await;

    // One row per bound the parser enforces, each a caller-chosen size: a run of
    // parentheses far past the nesting limit (unbounded recursion would abort the
    // process rather than fail the request), an expression past the documented 4 KB
    // length, a token count past the ceiling, and a truncated expression.
    let nested = format!("{}Artist = :a{}", "(".repeat(5000), ")".repeat(5000));
    let unbalanced = "(".repeat(20_000);
    let oversized = format!("Artist = :a AND SongTitle = {}", ":a ".repeat(2000));
    let expressions = [
        ("deeply nested", nested.as_str()),
        ("unbalanced and deeply nested", unbalanced.as_str()),
        ("oversized", oversized.as_str()),
        ("truncated", "Artist = "),
        ("empty", ""),
    ];
    for (case, expression) in expressions {
        let (status, body) = call(
            &router,
            "Query",
            &json!({
                "TableName": "Music",
                "KeyConditionExpression": expression,
                "ExpressionAttributeValues": {":a": {"S": "Acme"}},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case} KeyConditionExpression: expected 400 ValidationException, got {status} {body}"
        );
    }

    // A moderately nested expression inside the limit still parses and selects.
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": "Music",
            "Item": {"Artist": {"S": "Acme"}, "SongTitle": {"S": "A"}},
        }),
    )
    .await;
    let parenthesised = ok(
        &router,
        "Query",
        json!({
            "TableName": "Music",
            "KeyConditionExpression": "((Artist = :a) AND (SongTitle = :s))",
            "ExpressionAttributeValues": {":a": {"S": "Acme"}, ":s": {"S": "A"}},
        }),
    )
    .await;
    assert_eq!(
        returned_sort_keys(&parenthesised, "SongTitle", "S"),
        vec!["A".to_string()],
        "case parenthesised within the nesting limit: expected the one item, got {parenthesised}"
    );

    // An attribute nested past DynamoDB's documented 32 levels is refused rather
    // than walked.
    let mut deep = json!({"S": "bottom"});
    for _ in 0..40 {
        deep = json!({"L": [deep]});
    }
    let (status, refused) = call(
        &router,
        "PutItem",
        &json!({
            "TableName": "Music",
            "Item": {"Artist": {"S": "Acme"}, "SongTitle": {"S": "deep"}, "Nested": deep},
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case 40-level nested attribute: expected 400 ValidationException, got {status} {refused}"
    );

    // Within the limit it is stored and read back unchanged.
    let mut shallow = json!({"S": "bottom"});
    for _ in 0..8 {
        shallow = json!({"L": [shallow]});
    }
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": "Music",
            "Item": {
                "Artist": {"S": "Acme"},
                "SongTitle": {"S": "shallow"},
                "Nested": shallow.clone(),
            },
        }),
    )
    .await;
    let fetched = ok(
        &router,
        "GetItem",
        json!({
            "TableName": "Music",
            "Key": {"Artist": {"S": "Acme"}, "SongTitle": {"S": "shallow"}},
        }),
    )
    .await;
    assert_eq!(
        fetched["Item"]["Nested"], shallow,
        "case 8-level nested attribute: expected the stored value verbatim, got {fetched}"
    );
}

#[tokio::test]
async fn a_table_that_is_still_creating_cannot_be_deleted() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Fleeting",
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let (status, refused) = call(&router, "DeleteTable", &json!({"TableName": "Fleeting"})).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ResourceInUseException"),
        "case delete while CREATING: expected 400 ResourceInUseException, got {status} {refused}"
    );

    wait_until_active(&router, "Fleeting").await;
    let dropped = ok(&router, "DeleteTable", json!({"TableName": "Fleeting"})).await;
    assert_eq!(
        dropped["TableDescription"]["TableStatus"],
        json!("DELETING"),
        "case delete once ACTIVE: expected DELETING, got {dropped}"
    );
}

#[tokio::test]
async fn list_tables_pages_in_name_order() {
    let router = emulator();
    for name in ["Delta", "Alpha", "Charlie", "Bravo"] {
        create_active_table(&router, name, &[("Part", "HASH", "S")]).await;
    }

    let first = ok(&router, "ListTables", json!({"Limit": 2})).await;
    assert_eq!(
        first["TableNames"],
        json!(["Alpha", "Bravo"]),
        "case list page 1: expected the first two names in order, got {first}"
    );
    assert_eq!(
        first["LastEvaluatedTableName"],
        json!("Bravo"),
        "case list page 1: expected the name the page stopped on, got {first}"
    );

    let second = ok(
        &router,
        "ListTables",
        json!({"Limit": 2, "ExclusiveStartTableName": "Bravo"}),
    )
    .await;
    assert_eq!(
        second["TableNames"],
        json!(["Charlie", "Delta"]),
        "case list page 2: expected the names after the start name, got {second}"
    );
    assert_eq!(
        second.get("LastEvaluatedTableName"),
        None,
        "case list page 2: the last page carries no LastEvaluatedTableName, got {second}"
    );
}

#[tokio::test]
async fn a_legal_item_is_stored_however_much_the_table_already_holds() {
    let router = emulator();
    create_active_table(
        &router,
        "Bulk",
        &[("Part", "HASH", "S"), ("Sort", "RANGE", "N")],
    )
    .await;

    // Items just under the documented 400 KB per-item limit, so a few dozen writes carry
    // far more than one process would casually hold. Each one goes to its OWN partition
    // key: the per-partition-key write ceiling is a different budget (a hot key is
    // throttled, not refused), and what this suite is about is the SIZE a table holds.
    let payload = "x".repeat(399 * 1024);
    let item = |index: usize| {
        json!({
            "TableName": "Bulk",
            "Item": {
                "Part": {"S": format!("p{index}")},
                "Sort": {"N": index.to_string()},
                "Blob": {"S": payload},
            },
        })
    };

    // 64 writes of 399 KB store about 25 MB — well past the 16 MiB this emulator used to
    // refuse a table at, and well under the host-protection ceiling that replaced it
    // (`state::MAX_TABLE_SIZE_BYTES`, a gigabyte): DynamoDB imposes no stored-size limit,
    // so every one of these writes is served.
    let mut stored_bytes = 0usize;
    for index in 0..64 {
        let (status, body) = call(&router, "PutItem", &item(index)).await;
        assert_eq!(
            status, 200,
            "case write {index} with {stored_bytes} bytes already stored: a legal item is \
             accepted however much the table holds, got {status} {body}"
        );
        stored_bytes += payload.len();
    }

    // Every one of them is readable back, so what was accepted was really stored.
    let read = ok(
        &router,
        "GetItem",
        json!({
            "TableName": "Bulk",
            "Key": {"Part": {"S": "p63"}, "Sort": {"N": "63"}},
        }),
    )
    .await;
    assert_eq!(
        read["Item"]["Part"]["S"],
        json!("p63"),
        "case read back the last item: expected the item the 64th write stored, got {read}"
    );

    // What a description reports is the metered snapshot, which those writes do not move.
    let described = ok(&router, "DescribeTable", json!({"TableName": "Bulk"})).await;
    assert_eq!(
        (
            described["Table"]["ItemCount"].clone(),
            described["Table"]["TableSizeBytes"].clone(),
        ),
        (json!(0), json!(0)),
        "case 64 writes stored: expected the metered figures, which those writes do not \
         move, got {described}"
    );

    // The one size-related refusal DynamoDB does state is the per-ITEM limit: "the
    // maximum item size in DynamoDB is 400 KB" ("Service, Account, and Table Quotas").
    let oversize = json!({
        "TableName": "Bulk",
        "Item": {
            "Part": {"S": "too-big"},
            "Sort": {"N": "0"},
            "Blob": {"S": "x".repeat(401 * 1024)},
        },
    });
    let (status, body) = call(&router, "PutItem", &oversize).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case an item past the documented 400 KB limit: expected 400 ValidationException, \
         got {status} {body}"
    );
}

/// The seed for the generated numbers below. A constant, so a failure is
/// reproducible; every assertion names it.
const NUMBER_SEED: u64 = 0x0020_2610_2121_5822;

/// A small deterministic generator — the test needs varied numbers, not randomness.
struct Generator(u64);

impl Generator {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    /// A DynamoDB `N` value: an optional sign, up to 9 integral digits, up to 6
    /// fractional digits, and sometimes an exponent.
    fn number(&mut self) -> String {
        let negative = self.next().is_multiple_of(2);
        let integral = self.next() % 1_000_000_000;
        let fractional = self.next() % 1_000_000;
        let exponent = i64::try_from(self.next() % 11).unwrap_or(0) - 5;
        let mut text = String::new();
        if negative {
            text.push('-');
        }
        text.push_str(&format!("{integral}.{fractional:06}"));
        if self.next().is_multiple_of(3) {
            text.push_str(&format!("e{exponent}"));
        }
        text
    }
}

#[test]
fn number_keys_order_numerically_not_textually() {
    // The catalog: spellings DynamoDB's `N` type accepts, with the numeric value
    // each one denotes. Ordering by the parsed key must agree with ordering by that
    // value — textual ordering would put "10" before "9".
    let catalog: [(&str, f64); 12] = [
        ("-1e3", -1000.0),
        ("-10", -10.0),
        ("-9.5", -9.5),
        ("-0", 0.0),
        ("0", 0.0),
        ("0.00", 0.0),
        ("0.5", 0.5),
        ("+1", 1.0),
        ("1.0", 1.0),
        ("9", 9.0),
        ("10", 10.0),
        ("1e2", 100.0),
    ];
    for (left, left_value) in catalog {
        for (right, right_value) in catalog {
            let parsed_left = NumberKey::parse(left)
                .unwrap_or_else(|| panic!("case {left:?}: the catalog value must parse"));
            let parsed_right = NumberKey::parse(right)
                .unwrap_or_else(|| panic!("case {right:?}: the catalog value must parse"));
            let expected = left_value
                .partial_cmp(&right_value)
                .expect("the catalog holds no NaN");
            assert_eq!(
                parsed_left.cmp(&parsed_right),
                expected,
                "case {left:?} vs {right:?}: expected {expected:?}, got {:?}",
                parsed_left.cmp(&parsed_right)
            );
        }
    }

    // Ordering is not a number of special cases: generated values must order the
    // same way their arithmetic value does, and sorting by the key must leave the
    // values non-decreasing.
    let mut generator = Generator(NUMBER_SEED);
    let samples: Vec<(String, f64)> = (0..64)
        .map(|_| {
            let text = generator.number();
            let value = text
                .parse::<f64>()
                .unwrap_or_else(|_| panic!("seed {NUMBER_SEED:#x}: generated {text:?} is a number"));
            (text, value)
        })
        .collect();
    for (left, left_value) in &samples {
        for (right, right_value) in &samples {
            // Two generated spellings can denote values that differ by less than
            // f64 can resolve; the oracle only speaks where it can.
            if (left_value - right_value).abs()
                <= 1e-12 * left_value.abs().max(right_value.abs())
            {
                continue;
            }
            let parsed_left = NumberKey::parse(left).expect("a generated number parses");
            let parsed_right = NumberKey::parse(right).expect("a generated number parses");
            let expected = left_value
                .partial_cmp(right_value)
                .expect("a generated number is not NaN");
            assert_eq!(
                parsed_left.cmp(&parsed_right),
                expected,
                "seed {NUMBER_SEED:#x}, case {left:?} vs {right:?}: expected {expected:?}, got {:?}",
                parsed_left.cmp(&parsed_right)
            );
        }
    }

    let mut sorted = samples.clone();
    sorted.sort_by(|(left, _), (right, _)| {
        NumberKey::parse(left)
            .expect("a generated number parses")
            .cmp(&NumberKey::parse(right).expect("a generated number parses"))
    });
    for window in sorted.windows(2) {
        let (left, left_value) = &window[0];
        let (right, right_value) = &window[1];
        assert!(
            left_value <= right_value,
            "seed {NUMBER_SEED:#x}: sorting by key put {left:?} ({left_value}) before {right:?} ({right_value})"
        );
    }

    // A value that is not a number at all is not a key, and neither is one outside
    // the documented 38-significant-digit / 10^-130..10^125 range — including the
    // exponents that would otherwise overflow the decimal-point arithmetic.
    for text in [
        "",
        "-",
        "abc",
        "1.2.3",
        "1e",
        "0x10",
        " 1",
        "0.001e-2147483647",
        "1e-2147483648",
        "1e2147483647",
        "1e200",
        "1e-200",
        "123456789012345678901234567890123456789",
    ] {
        assert!(
            NumberKey::parse(text).is_none(),
            "case {text:?}: expected no key for a value outside the documented N range"
        );
    }

    // The documented bounds themselves still parse.
    for text in ["1e125", "1e-130", "9".repeat(38).as_str()] {
        assert!(
            NumberKey::parse(text).is_some(),
            "case {text:?}: a value inside the documented N range must parse"
        );
    }
}
