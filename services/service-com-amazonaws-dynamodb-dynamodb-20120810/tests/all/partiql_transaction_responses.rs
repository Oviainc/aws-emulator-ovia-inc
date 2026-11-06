// SPDX-License-Identifier: CC0-1.0

//! What `ExecuteTransaction` reports in `Responses`: the ITEMS its statements returned.
//!
//! `ItemResponse` carries a single optional `Item` and nothing else, so an entry for a
//! statement that returned none states nothing a caller can read. The rule the rows below
//! assert is that the list holds one entry per item a statement returned: a write returns
//! one only when its `RETURNING` clause asks for it, so a transaction of writes that ask
//! for no image reports an empty list rather than one empty structure per statement, while
//! a read returns the item its key addresses — an empty response where that key holds
//! none, exactly as a transactional read reports a missing item.
//!
//! The rows differ in how many statements the transaction carries, in which of them state
//! a `RETURNING` clause, in which image that clause asks for, and in which verb writes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, ok};
use crate::table_items::create_active_table;

/// The table every statement below addresses.
const TABLE: &str = "ledger";

/// One `TransactStatements` entry.
fn statement(text: &str) -> Value {
    json!({"Statement": text})
}

/// The `Responses` entries a transaction reported, in the order it reported them.
fn responses(body: &Value) -> Vec<Value> {
    body["Responses"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the transaction reports a Responses list, got {body}"))
}

/// Run one transaction that must succeed.
async fn transact(router: &MultiServiceRouter, case: &str, statements: Vec<Value>) -> Value {
    let (status, body) = call(
        router,
        "ExecuteTransaction",
        json!({"TransactStatements": statements}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// An emulator holding one table with three items in it.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("id", "HASH", "S")]).await;
    for (id, balance) in [("a", 10), ("b", 20), ("c", 30)] {
        ok(
            &router,
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"id": {"S": id}, "balance": {"N": balance.to_string()}},
            }),
        )
        .await;
    }
    router
}

#[tokio::test]
async fn a_transaction_reports_one_response_per_item_its_statements_returned() {
    let router = seeded().await;

    // Each row: the statements, and the whole `Responses` list the transaction must
    // report for them.
    let rows: Vec<(&str, Vec<Value>, Vec<Value>)> = vec![
        (
            "two writes that ask for no image",
            vec![
                statement("INSERT INTO ledger VALUE {'id': 'fresh', 'balance': 1}"),
                statement("UPDATE ledger SET balance = 11 WHERE id = 'a'"),
            ],
            vec![],
        ),
        (
            "one write of two asking for the image it left",
            vec![
                statement("UPDATE ledger SET balance = 12 WHERE id = 'a' RETURNING ALL NEW *"),
                statement("UPDATE ledger SET balance = 21 WHERE id = 'b'"),
            ],
            vec![json!({"Item": {"id": {"S": "a"}, "balance": {"N": "12"}}})],
        ),
        (
            "both writes asking for an image, one old and one new",
            vec![
                statement("UPDATE ledger SET balance = 13 WHERE id = 'a' RETURNING ALL OLD *"),
                statement("UPDATE ledger SET balance = 22 WHERE id = 'b' RETURNING ALL NEW *"),
            ],
            vec![
                json!({"Item": {"id": {"S": "a"}, "balance": {"N": "12"}}}),
                json!({"Item": {"id": {"S": "b"}, "balance": {"N": "22"}}}),
            ],
        ),
        (
            "a delete asking for the item it removed",
            vec![statement(
                "DELETE FROM ledger WHERE id = 'fresh' RETURNING ALL OLD *",
            )],
            vec![json!({"Item": {"id": {"S": "fresh"}, "balance": {"N": "1"}}})],
        ),
        (
            "a delete asking for no image",
            vec![statement("DELETE FROM ledger WHERE id = 'c'")],
            vec![],
        ),
    ];

    for (case, statements, expected) in rows {
        let body = transact(&router, case, statements).await;
        assert_eq!(
            responses(&body),
            expected,
            "case {case}: expected the responses {expected:?}, got {body}"
        );
    }

    // A READ reports the item its key addresses: one entry per statement, and an empty
    // response for the key that holds none.
    let read = transact(
        &router,
        "a read of a stored key and of an absent one",
        vec![
            statement("SELECT * FROM ledger WHERE id = 'a'"),
            statement("SELECT * FROM ledger WHERE id = 'absent'"),
            statement("SELECT balance FROM ledger WHERE id = 'b'"),
        ],
    )
    .await;
    assert_eq!(
        responses(&read),
        vec![
            json!({"Item": {"id": {"S": "a"}, "balance": {"N": "13"}}}),
            json!({}),
            json!({"Item": {"balance": {"N": "22"}}}),
        ],
        "case a read of a stored key and of an absent one: one response per statement, got \
         {read}"
    );

    // The writes above landed, so the entries each row reported are the items its
    // statements really returned.
    for (case, id, stored) in [
        ("the updated item", "a", json!({"id": {"S": "a"}, "balance": {"N": "13"}})),
        ("the second updated item", "b", json!({"id": {"S": "b"}, "balance": {"N": "22"}})),
        ("the deleted item", "c", Value::Null),
        ("the inserted item a later statement deleted", "fresh", Value::Null),
    ] {
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": TABLE, "Key": {"id": {"S": id}}}),
        )
        .await;
        assert_eq!(
            read.get("Item").cloned().unwrap_or(Value::Null),
            stored,
            "case {case}: expected the store to hold {stored}, got {read}"
        );
    }
}
