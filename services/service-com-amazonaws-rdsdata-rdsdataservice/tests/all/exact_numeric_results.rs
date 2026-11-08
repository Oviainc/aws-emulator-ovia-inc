// SPDX-License-Identifier: CC0-1.0

//! What an EXACT NUMBER a statement computes comes back as, over the real wire.
//!
//! Every call goes through the router, so routing, restJson1 deserialization, the
//! handler and serialization are all exercised; the client harness is the one
//! `sql_data_api` builds.

use serde_json::{json, Value};

use crate::sql_data_api::{list, DataApi};

/// The table every case below computes over: an exact column at two scales, a wide
/// integer column, an inexact column, a narrow integer column, and a second table to
/// join with, so a column's type is found through the join as well as directly.
async fn ledger() -> DataApi {
    let api = DataApi::new();
    api.run(json!({
        "sql": "CREATE TABLE sales (cust VARCHAR(16), amount NUMERIC(12,2), \
                fine NUMERIC(12,4), qty BIGINT, ratio DOUBLE PRECISION, lines INT)",
    }))
    .await;
    api.run(json!({"sql": "CREATE TABLE regions (customer VARCHAR(16), area VARCHAR(8))"}))
        .await;
    for (cust, amount, fine, qty, ratio, lines) in [
        ("alice", "10.25", "0.0001", 9_007_199_254_740_993_i64, 0.5, 1),
        ("alice", "20.50", "1.2345", 9_007_199_254_740_993_i64, 0.25, 2),
        ("bob", "5.00", "2.0000", 1, 1.5, 3),
    ] {
        api.run(json!({
            "sql": format!(
                "INSERT INTO sales (cust, amount, fine, qty, ratio, lines) \
                 VALUES ('{cust}', {amount}, {fine}, {qty}, {ratio}, {lines})"
            ),
        }))
        .await;
    }
    api.run(json!({
        "sql": "INSERT INTO regions (customer, area) VALUES ('alice', 'north'), \
                ('bob', 'south')",
    }))
    .await;
    api
}

/// The one value a one-row, one-column result set carries, in each of the two forms a
/// result set is returned in: the `Field` of `records`, and the JSON value of
/// `formattedRecords`.
async fn computed(api: &DataApi, projection: &str) -> (Value, Value) {
    let records = api
        .run(json!({"sql": format!("SELECT {projection} AS computed FROM sales")}))
        .await;
    let field = list(&records, "records")
        .first()
        .and_then(|row| row.get(0).cloned())
        .unwrap_or_else(|| panic!("one row of one column: {records}"));
    let formatted = api
        .run(json!({
            "sql": format!("SELECT {projection} AS computed FROM sales"),
            "formatRecordsAs": "JSON",
        }))
        .await;
    let text = formatted["formattedRecords"]
        .as_str()
        .unwrap_or_else(|| panic!("formattedRecords must be a string: {formatted}"));
    let document: Value = serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("formattedRecords must parse ({error}): {text}"));
    let rendered = document
        .get(0)
        .and_then(|row| row.get("computed").cloned())
        .unwrap_or_else(|| panic!("one row with the projected column: {text}"));
    (field, rendered)
}

/// RULE: an EXACT number — one of PostgreSQL's `numeric`/`decimal` types, and anything
/// an aggregate or a window function computes exactly over one (a SUM, an average, the
/// smallest or the largest value, and a SUM over a `bigint` column, which PostgreSQL
/// computes as `numeric` because a `bigint` could not hold the total) — comes back in
/// the `stringValue` arm of the `Field` union, carrying the digits and the scale the
/// operands have, and as a JSON STRING in `formattedRecords`: neither form turns an
/// exact number into a float. An INEXACT number (`real`, `double precision`, and what
/// an aggregate computes over one) is `doubleValue`, and a count or a sum over a
/// narrower integer is `longValue`, as `bigint` is.
///
/// The rows span what the rule generalises over: both number classes and three integer
/// widths, the aggregates and window functions whose result type its argument decides,
/// the scale the operands declare (including a total whose own digits are fewer), and a
/// total beyond the 2^53 a float holds exactly, which is where an inexact accumulation
/// would differ.
#[tokio::test]
async fn an_exact_number_comes_back_as_text_in_both_forms() {
    let api = ledger().await;
    // (case, the projection, the `Field` it comes back in, its JSON form)
    let cases = [
        (
            "the largest value of an exact column",
            "MAX(amount)",
            json!({"stringValue": "20.50"}),
            json!("20.50"),
        ),
        (
            "the smallest value of an exact column",
            "MIN(amount)",
            json!({"stringValue": "5.00"}),
            json!("5.00"),
        ),
        (
            "a SUM over an exact column",
            "SUM(amount)",
            json!({"stringValue": "35.75"}),
            json!("35.75"),
        ),
        (
            // 0.0001 + 1.2345 + 2.0000, at the scale the column declares.
            "a SUM over an exact column of another scale",
            "SUM(fine)",
            json!({"stringValue": "3.2346"}),
            json!("3.2346"),
        ),
        (
            // The total's own digits are fewer than the scale, so the scale is what
            // the text carries.
            "a SUM whose total is whole",
            "SUM(amount) FILTER (WHERE cust = 'bob')",
            json!({"stringValue": "5.00"}),
            json!("5.00"),
        ),
        (
            // 9007199254740993 twice: 18014398509481986, which no double holds.
            "a SUM over a wide integer column",
            "SUM(qty) FILTER (WHERE cust = 'alice')",
            json!({"stringValue": "18014398509481986"}),
            json!("18014398509481986"),
        ),
        (
            "an average over an exact column",
            "AVG(amount) FILTER (WHERE cust = 'alice')",
            json!({"stringValue": "15.375"}),
            json!("15.375"),
        ),
        (
            "a SUM over a narrower integer column",
            "SUM(lines)",
            json!({"longValue": 6}),
            json!(6),
        ),
        (
            "a count of rows",
            "COUNT(*)",
            json!({"longValue": 3}),
            json!(3),
        ),
        (
            "a SUM over an inexact column",
            "SUM(ratio)",
            json!({"doubleValue": 2.25}),
            json!(2.25),
        ),
        (
            "an average over an inexact column",
            "AVG(ratio)",
            json!({"doubleValue": 0.75}),
            json!(0.75),
        ),
    ];

    for (case, projection, as_field, as_json) in cases {
        let (field, rendered) = computed(&api, projection).await;
        assert_eq!(
            field, as_field,
            "case {case:?}: expected the records form {as_field}, got {field}"
        );
        assert_eq!(
            rendered, as_json,
            "case {case:?}: expected the JSON form {as_json}, got {rendered}"
        );
    }
}

/// RULE (the same one, over the statement forms a client really writes): a grouped
/// aggregate, an aggregate behind a HAVING clause, an aggregate over a JOIN and a
/// window function all have the type their argument gives them, so an exact one is
/// `stringValue` there too — the type comes from the column the aggregate reads,
/// wherever the statement reads it from.
#[tokio::test]
async fn a_grouped_or_joined_exact_aggregate_is_text_too() {
    let api = ledger().await;

    let grouped = api
        .run(json!({
            "sql": "SELECT cust, SUM(amount) AS total FROM sales GROUP BY cust \
                    HAVING SUM(amount) > 10 ORDER BY cust",
            "includeResultMetadata": true,
        }))
        .await;
    assert_eq!(
        grouped["records"],
        json!([[{"stringValue": "alice"}, {"stringValue": "30.75"}]]),
        "case \"a grouped SUM behind a HAVING clause\": got {grouped}"
    );
    assert_eq!(
        list(&grouped, "columnMetadata")[1]["typeName"],
        json!("numeric"),
        "case \"a grouped SUM behind a HAVING clause\": the total is an exact number, \
         got {grouped}"
    );

    let joined = api
        .run(json!({
            "sql": "SELECT r.area, SUM(s.amount) AS total FROM sales AS s \
                    JOIN regions AS r ON r.customer = s.cust GROUP BY r.area \
                    ORDER BY r.area",
        }))
        .await;
    assert_eq!(
        joined["records"],
        json!([
            [{"stringValue": "north"}, {"stringValue": "30.75"}],
            [{"stringValue": "south"}, {"stringValue": "5.00"}],
        ]),
        "case \"a SUM over a join\": got {joined}"
    );

    let windowed = api
        .run(json!({
            "sql": "SELECT cust, SUM(amount) OVER (PARTITION BY cust) AS running \
                    FROM sales ORDER BY cust, running",
        }))
        .await;
    assert_eq!(
        windowed["records"],
        json!([
            [{"stringValue": "alice"}, {"stringValue": "30.75"}],
            [{"stringValue": "alice"}, {"stringValue": "30.75"}],
            [{"stringValue": "bob"}, {"stringValue": "5.00"}],
        ]),
        "case \"a window SUM per customer\": got {windowed}"
    );
}
