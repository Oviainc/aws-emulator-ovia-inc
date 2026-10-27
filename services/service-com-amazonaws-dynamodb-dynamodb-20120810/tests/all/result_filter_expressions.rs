// SPDX-License-Identifier: CC0-1.0

//! `FilterExpression` on a `Scan` and on a `Query`: the filter the service applies
//! AFTER the read.
//!
//! The rule these suites pin: a `FilterExpression` is the `ConditionExpression`
//! language evaluated against each item the request already read, so it decides which
//! of those items the response CARRIES and nothing else — every item read is still
//! counted by `ScannedCount` and still charged for, `Count` counts the ones that
//! matched, and the page the request would have returned without the filter (its
//! `Limit`, its `LastEvaluatedKey`) does not move.
//!
//! The table spans what the rule generalises over: the comparator or function the
//! expression uses (`=`, `<>`, the ordering comparators, `BETWEEN`, `IN`,
//! `attribute_exists`, `attribute_not_exists`, `begins_with`, `contains`, `size`),
//! how the predicates are combined (`AND`, `OR`, `NOT`, parentheses), whether the
//! names and values come from substitution tokens or stand in the expression, the
//! attribute's type, and whether the match is every item, some of them or none.
//!
//! The harness is `crate::emulator`'s, and the table is created through
//! `crate::table_items::create_active_table`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every row below filters: one partition key, one sort key, and attributes
/// of several types for the expressions to speak about.
const TABLE: &str = "FilteredTracks";

/// The items the table holds. `Plays` is a number, `Title` a string, `Tags` a string
/// set, `Live` a boolean, and `Notes` is absent from some items so the
/// attribute-presence functions have something to say.
fn items() -> Vec<Value> {
    vec![
        json!({"Artist": {"S": "Acme"}, "Track": {"N": "1"}, "Title": {"S": "Alpha"},
               "Plays": {"N": "10"}, "Tags": {"SS": ["jazz", "live"]}, "Live": {"BOOL": true},
               "Notes": {"S": "first"}}),
        json!({"Artist": {"S": "Acme"}, "Track": {"N": "2"}, "Title": {"S": "Beta"},
               "Plays": {"N": "20"}, "Tags": {"SS": ["rock"]}, "Live": {"BOOL": false}}),
        json!({"Artist": {"S": "Acme"}, "Track": {"N": "3"}, "Title": {"S": "Alpine"},
               "Plays": {"N": "30"}, "Live": {"BOOL": true}, "Notes": {"S": "third"}}),
        json!({"Artist": {"S": "Beta"}, "Track": {"N": "1"}, "Title": {"S": "Gamma"},
               "Plays": {"N": "40"}, "Tags": {"SS": ["jazz"]}, "Live": {"BOOL": false}}),
    ]
}

/// One row: a filter expression with its substitutions, and the `Title`s of the items
/// it must leave in a scan of the whole table.
struct FilterCase {
    case: &'static str,
    expression: &'static str,
    names: Value,
    values: Value,
    /// The titles the filter keeps, in the order the scan reports them.
    titles: Vec<&'static str>,
}

fn cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            case: "equality on a number, through a value token",
            expression: "Plays = :p",
            names: json!({}),
            values: json!({":p": {"N": "20"}}),
            titles: vec!["Beta"],
        },
        FilterCase {
            case: "inequality on a number",
            expression: "Plays <> :p",
            names: json!({}),
            values: json!({":p": {"N": "20"}}),
            titles: vec!["Alpha", "Alpine", "Gamma"],
        },
        FilterCase {
            case: "an ordering comparator",
            expression: "Plays > :p",
            names: json!({}),
            values: json!({":p": {"N": "20"}}),
            titles: vec!["Alpine", "Gamma"],
        },
        FilterCase {
            case: "BETWEEN, which includes both bounds",
            expression: "Plays BETWEEN :low AND :high",
            names: json!({}),
            values: json!({":low": {"N": "20"}, ":high": {"N": "30"}}),
            titles: vec!["Beta", "Alpine"],
        },
        FilterCase {
            case: "IN over a list of values",
            expression: "Title IN (:a, :b)",
            names: json!({}),
            values: json!({":a": {"S": "Alpha"}, ":b": {"S": "Gamma"}}),
            titles: vec!["Alpha", "Gamma"],
        },
        FilterCase {
            case: "attribute_exists on an attribute only some items carry",
            expression: "attribute_exists(Notes)",
            names: json!({}),
            values: json!({}),
            titles: vec!["Alpha", "Alpine"],
        },
        FilterCase {
            case: "attribute_not_exists, the complement of the same reading",
            expression: "attribute_not_exists(Notes)",
            names: json!({}),
            values: json!({}),
            titles: vec!["Beta", "Gamma"],
        },
        FilterCase {
            case: "begins_with on a string, through a name token",
            expression: "begins_with(#t, :prefix)",
            names: json!({"#t": "Title"}),
            values: json!({":prefix": {"S": "Alp"}}),
            titles: vec!["Alpha", "Alpine"],
        },
        FilterCase {
            case: "contains on a string set",
            expression: "contains(Tags, :tag)",
            names: json!({}),
            values: json!({":tag": {"S": "jazz"}}),
            titles: vec!["Alpha", "Gamma"],
        },
        FilterCase {
            case: "size of a string",
            expression: "size(Title) = :n",
            names: json!({}),
            values: json!({":n": {"N": "4"}}),
            titles: vec!["Beta"],
        },
        FilterCase {
            case: "two predicates combined with AND",
            expression: "Plays > :p AND Live = :live",
            names: json!({}),
            values: json!({":p": {"N": "10"}, ":live": {"BOOL": true}}),
            titles: vec!["Alpine"],
        },
        FilterCase {
            case: "two predicates combined with OR",
            expression: "Plays = :low OR Plays = :high",
            names: json!({}),
            values: json!({":low": {"N": "10"}, ":high": {"N": "40"}}),
            titles: vec!["Alpha", "Gamma"],
        },
        FilterCase {
            case: "NOT over a parenthesised group",
            expression: "NOT (Plays = :low OR Plays = :high)",
            names: json!({}),
            values: json!({":low": {"N": "10"}, ":high": {"N": "40"}}),
            titles: vec!["Beta", "Alpine"],
        },
        FilterCase {
            case: "a filter every item matches",
            expression: "Plays >= :p",
            names: json!({}),
            values: json!({":p": {"N": "10"}}),
            titles: vec!["Alpha", "Beta", "Alpine", "Gamma"],
        },
        FilterCase {
            case: "a filter no item matches",
            expression: "Plays > :p",
            names: json!({}),
            values: json!({":p": {"N": "100"}}),
            titles: vec![],
        },
        FilterCase {
            case: "a filter on the partition key, which a scan allows",
            expression: "Artist = :artist",
            names: json!({}),
            values: json!({":artist": {"S": "Beta"}}),
            titles: vec!["Gamma"],
        },
    ]
}

/// Write the items into a freshly created table.
async fn seed(router: &MultiServiceRouter) {
    create_active_table(
        router,
        TABLE,
        &[("Artist", "HASH", "S"), ("Track", "RANGE", "N")],
    )
    .await;
    for item in items() {
        ok(router, "PutItem", json!({"TableName": TABLE, "Item": item})).await;
    }
}

/// The `Title` of each item a response carries, in the order it carries them.
fn titles(response: &Value) -> Vec<String> {
    response
        .get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item["Title"]["S"].as_str().unwrap_or("<no Title>").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One request with the row's expression and whichever substitution maps it needs: a
/// map the row leaves empty is not sent at all, because a substitution member with no
/// entries is not what the row is about.
fn with_substitutions(mut request: Value, row: &FilterCase) -> Value {
    let members = request.as_object_mut().expect("the request is an object");
    for (member, map) in [
        ("ExpressionAttributeNames", &row.names),
        ("ExpressionAttributeValues", &row.values),
    ] {
        if map.as_object().is_some_and(|map| !map.is_empty()) {
            members.insert(member.to_string(), map.clone());
        }
    }
    request
}

#[tokio::test]
async fn a_scan_filter_decides_which_items_the_response_carries() {
    let router = emulator();
    seed(&router).await;
    let total = json!(items().len());

    for row in cases() {
        let case = row.case;
        let scanned = ok(
            &router,
            "Scan",
            with_substitutions(
                json!({"TableName": TABLE, "FilterExpression": row.expression}),
                &row,
            ),
        )
        .await;
        assert_eq!(
            titles(&scanned),
            row.titles
                .iter()
                .map(|title| (*title).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the items {:?} match, got {scanned}",
            row.expression
        );
        assert_eq!(
            scanned["Count"],
            json!(row.titles.len()),
            "case {case:?}: Count is the number of items the filter kept, got {scanned}"
        );
        // "ScannedCount — the number of items evaluated, before any ScanFilter is
        // applied": the filter removes items from the RESPONSE, not from the read.
        assert_eq!(
            scanned["ScannedCount"], total,
            "case {case:?}: every item the scan read is still scanned, got {scanned}"
        );
        // COUNT reports the same figures without the items themselves.
        let counted = ok(
            &router,
            "Scan",
            with_substitutions(
                json!({"TableName": TABLE, "FilterExpression": row.expression,
                       "Select": "COUNT"}),
                &row,
            ),
        )
        .await;
        assert_eq!(
            (counted["Count"].clone(), counted["ScannedCount"].clone()),
            (json!(row.titles.len()), total.clone()),
            "case {case:?}: Select COUNT reports the filtered count, got {counted}"
        );
        assert_eq!(
            counted.get("Items"),
            None,
            "case {case:?}: Select COUNT carries no items, got {counted}"
        );
    }
}

/// The filter does not change the PAGE: `Limit` still bounds the items read, so a
/// filtered page reports the `LastEvaluatedKey` the unfiltered page would and a
/// filtered page may be empty while there is more to read.
#[tokio::test]
async fn a_filter_bounds_the_response_and_not_the_page() {
    let router = emulator();
    seed(&router).await;

    for limit in [1usize, 2, 3, 4, 7] {
        let unfiltered = ok(
            &router,
            "Scan",
            json!({"TableName": TABLE, "Limit": limit}),
        )
        .await;
        let filtered = ok(
            &router,
            "Scan",
            json!({"TableName": TABLE, "Limit": limit,
                   "FilterExpression": "Plays = :p",
                   "ExpressionAttributeValues": {":p": {"N": "30"}}}),
        )
        .await;
        assert_eq!(
            filtered["ScannedCount"], unfiltered["Count"],
            "case limit {limit}: the filter leaves the items read unchanged, got \
             {filtered} against {unfiltered}"
        );
        assert_eq!(
            filtered["LastEvaluatedKey"], unfiltered["LastEvaluatedKey"],
            "case limit {limit}: the filter leaves the page boundary unchanged, got \
             {filtered} against {unfiltered}"
        );
        // Paging with the filter reaches exactly the matching items, however small the
        // page is: a page whose items the filter removed is still a page.
        let mut matched: Vec<String> = Vec::new();
        let mut start: Option<Value> = None;
        for _ in 0..=items().len() {
            let mut request = json!({"TableName": TABLE, "Limit": limit,
                                     "FilterExpression": "Plays = :p",
                                     "ExpressionAttributeValues": {":p": {"N": "30"}}});
            if let Some(key) = start.clone() {
                request["ExclusiveStartKey"] = key;
            }
            let page = ok(&router, "Scan", request).await;
            matched.extend(titles(&page));
            match page.get("LastEvaluatedKey").filter(|key| !key.is_null()) {
                Some(key) => start = Some(key.clone()),
                None => break,
            }
        }
        assert_eq!(
            matched,
            vec!["Alpine".to_string()],
            "case limit {limit}: paging a filtered scan reaches the matching items, \
             got {matched:?}"
        );
    }
}

/// A `Query`'s filter is the same rule over the items its key condition selected, and
/// it "does not allow key attributes".
#[tokio::test]
async fn a_query_filter_applies_to_the_items_the_key_condition_selected() {
    let router = emulator();
    seed(&router).await;

    // (case, filter, values, the titles the query must report, the ScannedCount)
    let rows: Vec<(&str, &str, Value, Vec<&str>, i64)> = vec![
        (
            "a filter that keeps some of the partition's items",
            "Plays > :p",
            json!({":p": {"N": "10"}}),
            vec!["Beta", "Alpine"],
            3,
        ),
        (
            "a filter no item of the partition matches",
            "Plays > :p",
            json!({":p": {"N": "100"}}),
            vec![],
            3,
        ),
        (
            "a filter every item of the partition matches",
            "attribute_exists(Plays)",
            json!({}),
            vec!["Alpha", "Beta", "Alpine"],
            3,
        ),
    ];

    for (case, expression, values, expected, scanned) in rows {
        let mut request = json!({
            "TableName": TABLE,
            "KeyConditionExpression": "Artist = :artist",
            "FilterExpression": expression,
        });
        let members = request.as_object_mut().expect("the request is an object");
        let mut substitutions = json!({":artist": {"S": "Acme"}});
        let map = substitutions
            .as_object_mut()
            .expect("the substitutions are an object");
        for (placeholder, value) in values.as_object().expect("the values are an object") {
            map.insert(placeholder.clone(), value.clone());
        }
        members.insert("ExpressionAttributeValues".to_string(), substitutions);

        let queried = ok(&router, "Query", request).await;
        assert_eq!(
            titles(&queried),
            expected
                .iter()
                .map(|title| (*title).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the filtered items, got {queried}"
        );
        assert_eq!(
            (queried["Count"].clone(), queried["ScannedCount"].clone()),
            (json!(expected.len()), json!(scanned)),
            "case {case:?}: Count counts the matches and ScannedCount the items read, \
             got {queried}"
        );
    }

    // "A FilterExpression does not allow key attributes."
    for key in ["Artist", "Track"] {
        let (status, body) = call(
            &router,
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditionExpression": "Artist = :artist",
                "FilterExpression": format!("{key} = :v"),
                "ExpressionAttributeValues": {":artist": {"S": "Acme"}, ":v": {"S": "x"}},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {key}: a filter on a key attribute is refused, got {status} with {body}"
        );
        assert!(
            message(&body).contains(key),
            "case {key}: the refusal names the key attribute, got {body}"
        );
    }
}

/// A filter request the service refuses: an expression that is not the language, a
/// token the expression never resolves, and the legacy members `FilterExpression`
/// replaces.
#[tokio::test]
async fn a_filter_request_the_service_refuses() {
    let router = emulator();
    seed(&router).await;

    // (case, request, the phrase the message must carry)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an expression that is not the condition language",
            json!({"TableName": TABLE, "FilterExpression": "Plays >"}),
            "FilterExpression",
        ),
        (
            "a value token the request never defines",
            json!({"TableName": TABLE, "FilterExpression": "Plays = :missing"}),
            "attribute value",
        ),
        (
            "a name token the request never defines",
            json!({"TableName": TABLE, "FilterExpression": "#missing = :p",
                   "ExpressionAttributeValues": {":p": {"N": "1"}}}),
            "attribute name",
        ),
        (
            "a FilterExpression beside the legacy ScanFilter it replaces",
            json!({"TableName": TABLE, "FilterExpression": "Plays = :p",
                   "ExpressionAttributeValues": {":p": {"N": "1"}},
                   "ScanFilter": {"Plays": {"ComparisonOperator": "EQ",
                                            "AttributeValueList": [{"N": "1"}]}}}),
            "Can not use both expression and non-expression parameters",
        ),
        (
            "a FilterExpression beside the legacy ConditionalOperator",
            json!({"TableName": TABLE, "FilterExpression": "Plays = :p",
                   "ExpressionAttributeValues": {":p": {"N": "1"}},
                   "ConditionalOperator": "AND"}),
            "Can not use both expression and non-expression parameters",
        ),
    ];

    for (case, request, phrase) in rows {
        let (status, body) = call(&router, "Scan", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a validation refusal, got {status} with {body}"
        );
        assert!(
            message(&body).contains(phrase),
            "case {case:?}: expected the message to carry {phrase:?}, got {body}"
        );
    }
}
