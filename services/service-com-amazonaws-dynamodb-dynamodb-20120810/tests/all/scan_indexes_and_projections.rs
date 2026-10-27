// SPDX-License-Identifier: CC0-1.0

//! A `Scan` reads what the request pointed it AT, and reports of each item only the
//! attributes the request asked for.
//!
//! The rules these suites pin:
//!
//! * a scan naming an `IndexName` is served from that secondary index — "accessing every
//!   item in a table or a secondary index" — so it evaluates exactly the items that index
//!   covers and reports of each the attributes that index projects;
//! * the attributes a reported item carries are the request's projection
//!   (`ProjectionExpression` with its `ExpressionAttributeNames` aliases, or the legacy
//!   `AttributesToGet`), applied AFTER the page was read and filtered, so neither
//!   `ScannedCount` nor `ConsumedCapacity` moves with it;
//! * the legacy `ScanFilter`/`ConditionalOperator` pair selects exactly the items the
//!   equivalent `FilterExpression` selects;
//! * a strongly consistent read of a GLOBAL secondary index is a client error, while the
//!   same scan without the flag, the same scan of a local index, and a consistent scan of
//!   the base table are all served.
//!
//! Each suite checks its rule over a SET of inputs: every index kind and projection type
//! the table declares, every `Limit` from one to past the index's entry count, a table of
//! projection forms, and a table of legacy comparisons paired with their expression
//! equivalents.

use serde_json::{json, Map, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    MAX_IN_OPERANDS, MAX_LEGACY_CONDITIONS,
};

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};
use crate::table_scan::{last_evaluated_key, rendered, reported_items};

use smithy_server_runtime::MultiServiceRouter;

/// The table every suite here scans. One table name per emulator, and each suite builds
/// its own emulator, so the name is a constant.
const TABLE: &str = "ScanIndexed";

/// The items every suite seeds, chosen so that each index the table declares covers a
/// DIFFERENT subset of them: one carries no `status` (so no status index covers it), one
/// no `priority`, two no `assignee`, and only one an `archived` flag — which is what
/// makes the `archived` index sparse.
fn stored_items() -> Vec<Value> {
    vec![
        json!({"pk": {"S": "t1"}, "sk": {"S": "a"}, "status": {"S": "OPEN"},
               "priority": {"N": "1"}, "assignee": {"S": "ann"}, "note": {"S": "first"}}),
        json!({"pk": {"S": "t1"}, "sk": {"S": "b"}, "status": {"S": "OPEN"},
               "priority": {"N": "2"}, "assignee": {"S": "bob"}, "archived": {"S": "no"},
               "note": {"S": "second"}}),
        json!({"pk": {"S": "t2"}, "sk": {"S": "a"}, "status": {"S": "DONE"},
               "priority": {"N": "1"}, "note": {"S": "third"}}),
        json!({"pk": {"S": "t2"}, "sk": {"S": "b"}, "status": {"S": "DONE"},
               "assignee": {"S": "ann"}, "note": {"S": "fourth"}}),
        json!({"pk": {"S": "t3"}, "sk": {"S": "a"}, "note": {"S": "fifth"}}),
    ]
}

/// Create [`TABLE`] with one index of every kind and projection type the service serves,
/// wait out the `CREATING` -> `ACTIVE` transition, write every item of
/// [`stored_items`], and answer the items written.
async fn seeded(router: &MultiServiceRouter) -> Vec<Value> {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "status", "AttributeType": "S"},
                {"AttributeName": "priority", "AttributeType": "N"},
                {"AttributeName": "assignee", "AttributeType": "S"},
                {"AttributeName": "archived", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [
                {
                    "IndexName": "ByStatus",
                    "KeySchema": [{"AttributeName": "status", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": "ByStatusPriority",
                    "KeySchema": [
                        {"AttributeName": "status", "KeyType": "HASH"},
                        {"AttributeName": "priority", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                },
                {
                    "IndexName": "ByArchived",
                    "KeySchema": [{"AttributeName": "archived", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "INCLUDE",
                                   "NonKeyAttributes": ["assignee"]},
                },
            ],
            "LocalSecondaryIndexes": [
                {
                    "IndexName": "ByAssignee",
                    "KeySchema": [
                        {"AttributeName": "pk", "KeyType": "HASH"},
                        {"AttributeName": "assignee", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                },
            ],
        }),
    )
    .await;
    describe_when_active(router, "seed the indexed table", TABLE).await;
    let items = stored_items();
    for item in &items {
        ok(router, "PutItem", json!({"TableName": TABLE, "Item": item})).await;
    }
    items
}

/// One index to scan: the entries it holds, written out, and the attribute names a read
/// of it reports as `LastEvaluatedKey`.
struct IndexCase {
    case: &'static str,
    index: &'static str,
    /// Every entry the index holds, as the scan must report it. Written out rather than
    /// derived, from "KEYS_ONLY - Only the index and primary key attributes are projected
    /// ... INCLUDE - ... the attributes that are projected are those in NonKeyAttributes
    /// ... ALL - All of the table attributes are projected into the index"
    /// (`Projection$ProjectionType`), over the items of [`stored_items`] that carry every
    /// attribute of the index's key schema.
    entries: Vec<Value>,
    /// The index's own key attributes and the table's, which together address one entry
    /// of a secondary index.
    key_names: &'static [&'static str],
}

/// Every index the seeded table declares: both kinds, and each of the three projection
/// types, with one of them sparse and one keyed by a composite index key.
fn index_cases() -> Vec<IndexCase> {
    vec![
        IndexCase {
            case: "a global index projecting ALL",
            index: "ByStatus",
            entries: stored_items()
                .into_iter()
                .filter(|item| item.get("status").is_some())
                .collect(),
            key_names: &["status", "pk", "sk"],
        },
        IndexCase {
            case: "a global index with a composite key projecting KEYS_ONLY",
            index: "ByStatusPriority",
            entries: vec![
                json!({"status": {"S": "OPEN"}, "priority": {"N": "1"},
                       "pk": {"S": "t1"}, "sk": {"S": "a"}}),
                json!({"status": {"S": "OPEN"}, "priority": {"N": "2"},
                       "pk": {"S": "t1"}, "sk": {"S": "b"}}),
                json!({"status": {"S": "DONE"}, "priority": {"N": "1"},
                       "pk": {"S": "t2"}, "sk": {"S": "a"}}),
            ],
            key_names: &["status", "priority", "pk", "sk"],
        },
        IndexCase {
            case: "a sparse global index projecting INCLUDE",
            index: "ByArchived",
            entries: vec![json!({"archived": {"S": "no"}, "assignee": {"S": "bob"},
                                 "pk": {"S": "t1"}, "sk": {"S": "b"}})],
            key_names: &["archived", "pk", "sk"],
        },
        IndexCase {
            case: "a local index projecting KEYS_ONLY",
            index: "ByAssignee",
            entries: vec![
                json!({"pk": {"S": "t1"}, "assignee": {"S": "ann"}, "sk": {"S": "a"}}),
                json!({"pk": {"S": "t1"}, "assignee": {"S": "bob"}, "sk": {"S": "b"}}),
                json!({"pk": {"S": "t2"}, "assignee": {"S": "ann"}, "sk": {"S": "b"}}),
            ],
            key_names: &["pk", "assignee", "sk"],
        },
    ]
}

/// One item narrowed to the attributes a projection named: "If any of the requested
/// attributes are not found, they will not appear in the result"
/// (`Scan$ProjectionExpression`), so a name the item does not carry contributes nothing.
fn narrowed(item: &Value, names: &[&str]) -> Value {
    let mut narrowed = Map::new();
    for name in names {
        if let Some(value) = item.get(*name) {
            narrowed.insert((*name).to_string(), value.clone());
        }
    }
    Value::Object(narrowed)
}

/// The `note` of every item a response reported, sorted — the handle each row below names
/// its expected selection by.
fn reported_notes(response: &Value) -> Vec<String> {
    let mut notes: Vec<String> = response["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["note"]["S"].as_str().unwrap_or_default().to_string())
        .collect();
    notes.sort();
    notes
}

/// The same list from a row's expectation.
fn expected_notes(notes: &[&str]) -> Vec<String> {
    let mut expected: Vec<String> = notes.iter().map(|note| (*note).to_string()).collect();
    expected.sort();
    expected
}

#[tokio::test]
async fn an_index_scan_reports_the_entries_that_index_holds() {
    let router = emulator();
    let items = seeded(&router).await;

    // The base-table scan is the control: every index below reports a NARROWER set, so a
    // handler that quietly scanned the table instead would fail here.
    let base = ok(&router, "Scan", json!({"TableName": TABLE})).await;
    assert_eq!(
        reported_items(&base),
        rendered(&items),
        "case base table: a scan of the table reports every item it holds, got {base}"
    );

    for case in index_cases() {
        let scanned = ok(
            &router,
            "Scan",
            json!({"TableName": TABLE, "IndexName": case.index}),
        )
        .await;
        assert_eq!(
            reported_items(&scanned),
            rendered(&case.entries),
            "case {:?}: the scan reports the entries the index holds, got {scanned}",
            case.case
        );
        assert_eq!(
            scanned["Count"],
            json!(case.entries.len()),
            "case {:?}: Count is the number of entries answered, got {scanned}",
            case.case
        );
        assert_eq!(
            scanned["ScannedCount"], scanned["Count"],
            "case {:?}: with no filter ScannedCount equals Count, got {scanned}",
            case.case
        );
        assert!(
            case.entries.len() < items.len(),
            "case {:?}: the fixture must leave at least one item out of the index, got {} \
             of {}",
            case.case,
            case.entries.len(),
            items.len()
        );
    }
}

#[tokio::test]
async fn an_index_scan_pages_the_index_exactly_once() {
    let router = emulator();
    seeded(&router).await;

    for case in index_cases() {
        // Every `Limit` from one entry to past the whole index, which is the range the
        // paging invariant has to hold over.
        for limit in 1..=case.entries.len() + 1 {
            let mut seen: Vec<String> = Vec::new();
            let mut start: Option<Value> = None;
            for _ in 0..case.entries.len() + 2 {
                let mut request =
                    json!({"TableName": TABLE, "IndexName": case.index, "Limit": limit});
                if let Some(key) = &start {
                    request["ExclusiveStartKey"] = key.clone();
                }
                let page = ok(&router, "Scan", request).await;
                seen.extend(reported_items(&page));
                match last_evaluated_key(&page) {
                    Some(key) => {
                        let mut names: Vec<String> = key
                            .as_object()
                            .cloned()
                            .unwrap_or_default()
                            .keys()
                            .cloned()
                            .collect();
                        names.sort();
                        let mut expected: Vec<String> =
                            case.key_names.iter().map(|n| (*n).to_string()).collect();
                        expected.sort();
                        assert_eq!(
                            names, expected,
                            "case {:?} limit {limit}: the key an index page reports names \
                             the index's key attributes and the table's, got {key}",
                            case.case
                        );
                        start = Some(key);
                    }
                    None => break,
                }
            }
            seen.sort();
            assert_eq!(
                seen,
                rendered(&case.entries),
                "case {:?} limit {limit}: the pages together report every entry exactly \
                 once, got {seen:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_projection_reports_only_the_attributes_it_names() {
    let router = emulator();
    let items = seeded(&router).await;

    // What the same scan reports and charges with NO projection: the projection is
    // "applied after a Scan finishes", so neither figure may move with it.
    let whole = ok(
        &router,
        "Scan",
        json!({"TableName": TABLE, "ReturnConsumedCapacity": "TOTAL"}),
    )
    .await;

    // Each row: the case, the members that state the projection, and the attribute names
    // every reported item must then be narrowed to.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a ProjectionExpression naming the partition key",
            json!({"ProjectionExpression": "pk"}),
            vec!["pk"],
        ),
        (
            "a ProjectionExpression naming both key attributes",
            json!({"ProjectionExpression": "pk, sk"}),
            vec!["pk", "sk"],
        ),
        (
            "a ProjectionExpression aliasing the reserved word status",
            json!({"ProjectionExpression": "#s", "ExpressionAttributeNames": {"#s": "status"}}),
            vec!["status"],
        ),
        (
            "a ProjectionExpression mixing an alias and a written-out name",
            json!({"ProjectionExpression": "pk, #s",
                   "ExpressionAttributeNames": {"#s": "status"}}),
            vec!["pk", "status"],
        ),
        (
            "a ProjectionExpression naming an attribute no item carries",
            json!({"ProjectionExpression": "pk, nowhere"}),
            vec!["pk", "nowhere"],
        ),
        (
            "the legacy AttributesToGet",
            json!({"AttributesToGet": ["pk", "note"]}),
            vec!["pk", "note"],
        ),
        (
            "Select SPECIFIC_ATTRIBUTES beside the projection",
            json!({"Select": "SPECIFIC_ATTRIBUTES", "ProjectionExpression": "note"}),
            vec!["note"],
        ),
    ];

    for (case, members, names) in cases {
        let mut request = json!({"TableName": TABLE, "ReturnConsumedCapacity": "TOTAL"});
        for (member, value) in members.as_object().expect("the row states members") {
            request[member.clone()] = value.clone();
        }
        let scanned = ok(&router, "Scan", request).await;
        let expected: Vec<Value> = items.iter().map(|item| narrowed(item, &names)).collect();
        assert_eq!(
            reported_items(&scanned),
            rendered(&expected),
            "case {case:?}: each item carries only the projected attributes, got {scanned}"
        );
        assert_eq!(
            scanned["ScannedCount"],
            json!(items.len()),
            "case {case:?}: the projection is applied after the page is read, so every \
             item is still evaluated, got {scanned}"
        );
        assert_eq!(
            scanned["Count"], scanned["ScannedCount"],
            "case {case:?}: with no filter Count equals ScannedCount, got {scanned}"
        );
        assert_eq!(
            scanned["ConsumedCapacity"]["CapacityUnits"],
            whole["ConsumedCapacity"]["CapacityUnits"],
            "case {case:?}: the projection costs what the unprojected scan costs, got \
             {scanned}"
        );
    }
}

#[tokio::test]
async fn the_legacy_scan_filter_selects_what_the_expression_selects() {
    let router = emulator();
    let items = seeded(&router).await;

    // Each row: the case, the legacy `ScanFilter` members, the `FilterExpression`
    // members that state the same filter, and the `note` of every item both must select.
    // The rows span the comparison vocabulary the legacy surface has — the existence
    // pair, equality and its negation, the orderings, the range, the membership test and
    // the string and set tests — and both ways its entries combine.
    let cases: Vec<(&str, Value, Value, Vec<&str>)> = vec![
        (
            "NOT_NULL and attribute_exists",
            json!({"ScanFilter": {"archived": {"ComparisonOperator": "NOT_NULL"}}}),
            json!({"FilterExpression": "attribute_exists(archived)"}),
            vec!["second"],
        ),
        (
            "NULL and attribute_not_exists",
            json!({"ScanFilter": {"archived": {"ComparisonOperator": "NULL"}}}),
            json!({"FilterExpression": "attribute_not_exists(archived)"}),
            vec!["first", "third", "fourth", "fifth"],
        ),
        (
            "EQ and =",
            json!({"ScanFilter": {"status": {"ComparisonOperator": "EQ",
                                             "AttributeValueList": [{"S": "OPEN"}]}}}),
            json!({"FilterExpression": "#s = :v",
                   "ExpressionAttributeNames": {"#s": "status"},
                   "ExpressionAttributeValues": {":v": {"S": "OPEN"}}}),
            vec!["first", "second"],
        ),
        (
            "NE and <>",
            json!({"ScanFilter": {"status": {"ComparisonOperator": "NE",
                                             "AttributeValueList": [{"S": "OPEN"}]}}}),
            json!({"FilterExpression": "#s <> :v",
                   "ExpressionAttributeNames": {"#s": "status"},
                   "ExpressionAttributeValues": {":v": {"S": "OPEN"}}}),
            vec!["third", "fourth"],
        ),
        (
            "GT and >",
            json!({"ScanFilter": {"priority": {"ComparisonOperator": "GT",
                                               "AttributeValueList": [{"N": "1"}]}}}),
            json!({"FilterExpression": "priority > :v",
                   "ExpressionAttributeValues": {":v": {"N": "1"}}}),
            vec!["second"],
        ),
        (
            "LE and <=",
            json!({"ScanFilter": {"priority": {"ComparisonOperator": "LE",
                                               "AttributeValueList": [{"N": "1"}]}}}),
            json!({"FilterExpression": "priority <= :v",
                   "ExpressionAttributeValues": {":v": {"N": "1"}}}),
            vec!["first", "third"],
        ),
        (
            "BETWEEN and BETWEEN",
            json!({"ScanFilter": {"priority": {"ComparisonOperator": "BETWEEN",
                                               "AttributeValueList": [{"N": "1"},
                                                                      {"N": "2"}]}}}),
            json!({"FilterExpression": "priority BETWEEN :lo AND :hi",
                   "ExpressionAttributeValues": {":lo": {"N": "1"}, ":hi": {"N": "2"}}}),
            vec!["first", "second", "third"],
        ),
        (
            "IN and IN",
            json!({"ScanFilter": {"status": {"ComparisonOperator": "IN",
                                             "AttributeValueList": [{"S": "OPEN"},
                                                                    {"S": "DONE"}]}}}),
            json!({"FilterExpression": "#s IN (:a, :b)",
                   "ExpressionAttributeNames": {"#s": "status"},
                   "ExpressionAttributeValues": {":a": {"S": "OPEN"}, ":b": {"S": "DONE"}}}),
            vec!["first", "second", "third", "fourth"],
        ),
        (
            "BEGINS_WITH and begins_with",
            json!({"ScanFilter": {"note": {"ComparisonOperator": "BEGINS_WITH",
                                           "AttributeValueList": [{"S": "f"}]}}}),
            json!({"FilterExpression": "begins_with(note, :v)",
                   "ExpressionAttributeValues": {":v": {"S": "f"}}}),
            vec!["first", "fourth", "fifth"],
        ),
        (
            "CONTAINS and contains",
            json!({"ScanFilter": {"note": {"ComparisonOperator": "CONTAINS",
                                           "AttributeValueList": [{"S": "ir"}]}}}),
            json!({"FilterExpression": "contains(note, :v)",
                   "ExpressionAttributeValues": {":v": {"S": "ir"}}}),
            vec!["first", "third"],
        ),
        (
            "two entries combined by the default AND",
            json!({"ScanFilter": {
                       "status": {"ComparisonOperator": "EQ",
                                  "AttributeValueList": [{"S": "OPEN"}]},
                       "priority": {"ComparisonOperator": "EQ",
                                    "AttributeValueList": [{"N": "2"}]}}}),
            json!({"FilterExpression": "#s = :s AND priority = :p",
                   "ExpressionAttributeNames": {"#s": "status"},
                   "ExpressionAttributeValues": {":s": {"S": "OPEN"}, ":p": {"N": "2"}}}),
            vec!["second"],
        ),
        (
            "two entries combined by ConditionalOperator OR",
            json!({"ConditionalOperator": "OR",
                   "ScanFilter": {
                       "status": {"ComparisonOperator": "EQ",
                                  "AttributeValueList": [{"S": "DONE"}]},
                       "archived": {"ComparisonOperator": "NOT_NULL"}}}),
            json!({"FilterExpression": "#s = :s OR attribute_exists(archived)",
                   "ExpressionAttributeNames": {"#s": "status"},
                   "ExpressionAttributeValues": {":s": {"S": "DONE"}}}),
            vec!["second", "third", "fourth"],
        ),
    ];

    for (case, legacy_members, expression_members, selected) in cases {
        let scan = |members: &Value| {
            let mut request = json!({"TableName": TABLE});
            for (member, value) in members.as_object().expect("the row states members") {
                request[member.clone()] = value.clone();
            }
            request
        };
        let legacy = ok(&router, "Scan", scan(&legacy_members)).await;
        let expression = ok(&router, "Scan", scan(&expression_members)).await;
        assert_eq!(
            reported_notes(&legacy),
            expected_notes(&selected),
            "case {case:?}: the legacy filter selects the stated items, got {legacy}"
        );
        assert_eq!(
            reported_items(&legacy),
            reported_items(&expression),
            "case {case:?}: the legacy filter and the expression select the same items, \
             got {legacy} and {expression}"
        );
        assert_eq!(
            legacy["Count"], expression["Count"],
            "case {case:?}: both forms report the same Count, got {legacy} and {expression}"
        );
        assert_eq!(
            legacy["ScannedCount"],
            json!(items.len()),
            "case {case:?}: a filter is applied after the page is read, so every item is \
             still evaluated, got {legacy}"
        );
    }
}

#[tokio::test]
async fn read_consistency_and_select_over_a_secondary_index() {
    let router = emulator();
    let items = seeded(&router).await;

    // Each row: the case, the request, the HTTP status it must be answered with, and the
    // wire error code (empty where the row must succeed). The rows span the consistency
    // contract on both index kinds and the base table, and each `Select` an index scan
    // does and does not accept.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "ConsistentRead on a global secondary index",
            json!({"TableName": TABLE, "IndexName": "ByStatus", "ConsistentRead": true}),
            400,
            "ValidationException",
        ),
        (
            "the same global index scan without ConsistentRead",
            json!({"TableName": TABLE, "IndexName": "ByStatus", "ConsistentRead": false}),
            200,
            "",
        ),
        (
            "ConsistentRead on a local secondary index",
            json!({"TableName": TABLE, "IndexName": "ByAssignee", "ConsistentRead": true}),
            200,
            "",
        ),
        (
            "ConsistentRead on the base table",
            json!({"TableName": TABLE, "ConsistentRead": true}),
            200,
            "",
        ),
        (
            "Select ALL_ATTRIBUTES on a global index projecting KEYS_ONLY",
            json!({"TableName": TABLE, "IndexName": "ByStatusPriority",
                   "Select": "ALL_ATTRIBUTES"}),
            400,
            "ValidationException",
        ),
        (
            "Select ALL_ATTRIBUTES on a global index projecting INCLUDE",
            json!({"TableName": TABLE, "IndexName": "ByArchived",
                   "Select": "ALL_ATTRIBUTES"}),
            400,
            "ValidationException",
        ),
        (
            "Select ALL_ATTRIBUTES on a global index projecting ALL",
            json!({"TableName": TABLE, "IndexName": "ByStatus", "Select": "ALL_ATTRIBUTES"}),
            200,
            "",
        ),
        (
            "Select ALL_ATTRIBUTES on a local index projecting KEYS_ONLY",
            json!({"TableName": TABLE, "IndexName": "ByAssignee",
                   "Select": "ALL_ATTRIBUTES"}),
            200,
            "",
        ),
        (
            "Select ALL_PROJECTED_ATTRIBUTES on an index",
            json!({"TableName": TABLE, "IndexName": "ByAssignee",
                   "Select": "ALL_PROJECTED_ATTRIBUTES"}),
            200,
            "",
        ),
    ];

    for (case, request, status, code) in cases {
        let (actual, body) = call(&router, "Scan", request).await;
        assert_eq!(
            actual, status,
            "case {case:?}: expected HTTP {status}, got {actual} with {body}"
        );
        if code.is_empty() {
            continue;
        }
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the error code {code}, got {body}"
        );
        assert!(
            body.get("Items").is_none_or(Value::is_null),
            "case {case:?}: a refused scan reports no items, got {body}"
        );
    }

    // The refusal of a consistent read of a global index states the rule it broke.
    let (_, refused) = call(
        &router,
        "Scan",
        json!({"TableName": TABLE, "IndexName": "ByStatus", "ConsistentRead": true}),
    )
    .await;
    let stated = message(&refused);
    assert!(
        stated.contains("global secondary indexes"),
        "case the consistency refusal: the message names the index kind that does not \
         support it, got {stated:?}"
    );

    // "If any of the requested attributes are not projected into the local secondary
    // index, DynamoDB fetches each of these attributes from the parent table": a local
    // index that projects only its keys still answers ALL_ATTRIBUTES with whole items,
    // while ALL_PROJECTED_ATTRIBUTES answers with what the index holds.
    let local = index_cases()
        .into_iter()
        .find(|case| case.index == "ByAssignee")
        .expect("the local index is one of the cases");
    let whole = ok(
        &router,
        "Scan",
        json!({"TableName": TABLE, "IndexName": "ByAssignee", "Select": "ALL_ATTRIBUTES"}),
    )
    .await;
    let fetched: Vec<Value> = items
        .iter()
        .filter(|item| item.get("assignee").is_some())
        .cloned()
        .collect();
    assert_eq!(
        reported_items(&whole),
        rendered(&fetched),
        "case ALL_ATTRIBUTES on a local index: the unprojected attributes are fetched \
         from the parent table, got {whole}"
    );
    let projected = ok(
        &router,
        "Scan",
        json!({"TableName": TABLE, "IndexName": "ByAssignee",
               "Select": "ALL_PROJECTED_ATTRIBUTES"}),
    )
    .await;
    assert_eq!(
        reported_items(&projected),
        rendered(&local.entries),
        "case ALL_PROJECTED_ATTRIBUTES on a local index: only the projected attributes \
         are reported, got {projected}"
    );

    // "COUNT - Returns the number of matching items, rather than the matching items
    // themselves", counted over the index's entries.
    let counted = ok(
        &router,
        "Scan",
        json!({"TableName": TABLE, "IndexName": "ByStatusPriority", "Select": "COUNT"}),
    )
    .await;
    assert_eq!(
        counted["Count"],
        json!(3),
        "case COUNT on an index: the count is over the entries the index holds, got \
         {counted}"
    );
    assert!(
        counted.get("Items").is_none_or(Value::is_null),
        "case COUNT on an index: the count is reported rather than the entries, got \
         {counted}"
    );

    // A projection over an index reports only the names it asked for, out of what the
    // index holds.
    let projected = ok(
        &router,
        "Scan",
        json!({"TableName": TABLE, "IndexName": "ByStatus", "ProjectionExpression": "pk, #s",
               "ExpressionAttributeNames": {"#s": "status"}}),
    )
    .await;
    let expected: Vec<Value> = items
        .iter()
        .filter(|item| item.get("status").is_some())
        .map(|item| narrowed(item, &["pk", "status"]))
        .collect();
    assert_eq!(
        reported_items(&projected),
        rendered(&expected),
        "case a projection over an index: only the projected names are reported, got \
         {projected}"
    );
}

#[tokio::test]
async fn the_legacy_filter_surface_is_bounded() {
    let router = emulator();
    let items = seeded(&router).await;

    // The ceilings the service holds the legacy surface to, read from the service
    // itself so the rows cannot drift from the constants they are about: how many
    // entries one condition map may state, and how many values one `IN` entry may list.
    let conditions = |count: usize| {
        let mut map = Map::new();
        for at in 0..count {
            map.insert(
                format!("attribute{at}"),
                json!({"ComparisonOperator": "NULL"}),
            );
        }
        Value::Object(map)
    };
    let in_values = |count: usize| {
        let values: Vec<Value> = (0..count).map(|at| json!({"S": format!("v{at}")})).collect();
        json!({"note": {"ComparisonOperator": "IN", "AttributeValueList": values}})
    };

    // Each row: the case, the `ScanFilter` it sends, and the HTTP status it must be
    // answered with — each published bound, and one past it.
    let cases: Vec<(&str, Value, u16)> = vec![
        ("one condition", conditions(1), 200),
        ("as many conditions as the ceiling", conditions(MAX_LEGACY_CONDITIONS), 200),
        ("one condition past the ceiling", conditions(MAX_LEGACY_CONDITIONS + 1), 400),
        ("an IN entry with one value", in_values(1), 200),
        ("an IN entry at the operand ceiling", in_values(MAX_IN_OPERANDS), 200),
        ("an IN entry one operand past the ceiling", in_values(MAX_IN_OPERANDS + 1), 400),
        (
            "an IN entry with no values at all",
            json!({"note": {"ComparisonOperator": "IN", "AttributeValueList": []}}),
            400,
        ),
    ];
    for (case, filter, status) in cases {
        let (actual, body) = call(
            &router,
            "Scan",
            json!({"TableName": TABLE, "ScanFilter": filter}),
        )
        .await;
        assert_eq!(
            actual, status,
            "case {case:?}: expected HTTP {status}, got {actual} with {body}"
        );
        if status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: a filter past a ceiling is the request's own mistake, got \
                 {body}"
            );
        } else {
            assert_eq!(
                body["ScannedCount"],
                json!(items.len()),
                "case {case:?}: a served filter still evaluates every item, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_index_scan_charges_its_capacity_to_the_index() {
    let router = emulator();
    seeded(&router).await;

    // Each row: the case, the index scanned (`None` for the base table), and the member
    // of the `INDEXES` breakdown the units must appear under.
    let cases: [(&str, Option<&str>, &str); 3] = [
        ("the base table", None, "Table"),
        (
            "a global secondary index",
            Some("ByStatusPriority"),
            "GlobalSecondaryIndexes",
        ),
        (
            "a local secondary index",
            Some("ByAssignee"),
            "LocalSecondaryIndexes",
        ),
    ];
    for (case, index, member) in cases {
        let mut request = json!({"TableName": TABLE, "ReturnConsumedCapacity": "INDEXES"});
        if let Some(index) = index {
            request["IndexName"] = json!(index);
        }
        let scanned = ok(&router, "Scan", request).await;
        let total = scanned["ConsumedCapacity"]["CapacityUnits"].clone();
        assert_eq!(
            scanned["ConsumedCapacity"]["TableName"],
            json!(TABLE),
            "case {case:?}: the capacity names the table that served the scan, got {scanned}"
        );
        let charged = match index {
            Some(index) => scanned["ConsumedCapacity"][member][index]["CapacityUnits"].clone(),
            None => scanned["ConsumedCapacity"][member]["CapacityUnits"].clone(),
        };
        assert_eq!(
            charged, total,
            "case {case:?}: the units are charged to what the scan read, got {scanned}"
        );
        if index.is_some() {
            assert_eq!(
                scanned["ConsumedCapacity"]["Table"]["CapacityUnits"],
                json!(0.0),
                "case {case:?}: a read served from the index spends nothing on the table \
                 itself, got {scanned}"
            );
        }
        for other in ["GlobalSecondaryIndexes", "LocalSecondaryIndexes"] {
            if other == member {
                continue;
            }
            assert!(
                scanned["ConsumedCapacity"]
                    .get(other)
                    .is_none_or(Value::is_null),
                "case {case:?}: an index the scan did not read is not in the breakdown, \
                 got {scanned}"
            );
        }
    }
}
