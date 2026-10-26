// SPDX-License-Identifier: CC0-1.0

//! What a published stream record CARRIES, over the whole attribute vocabulary.
//!
//! The rule pinned here: every item-level change a stream-enabled table accepts
//! publishes one record on that table's stream, whose `Keys` are the item's primary
//! key attributes alone and whose `NewImage`/`OldImage` are the item exactly as the
//! table STORES it either side of the change — for every member of the modelled
//! `AttributeValue` union (`S`, `N`, `B`, `SS`, `NS`, `BS`, `M`, `L`, `NULL`, `BOOL`)
//! at any nesting the table accepts, and never the lexeme a writer sent where the
//! table stores another form. A change the table REFUSES is no change, so it
//! publishes nothing.
//!
//! The suites below are tables spanning what the rule generalises over — the member of
//! the union, nesting depth at the documented bound and one level past it, and how many
//! tables one request's writes land on — rather than one recorded item. Each stored
//! image is compared against what `GetItem` reports for the same key, so what the
//! record must carry is read off the table's own surface instead of being restated
//! here, and everything is driven over the wire against DynamoDB and DynamoDB Streams
//! on one engine.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::MAX_ATTRIBUTE_DEPTH;

use crate::emulator::{call, drain_stream, emulator_with_streams, error_code, event_names};

/// How long a poll for the documented `CREATING` -> `ACTIVE` transition may take
/// before this suite calls it a hang. Generous slack over the emulator's own span,
/// because a loaded machine is not a defect.
const ACTIVE_DEADLINE: Duration = Duration::from_secs(10);

/// How long one poll sleeps between `DescribeTable` calls.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One row of the vocabulary table: an attribute value the item carries, and whether
/// the table stores a different form of it than the one written.
struct Vocabulary {
    case: String,
    /// The `Payload` attribute's value, as a client writes it.
    written: Value,
    /// Whether the table canonicalises what was written into another spelling, so the
    /// image must differ from `written`.
    restated: bool,
}

/// The attribute values one item's `Payload` is written as, one row per member of the
/// modelled `AttributeValue` union (`S`, `N`, `B`, `SS`, `NS`, `BS`, `M`, `L`, `NULL`,
/// `BOOL` — the ten the union declares) plus the nesting rows the depth bound calls
/// for.
fn vocabulary() -> Vec<Vocabulary> {
    vec![
        Vocabulary {
            case: "S, outside ASCII".to_string(),
            written: json!({"S": "Ωmega"}),
            restated: false,
        },
        Vocabulary {
            case: "N in exponent form".to_string(),
            // The table stores the number a lexeme DENOTES, so the image carries a
            // different spelling than the one written.
            written: json!({"N": "2e1"}),
            restated: true,
        },
        Vocabulary {
            case: "B".to_string(),
            written: json!({"B": "AQIDBA=="}),
            restated: false,
        },
        Vocabulary {
            case: "SS".to_string(),
            written: json!({"SS": ["beta", "alpha"]}),
            restated: false,
        },
        Vocabulary {
            case: "NS".to_string(),
            written: json!({"NS": ["1", "2e1"]}),
            restated: true,
        },
        Vocabulary {
            case: "BS".to_string(),
            written: json!({"BS": ["AQI=", "AwQ="]}),
            restated: false,
        },
        Vocabulary {
            case: "NULL".to_string(),
            written: json!({"NULL": true}),
            restated: false,
        },
        Vocabulary {
            case: "BOOL".to_string(),
            written: json!({"BOOL": false}),
            restated: false,
        },
        Vocabulary {
            case: "M over every scalar member".to_string(),
            written: json!({"M": {
                "s": {"S": "x"},
                "n": {"N": "1"},
                "b": {"B": "AQI="},
                "null": {"NULL": true},
                "bool": {"BOOL": true},
                "ss": {"SS": ["a"]},
            }}),
            restated: false,
        },
        Vocabulary {
            case: "L of mixed members, one of them a map".to_string(),
            written: json!({"L": [
                {"S": "x"},
                {"NULL": true},
                {"M": {"inner": {"N": "7"}}},
                {"L": [{"BOOL": false}]},
            ]}),
            restated: false,
        },
        Vocabulary {
            case: "nesting one level deep".to_string(),
            written: nested_to(1),
            restated: false,
        },
        Vocabulary {
            case: "nesting at the documented depth bound".to_string(),
            written: nested_to(MAX_ATTRIBUTE_DEPTH),
            restated: false,
        },
    ]
}

/// An attribute value nesting exactly `depth` levels: the value itself is level one,
/// and each `M`/`L` wrapper around the scalar at the bottom is one more. `M` and `L`
/// alternate, so both nesting members are walked on the way down.
fn nested_to(depth: usize) -> Value {
    let mut value = json!({"S": "bottom"});
    for level in 1..depth {
        value = if level % 2 == 0 {
            json!({"L": [value]})
        } else {
            json!({"M": {"nested": value}})
        };
    }
    value
}

/// Send one DynamoDB operation that must answer 200, returning its body.
async fn ddb_ok(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// A `CreateTable` body for a table keyed on `Pk` alone, with the stream `stream`
/// asks for.
fn create_body(name: &str, stream: Option<Value>) -> Value {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Pk", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    });
    if let Some(specification) = stream {
        body["StreamSpecification"] = specification;
    }
    body
}

/// Create `name` with a `NEW_AND_OLD_IMAGES` stream, wait for it to serve writes, and
/// answer the ARN of the stream it registered.
async fn table_with_stream(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
) -> String {
    let created = ddb_ok(
        router,
        case,
        "CreateTable",
        create_body(
            name,
            Some(json!({"StreamEnabled": true, "StreamViewType": "NEW_AND_OLD_IMAGES"})),
        ),
    )
    .await;
    let stream_arn = created["TableDescription"]["LatestStreamArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !stream_arn.is_empty(),
        "case {case:?}: an enabled stream must be reported, got {created}"
    );
    wait_until_active(router, case, name).await;
    stream_arn
}

/// Poll `DescribeTable` until the table reports `ACTIVE`, so the writes below reach a
/// table that serves them.
async fn wait_until_active(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
) {
    let started = std::time::Instant::now();
    let mut last = Value::Null;
    while started.elapsed() < ACTIVE_DEADLINE {
        let described = ddb_ok(router, case, "DescribeTable", json!({"TableName": name})).await;
        if described["Table"]["TableStatus"] == json!("ACTIVE") {
            return;
        }
        last = described;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: {name} never reached ACTIVE within {ACTIVE_DEADLINE:?}, last saw {last}");
}

/// The item `GetItem` reports for `key` — the table's own account of what it stores,
/// which is what a record's images must match.
async fn stored_item(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    table: &str,
    key: &Value,
) -> Value {
    let read = ddb_ok(
        router,
        case,
        "GetItem",
        json!({"TableName": table, "Key": key, "ConsistentRead": true}),
    )
    .await;
    assert!(
        read["Item"].is_object(),
        "case {case:?}: the item just written must read back, got {read}"
    );
    read["Item"].clone()
}

#[tokio::test]
async fn a_records_images_are_the_item_as_the_table_stores_it_for_every_attribute_type() {
    let case = "the attribute vocabulary";
    let router = emulator_with_streams();
    let table = "images";
    let stream_arn = table_with_stream(&router, case, table).await;

    let rows = vocabulary();
    // What each row's three changes must leave on the stream: the images read off the
    // table, in the order the writes were made.
    let mut expected: Vec<(Value, Value, Value, String)> = Vec::new();
    for row in &rows {
        let case = &row.case;
        let id = format!("row-{case}");
        let key = json!({"Pk": {"S": id}});

        // A key that held no item, then the same key again with one more attribute,
        // then the key removed: one INSERT, one MODIFY and one REMOVE.
        ddb_ok(
            &router,
            case,
            "PutItem",
            json!({
                "TableName": table,
                "Item": {"Pk": {"S": id}, "Payload": row.written.clone()},
            }),
        )
        .await;
        let after_insert = stored_item(&router, case, table, &key).await;
        ddb_ok(
            &router,
            case,
            "PutItem",
            json!({
                "TableName": table,
                "Item": {
                    "Pk": {"S": id},
                    "Payload": row.written.clone(),
                    "Marker": {"S": "second"},
                },
            }),
        )
        .await;
        let after_modify = stored_item(&router, case, table, &key).await;
        ddb_ok(
            &router,
            case,
            "DeleteItem",
            json!({"TableName": table, "Key": key.clone()}),
        )
        .await;

        assert!(
            !after_insert["Payload"].is_null(),
            "case {case:?}: the written attribute must be stored, got {after_insert}"
        );
        if row.restated {
            assert_ne!(
                after_insert["Payload"], row.written,
                "case {case:?}: the table stores a canonical form of this value, so the row \
                 would not test that an image is the STORED value; got {after_insert}"
            );
        }
        expected.push((key, after_insert, after_modify, row.case.clone()));
    }

    let records = drain_stream(&router, case, &stream_arn).await;
    let names = event_names(&records);
    let wanted: Vec<&str> = rows
        .iter()
        .flat_map(|_| ["INSERT", "MODIFY", "REMOVE"])
        .collect();
    assert_eq!(
        names, wanted,
        "case {case:?}: each row's three changes must appear in the order they were made, got \
         {names:?}"
    );

    for (position, (key, after_insert, after_modify, row)) in expected.iter().enumerate() {
        let insert = &records[position * 3]["dynamodb"];
        let modify = &records[position * 3 + 1]["dynamodb"];
        let remove = &records[position * 3 + 2]["dynamodb"];
        for (event, change) in [("INSERT", insert), ("MODIFY", modify), ("REMOVE", remove)] {
            assert_eq!(
                change["Keys"], *key,
                "case {row:?}: the {event} record must carry the item's key alone, got {change}"
            );
        }
        assert_eq!(
            insert["NewImage"], *after_insert,
            "case {row:?}: the INSERT record's NewImage must be the item the table stores, got \
             {insert}"
        );
        assert!(
            insert.get("OldImage").is_none(),
            "case {row:?}: an INSERT has no before image, got {insert}"
        );
        assert_eq!(
            modify["OldImage"], *after_insert,
            "case {row:?}: the MODIFY record's OldImage must be the item it replaced, got {modify}"
        );
        assert_eq!(
            modify["NewImage"], *after_modify,
            "case {row:?}: the MODIFY record's NewImage must be the item the write stored, got \
             {modify}"
        );
        assert_eq!(
            remove["OldImage"], *after_modify,
            "case {row:?}: the REMOVE record's OldImage must be the item that was removed, got \
             {remove}"
        );
        assert!(
            remove.get("NewImage").is_none(),
            "case {row:?}: a REMOVE has no after image, got {remove}"
        );
        // "The size of the stream record, in bytes" (`StreamRecord$SizeBytes`) is read
        // off the images the record carries, so the MODIFY — which carries the image
        // either side of the change — is larger than the INSERT, which carries one.
        let sized = |change: &Value| change["SizeBytes"].as_i64().unwrap_or_default();
        assert!(
            sized(insert) > 0 && sized(modify) > sized(insert) && sized(remove) > 0,
            "case {row:?}: each record's SizeBytes must measure the images it carries, got \
             INSERT {}, MODIFY {}, REMOVE {}",
            sized(insert),
            sized(modify),
            sized(remove)
        );
    }

    // Every record names the stream's own view type and the format the emulator emits,
    // and the sequence numbers run in the order the changes were made: a record is a
    // position in a time-ordered sequence, which is what a consumer resumes from.
    let mut previous: Option<(usize, String)> = None;
    for record in &records {
        let change = &record["dynamodb"];
        assert_eq!(
            change["StreamViewType"], json!("NEW_AND_OLD_IMAGES"),
            "case {case:?}: a record must name the view type its stream captures, got {record}"
        );
        assert!(
            change["ApproximateCreationDateTime"].is_number(),
            "case {case:?}: a record must carry when the change was made, got {record}"
        );
        let sequence = change["SequenceNumber"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit()),
            "case {case:?}: a sequence number holds numeric characters only, got {record}"
        );
        // Ordered by significant-digit count and then lexicographically, which is how
        // a decimal string of unconstrained width compares.
        let ordinal = (sequence.trim_start_matches('0').len(), sequence);
        if let Some(earlier) = &previous {
            assert!(
                *earlier < ordinal,
                "case {case:?}: sequence numbers must increase with the changes, got {earlier:?} \
                 then {ordinal:?}"
            );
        }
        previous = Some(ordinal);
    }
}

#[tokio::test]
async fn an_item_the_table_refuses_for_nesting_past_the_bound_publishes_nothing() {
    // The two halves of the seam hold the same documented nesting quota, so the depth
    // bound is where a write is refused and never where a landed write's record is
    // dropped: the level at the bound is stored AND published (the suite above), and
    // one past it is refused by the table, which makes no change to publish.
    let case = "one level past the documented nesting bound";
    let router = emulator_with_streams();
    let table = "deep";
    let stream_arn = table_with_stream(&router, case, table).await;

    let (status, refused) = call(
        &router,
        "PutItem",
        json!({
            "TableName": table,
            "Item": {"Pk": {"S": "too-deep"}, "Payload": nested_to(MAX_ATTRIBUTE_DEPTH + 1)},
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: the write must be refused, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {refused}"
    );
    assert_eq!(
        drain_stream(&router, case, &stream_arn).await,
        Vec::<Value>::new(),
        "case {case:?}: a refused write changed no item, so its stream carries no record"
    );
}

#[tokio::test]
async fn one_requests_writes_reach_the_stream_of_each_table_they_land_on() {
    // A change is published to the stream of the table it was made to, so one request
    // writing to several tables leaves each table's own changes on its own stream — and
    // a table in the same request whose stream is off leaves none anywhere.
    let case = "two streamed tables and one unstreamed in one BatchWriteItem";
    let router = emulator_with_streams();
    let left = table_with_stream(&router, case, "left").await;
    let right = table_with_stream(&router, case, "right").await;
    ddb_ok(&router, case, "CreateTable", create_body("quiet", None)).await;
    wait_until_active(&router, case, "quiet").await;

    ddb_ok(
        &router,
        case,
        "BatchWriteItem",
        json!({
            "RequestItems": {
                "left": [
                    {"PutRequest": {"Item": {"Pk": {"S": "l1"}, "Payload": {"N": "1"}}}},
                    {"PutRequest": {"Item": {"Pk": {"S": "l2"}, "Payload": {"L": [{"BOOL": true}]}}}},
                ],
                "right": [
                    {"PutRequest": {"Item": {"Pk": {"S": "r1"}, "Payload": {"SS": ["a", "b"]}}}},
                ],
                "quiet": [
                    {"PutRequest": {"Item": {"Pk": {"S": "q1"}, "Payload": {"S": "unwatched"}}}},
                ],
            }
        }),
    )
    .await;

    for (table, stream_arn, keys) in [
        ("left", &left, vec!["l1", "l2"]),
        ("right", &right, vec!["r1"]),
    ] {
        let records = drain_stream(&router, case, stream_arn).await;
        assert_eq!(
            event_names(&records),
            keys.iter().map(|_| "INSERT").collect::<Vec<_>>(),
            "case {case:?}: {table} must carry one record per item it took, got {records:?}"
        );
        let mut carried: Vec<String> = records
            .iter()
            .map(|record| record["dynamodb"]["Keys"]["Pk"]["S"].as_str().unwrap_or_default().to_string())
            .collect();
        carried.sort();
        assert_eq!(
            carried, keys,
            "case {case:?}: {table}'s stream must carry only the keys written to {table}, got \
             {records:?}"
        );
        for (position, key) in keys.iter().enumerate() {
            let stored = stored_item(&router, case, table, &json!({"Pk": {"S": key}})).await;
            let published = records
                .iter()
                .find(|record| record["dynamodb"]["Keys"]["Pk"]["S"] == json!(key))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {table} must publish a record for {key:?}")
                });
            assert_eq!(
                published["dynamodb"]["NewImage"], stored,
                "case {case:?}: record {position} on {table} must carry the item the batch stored, \
                 got {published}"
            );
        }
    }
}
