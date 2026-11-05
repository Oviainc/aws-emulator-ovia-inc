// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PartiQL surface: `ExecuteStatement` over the SAME item
//! store the classic item API reads and writes.
//!
//! The rule every table here asserts is one rule: a statement reads and mutates the
//! stored items, so a statement's write is visible to every later read — classic or
//! PartiQL — and a classic write is visible to every later statement. The rows span
//! what the rule generalises over rather than one recorded case: every
//! `AttributeValue` type an item can hold, every predicate form a `WHERE` clause can
//! state, every clause an `UPDATE` can apply, and both `RETURNING` directions.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    MAX_ATTRIBUTE_DEPTH, MAX_STATEMENT_DEPTH, MAX_STATEMENT_TOKENS, STATUS_TRANSITION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// Run one statement that must succeed, with the parameters it names (if any).
async fn run(router: &MultiServiceRouter, statement: &str, parameters: Value) -> Value {
    let mut body = json!({"Statement": statement});
    if !parameters.is_null() {
        body["Parameters"] = parameters;
    }
    ok(router, "ExecuteStatement", body).await
}

/// The `(pk, sk)` pairs one statement reported, in the order it reported them.
fn addressed(answered: &Value) -> Vec<(String, String)> {
    answered["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            (
                item["pk"]["S"].as_str().unwrap_or_default().to_string(),
                item["sk"]["N"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The same value with every SET's members sorted.
///
/// A DynamoDB set is unordered — "Amazon DynamoDB does not preserve the order of the
/// members of a set" — so a suite that compares a stored set to an expected one
/// compares the MEMBERS, not the order the wire happened to list them in.
fn with_sorted_sets(value: &Value) -> Value {
    match value {
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(name, inner)| {
                    let sorted = if matches!(name.as_str(), "SS" | "NS" | "BS") {
                        let mut members: Vec<Value> =
                            inner.as_array().cloned().unwrap_or_default();
                        members.sort_by_key(|member| member.to_string());
                        Value::Array(members)
                    } else {
                        with_sorted_sets(inner)
                    };
                    (name.clone(), sorted)
                })
                .collect(),
        ),
        Value::Array(members) => Value::Array(members.iter().map(with_sorted_sets).collect()),
        other => other.clone(),
    }
}

/// One `INSERT` row: the statement, the parameters it names, and the item a later
/// `GetItem` must report.
struct InsertRow {
    case: &'static str,
    statement: &'static str,
    parameters: Value,
    key: &'static str,
    stored: Value,
}

/// The rows differ in the value TYPES the inserted tuple carries — every one of the
/// ten `AttributeValue` variants appears, at the top level and nested — and in how the
/// statement spells them: as literals, and as `?` markers the `Parameters` list fills.
fn insert_table() -> Vec<InsertRow> {
    vec![
        InsertRow {
            case: "a string and a number literal",
            statement: "INSERT INTO items VALUE {'pk': 'a', 's': 'text', 'n': 7}",
            parameters: Value::Null,
            key: "a",
            stored: json!({"pk": {"S": "a"}, "s": {"S": "text"}, "n": {"N": "7"}}),
        },
        InsertRow {
            case: "a number of the documented 38 significant digits",
            statement:
                "INSERT INTO items VALUE {'pk': 'b', 'n': 12345678901234567890123456789012345678}",
            parameters: Value::Null,
            key: "b",
            stored: json!({
                "pk": {"S": "b"},
                "n": {"N": "12345678901234567890123456789012345678"},
            }),
        },
        InsertRow {
            case: "numbers stored canonically, whatever lexeme spelled them",
            statement: "INSERT INTO items VALUE {'pk': 'c', 'trailing': 2.50, 'exponent': 1e3, \
                        'zero': -0.0, 'negative': -17}",
            parameters: Value::Null,
            key: "c",
            stored: json!({
                "pk": {"S": "c"},
                "trailing": {"N": "2.5"},
                "exponent": {"N": "1000"},
                "zero": {"N": "0"},
                "negative": {"N": "-17"},
            }),
        },
        InsertRow {
            case: "booleans and the NULL type",
            statement: "INSERT INTO items VALUE {'pk': 'd', 'yes': true, 'no': false, \
                        'nothing': NULL}",
            parameters: Value::Null,
            key: "d",
            stored: json!({
                "pk": {"S": "d"},
                "yes": {"BOOL": true},
                "no": {"BOOL": false},
                "nothing": {"NULL": true},
            }),
        },
        InsertRow {
            case: "a list and a map, nested",
            statement: "INSERT INTO items VALUE {'pk': 'e', 'l': [1, 'two', {'deep': true}], \
                        'm': {'inner': [3]}}",
            parameters: Value::Null,
            key: "e",
            stored: json!({
                "pk": {"S": "e"},
                "l": {"L": [{"N": "1"}, {"S": "two"}, {"M": {"deep": {"BOOL": true}}}]},
                "m": {"M": {"inner": {"L": [{"N": "3"}]}}},
            }),
        },
        InsertRow {
            case: "a string set and a number set",
            statement: "INSERT INTO items VALUE {'pk': 'f', 'ss': <<'x', 'y'>>, 'ns': <<1, 2>>}",
            parameters: Value::Null,
            key: "f",
            stored: json!({
                "pk": {"S": "f"},
                "ss": {"SS": ["x", "y"]},
                "ns": {"NS": ["1", "2"]},
            }),
        },
        InsertRow {
            case: "parameters, including a binary value no literal can spell",
            statement: "INSERT INTO items VALUE {'pk': ?, 'b': ?, 'n': ?}",
            parameters: json!([{"S": "g"}, {"B": "AAH+/w=="}, {"N": "42"}]),
            key: "g",
            stored: json!({
                "pk": {"S": "g"},
                "b": {"B": "AAH+/w=="},
                "n": {"N": "42"},
            }),
        },
        InsertRow {
            case: "an empty string, an empty list and an empty map",
            statement: "INSERT INTO items VALUE {'pk': 'h', 'empty': '', 'list': [], 'map': {}}",
            parameters: Value::Null,
            key: "h",
            stored: json!({
                "pk": {"S": "h"},
                "empty": {"S": ""},
                "list": {"L": []},
                "map": {"M": {}},
            }),
        },
        InsertRow {
            case: "attribute names outside the bare-word character set",
            statement: "INSERT INTO items VALUE {'pk': 'i', 'a-b': 'dash', 'with space': 1}",
            parameters: Value::Null,
            key: "i",
            stored: json!({
                "pk": {"S": "i"},
                "a-b": {"S": "dash"},
                "with space": {"N": "1"},
            }),
        },
    ]
}

#[tokio::test]
async fn an_inserted_item_is_what_the_classic_api_reads_back() {
    let router = emulator();
    create_active_table(&router, "items", &[("pk", "HASH", "S")]).await;

    for row in insert_table() {
        let answered = run(&router, row.statement, row.parameters.clone()).await;
        assert_eq!(
            answered,
            json!({"Items": []}),
            "case {}: a write statement reports an empty Items list, got {answered}",
            row.case
        );

        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": "items", "Key": {"pk": {"S": row.key}}}),
        )
        .await;
        assert_eq!(
            with_sorted_sets(&read["Item"]),
            with_sorted_sets(&row.stored),
            "case {}: GetItem must report the inserted item, got {read}",
            row.case
        );

        // The same item, read back through a statement rather than through GetItem:
        // both read the one store.
        let selected = run(
            &router,
            &format!("SELECT * FROM items WHERE pk = '{}'", row.key),
            Value::Null,
        )
        .await;
        assert_eq!(
            with_sorted_sets(&selected["Items"]),
            with_sorted_sets(&json!([row.stored])),
            "case {}: a SELECT must report the inserted item too, got {selected}",
            row.case
        );
    }
}

/// One `SELECT` row: the predicate, and the items it must report.
struct SelectRow {
    case: &'static str,
    statement: &'static str,
    parameters: Value,
    reported: &'static [(&'static str, &'static str)],
}

/// The rows differ in the predicate FORM: equality, every inequality, `BETWEEN`,
/// `IN`, the documented functions, `IS NULL`/`IS MISSING`, `NOT`, `OR`, no predicate
/// at all, and a predicate whose values arrive as parameters.
const SELECT_TABLE: &[SelectRow] = &[
    SelectRow {
        case: "every item of one partition",
        statement: "SELECT * FROM items WHERE pk = 'p'",
        parameters: Value::Null,
        reported: &[("p", "1"), ("p", "2"), ("p", "3"), ("p", "10")],
    },
    SelectRow {
        case: "a range on the sort key",
        statement: "SELECT * FROM items WHERE pk = 'p' AND sk > 2",
        parameters: Value::Null,
        reported: &[("p", "3"), ("p", "10")],
    },
    SelectRow {
        case: "BETWEEN on the sort key",
        statement: "SELECT * FROM items WHERE pk = 'p' AND sk BETWEEN 2 AND 3",
        parameters: Value::Null,
        reported: &[("p", "2"), ("p", "3")],
    },
    SelectRow {
        case: "IN on the sort key",
        statement: "SELECT * FROM items WHERE pk = 'p' AND sk IN [1, 10]",
        parameters: Value::Null,
        reported: &[("p", "1"), ("p", "10")],
    },
    SelectRow {
        case: "begins_with on a non-key attribute",
        statement: "SELECT * FROM items WHERE pk = 'p' AND begins_with(s, 'alpha')",
        parameters: Value::Null,
        reported: &[("p", "1"), ("p", "2")],
    },
    SelectRow {
        case: "contains on a non-key attribute",
        statement: "SELECT * FROM items WHERE pk = 'p' AND contains(s, 'habe')",
        parameters: Value::Null,
        reported: &[("p", "2")],
    },
    SelectRow {
        case: "attribute_type",
        statement: "SELECT * FROM items WHERE pk = 'p' AND attribute_type(nothing, 'NULL')",
        parameters: Value::Null,
        reported: &[("p", "2")],
    },
    SelectRow {
        case: "size of a string",
        statement: "SELECT * FROM items WHERE pk = 'p' AND size(s) = 5",
        parameters: Value::Null,
        reported: &[("p", "1"), ("p", "10")],
    },
    SelectRow {
        case: "IS NOT MISSING, which only the item carrying the attribute satisfies",
        statement: "SELECT * FROM items WHERE pk = 'p' AND extra IS NOT MISSING",
        parameters: Value::Null,
        reported: &[("p", "3")],
    },
    SelectRow {
        case: "IS MISSING, which the others satisfy",
        statement: "SELECT * FROM items WHERE pk = 'p' AND extra IS MISSING",
        parameters: Value::Null,
        reported: &[("p", "1"), ("p", "2"), ("p", "10")],
    },
    SelectRow {
        case: "IS NULL, which an absent attribute satisfies as well as a NULL one",
        statement: "SELECT * FROM items WHERE pk = 'p' AND nothing IS NULL",
        parameters: Value::Null,
        reported: &[("p", "1"), ("p", "2"), ("p", "3"), ("p", "10")],
    },
    SelectRow {
        case: "NOT around an equality",
        statement: "SELECT * FROM items WHERE pk = 'p' AND NOT (t = 'A')",
        parameters: Value::Null,
        reported: &[("p", "2"), ("p", "10")],
    },
    SelectRow {
        case: "the inequality operator",
        statement: "SELECT * FROM items WHERE pk = 'p' AND t <> 'A'",
        parameters: Value::Null,
        reported: &[("p", "2"), ("p", "10")],
    },
    SelectRow {
        case: "an OR that spans partitions, so the whole table is read",
        statement: "SELECT * FROM items WHERE t = 'C' OR s = 'zeta'",
        parameters: Value::Null,
        reported: &[("p", "10"), ("q", "1")],
    },
    SelectRow {
        case: "no predicate at all, so every stored item",
        statement: "SELECT * FROM items",
        parameters: Value::Null,
        reported: &[
            ("p", "1"),
            ("p", "2"),
            ("p", "3"),
            ("p", "10"),
            ("q", "1"),
        ],
    },
    SelectRow {
        case: "the whole primary key, as parameters",
        statement: "SELECT * FROM items WHERE pk = ? AND sk = ?",
        parameters: Value::Null,
        reported: &[("p", "3")],
    },
];

/// The items `SELECT_TABLE` reads, written through the CLASSIC api: what a statement
/// reports is what another request wrote.
async fn seed_select_items(router: &MultiServiceRouter) {
    let items = [
        json!({
            "pk": {"S": "p"}, "sk": {"N": "1"}, "s": {"S": "alpha"}, "t": {"S": "A"},
            "m": {"M": {"nested": {"S": "deep"}, "other": {"N": "1"}}},
        }),
        json!({
            "pk": {"S": "p"}, "sk": {"N": "2"}, "s": {"S": "alphabet"}, "t": {"S": "B"},
            "nothing": {"NULL": true},
        }),
        json!({
            "pk": {"S": "p"}, "sk": {"N": "3"}, "s": {"S": "beta"}, "t": {"S": "A"},
            "extra": {"S": "here"},
        }),
        json!({"pk": {"S": "p"}, "sk": {"N": "10"}, "s": {"S": "gamma"}, "t": {"S": "C"}}),
        json!({"pk": {"S": "q"}, "sk": {"N": "1"}, "s": {"S": "zeta"}, "t": {"S": "A"}}),
    ];
    for item in items {
        ok(router, "PutItem", json!({"TableName": "items", "Item": item})).await;
    }
}

#[tokio::test]
async fn a_select_reports_the_stored_items_its_predicate_matches() {
    let router = emulator();
    create_active_table(&router, "items", &[("pk", "HASH", "S"), ("sk", "RANGE", "N")]).await;
    seed_select_items(&router).await;

    for row in SELECT_TABLE {
        let parameters = if row.statement.contains('?') {
            json!([{"S": "p"}, {"N": "3"}])
        } else {
            row.parameters.clone()
        };
        let answered = run(&router, row.statement, parameters).await;
        let reported = addressed(&answered);
        let expected: Vec<(String, String)> = row
            .reported
            .iter()
            .map(|(partition, sort)| ((*partition).to_string(), (*sort).to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {}: expected the statement to report {expected:?}, got {reported:?} from \
             {answered}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_select_reports_only_the_attributes_it_projects_and_the_order_it_asks_for() {
    let router = emulator();
    create_active_table(&router, "items", &[("pk", "HASH", "S"), ("sk", "RANGE", "N")]).await;
    seed_select_items(&router).await;

    // A projection reports one entry per path it names, under the path's last named
    // step or the alias the statement gave it, and nothing for a path the item does
    // not hold.
    for (case, statement, expected) in [
        (
            "a top-level attribute, and one the item does not carry",
            "SELECT s, absent FROM items WHERE pk = 'p' AND sk = 1",
            json!([{"s": {"S": "alpha"}}]),
        ),
        (
            "a path into a map, reported under its last step",
            "SELECT m.nested FROM items WHERE pk = 'p' AND sk = 1",
            json!([{"nested": {"S": "deep"}}]),
        ),
        (
            "a path into a map, reported under the alias the statement gave it",
            "SELECT m.nested AS renamed FROM items WHERE pk = 'p' AND sk = 1",
            json!([{"renamed": {"S": "deep"}}]),
        ),
        (
            "a path into a map the item does not hold",
            "SELECT m.absent FROM items WHERE pk = 'p' AND sk = 1",
            json!([{}]),
        ),
        (
            "several paths at once",
            "SELECT pk, m.other FROM items WHERE pk = 'p' AND sk = 1",
            json!([{"pk": {"S": "p"}, "other": {"N": "1"}}]),
        ),
    ] {
        let projected = run(&router, statement, Value::Null).await;
        assert_eq!(
            projected["Items"], expected,
            "case {case}: expected the statement to report {expected}, got {projected}"
        );
    }

    // "ORDER BY clause can only be used on a key attribute": ascending is the store's
    // own order, descending is its reverse.
    for (case, statement, expected) in [
        (
            "ascending by the sort key",
            "SELECT * FROM items WHERE pk = 'p' ORDER BY sk ASC",
            vec!["1", "2", "3", "10"],
        ),
        (
            "descending by the sort key",
            "SELECT * FROM items WHERE pk = 'p' ORDER BY sk DESC",
            vec!["10", "3", "2", "1"],
        ),
    ] {
        let answered = run(&router, statement, Value::Null).await;
        let order: Vec<String> = addressed(&answered)
            .into_iter()
            .map(|(_, sort)| sort)
            .collect();
        assert_eq!(
            order, expected,
            "case {case}: expected the sort keys in order {expected:?}, got {order:?} from \
             {answered}"
        );
    }
}

#[tokio::test]
async fn a_select_pages_at_its_limit_and_the_next_token_resumes_after_it() {
    let router = emulator();
    create_active_table(&router, "items", &[("pk", "HASH", "S"), ("sk", "RANGE", "N")]).await;
    seed_select_items(&router).await;

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..3 {
        let mut body = json!({"Statement": "SELECT * FROM items WHERE pk = 'p'", "Limit": 2});
        if let Some(token) = token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let answered = ok(&router, "ExecuteStatement", body).await;
        let keys: Vec<String> = addressed(&answered)
            .into_iter()
            .map(|(_, sort)| sort)
            .collect();
        assert!(
            keys.len() <= 2,
            "case page {page}: a page holds at most the Limit, got {keys:?} from {answered}"
        );
        seen.extend(keys);
        token = answered["NextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen,
        vec!["1", "2", "3", "10"],
        "case pagination: the pages together report every matching item once, in order, got \
         {seen:?}"
    );
    assert!(
        token.is_none(),
        "case pagination: the last page reports no NextToken, got {token:?}"
    );
}

/// One `UPDATE` row: the statement, and the item it must leave behind.
struct UpdateRow {
    case: &'static str,
    statement: &'static str,
    parameters: Value,
    stored: Value,
}

/// The item every `UPDATE` row starts from, written through the classic api.
fn update_subject() -> Value {
    json!({
        "pk": {"S": "u"},
        "n": {"N": "5"},
        "s": {"S": "old"},
        "l": {"L": [{"N": "1"}]},
        "ns": {"NS": ["1", "2"]},
        "gone": {"S": "bye"},
    })
}

/// The rows differ in the CLAUSE the statement applies: a literal, arithmetic in both
/// directions, a removal, both at once, `list_append`, the two set actions, a nested
/// path, a parameter, and a predicate that reads more than the key.
fn update_table() -> Vec<UpdateRow> {
    let mut rows = Vec::new();
    let mut row = |case, statement, parameters, change: Vec<(&str, Option<Value>)>| {
        let mut stored = update_subject();
        for (attribute, value) in change {
            match value {
                Some(value) => {
                    stored[attribute] = value;
                }
                None => {
                    stored.as_object_mut().expect("an item is an object").remove(attribute);
                }
            }
        }
        rows.push(UpdateRow {
            case,
            statement,
            parameters,
            stored,
        });
    };
    row(
        "SET a string literal",
        "UPDATE items SET s = 'new' WHERE pk = 'u'",
        Value::Null,
        vec![("s", Some(json!({"S": "new"})))],
    );
    row(
        "SET a sum of the stored number and a literal",
        "UPDATE items SET n = n + 3 WHERE pk = 'u'",
        Value::Null,
        vec![("n", Some(json!({"N": "8"})))],
    );
    row(
        "SET a difference",
        "UPDATE items SET n = n - 5 WHERE pk = 'u'",
        Value::Null,
        vec![("n", Some(json!({"N": "0"})))],
    );
    row(
        "REMOVE an attribute",
        "UPDATE items REMOVE gone WHERE pk = 'u'",
        Value::Null,
        vec![("gone", None)],
    );
    row(
        "SET and REMOVE in one statement",
        "UPDATE items SET s = 'both' REMOVE gone WHERE pk = 'u'",
        Value::Null,
        vec![("s", Some(json!({"S": "both"}))), ("gone", None)],
    );
    row(
        "two assignments, one SET clause each",
        "UPDATE items SET s = 'two' SET n = 1 WHERE pk = 'u'",
        Value::Null,
        vec![
            ("s", Some(json!({"S": "two"}))),
            ("n", Some(json!({"N": "1"}))),
        ],
    );
    row(
        "list_append",
        "UPDATE items SET l = list_append(l, [2]) WHERE pk = 'u'",
        Value::Null,
        vec![("l", Some(json!({"L": [{"N": "1"}, {"N": "2"}]})))],
    );
    row(
        "set_add, which adds members to the stored set",
        "UPDATE items SET ns = set_add(ns, <<3>>) WHERE pk = 'u'",
        Value::Null,
        vec![("ns", Some(json!({"NS": ["1", "2", "3"]})))],
    );
    row(
        "set_delete, which takes members out of it",
        "UPDATE items SET ns = set_delete(ns, <<2>>) WHERE pk = 'u'",
        Value::Null,
        vec![("ns", Some(json!({"NS": ["1"]})))],
    );
    row(
        "a nested path inside a list",
        "UPDATE items SET l[0] = 9 WHERE pk = 'u'",
        Value::Null,
        vec![("l", Some(json!({"L": [{"N": "9"}]})))],
    );
    row(
        "a parameter for the value and for the key",
        "UPDATE items SET s = ? WHERE pk = ?",
        json!([{"S": "param"}, {"S": "u"}]),
        vec![("s", Some(json!({"S": "param"})))],
    );
    row(
        "a predicate that reads a non-key attribute as well as the key",
        "UPDATE items SET s = 'conditional' WHERE pk = 'u' AND n = 5",
        Value::Null,
        vec![("s", Some(json!({"S": "conditional"})))],
    );
    rows
}

#[tokio::test]
async fn an_update_statement_leaves_the_item_the_classic_api_then_reads() {
    let router = emulator();
    create_active_table(&router, "items", &[("pk", "HASH", "S")]).await;

    for row in update_table() {
        // Each row starts from the same stored item, so what it asserts is its own
        // clause and nothing a previous row left behind.
        ok(
            &router,
            "PutItem",
            json!({"TableName": "items", "Item": update_subject()}),
        )
        .await;
        let answered = run(&router, row.statement, row.parameters.clone()).await;
        assert_eq!(
            answered,
            json!({"Items": []}),
            "case {}: an UPDATE with no RETURNING clause reports an empty Items list, got \
             {answered}",
            row.case
        );

        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": "items", "Key": {"pk": {"S": "u"}}}),
        )
        .await;
        assert_eq!(
            with_sorted_sets(&read["Item"]),
            with_sorted_sets(&row.stored),
            "case {}: GetItem must report the updated item, got {read}",
            row.case
        );
    }
}

#[tokio::test]
async fn returning_reports_the_item_as_it_stood_immediately_before_or_after_the_write() {
    let router = emulator();
    create_active_table(&router, "items", &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "items", "Item": update_subject()}),
    )
    .await;

    // Each row writes over what the previous one left, so the "old" copy a row
    // reports is the item as it stood IMMEDIATELY before that row — not an earlier
    // version of it.
    let all_old = run(
        &router,
        "UPDATE items SET s = 'first' WHERE pk = 'u' RETURNING ALL OLD *",
        Value::Null,
    )
    .await;
    assert_eq!(
        with_sorted_sets(&all_old["Items"]),
        with_sorted_sets(&json!([update_subject()])),
        "case ALL OLD: the statement reports the item it replaced, got {all_old}"
    );

    let modified_new = run(
        &router,
        "UPDATE items SET s = 'second' WHERE pk = 'u' RETURNING MODIFIED NEW *",
        Value::Null,
    )
    .await;
    assert_eq!(
        modified_new["Items"],
        json!([{"s": {"S": "second"}}]),
        "case MODIFIED NEW: the statement reports just what it changed, got {modified_new}"
    );

    let modified_old = run(
        &router,
        "UPDATE items SET s = 'third' WHERE pk = 'u' RETURNING MODIFIED OLD *",
        Value::Null,
    )
    .await;
    assert_eq!(
        modified_old["Items"],
        json!([{"s": {"S": "second"}}]),
        "case MODIFIED OLD: the statement reports what the previous write left, got \
         {modified_old}"
    );

    let all_new = run(
        &router,
        "UPDATE items SET s = 'fourth' WHERE pk = 'u' RETURNING ALL NEW *",
        Value::Null,
    )
    .await;
    let mut expected = update_subject();
    expected["s"] = json!({"S": "fourth"});
    assert_eq!(
        with_sorted_sets(&all_new["Items"]),
        with_sorted_sets(&json!([expected])),
        "case ALL NEW: the statement reports the item it left, got {all_new}"
    );

    // A DELETE removes the item, and the copy it returns is the item as it stood
    // immediately before that — after every update above.
    let deleted = run(
        &router,
        "DELETE FROM items WHERE pk = 'u' RETURNING ALL OLD *",
        Value::Null,
    )
    .await;
    assert_eq!(
        with_sorted_sets(&deleted["Items"]),
        with_sorted_sets(&json!([expected])),
        "case DELETE ALL OLD: the statement reports the item it removed, got {deleted}"
    );
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "items", "Key": {"pk": {"S": "u"}}}),
    )
    .await;
    assert_eq!(
        read,
        json!({}),
        "case DELETE: the item is gone from the store, got {read}"
    );
}

#[tokio::test]
async fn one_store_serves_the_classic_api_and_the_statement_surface_in_both_directions() {
    let router = emulator();
    create_active_table(&router, "items", &[("pk", "HASH", "S")]).await;

    // A statement writes; the classic api reads.
    run(
        &router,
        "INSERT INTO items VALUE {'pk': 'shared', 'body': 'inserted'}",
        Value::Null,
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "items", "Key": {"pk": {"S": "shared"}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["body"],
        json!({"S": "inserted"}),
        "case insert then GetItem: the statement's item is in the store, got {read}"
    );

    // The classic api writes; a statement reads.
    ok(
        &router,
        "UpdateItem",
        json!({
            "TableName": "items",
            "Key": {"pk": {"S": "shared"}},
            "UpdateExpression": "SET body = :body",
            "ExpressionAttributeValues": {":body": {"S": "updated"}},
        }),
    )
    .await;
    let selected = run(
        &router,
        "SELECT * FROM items WHERE pk = 'shared'",
        Value::Null,
    )
    .await;
    assert_eq!(
        selected["Items"],
        json!([{"pk": {"S": "shared"}, "body": {"S": "updated"}}]),
        "case UpdateItem then SELECT: the statement reports the classic write, got {selected}"
    );

    // A statement updates; Query reports the updated item.
    run(
        &router,
        "UPDATE items SET body = 'partiql-updated' WHERE pk = 'shared'",
        Value::Null,
    )
    .await;
    let queried = ok(
        &router,
        "Query",
        json!({
            "TableName": "items",
            "KeyConditionExpression": "pk = :pk",
            "ExpressionAttributeValues": {":pk": {"S": "shared"}},
        }),
    )
    .await;
    assert_eq!(
        queried["Count"],
        json!(1),
        "case UPDATE then Query: the updated item is visible to a Query, got {queried}"
    );
    assert_eq!(
        queried["Items"][0]["body"],
        json!({"S": "partiql-updated"}),
        "case UPDATE then Query: the Query reports the statement's write, got {queried}"
    );

    // A statement deletes; both surfaces agree it is gone.
    run(&router, "DELETE FROM items WHERE pk = 'shared'", Value::Null).await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "items", "Key": {"pk": {"S": "shared"}}}),
    )
    .await;
    assert_eq!(
        read,
        json!({}),
        "case DELETE then GetItem: the item is gone, got {read}"
    );
    let selected = run(
        &router,
        "SELECT * FROM items WHERE pk = 'shared'",
        Value::Null,
    )
    .await;
    assert_eq!(
        selected["Items"],
        json!([]),
        "case DELETE then SELECT: the statement reports no items, got {selected}"
    );
}

#[tokio::test]
async fn a_select_over_an_index_reports_the_items_the_index_holds_as_it_projects_them() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "items",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "indexed", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [{
                "IndexName": "by-indexed",
                "KeySchema": [{"AttributeName": "indexed", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "KEYS_ONLY"},
            }],
        }),
    )
    .await;
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));

    for item in [
        json!({"pk": {"S": "in"}, "indexed": {"S": "yes"}, "other": {"S": "dropped"}}),
        json!({"pk": {"S": "out"}, "other": {"S": "no index key"}}),
    ] {
        ok(&router, "PutItem", json!({"TableName": "items", "Item": item})).await;
    }

    let selected = run(&router, "SELECT * FROM \"items\".\"by-indexed\"", Value::Null).await;
    assert_eq!(
        selected["Items"],
        json!([{"pk": {"S": "in"}, "indexed": {"S": "yes"}}]),
        "case index source: an index holds only the items carrying its key, projected as it \
         declares, got {selected}"
    );

    let (status, refused) = call(
        &router,
        "ExecuteStatement",
        json!({"Statement": "SELECT * FROM \"items\".\"absent\""}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case unknown index: expected HTTP 400, got {status} with {refused}"
    );
    // An index the table does not declare is a bad STATEMENT — the table was resolved,
    // and nothing it carries answers to that name — so AWS states it as such rather than
    // as a resource that could not be found.
    assert_eq!(
        (
            error_code(&refused),
            message(&refused),
        ),
        (
            "ValidationException".to_string(),
            "The table does not have the specified index".to_string(),
        ),
        "case unknown index: expected the index-unknown ValidationException, got {refused}"
    );
}

/// One refusal row: the statement, the status and the wire error code it must be
/// answered with.
struct RefusalRow {
    case: &'static str,
    statement: &'static str,
    parameters: Value,
    code: &'static str,
}

/// The rows span the ways a statement can be wrong: text that is no statement, a
/// write whose key is already stored, a write that addresses no stored item, a write
/// that addresses more than one, a source that does not exist, an order this surface
/// does not serve, a value no set can hold and a parameter list that does not match
/// the statement.
fn refusal_table() -> Vec<RefusalRow> {
    vec![
        RefusalRow {
            case: "text that is not a statement",
            statement: "SELCT * FROM items",
            parameters: Value::Null,
            code: "ValidationException",
        },
        RefusalRow {
            case: "an INSERT of a key the table already holds",
            statement: "INSERT INTO items VALUE {'pk': 'r', 'n': 1}",
            parameters: Value::Null,
            code: "DuplicateItemException",
        },
        RefusalRow {
            case: "an UPDATE of a key that holds no item",
            statement: "UPDATE items SET n = 2 WHERE pk = 'absent'",
            parameters: Value::Null,
            code: "ConditionalCheckFailedException",
        },
        RefusalRow {
            case: "a DELETE of a key that holds no item",
            statement: "DELETE FROM items WHERE pk = 'absent'",
            parameters: Value::Null,
            code: "ConditionalCheckFailedException",
        },
        RefusalRow {
            case: "an UPDATE whose predicate the stored item does not satisfy",
            statement: "UPDATE items SET n = 2 WHERE pk = 'r' AND n = 99",
            parameters: Value::Null,
            code: "ConditionalCheckFailedException",
        },
        RefusalRow {
            case: "an UPDATE whose WHERE clause pins no key, so it addresses no one item",
            statement: "UPDATE items SET n = 2 WHERE n = 1",
            parameters: Value::Null,
            code: "ValidationException",
        },
        RefusalRow {
            case: "an UPDATE whose WHERE clause pins the key only inside an OR",
            statement: "UPDATE items SET n = 2 WHERE pk = 'r' OR pk = 'other'",
            parameters: Value::Null,
            code: "ValidationException",
        },
        RefusalRow {
            case: "an UPDATE of a key attribute",
            statement: "UPDATE items SET pk = 'moved' WHERE pk = 'r'",
            parameters: Value::Null,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a source that is not a table of this store",
            statement: "SELECT * FROM absent",
            parameters: Value::Null,
            code: "ResourceNotFoundException",
        },
        RefusalRow {
            case: "ORDER BY on an attribute that is not a key",
            statement: "SELECT * FROM items WHERE pk = 'r' ORDER BY n",
            parameters: Value::Null,
            code: "ValidationException",
        },
        RefusalRow {
            case: "ORDER BY without an equality on the partition key, which spans partitions",
            statement: "SELECT * FROM items ORDER BY pk",
            parameters: Value::Null,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a set literal with no members",
            statement: "INSERT INTO items VALUE {'pk': 'empty-set', 's': <<>>}",
            parameters: Value::Null,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a set literal whose members are not all of one type",
            statement: "INSERT INTO items VALUE {'pk': 'mixed-set', 's': <<1, 'two'>>}",
            parameters: Value::Null,
            code: "ValidationException",
        },
        RefusalRow {
            case: "more parameters than the statement names",
            statement: "SELECT * FROM items WHERE pk = ?",
            parameters: json!([{"S": "r"}, {"S": "spare"}]),
            code: "ValidationException",
        },
        RefusalRow {
            case: "fewer parameters than the statement names",
            statement: "SELECT * FROM items WHERE pk = ? AND n = ?",
            parameters: json!([{"S": "r"}]),
            code: "ValidationException",
        },
        RefusalRow {
            case: "a DELETE asked to return the item it did not leave",
            statement: "DELETE FROM items WHERE pk = 'r' RETURNING ALL NEW *",
            parameters: Value::Null,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a number past what an N attribute can hold",
            statement: "INSERT INTO items VALUE {'pk': 'huge', 'n': \
                        123456789012345678901234567890123456789}",
            parameters: Value::Null,
            code: "ValidationException",
        },
    ]
}

#[tokio::test]
async fn a_statement_this_surface_cannot_serve_is_refused_rather_than_guessed_at() {
    let router = emulator();
    create_active_table(&router, "items", &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "items", "Item": {"pk": {"S": "r"}, "n": {"N": "1"}}}),
    )
    .await;

    for row in refusal_table() {
        let mut body = json!({"Statement": row.statement});
        if !row.parameters.is_null() {
            body["Parameters"] = row.parameters.clone();
        }
        let (status, refused) = call(&router, "ExecuteStatement", body).await;
        assert_eq!(
            status, 400,
            "case {}: expected HTTP 400, got {status} with {refused}",
            row.case
        );
        assert_eq!(
            error_code(&refused),
            row.code,
            "case {}: expected {}, got {refused}",
            row.case,
            row.code
        );
    }

    // Nothing any of those refusals addressed changed in the store.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "items", "Key": {"pk": {"S": "r"}}}),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({"pk": {"S": "r"}, "n": {"N": "1"}}),
        "case refusals leave the store alone: got {read}"
    );
}

#[tokio::test]
async fn a_statement_past_this_surface_s_own_bounds_is_refused() {
    let router = emulator();
    create_active_table(&router, "items", &[("pk", "HASH", "S")]).await;

    // A value nested past the DOCUMENTED attribute depth (the ceiling every write path
    // holds a value to), a predicate nested past the grammar's own depth, and a
    // statement holding more tokens than this surface parses: each bound is a named
    // constant, so each row is generated from that constant rather than hard-coded.
    let nested_value = {
        let mut text = "0".to_string();
        for _ in 0..=MAX_ATTRIBUTE_DEPTH {
            text = format!("{{'a': {text}}}");
        }
        format!("INSERT INTO items VALUE {{'pk': 'deep', 'v': {text}}}")
    };
    let nested_predicate = {
        let mut text = "pk = 'r'".to_string();
        for _ in 0..=MAX_STATEMENT_DEPTH {
            text = format!("({text})");
        }
        format!("SELECT * FROM items WHERE {text}")
    };
    let many_tokens = format!(
        "SELECT * FROM items WHERE pk IN [{}]",
        "1,".repeat(MAX_STATEMENT_TOKENS / 2).trim_end_matches(',')
    );

    // A value nested deeper than the GRAMMAR's own ceiling but inside the documented
    // attribute depth is stored and read back: the statement surface accepts the items
    // the classic api accepts.
    let mut deep_text = "0".to_string();
    let mut deep_value = json!({"N": "0"});
    for _ in 0..MAX_STATEMENT_DEPTH + 4 {
        deep_text = format!("{{'a': {deep_text}}}");
        deep_value = json!({"M": {"a": deep_value}});
    }
    run(
        &router,
        &format!("INSERT INTO items VALUE {{'pk': 'nested', 'v': {deep_text}}}"),
        Value::Null,
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "items", "Key": {"pk": {"S": "nested"}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["v"], deep_value,
        "case a value nested inside the documented attribute depth: expected {deep_value}, \
         got {read}"
    );

    for (case, statement) in [
        (
            "a statement that stops in the middle of its FROM clause",
            "SELECT * FROM".to_string(),
        ),
        (
            "a statement that stops after SET",
            "UPDATE items SET".to_string(),
        ),
        (
            "a statement that is one unterminated string literal",
            "SELECT * FROM items WHERE pk = 'unterminated".to_string(),
        ),
        (
            "a value nested past the documented attribute depth",
            nested_value,
        ),
        (
            "a predicate nested past the depth it parses",
            nested_predicate,
        ),
        (
            "a statement holding more tokens than it parses",
            many_tokens,
        ),
    ] {
        let (status, refused) = call(
            &router,
            "ExecuteStatement",
            json!({"Statement": statement}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {refused}"
        );
    }
}

#[tokio::test]
async fn a_failed_condition_reports_the_stored_item_when_the_request_asks_for_it() {
    let router = emulator();
    create_active_table(&router, "items", &[("pk", "HASH", "S")]).await;
    let stored = json!({"pk": {"S": "c"}, "n": {"N": "1"}});
    ok(
        &router,
        "PutItem",
        json!({"TableName": "items", "Item": stored}),
    )
    .await;

    for (case, asked, expected) in [
        ("without the member", Value::Null, Value::Null),
        ("with ALL_OLD", json!("ALL_OLD"), stored.clone()),
        ("with NONE", json!("NONE"), Value::Null),
    ] {
        let mut body = json!({"Statement": "UPDATE items SET n = 2 WHERE pk = 'c' AND n = 99"});
        if !asked.is_null() {
            body["ReturnValuesOnConditionCheckFailure"] = asked.clone();
        }
        let (status, refused) = call(&router, "ExecuteStatement", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConditionalCheckFailedException",
            "case {case}: expected ConditionalCheckFailedException, got {refused}"
        );
        let reported = refused.get("Item").cloned().unwrap_or(Value::Null);
        assert_eq!(
            reported, expected,
            "case {case}: expected the refusal to carry {expected}, got {refused}"
        );
    }
}
