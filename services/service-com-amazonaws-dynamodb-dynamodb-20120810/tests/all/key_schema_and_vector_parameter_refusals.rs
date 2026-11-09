// SPDX-License-Identifier: CC0-1.0

//! How `CreateTable` and a vector WRITE word the parameter they refuse, end to end over
//! the wire.
//!
//! Six rules are under test, each over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. an INDEX key schema whose partition keys do not all precede its sort keys is
//!    refused with the fixed sentence that states that order, and not with the ordinal of
//!    an element: an index key may carry several partition keys, so no single position is
//!    the one at fault;
//! 2. a TABLE's own key schema naming an attribute `AttributeDefinitions` does not declare
//!    is refused with its own fixed sentence, which interpolates nothing, while every
//!    declared definition is used by some key schema; a request that also declares one no
//!    key schema uses has the two sets differing both ways round and is told what each of
//!    them holds, in the wording rule 3 describes;
//! 3. an INDEX key schema naming one is refused with the parameter-value sentence that
//!    carries the index's whole key schema beside the declared attribute names — and
//!    NEITHER list is in the order the request stated it, so the same names declared in
//!    any order are reported identically;
//! 4. a vector index's `Dimensions` is held to an inclusive range, stated as one fixed
//!    sentence wherever an index is declared (with the table, or by `UpdateTable`);
//! 5. a table created on-demand that also states a provisioned throughput is refused by
//!    the two CAPACITY members it carries, which are what an on-demand table has no
//!    setting for;
//! 6. a write whose vector attribute is not the shape its index was built for reports
//!    that attribute as the PARAMETER at fault beside the index that requires it: the type
//!    the parameter has to be, the position and the `AttributeValue` type of the element
//!    that is not one, or the count the index was built for beside the count supplied. The
//!    TYPE is judged before the COUNT.
//!
//! Rule 7, over `ConsumedCapacity`: a vector write meters the index ENTRY and not the item
//! around it — the key the entry is held under plus four bytes per dimension of the index,
//! held to a per-write floor — so the figure follows the index's `Dimensions` and the key,
//! and the attributes the item carries beside the vector do not enter it.
//!
//! Rule 8: a vector index spends from the same projected-attribute budget as the table's
//! other indexes, and its own attributes spend from it too — the vector attribute counts as
//! one, as does each `INLINE_FILTER` search-schema element.
//!
//! The rows differ along what each rule generalises over: the length of a key schema and
//! where the misplaced key sits in it, the index kind (global and local), which element of
//! a key schema has no definition, what consumes the definitions declared beside it, the
//! number of attribute names in each reported list and the order the request declares them
//! in, the `Dimensions` asked for (at each published bound and one past it), the capacity
//! figures, the number of components in a vector, the position and type of a bad
//! component, the type of a non-list vector, the key the item is written under, the size of
//! the item that carries the vector, and the number of attributes an index projects (at the
//! budget and one past it).
//!
//! The published dimension bounds (1 and 4096) are the vector-search entry of "Service,
//! Account, and Table Quotas" in the Amazon DynamoDB Developer Guide, which the model
//! bounds below as a `@range` minimum on `VectorIndex$Dimensions`; the 100-attribute
//! projection budget is `Projection$NonKeyAttributes` ("the total count of
//! NonKeyAttributes summed across all of the secondary indexes, must not exceed 100") with
//! the vector index's share from `CreateTable$VectorIndexes`; the 1 KB per-write floor and
//! the four bytes a stored dimension costs are the recorded metering of
//! `ConsumedCapacity$VectorIndexes`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The fixed sentence a key schema whose HASH keys do not all precede its RANGE keys
/// earns.
const HASH_AFTER_RANGE: &str = "One or more parameter values were invalid: All HASH key attributes \
                                must precede RANGE key attributes in the KeySchema";

/// The fixed sentence a TABLE's own key schema earns when an element of it has no
/// `AttributeDefinitions` entry.
const UNDEFINED_TABLE_KEY: &str = "Invalid KeySchema: Some index key attribute have no definition";

/// The fixed sentence a `Dimensions` outside the published range earns.
const DIMENSION_RANGE: &str =
    "One or more parameter values were invalid: Number of dimensions must be between 1 and 4096 \
     inclusive.";

/// The fixed sentence a table created on-demand with a provisioned throughput earns.
const CAPACITY_ON_DEMAND: &str =
    "One or more parameter values were invalid: Neither ReadCapacityUnits nor WriteCapacityUnits \
     can be specified when BillingMode is PAY_PER_REQUEST";

/// One key schema, as `(attribute name, key type)` pairs.
type Keys = &'static [(&'static str, &'static str)];

/// A list of attribute names, in the order a request declares them.
type Names = &'static [&'static str];

/// One `KeySchemaElement`.
fn key(name: &str, key_type: &str) -> Value {
    json!({"AttributeName": name, "KeyType": key_type})
}

/// The `AttributeDefinitions` declaring every name in `names` as a string attribute.
fn definitions(names: &[&str]) -> Vec<Value> {
    names
        .iter()
        .map(|name| json!({"AttributeName": name, "AttributeType": "S"}))
        .collect()
}

/// One index key schema, as `(attribute, key type)` pairs.
fn elements(keys: &[(&str, &str)]) -> Vec<Value> {
    keys.iter().map(|(name, kind)| key(name, kind)).collect()
}

/// Every attribute name an index key schema addresses.
fn named(keys: &[(&str, &str)]) -> Vec<String> {
    keys.iter().map(|(name, _)| (*name).to_string()).collect()
}

/// Assert `body` is a 400 `ValidationException` and answer the message it carries.
fn refusal(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected a 400 ValidationException, got {status} with {body}"
    );
    message(body)
}

/// Rule 1: a partition key that follows a sort key is refused with the ORDER rule itself,
/// whichever index kind declares the schema and however long it is.
#[tokio::test]
async fn a_partition_key_after_a_sort_key_is_refused_with_the_order_rule() {
    let router = emulator();

    // Rows differ along the length of the key schema and where the misplaced partition key
    // sits in it, plus the index kind that declares it.
    let rows: [(&str, Keys); 5] = [
        (
            "a partition key straight after the sort key",
            &[("a", "HASH"), ("b", "RANGE"), ("c", "HASH")],
        ),
        (
            "a partition key after two sort keys",
            &[("a", "HASH"), ("b", "RANGE"), ("c", "RANGE"), ("d", "HASH")],
        ),
        (
            "a partition key after a pair of partition keys and a sort key",
            &[("a", "HASH"), ("b", "HASH"), ("c", "RANGE"), ("d", "HASH")],
        ),
        (
            "a partition key between two sort keys",
            &[("a", "HASH"), ("b", "RANGE"), ("c", "HASH"), ("d", "RANGE")],
        ),
        (
            "a local index whose last element is a partition key",
            &[("pk", "HASH"), ("b", "RANGE"), ("c", "HASH")],
        ),
    ];

    for (ordinal, (case, key_schema)) in rows.iter().enumerate() {
        let local = case.starts_with("a local");
        let table = format!("ordered-{ordinal}");
        let mut names: Vec<&str> = key_schema.iter().map(|(name, _)| *name).collect();
        if !names.contains(&"pk") {
            names.push("pk");
        }
        let index = json!({
            "IndexName": "idx",
            "KeySchema": elements(key_schema),
            "Projection": {"ProjectionType": "ALL"},
        });
        let mut request = json!({
            "TableName": table,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [key("pk", "HASH")],
            "AttributeDefinitions": definitions(&names),
        });
        match local {
            true => request["LocalSecondaryIndexes"] = json!([index]),
            false => request["GlobalSecondaryIndexes"] = json!([index]),
        }
        let (status, body) = call(&router, "CreateTable", request).await;
        assert_eq!(
            refusal(case, status, &body),
            HASH_AFTER_RANGE,
            "case {case:?}: expected the order rule's own sentence, got {body}"
        );
    }

    // The controls: the same attributes in an order that keeps every partition key ahead
    // of every sort key are accepted, and the stored index reports that schema back.
    for (ordinal, (case, key_schema)) in [
        (
            "one partition key and one sort key",
            vec![("a", "HASH"), ("b", "RANGE")],
        ),
        (
            "two partition keys and two sort keys",
            vec![
                ("a", "HASH"),
                ("b", "HASH"),
                ("c", "RANGE"),
                ("d", "RANGE"),
            ],
        ),
    ]
    .iter()
    .enumerate()
    {
        let table = format!("in-order-{ordinal}");
        let mut names: Vec<&str> = key_schema.iter().map(|(name, _)| *name).collect();
        names.push("pk");
        ok(
            &router,
            "CreateTable",
            json!({
                "TableName": table,
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [key("pk", "HASH")],
                "AttributeDefinitions": definitions(&names),
                "GlobalSecondaryIndexes": [{
                    "IndexName": "idx",
                    "KeySchema": elements(key_schema),
                    "Projection": {"ProjectionType": "ALL"},
                }],
            }),
        )
        .await;
        let described = describe_when_active(&router, case, &table).await;
        assert_eq!(
            crate::emulator::index_named(&described, "idx")["KeySchema"],
            json!(elements(key_schema)),
            "case {case:?}: expected the index key schema it was created with, got {described}"
        );
    }
}

/// Rule 2: a TABLE key attribute with no definition earns the fixed sentence while every
/// declared definition is used; a request that also declares one no key schema uses has
/// the two sets differing both ways round and is told what each of them holds.
#[tokio::test]
async fn a_table_key_with_no_definition_is_worded_by_how_the_two_sets_differ() {
    let router = emulator();

    // Rows differ along WHICH element has no definition, the number of definitions the
    // request declares, and what consumes them — every one of them used, so the key with
    // nothing declared for it is the only fact there is to state.
    let rows: [(&str, Keys, Names, bool); 4] = [
        (
            "the sort key has no definition",
            &[("pk", "HASH"), ("sk", "RANGE")],
            &["pk"],
            false,
        ),
        (
            "the partition key has no definition",
            &[("Alpha", "HASH"), ("Beta", "RANGE")],
            &["Beta"],
            false,
        ),
        (
            "a definition used by an index rather than by the table key",
            &[("pk", "HASH"), ("sk", "RANGE")],
            &["pk", "indexed"],
            true,
        ),
        (
            "an undefined sort key with every definition consumed elsewhere",
            &[("one", "HASH"), ("two", "RANGE")],
            &["one", "indexed"],
            true,
        ),
    ];

    for (ordinal, (case, key_schema, declared, indexed)) in rows.iter().enumerate() {
        let table = format!("undefined-{ordinal}");
        let mut request = json!({
            "TableName": table,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": elements(key_schema),
            "AttributeDefinitions": definitions(declared),
        });
        if *indexed {
            request["GlobalSecondaryIndexes"] = json!([{
                "IndexName": "idx",
                "KeySchema": [key("indexed", "HASH")],
                "Projection": {"ProjectionType": "ALL"},
            }]);
        }
        let (status, body) = call(&router, "CreateTable", request).await;
        assert_eq!(
            refusal(case, status, &body),
            UNDEFINED_TABLE_KEY,
            "case {case:?}: expected the table key schema's own fixed sentence, got {body}"
        );
        let (status, listed) = call(&router, "ListTables", json!({})).await;
        assert_eq!(status, 200, "case {case:?}: ListTables answered {listed}");
        assert!(
            !listed["TableNames"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .contains(&json!(table)),
            "case {case:?}: a refused create stores no table, got {listed}"
        );
    }

    // The sets differing BOTH ways round — a key with no definition beside a definition
    // no key schema uses — is told what each list holds, in the order a name-keyed table
    // renders them and not the one the request stated.
    let (status, body) = call(
        &router,
        "CreateTable",
        json!({
            "TableName": "both-ways",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [key("PK", "HASH"), key("SK", "RANGE")],
            "AttributeDefinitions": definitions(&["PK", "EXTRA"]),
        }),
    )
    .await;
    assert_eq!(
        refusal("an undefined key beside a definition nothing uses", status, &body),
        "One or more parameter values were invalid: Some index key attributes are not defined in \
         AttributeDefinitions. Keys: [SK, PK], AttributeDefinitions: [EXTRA, PK]",
        "case \"an undefined key beside a definition nothing uses\": expected both lists, got \
         {body}"
    );

    // The control: every element defined is accepted and read back.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "defined",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [key("pk", "HASH"), key("sk", "RANGE")],
            "AttributeDefinitions": definitions(&["pk", "sk"]),
        }),
    )
    .await;
    let described = describe_when_active(&router, "every element defined", "defined").await;
    assert_eq!(
        described["KeySchema"],
        json!([key("pk", "HASH"), key("sk", "RANGE")]),
        "case \"every element defined\": expected the key schema it was created with, got \
         {described}"
    );
}

/// The two bracketed lists of a `Keys: [..], AttributeDefinitions: [..]` message.
fn reported_lists(stated: &str) -> (Vec<String>, Vec<String>) {
    let names = |section: &str| -> Vec<String> {
        section
            .split(", ")
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect()
    };
    let mut lists = stated.split('[').skip(1).map(|rest| {
        names(
            rest.split(']')
                .next()
                .expect("a bracketed list closes its bracket"),
        )
    });
    (
        lists.next().unwrap_or_default(),
        lists.next().unwrap_or_default(),
    )
}

/// Rule 3: an INDEX key attribute with no definition reports both lists, neither of them
/// in the order the request declared it — so the message depends on the NAMES alone.
#[tokio::test]
async fn an_undefined_index_key_reports_both_lists_out_of_request_order() {
    let router = emulator();

    // Each case is one name set, declared in two different request orders: the attribute
    // definitions are permuted, and the index key schema's own order is turned around by
    // swapping which of its attributes is the partition key. Rows differ along the number
    // of names in each list.
    let cases: [(&str, Names, Names, Keys, Keys); 2] = [
        (
            "two declared names and a two-element index key",
            &["PK", "EXTRA"],
            &["EXTRA", "PK"],
            &[("PK", "HASH"), ("SK", "RANGE")],
            &[("SK", "HASH"), ("PK", "RANGE")],
        ),
        (
            "three declared names and a two-element index key",
            &["Alpha", "Beta", "Gamma"],
            &["Gamma", "Alpha", "Beta"],
            &[("Beta", "HASH"), ("Missing", "RANGE")],
            &[("Missing", "HASH"), ("Beta", "RANGE")],
        ),
    ];

    let mut first_case_message = String::new();
    for (ordinal, (case, declared, permuted, keys, turned)) in cases.iter().enumerate() {
        let mut stated: Vec<String> = Vec::new();
        for (variant, (declared, keys)) in [(declared, keys), (permuted, turned)]
            .into_iter()
            .enumerate()
        {
            let table = format!("undeclared-{ordinal}-{variant}");
            // Every declared attribute is used by some key schema, so the request is
            // refused for the index key with no DEFINITION and not for a definition no
            // key schema uses: the table keys on the first declared name, the index under
            // test on `keys`, and a second index on each remaining declared name.
            let mut indexes = vec![json!({
                "IndexName": "idx",
                "KeySchema": elements(keys),
                "Projection": {"ProjectionType": "ALL"},
            })];
            for (position, name) in declared.iter().enumerate().skip(1) {
                indexes.push(json!({
                    "IndexName": format!("used-{position}"),
                    "KeySchema": [key(name, "HASH")],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                }));
            }
            let (status, body) = call(
                &router,
                "CreateTable",
                json!({
                    "TableName": table,
                    "BillingMode": "PAY_PER_REQUEST",
                    "KeySchema": [key(declared[0], "HASH")],
                    "AttributeDefinitions": definitions(declared),
                    "GlobalSecondaryIndexes": indexes,
                }),
            )
            .await;
            let said = refusal(case, status, &body);
            assert!(
                said.starts_with(
                    "One or more parameter values were invalid: Some index key attributes are \
                     not defined in AttributeDefinitions. Keys: ["
                ),
                "case {case:?} variant {variant}: expected the parameter-value template, got \
                 {said:?}"
            );
            // Both lists carry exactly the names the request declared, whatever order
            // they are reported in.
            let (keys_reported, definitions_reported) = reported_lists(&said);
            let mut sorted_keys = keys_reported.clone();
            sorted_keys.sort();
            let mut expected_keys = named(keys);
            expected_keys.sort();
            let mut sorted_definitions = definitions_reported.clone();
            sorted_definitions.sort();
            let mut expected_definitions: Vec<String> =
                declared.iter().map(|name| (*name).to_string()).collect();
            expected_definitions.sort();
            assert_eq!(
                (sorted_keys, sorted_definitions),
                (expected_keys, expected_definitions),
                "case {case:?} variant {variant}: expected the index's whole key schema beside \
                 every declared name, got {said:?}"
            );
            stated.push(said);
        }
        assert_eq!(
            stated[0], stated[1],
            "case {case:?}: the same names declared in another order must be reported the same \
             way, got {stated:?}"
        );
        if ordinal == 0 {
            first_case_message = stated[0].clone();
        }
    }

    // The recorded order for the first case's names: neither list follows the request.
    assert_eq!(
        first_case_message,
        "One or more parameter values were invalid: Some index key attributes are not defined in \
         AttributeDefinitions. Keys: [SK, PK], AttributeDefinitions: [EXTRA, PK]",
        "case \"two declared names and a two-element index key\": expected the recorded order"
    );

    // The recorded order for a FIVE-name definition list, which is what settles that the
    // table the names are rendered out of grows with the list rather than having a fixed
    // size: declared [ContentId, TaskId, CreationTime, AssignedTo, Priority], reported
    // [CreationTime, AssignedTo, ContentId, TaskId, Priority]. Every declared name is
    // consumed by a key schema — the table's own and one index each — so the request is
    // refused for the index key with no DEFINITION.
    let (status, body) = call(
        &router,
        "CreateTable",
        json!({
            "TableName": "five-names",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [key("ContentId", "HASH")],
            "AttributeDefinitions": definitions(&[
                "ContentId", "TaskId", "CreationTime", "AssignedTo", "Priority",
            ]),
            "GlobalSecondaryIndexes": [
                {
                    "IndexName": "by-status",
                    "KeySchema": [key("TaskStatus", "HASH"), key("CreationTime", "RANGE")],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": "by-task",
                    "KeySchema": [key("TaskId", "HASH")],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                },
                {
                    "IndexName": "by-assignee",
                    "KeySchema": [key("AssignedTo", "HASH"), key("Priority", "RANGE")],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                },
            ],
        }),
    )
    .await;
    assert_eq!(
        refusal("five declared names", status, &body),
        "One or more parameter values were invalid: Some index key attributes are not defined in \
         AttributeDefinitions. Keys: [TaskStatus, CreationTime], AttributeDefinitions: \
         [CreationTime, AssignedTo, ContentId, TaskId, Priority]",
        "case \"five declared names\": expected the recorded order, got {body}"
    );
}

/// One vector index declaration of `dimensions` dimensions over `emb`.
fn vector_index(dimensions: i64) -> Value {
    json!({
        "IndexName": "vectors",
        "VectorAttribute": {"AttributeName": "emb"},
        "Dimensions": dimensions,
        "DistanceFunction": "COSINE",
        "Projection": {"ProjectionType": "ALL"},
    })
}

/// Rule 4: `Dimensions` is held to the published range with one sentence, wherever an
/// index is declared.
#[tokio::test]
async fn the_vector_dimension_range_is_stated_with_its_inclusive_bounds() {
    let router = emulator();

    // Rows differ along how far past the ceiling the request asks for.
    for (case, dimensions) in [
        ("one past the ceiling", 4097),
        ("far past the ceiling", 100_000),
        ("a dimension count no index could hold", 1_000_000_000),
    ] {
        let table = format!("dimensioned-{dimensions}");
        let (status, body) = call(
            &router,
            "CreateTable",
            json!({
                "TableName": table,
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [key("pk", "HASH")],
                "AttributeDefinitions": definitions(&["pk"]),
                "VectorIndexes": [vector_index(dimensions)],
            }),
        )
        .await;
        assert_eq!(
            refusal(case, status, &body),
            DIMENSION_RANGE,
            "case {case:?}: expected the dimension range's own sentence, got {body}"
        );
    }

    // Each published bound itself is accepted, and the stored index reports it.
    for (case, dimensions) in [("the floor of the range", 1), ("the ceiling itself", 4096)] {
        let table = format!("at-the-bound-{dimensions}");
        ok(
            &router,
            "CreateTable",
            json!({
                "TableName": table,
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [key("pk", "HASH")],
                "AttributeDefinitions": definitions(&["pk"]),
                "VectorIndexes": [vector_index(dimensions)],
            }),
        )
        .await;
        let described = describe_when_active(&router, case, &table).await;
        assert_eq!(
            described["VectorIndexes"][0]["Dimensions"],
            json!(dimensions),
            "case {case:?}: expected the index to report the dimensions it was created with, got \
             {described}"
        );
    }

    // The same sentence holds for an index declared by `UpdateTable`: it is one rule about
    // one member, not a rule of the operation that carried it.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "added-later",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [key("pk", "HASH")],
            "AttributeDefinitions": definitions(&["pk"]),
        }),
    )
    .await;
    describe_when_active(&router, "the table an index is added to", "added-later").await;
    let mut create = vector_index(4097);
    create["IndexName"] = json!("added");
    let (status, body) = call(
        &router,
        "UpdateTable",
        json!({"TableName": "added-later", "VectorIndexUpdates": [{"Create": create}]}),
    )
    .await;
    assert_eq!(
        refusal("an index added past the ceiling", status, &body),
        DIMENSION_RANGE,
        "case \"an index added past the ceiling\": expected the same sentence, got {body}"
    );
}

/// Rule 5: an on-demand create that states a provisioned throughput is refused by the two
/// capacity members.
#[tokio::test]
async fn an_on_demand_create_is_refused_by_the_two_capacity_members() {
    let router = emulator();

    // Rows differ along the capacity figures stated and the member the mode arrived in.
    let rows: [(&str, Value); 3] = [
        ("one unit of each", json!({"BillingMode": "PAY_PER_REQUEST",
                                    "ProvisionedThroughput": {"ReadCapacityUnits": 1,
                                                              "WriteCapacityUnits": 1}})),
        ("asymmetric capacity", json!({"BillingMode": "PAY_PER_REQUEST",
                                       "ProvisionedThroughput": {"ReadCapacityUnits": 500,
                                                                 "WriteCapacityUnits": 7}})),
        ("the mode in the throughput-mode member", json!({
            "TableThroughputMode": "PAY_PER_REQUEST",
            "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 3}})),
    ];

    for (ordinal, (case, members)) in rows.iter().enumerate() {
        let mut request = json!({
            "TableName": format!("on-demand-{ordinal}"),
            "KeySchema": [key("pk", "HASH")],
            "AttributeDefinitions": definitions(&["pk"]),
        });
        for (member, value) in members.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        let (status, body) = call(&router, "CreateTable", request).await;
        assert_eq!(
            refusal(case, status, &body),
            CAPACITY_ON_DEMAND,
            "case {case:?}: expected the two capacity members to be named, got {body}"
        );
    }

    // The control: the same throughput under the mode that provisions capacity is stored.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "provisioned",
            "BillingMode": "PROVISIONED",
            "KeySchema": [key("pk", "HASH")],
            "AttributeDefinitions": definitions(&["pk"]),
            "ProvisionedThroughput": {"ReadCapacityUnits": 4, "WriteCapacityUnits": 6},
        }),
    )
    .await;
    let described = describe_when_active(&router, "a provisioned table", "provisioned").await;
    assert_eq!(
        (
            described["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            described["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
        ),
        (json!(4), json!(6)),
        "case \"a provisioned table\": expected the capacity it was created with, got {described}"
    );
}

/// The dimensions the table under rule 6 indexes.
const DIMENSIONS: usize = 3;

/// Create a table whose vector index over `emb` holds `dimensions`-dimension vectors, and
/// wait for it to become `ACTIVE`.
async fn vector_table(router: &MultiServiceRouter, name: &str, dimensions: i64) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [key("pk", "HASH")],
            "AttributeDefinitions": definitions(&["pk"]),
            "VectorIndexes": [vector_index(dimensions)],
        }),
    )
    .await;
    describe_when_active(router, name, name).await;
}

/// A vector of `count` components.
fn vector_of(count: usize) -> Value {
    json!({"L": (0..count).map(|index| json!({"N": index.to_string()})).collect::<Vec<Value>>()})
}

/// Rule 6: the refusal names the attribute as the parameter at fault, with the position
/// and type of the element that is not a component or the counts that do not match.
#[tokio::test]
async fn a_vector_write_reports_the_parameter_and_the_element_at_fault() {
    let router = emulator();
    let table = "VectorParameters";
    vector_table(&router, table, DIMENSIONS as i64).await;

    let size = |actual: usize| {
        format!(
            "One or more parameter values were invalid. Invalid size for parameter emb, Expected: \
             {DIMENSIONS}, Actual: {actual} IndexName: vectors"
        )
    };
    let element = |position: usize, actual: &str| {
        format!(
            "One or more parameter values were invalid. Invalid type for parameter \
             emb[{position}], Expected: 32-bit floating point number, Actual: {actual}. \
             IndexName: vectors"
        )
    };
    let not_a_list = "One or more parameter values were invalid. Invalid type for parameter emb, \
                      Expected: 32-bit floating point number list IndexName: vectors"
        .to_string();

    // Rows differ along the number of components, the position and `AttributeValue` type
    // of a component that is not a 32-bit float, and the type of a value that is no list
    // at all. The last row carries BOTH defects: the type is judged first.
    let rows: [(&str, Value, String); 13] = [
        ("no components", json!({"L": []}), size(0)),
        ("one component", vector_of(1), size(1)),
        ("one component short", vector_of(DIMENSIONS - 1), size(2)),
        ("one component long", vector_of(DIMENSIONS + 1), size(4)),
        ("far more components", vector_of(9), size(9)),
        (
            "a string first",
            json!({"L": [{"S": "1"}, {"N": "2"}, {"N": "3"}]}),
            element(0, "S"),
        ),
        (
            "a boolean in the middle",
            json!({"L": [{"N": "1"}, {"BOOL": false}, {"N": "3"}]}),
            element(1, "BOOL"),
        ),
        (
            "a null last",
            json!({"L": [{"N": "1"}, {"N": "2"}, {"NULL": true}]}),
            element(2, "NULL"),
        ),
        (
            "a nested list",
            json!({"L": [{"N": "1"}, {"N": "2"}, {"L": [{"N": "3"}]}]}),
            element(2, "L"),
        ),
        (
            "a number set",
            json!({"L": [{"NS": ["1", "2"]}, {"N": "2"}, {"N": "3"}]}),
            element(0, "NS"),
        ),
        (
            "a component past the 32-bit float range",
            json!({"L": [{"N": "1"}, {"N": "1e40"}, {"N": "3"}]}),
            element(1, "N"),
        ),
        ("a string, not a list", json!({"S": "1,2,3"}), not_a_list),
        (
            "a one-element list of the wrong type",
            json!({"L": [{"S": "only"}]}),
            element(0, "S"),
        ),
    ];

    for (case, written, expected) in rows {
        let item = json!({"pk": {"S": case}, "emb": written});
        let (status, body) = call(
            &router,
            "PutItem",
            json!({"TableName": table, "Item": item}),
        )
        .await;
        assert_eq!(
            refusal(case, status, &body),
            expected,
            "case {case:?}: expected the parameter at fault to be reported, got {body}"
        );
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": table, "Key": {"pk": {"S": case}}}),
        )
        .await;
        assert!(
            read.get("Item").is_none_or(|item| !item.is_object()),
            "case {case:?}: a refused write must land nothing, got {read}"
        );
    }

    // The rule is the TABLE's, so the same sentence arrives on another write operation,
    // and a vector of the index's own shape is stored.
    let (status, body) = call(
        &router,
        "UpdateItem",
        json!({
            "TableName": table,
            "Key": {"pk": {"S": "updated"}},
            "UpdateExpression": "SET emb = :v",
            "ExpressionAttributeValues": {":v": json!({"L": [{"N": "1"}, {"S": "2"}, {"N": "3"}]})},
        }),
    )
    .await;
    assert_eq!(
        refusal("an update that writes a bad component", status, &body),
        element(1, "S"),
        "case \"an update that writes a bad component\": expected the same sentence, got {body}"
    );
    ok(
        &router,
        "PutItem",
        json!({"TableName": table, "Item": {"pk": {"S": "good"}, "emb": vector_of(DIMENSIONS)}}),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": table, "Key": {"pk": {"S": "good"}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["emb"],
        vector_of(DIMENSIONS),
        "case \"the index's own shape\": expected the vector stored as written, got {read}"
    );
}

/// Rule 7: the metered bytes follow the index's `Dimensions`, the key the entry is held
/// under and the per-write floor — and not the size of the item that carries the vector.
#[tokio::test]
async fn a_vector_write_meters_the_vector_and_not_the_item_around_it() {
    let router = emulator();

    // Rows differ along the index's Dimensions: well below the floor, around the
    // dimension count the floor covers, and far past it.
    for (case, dimensions) in [
        ("four dimensions, far below the floor", 4_i64),
        ("the dimensions the floor nearly covers", 256),
        ("one dimension more", 257),
        ("many dimensions", 2_000),
    ] {
        let table = format!("metered-{dimensions}");
        vector_table(&router, &table, dimensions).await;
        let vector = vector_of(usize::try_from(dimensions).unwrap_or_default());

        // The same vector under keys of two lengths, and once in an item carrying 8 KB
        // of other attributes: the entry holds the key and the vector, so the key's
        // length moves the figure and the rest of the item does not.
        for (shape, key_value, payload) in [
            ("a short key", "s", None),
            ("a longer key", "a-much-longer-item-key", None),
            (
                "an item of several kilobytes beside the vector",
                "payloaded",
                Some(8_192),
            ),
        ] {
            let mut item = json!({"pk": {"S": key_value}, "emb": vector.clone()});
            if let Some(bytes) = payload {
                item["payload"] = json!({"S": "x".repeat(bytes)});
            }
            // The entry sized the way the Developer Guide sizes an item ("Item sizes and
            // formats": each attribute's name plus its value bytes) — the key attribute
            // `pk` and its value, the vector attribute's name `emb`, and four bytes per
            // stored dimension — never less than the per-write floor.
            let expected = ((dimensions * 4) + 2 + key_value.len() as i64 + 3).max(1024) as f64;
            let written = ok(
                &router,
                "PutItem",
                json!({"TableName": table, "Item": item, "ReturnConsumedCapacity": "INDEXES"}),
            )
            .await;
            let metered = written["ConsumedCapacity"]["VectorIndexes"]["vectors"]
                ["VectorWriteRequestBytes"]
                .as_f64();
            assert_eq!(
                metered,
                Some(expected),
                "case {case:?} with {shape}: expected {expected} bytes (the key plus four per \
                 dimension, never below the floor), got {written}"
            );
        }
    }

    // The vector ATTRIBUTE belongs to the entry too: two indexes of the same dimensions
    // over attributes whose names differ in length meter figures differing by exactly
    // that difference.
    let mut by_attribute: Vec<f64> = Vec::new();
    for attribute in ["emb", "embedding"] {
        let table = format!("named-{attribute}");
        let mut index = vector_index(300);
        index["VectorAttribute"] = json!({"AttributeName": attribute});
        ok(
            &router,
            "CreateTable",
            json!({
                "TableName": table,
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [key("pk", "HASH")],
                "AttributeDefinitions": definitions(&["pk"]),
                "VectorIndexes": [index],
            }),
        )
        .await;
        describe_when_active(&router, attribute, &table).await;
        let mut item = json!({"pk": {"S": "k"}});
        item[attribute] = vector_of(300);
        let written = ok(
            &router,
            "PutItem",
            json!({"TableName": table, "Item": item, "ReturnConsumedCapacity": "INDEXES"}),
        )
        .await;
        let metered = written["ConsumedCapacity"]["VectorIndexes"]["vectors"]
            ["VectorWriteRequestBytes"]
            .as_f64();
        let expected = (300 * 4 + 2 + 1 + attribute.len() as i64) as f64;
        assert_eq!(
            metered,
            Some(expected),
            "case {attribute:?}: expected {expected} bytes (the key, the attribute's name and \
             four per dimension), got {written}"
        );
        by_attribute.push(metered.unwrap_or_default());
    }
    assert_eq!(
        by_attribute[1] - by_attribute[0],
        6.0,
        "case \"two attribute names\": expected the figures to differ by the names' length \
         difference, got {by_attribute:?}"
    );
}

/// Rule 8: a vector index spends from the table's projected-attribute budget, its own
/// vector attribute and each `INLINE_FILTER` search-schema element included.
#[tokio::test]
async fn a_vector_index_spends_its_own_attributes_from_the_projection_budget() {
    let router = emulator();
    // The budget every secondary index of a table shares, and the most names ONE index's
    // projection may carry (`Projection$NonKeyAttributes`, a list of length 1..20), which
    // is why the budget is reached with several indexes rather than one.
    const BUDGET: usize = 100;
    const PER_INDEX: usize = 20;

    // Each row: how many vector indexes the table declares, how many names each of them
    // projects, how many a global index beside them projects, and whether the first vector
    // index also carries an `INLINE_FILTER` element. Rows sit at the budget and one past
    // it, with the share that tips it moving between the vector attributes, the global
    // index and the inline filter.
    let rows: [(&str, usize, usize, usize, bool); 5] = [
        (
            "five vector indexes projecting the per-index maximum each",
            5,
            PER_INDEX,
            0,
            false,
        ),
        ("four of the same", 4, PER_INDEX, 0, false),
        (
            "four vector indexes and a global index exactly at the budget",
            4,
            PER_INDEX,
            16,
            false,
        ),
        (
            "the same with one projected name more",
            4,
            PER_INDEX,
            17,
            false,
        ),
        (
            "an inline filter taking the share past the budget",
            4,
            PER_INDEX,
            16,
            true,
        ),
    ];

    for (ordinal, (case, vector_indexes, names_each, index_names, inline)) in
        rows.iter().enumerate()
    {
        let table = format!("projected-{ordinal}");
        let projected = |count: usize, prefix: &str| -> Vec<String> {
            (0..count).map(|index| format!("{prefix}{index}")).collect()
        };
        let mut vectors: Vec<Value> = Vec::new();
        for position in 0..*vector_indexes {
            let mut index = vector_index(4);
            index["IndexName"] = json!(format!("vectors-{position}"));
            index["VectorAttribute"] = json!({"AttributeName": format!("emb{position}")});
            index["Projection"] = json!({
                "ProjectionType": "INCLUDE",
                "NonKeyAttributes": projected(*names_each, &format!("v{position}-")),
            });
            if *inline && position == 0 {
                index["SearchSchema"] =
                    json!([{"AttributeName": "tier", "SearchSchemaElementType": "INLINE_FILTER"}]);
            }
            vectors.push(index);
        }
        let mut declared: Vec<&str> = vec!["pk"];
        if *inline {
            declared.push("tier");
        }
        if *index_names > 0 {
            declared.push("gsi");
        }
        let mut request = json!({
            "TableName": table,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [key("pk", "HASH")],
            "AttributeDefinitions": definitions(&declared),
            "VectorIndexes": vectors,
        });
        if *index_names > 0 {
            request["GlobalSecondaryIndexes"] = json!([{
                "IndexName": "idx",
                "KeySchema": [key("gsi", "HASH")],
                "Projection": {"ProjectionType": "INCLUDE",
                               "NonKeyAttributes": projected(*index_names, "g")},
            }]);
        }
        // Each vector index spends its projected names plus one for the vector attribute,
        // and one more for each INLINE_FILTER element.
        let spent =
            vector_indexes * (names_each + 1) + index_names + usize::from(*inline);
        let (status, body) = call(&router, "CreateTable", request).await;
        match spent > BUDGET {
            true => assert_eq!(
                refusal(case, status, &body),
                format!(
                    "One or more parameter values were invalid: Number of projected attributes \
                     in all indexes exceeds limit of {BUDGET}, number of projected \
                     attributes:{spent}"
                ),
                "case {case:?}: expected the budget to be stated with what the table spends, \
                 got {body}"
            ),
            false => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: {spent} projected attributes are within the budget, got \
                     {status} with {body}"
                );
                let described = describe_when_active(&router, case, &table).await;
                assert_eq!(
                    described["VectorIndexes"]
                        .as_array()
                        .map(|indexes| indexes.len()),
                    Some(*vector_indexes),
                    "case {case:?}: expected every vector index stored, got {described}"
                );
            }
        }
    }
}
