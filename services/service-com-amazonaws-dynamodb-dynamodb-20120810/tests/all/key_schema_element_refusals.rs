// SPDX-License-Identifier: CC0-1.0

//! Which ELEMENT of a `KeySchema` a refusal names, end to end over the wire.
//!
//! One rule is under test, over a table of key schemas rather than one recorded
//! request: a key schema is validated element by element in the order the request
//! lists it — the partition keys lead and the sort keys follow — so the first element
//! whose `KeyType` is not the one its position requires is refused with a 400
//! `ValidationException` naming that element's ordinal position and the key type it
//! should have carried (`Invalid KeySchema: The second KeySchemaElement is not a RANGE
//! key type`), and a schema whose every position is right is accepted and read back. One
//! defect is stated as the rule instead of as a position: a partition key that FOLLOWS a
//! sort key, where a schema may carry several partition keys and no single element is the
//! one at fault.
//!
//! The rows differ along what the rule generalises over: the key type at each position,
//! the number of elements, the order they appear in, the attribute names and their
//! scalar types, and the key schema's owner (a table's primary key, an index key
//! declared with the table, and an index key added later by `UpdateTable`).
//!
//! The counts the index rows are tested at and one past are the published bounds from
//! the Smithy model's `CreateTable$GlobalSecondaryIndexes` doc — "Each global secondary
//! index supports up to 4 partition keys and up to 4 sort keys" — which this crate
//! holds as `MAX_INDEX_PARTITION_KEYS` / `MAX_INDEX_SORT_KEYS`; a table's own primary
//! key is "one KeySchemaElement (for the partition key)" plus, when composite, "another
//! KeySchemaElement for the sort key" (the `KeySchemaElement` doc), i.e.
//! `MAX_TABLE_KEY_ELEMENTS`. The scalar key types S, N and B come from the
//! `KeySchemaElement` doc ("The data type must be one of String, Number, or Binary").

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    MAX_INDEX_PARTITION_KEYS, MAX_INDEX_SORT_KEYS, MAX_TABLE_KEY_ELEMENTS, STATUS_TRANSITION,
};

use crate::emulator::{call, emulator, error_code, index_named, message};

/// One key element of a case: the attribute name, the `KeyType` the request gives it,
/// and the `AttributeType` its definition declares.
type Key = (&'static str, &'static str, &'static str);

/// One row: a key schema, and the whole message its refusal must carry — `None` where
/// the schema is legal and must be accepted.
struct KeyRow {
    case: &'static str,
    key_schema: &'static [Key],
    refusal: Option<&'static str>,
}

/// The `KeySchema` the request sends for `keys`.
fn elements(keys: &[Key]) -> Vec<Value> {
    keys.iter()
        .map(|(name, key_type, _)| json!({"AttributeName": name, "KeyType": key_type}))
        .collect()
}

/// The `AttributeDefinitions` those keys need, each attribute declared once.
fn definitions(keys: &[Key]) -> Vec<Value> {
    let mut declared: Vec<&str> = Vec::new();
    let mut definitions = Vec::new();
    for (name, _, attribute_type) in keys {
        if declared.contains(name) {
            continue;
        }
        declared.push(name);
        definitions.push(json!({"AttributeName": name, "AttributeType": attribute_type}));
    }
    definitions
}

/// A table's primary key: position 1 is the partition key and position 2, when present,
/// the sort key.
const TABLE_KEY_ROWS: &[KeyRow] = &[
    KeyRow {
        case: "one HASH element on a string attribute",
        key_schema: &[("pk", "HASH", "S")],
        refusal: None,
    },
    KeyRow {
        case: "a HASH element followed by a RANGE element",
        key_schema: &[("pk", "HASH", "S"), ("sk", "RANGE", "S")],
        refusal: None,
    },
    KeyRow {
        case: "the same pair on a number partition key and a binary sort key",
        key_schema: &[("part", "HASH", "N"), ("sort", "RANGE", "B")],
        refusal: None,
    },
    KeyRow {
        case: "two HASH elements",
        key_schema: &[("a", "HASH", "S"), ("b", "HASH", "S")],
        refusal: Some("Invalid KeySchema: The second KeySchemaElement is not a RANGE key type"),
    },
    KeyRow {
        case: "two HASH elements on number attributes",
        key_schema: &[("one", "HASH", "N"), ("two", "HASH", "N")],
        refusal: Some("Invalid KeySchema: The second KeySchemaElement is not a RANGE key type"),
    },
    KeyRow {
        case: "a single RANGE element",
        key_schema: &[("only", "RANGE", "S")],
        refusal: Some("Invalid KeySchema: The first KeySchemaElement is not a HASH key type"),
    },
    KeyRow {
        case: "the right key types in the wrong order",
        key_schema: &[("sorted", "RANGE", "S"), ("parted", "HASH", "S")],
        refusal: Some("Invalid KeySchema: The first KeySchemaElement is not a HASH key type"),
    },
    KeyRow {
        case: "two RANGE elements",
        key_schema: &[("first", "RANGE", "B"), ("second", "RANGE", "N")],
        refusal: Some("Invalid KeySchema: The first KeySchemaElement is not a HASH key type"),
    },
];

#[tokio::test]
async fn a_table_primary_key_refusal_names_the_element_that_broke_the_rule() {
    let router = emulator();

    for (ordinal, row) in TABLE_KEY_ROWS.iter().enumerate() {
        let case = row.case;
        let table = format!("keys-{ordinal}");
        let (status, body) = call(
            &router,
            "CreateTable",
            json!({
                "TableName": table,
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": elements(row.key_schema),
                "AttributeDefinitions": definitions(row.key_schema),
            }),
        )
        .await;
        match row.refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a legal key schema to be accepted, got {status} \
                     with {body}"
                );
                // The accepted schema is read back through DescribeTable, so the
                // acceptance is about the STORED table and not only the reply.
                let (read_status, described) =
                    call(&router, "DescribeTable", json!({"TableName": table})).await;
                assert_eq!(
                    read_status, 200,
                    "case {case:?}: expected the created table to be describable, got \
                     {read_status} with {described}"
                );
                assert_eq!(
                    described["Table"]["KeySchema"],
                    json!(elements(row.key_schema)),
                    "case {case:?}: expected the key schema it was created with, got {described}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: expected a 400 ValidationException, got {status} with {body}"
                );
                let stated = message(&body);
                assert_eq!(
                    stated, expected,
                    "case {case:?}: expected the refusal to name the offending element, got \
                     {stated:?}"
                );
            }
        }
    }

    // One past the element count a table's primary key has positions for: a third
    // element is refused whatever its key type, and the refusal states the bound rather
    // than naming an element, because no position required a different type.
    let over: Vec<Key> = vec![
        ("pk", "HASH", "S"),
        ("sk", "RANGE", "S"),
        ("extra", "RANGE", "S"),
    ];
    let (status, body) = call(
        &router,
        "CreateTable",
        json!({
            "TableName": "too-many-key-elements",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": elements(&over),
            "AttributeDefinitions": definitions(&over),
        }),
    )
    .await;
    let case = "one element past a table's primary key";
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected a 400 ValidationException, got {status} with {body}"
    );
    let stated = message(&body);
    assert!(
        stated.contains("KeySchema") && stated.contains(&MAX_TABLE_KEY_ELEMENTS.to_string()),
        "case {case:?}: expected the refusal to name KeySchema and the {MAX_TABLE_KEY_ELEMENTS} \
         element bound, got {stated:?}"
    );
}

/// `count` HASH elements and `sort_keys` RANGE elements, in that order, as an index key.
fn index_key(count: usize, sort_keys: usize) -> Vec<Value> {
    (0..count)
        .map(|which| json!({"AttributeName": format!("h{which}"), "KeyType": "HASH"}))
        .chain(
            (0..sort_keys)
                .map(|which| json!({"AttributeName": format!("r{which}"), "KeyType": "RANGE"})),
        )
        .collect()
}

/// The definitions one index-key case needs, each attribute declared once: the table's
/// own partition key `pk`, plus every attribute the index key names — a partition key
/// as a string and a sort key as a number. A definition no key schema uses is itself a
/// bad request, so each case declares exactly its own.
fn definitions_for(key_schema: &[Value]) -> Vec<Value> {
    let mut definitions = vec![json!({"AttributeName": "pk", "AttributeType": "S"})];
    for element in key_schema {
        let name = element["AttributeName"].clone();
        if definitions
            .iter()
            .any(|declared| declared["AttributeName"] == name)
        {
            continue;
        }
        let attribute_type = if element["KeyType"] == json!("RANGE") {
            "N"
        } else {
            "S"
        };
        definitions.push(json!({"AttributeName": name, "AttributeType": attribute_type}));
    }
    definitions
}

#[tokio::test]
async fn an_index_key_refusal_names_the_element_that_broke_the_rule() {
    let router = emulator();

    // Each row: the case, the index key schema, and the whole message its refusal must
    // carry — `None` where the key is legal. An index key leads with up to
    // MAX_INDEX_PARTITION_KEYS partition keys and follows with up to MAX_INDEX_SORT_KEYS
    // sort keys, so a HASH element past the leading run is the element out of place.
    let rows: Vec<(&str, Vec<Value>, Option<String>)> = vec![
        ("one HASH element", index_key(1, 0), None),
        ("a HASH element and a RANGE element", index_key(1, 1), None),
        (
            "two HASH elements, which an index key allows",
            index_key(2, 0),
            None,
        ),
        (
            "both published bounds at once",
            index_key(MAX_INDEX_PARTITION_KEYS, MAX_INDEX_SORT_KEYS),
            None,
        ),
        (
            "a single RANGE element",
            vec![json!({"AttributeName": "r0", "KeyType": "RANGE"})],
            Some(
                "Invalid KeySchema for index idx: The first KeySchemaElement is not a HASH key \
                 type"
                    .to_string(),
            ),
        ),
        (
            "a HASH element after a RANGE element",
            vec![
                json!({"AttributeName": "h0", "KeyType": "HASH"}),
                json!({"AttributeName": "r0", "KeyType": "RANGE"}),
                json!({"AttributeName": "h1", "KeyType": "HASH"}),
            ],
            // The ORDER of the two kinds of key is stated as the rule itself: with
            // several partition keys allowed, no single element is the one at fault.
            Some(
                "One or more parameter values were invalid: All HASH key attributes must \
                 precede RANGE key attributes in the KeySchema"
                    .to_string(),
            ),
        ),
        (
            "one HASH element past the partition-key bound",
            index_key(MAX_INDEX_PARTITION_KEYS + 1, 0),
            Some(
                "Invalid KeySchema for index idx: The fifth KeySchemaElement is not a RANGE key \
                 type"
                    .to_string(),
            ),
        ),
    ];

    for (ordinal, (case, key_schema, refusal)) in rows.iter().enumerate() {
        let table = format!("indexed-{ordinal}");
        let (status, body) = call(
            &router,
            "CreateTable",
            json!({
                "TableName": table,
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": definitions_for(key_schema),
                "GlobalSecondaryIndexes": [{
                    "IndexName": "idx",
                    "KeySchema": key_schema,
                    "Projection": {"ProjectionType": "ALL"},
                }],
            }),
        )
        .await;
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a legal index key to be accepted, got {status} \
                     with {body}"
                );
                let (read_status, described) =
                    call(&router, "DescribeTable", json!({"TableName": table})).await;
                assert_eq!(
                    read_status, 200,
                    "case {case:?}: expected the created table to be describable, got \
                     {read_status} with {described}"
                );
                assert_eq!(
                    index_named(&described["Table"], "idx")["KeySchema"],
                    json!(key_schema),
                    "case {case:?}: expected the index key schema it was created with, got \
                     {described}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: expected a 400 ValidationException, got {status} with {body}"
                );
                let stated = message(&body);
                assert_eq!(
                    &stated, expected,
                    "case {case:?}: expected the refusal to name the offending element and the \
                     index, got {stated:?}"
                );
            }
        }
    }

    // One past the sort-key bound: the elements are all in the right order, so the
    // refusal states the bound instead of naming an element.
    let (status, body) = call(
        &router,
        "CreateTable",
        json!({
            "TableName": "too-many-sort-keys",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": definitions_for(&index_key(1, MAX_INDEX_SORT_KEYS + 1)),
            "GlobalSecondaryIndexes": [{
                "IndexName": "idx",
                "KeySchema": index_key(1, MAX_INDEX_SORT_KEYS + 1),
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    let case = "one element past the index sort-key bound";
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected a 400 ValidationException, got {status} with {body}"
    );
    let stated = message(&body);
    assert!(
        stated.contains("KeySchema") && stated.contains(&MAX_INDEX_SORT_KEYS.to_string()),
        "case {case:?}: expected the refusal to name KeySchema and the {MAX_INDEX_SORT_KEYS} \
         sort-key bound, got {stated:?}"
    );
}

#[tokio::test]
async fn an_index_added_later_is_refused_by_the_same_element_rule() {
    let router = emulator();
    let (status, created) = call(
        &router,
        "CreateTable",
        json!({
            "TableName": "grown",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": definitions_for(&index_key(1, 0)),
            "GlobalSecondaryIndexes": [{
                "IndexName": "born-with",
                "KeySchema": index_key(1, 0),
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create the table UpdateTable grows\": expected 200, got {status} with {created}"
    );
    // UpdateTable refuses a table that is still CREATING, so the clock-owed transition
    // is waited out before the index is added.
    tokio::time::sleep(STATUS_TRANSITION + Duration::from_millis(200)).await;

    let case = "an added index whose first element is a RANGE";
    let (status, refused) = call(
        &router,
        "UpdateTable",
        json!({
            "TableName": "grown",
            "AttributeDefinitions": definitions_for(&index_key(1, 1)),
            "GlobalSecondaryIndexUpdates": [{
                "Create": {
                    "IndexName": "added",
                    "KeySchema": [
                        {"AttributeName": "r0", "KeyType": "RANGE"},
                        {"AttributeName": "h0", "KeyType": "HASH"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
            }],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected a 400 ValidationException, got {status} with {refused}"
    );
    let stated = message(&refused);
    assert_eq!(
        stated,
        "Invalid KeySchema for index added: The first KeySchemaElement is not a HASH key type",
        "case {case:?}: expected the refusal to name the offending element and the index, got \
         {stated:?}"
    );

    // The same two attributes in the order an index key requires are accepted, so the
    // refusal above is about the position of the elements and nothing else.
    let case = "the same attributes with the partition key first";
    let (status, updated) = call(
        &router,
        "UpdateTable",
        json!({
            "TableName": "grown",
            "AttributeDefinitions": definitions_for(&index_key(1, 1)),
            "GlobalSecondaryIndexUpdates": [{
                "Create": {
                    "IndexName": "added",
                    "KeySchema": [
                        {"AttributeName": "h0", "KeyType": "HASH"},
                        {"AttributeName": "r0", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the index to be added, got {status} with {updated}"
    );
    let (read_status, described) =
        call(&router, "DescribeTable", json!({"TableName": "grown"})).await;
    assert_eq!(
        read_status, 200,
        "case {case:?}: expected the table to be describable, got {read_status} with {described}"
    );
    assert_eq!(
        index_named(&described["Table"], "added")["KeySchema"],
        json!([
            {"AttributeName": "h0", "KeyType": "HASH"},
            {"AttributeName": "r0", "KeyType": "RANGE"},
        ]),
        "case {case:?}: expected the added index to carry the key schema it was created with, \
         got {described}"
    );
}
