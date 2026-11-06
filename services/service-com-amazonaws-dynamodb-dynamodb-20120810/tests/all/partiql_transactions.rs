// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ExecuteTransaction`: a list of PartiQL statements run as ONE
//! transaction over the SAME item store the classic item API and `ExecuteStatement`
//! read and write.
//!
//! The rules these tables assert are the transaction's own, and each table spans what
//! the rule generalises over rather than one recorded call:
//!
//! 1. A WRITE transaction is all-or-nothing: every statement's `WHERE` clause is read
//!    against the state as it stood BEFORE the transaction, and one unmet predicate
//!    leaves the store untouched and answers `TransactionCanceledException` with one
//!    reason per statement, in request order. The rows span the ways one statement can
//!    fail (a key an `INSERT` would replace, an `UPDATE` or a `DELETE` of a key holding
//!    no item, a further predicate the stored item does not satisfy), which POSITION it
//!    fails in, and which of the two tables it addresses.
//! 2. A READ transaction reports the ONE item each `SELECT` addresses, in statement
//!    order, projected as the statement asks, and nothing for a key that holds no item.
//! 3. A transaction is refused as a WHOLE — and applies nothing — when the request
//!    itself is wrong: reads mixed with writes, a statement that does not address one
//!    item, and one item addressed twice.
//! 4. A `ClientRequestToken` makes a write transaction idempotent: a repeat carrying the
//!    same statements applies nothing, and a repeat carrying different statements is
//!    `IdempotentParameterMismatchException`.
//! 5. A transactional read or write consumes twice what the same single-item one does,
//!    reported one entry per statement.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, ok};
use crate::table_items::create_active_table;

/// The table a transaction's first statement addresses.
const ACCOUNTS: &str = "accounts";

/// The SECOND table, so every table below spans a transaction over more than one of
/// them: "these actions can target items in different tables".
const AUDIT: &str = "audit";

/// Create both tables of the two-table fixture, each keyed by one string attribute.
async fn two_tables(router: &MultiServiceRouter) {
    create_active_table(router, ACCOUNTS, &[("id", "HASH", "S")]).await;
    create_active_table(router, AUDIT, &[("id", "HASH", "S")]).await;
}

/// The `TransactStatements` entry one statement is sent as.
fn statement(text: &str) -> Value {
    json!({"Statement": text})
}

/// The same, carrying the values its `?` markers stand for.
fn parameterized(text: &str, parameters: Value) -> Value {
    json!({"Statement": text, "Parameters": parameters})
}

/// Run one transaction and return its HTTP status and parsed body.
async fn transact(router: &MultiServiceRouter, statements: Vec<Value>) -> (u16, Value) {
    call(
        router,
        "ExecuteTransaction",
        json!({"TransactStatements": statements}),
    )
    .await
}

/// The item stored at `id` of `table`, read back through the classic reader, or `Null`
/// when that key holds none.
async fn stored(router: &MultiServiceRouter, table: &str, id: &str) -> Value {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": table, "Key": {"id": {"S": id}}}),
    )
    .await;
    read.get("Item").cloned().unwrap_or(Value::Null)
}

/// The `CancellationReasons` codes a refusal carries, in the order it carries them.
fn reason_codes(body: &Value) -> Vec<String> {
    body["CancellationReasons"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|reason| {
            reason["Code"]
                .as_str()
                .unwrap_or("<no Code>")
                .to_string()
        })
        .collect()
}

/// The `Responses` entries a transaction reported, in the order it reported them.
fn responses(body: &Value) -> Vec<Value> {
    body["Responses"].as_array().cloned().unwrap_or_default()
}

/// The `CapacityUnits` of each `ConsumedCapacity` entry, in order.
fn capacity_units(body: &Value) -> Vec<f64> {
    body["ConsumedCapacity"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["CapacityUnits"].as_f64().unwrap_or(f64::NAN))
        .collect()
}

/// The `(WriteCapacityUnits, ReadCapacityUnits)` of each `ConsumedCapacity` entry, in
/// order: a transaction is wholly writes or wholly reads, so exactly one of the two is
/// reported, and it is the aggregate.
fn units_by_kind(body: &Value) -> Vec<(Option<f64>, Option<f64>)> {
    body["ConsumedCapacity"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["WriteCapacityUnits"].as_f64(),
                entry["ReadCapacityUnits"].as_f64(),
            )
        })
        .collect()
}

/// One row of the cancellation table: the statements the transaction carries and the
/// reason code each of them must report.
struct CancelRow {
    case: &'static str,
    statements: Vec<Value>,
    codes: Vec<&'static str>,
}

/// The rows differ in HOW one statement fails (a key an `INSERT` would replace, an
/// `UPDATE` of a key holding no item, a further predicate the stored item does not
/// satisfy, a `DELETE` of a key holding no item), in WHICH position in the list fails,
/// in which of the two tables that statement addresses, and in how many statements the
/// transaction carries.
fn cancel_table() -> Vec<CancelRow> {
    vec![
        CancelRow {
            case: "an INSERT of a key the table already holds, first of two",
            statements: vec![
                statement("INSERT INTO accounts VALUE {'id': 'a1', 'balance': 1}"),
                statement("INSERT INTO audit VALUE {'id': 'fresh', 'note': 'unwritten'}"),
            ],
            codes: vec!["ConditionalCheckFailed", "None"],
        },
        CancelRow {
            case: "an UPDATE of a key that holds no item, second of two",
            statements: vec![
                statement("UPDATE accounts SET balance = 50 WHERE id = 'a1'"),
                statement("UPDATE audit SET note = 'moved' WHERE id = 'absent'"),
            ],
            codes: vec!["None", "ConditionalCheckFailed"],
        },
        CancelRow {
            case: "a further predicate the stored item does not satisfy, second of three",
            statements: vec![
                statement("UPDATE audit SET note = 'first' WHERE id = 'e1'"),
                statement("UPDATE accounts SET balance = 7 WHERE id = 'a1' AND balance = 999"),
                statement("INSERT INTO audit VALUE {'id': 'fresh', 'note': 'unwritten'}"),
            ],
            codes: vec!["None", "ConditionalCheckFailed", "None"],
        },
        CancelRow {
            case: "a DELETE of a key that holds no item, alone",
            statements: vec![statement("DELETE FROM audit WHERE id = 'absent'")],
            codes: vec!["ConditionalCheckFailed"],
        },
        CancelRow {
            case: "two statements that both fail, in two tables",
            statements: vec![
                statement("DELETE FROM accounts WHERE id = 'absent'"),
                statement("INSERT INTO audit VALUE {'id': 'e1', 'note': 'replaced'}"),
            ],
            codes: vec!["ConditionalCheckFailed", "ConditionalCheckFailed"],
        },
    ]
}

#[tokio::test]
async fn a_write_transaction_lands_every_statement_or_none() {
    let router = emulator();
    two_tables(&router).await;

    // A transaction every statement of which applies: both tables are written, and the
    // writes are visible to the classic reader.
    let applied = ok(
        &router,
        "ExecuteTransaction",
        json!({
            "TransactStatements": [
                parameterized(
                    "INSERT INTO accounts VALUE {'id': ?, 'balance': ?}",
                    json!([{"S": "a1"}, {"N": "100"}]),
                ),
                statement("INSERT INTO audit VALUE {'id': 'e1', 'note': 'opened'}"),
            ],
            "ReturnConsumedCapacity": "TOTAL",
        }),
    )
    .await;
    // Neither `INSERT` states a `RETURNING` clause, so neither produced an item: an
    // `ItemResponse` carries nothing but an item, so the transaction reports no entry at
    // all rather than one empty structure per statement.
    assert_eq!(
        responses(&applied),
        Vec::<Value>::new(),
        "case multi-table INSERT: a write returning no image reports no response entry, got \
         {applied}"
    );
    // "Transactional write requests require two write capacity units": each statement
    // writes one small item, so each entry is twice the one unit the same single-item
    // write costs, reported in statement order.
    assert_eq!(
        capacity_units(&applied),
        vec![2.0, 2.0],
        "case multi-table INSERT: each statement consumes twice a single-item write, got \
         {applied}"
    );
    assert_eq!(
        units_by_kind(&applied),
        vec![(Some(2.0), None), (Some(2.0), None)],
        "case multi-table INSERT: a write transaction bills write capacity only, got {applied}"
    );
    assert_eq!(
        stored(&router, ACCOUNTS, "a1").await,
        json!({"id": {"S": "a1"}, "balance": {"N": "100"}}),
        "case multi-table INSERT: the first statement's item is stored"
    );
    assert_eq!(
        stored(&router, AUDIT, "e1").await,
        json!({"id": {"S": "e1"}, "note": {"S": "opened"}}),
        "case multi-table INSERT: the second statement's item is stored"
    );

    // Every row below must leave exactly that state behind, whichever statement of it
    // failed.
    for row in cancel_table() {
        let (status, body) = transact(&router, row.statements.clone()).await;
        assert_eq!(
            status, 400,
            "case {}: a cancelled transaction is refused, got {status} with {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "TransactionCanceledException",
            "case {}: expected TransactionCanceledException, got {body}",
            row.case
        );
        assert_eq!(
            reason_codes(&body),
            row.codes,
            "case {}: expected the reasons {:?}, got {body}",
            row.case,
            row.codes
        );
        assert_eq!(
            stored(&router, ACCOUNTS, "a1").await,
            json!({"id": {"S": "a1"}, "balance": {"N": "100"}}),
            "case {}: a cancelled transaction writes nothing",
            row.case
        );
        assert_eq!(
            stored(&router, AUDIT, "e1").await,
            json!({"id": {"S": "e1"}, "note": {"S": "opened"}}),
            "case {}: a cancelled transaction writes nothing to the other table",
            row.case
        );
        assert_eq!(
            stored(&router, AUDIT, "fresh").await,
            Value::Null,
            "case {}: the statement beside the failing one landed nothing either",
            row.case
        );
    }

    // An UPDATE and a DELETE in one transaction, every predicate of which holds, apply
    // together — and a `RETURNING` clause reports the item through that statement's own
    // response.
    let changed = ok(
        &router,
        "ExecuteTransaction",
        json!({"TransactStatements": [
            statement("UPDATE accounts SET balance = balance - 40 WHERE id = 'a1' RETURNING ALL NEW *"),
            statement("DELETE FROM audit WHERE id = 'e1'"),
        ]}),
    )
    .await;
    assert_eq!(
        responses(&changed).first().cloned().unwrap_or(Value::Null),
        json!({"Item": {"id": {"S": "a1"}, "balance": {"N": "60"}}}),
        "case UPDATE then DELETE: RETURNING ALL NEW reports the item the write left, got \
         {changed}"
    );
    assert_eq!(
        stored(&router, ACCOUNTS, "a1").await,
        json!({"id": {"S": "a1"}, "balance": {"N": "60"}}),
        "case UPDATE then DELETE: the arithmetic the statement asked for is stored"
    );
    assert_eq!(
        stored(&router, AUDIT, "e1").await,
        Value::Null,
        "case UPDATE then DELETE: the delete landed in the other table"
    );
}

/// One row of the read table: the statement, and the response its slot must carry.
struct ReadRow {
    case: &'static str,
    statement: Value,
    response: Value,
}

/// The rows differ in which table the statement reads, in whether its key holds an item
/// at all, in whether a further predicate holds, in whether the key is pinned by
/// literals or by `?` markers, and in whether the statement projects attributes. Each
/// row addresses an item of its own, because one transaction cannot address one item
/// twice.
fn read_table() -> Vec<ReadRow> {
    vec![
        ReadRow {
            case: "every key attribute pinned by a literal",
            statement: statement("SELECT * FROM accounts WHERE id = 'a1'"),
            response: json!({"Item": {"id": {"S": "a1"}, "balance": {"N": "100"}}}),
        },
        ReadRow {
            case: "the key pinned by a ? marker",
            statement: parameterized("SELECT * FROM audit WHERE id = ?", json!([{"S": "e1"}])),
            response: json!({"Item": {"id": {"S": "e1"}, "note": {"S": "opened"}}}),
        },
        ReadRow {
            case: "a projection reports only the attributes it names",
            statement: statement("SELECT balance FROM accounts WHERE id = 'a2'"),
            response: json!({"Item": {"balance": {"N": "200"}}}),
        },
        ReadRow {
            case: "the addressed key holds no item",
            statement: statement("SELECT * FROM accounts WHERE id = 'absent'"),
            response: json!({}),
        },
        ReadRow {
            case: "a further predicate the stored item does not satisfy",
            statement: statement("SELECT * FROM accounts WHERE id = 'a3' AND balance = 999"),
            response: json!({}),
        },
    ]
}

/// One row of the refusal table: the statements, and the error code the whole request
/// must be refused with.
struct RefusalRow {
    case: &'static str,
    statements: Vec<Value>,
}

/// Each row is a request that is wrong in ITSELF — not a statement whose predicate
/// failed — so each is a `ValidationException` and none of them writes anything.
fn refusal_table() -> Vec<RefusalRow> {
    vec![
        RefusalRow {
            case: "a read mixed with a write",
            statements: vec![
                statement("SELECT * FROM accounts WHERE id = 'a1'"),
                statement("INSERT INTO audit VALUE {'id': 'mixed', 'note': 'unwritten'}"),
            ],
        },
        RefusalRow {
            case: "a read that pins no key at all",
            statements: vec![statement("SELECT * FROM accounts")],
        },
        RefusalRow {
            case: "a read whose key is pinned by an inequality",
            statements: vec![statement("SELECT * FROM accounts WHERE id > 'a'")],
        },
        RefusalRow {
            case: "a read asking for an order",
            statements: vec![statement(
                "SELECT * FROM accounts WHERE id = 'a1' ORDER BY id DESC",
            )],
        },
        RefusalRow {
            case: "two reads of one item",
            statements: vec![
                statement("SELECT * FROM accounts WHERE id = 'a1'"),
                statement("SELECT balance FROM accounts WHERE id = 'a1'"),
            ],
        },
        RefusalRow {
            case: "two writes of one item",
            statements: vec![
                statement("UPDATE accounts SET balance = 1 WHERE id = 'a1'"),
                statement("DELETE FROM accounts WHERE id = 'a1'"),
            ],
        },
    ]
}

#[tokio::test]
async fn a_read_transaction_reports_the_item_each_statement_addresses() {
    let router = emulator();
    two_tables(&router).await;
    ok(
        &router,
        "ExecuteTransaction",
        json!({"TransactStatements": [
            statement("INSERT INTO accounts VALUE {'id': 'a1', 'balance': 100}"),
            statement("INSERT INTO accounts VALUE {'id': 'a2', 'balance': 200}"),
            statement("INSERT INTO accounts VALUE {'id': 'a3', 'balance': 300}"),
            statement("INSERT INTO audit VALUE {'id': 'e1', 'note': 'opened'}"),
        ]}),
    )
    .await;

    let rows = read_table();
    let read = ok(
        &router,
        "ExecuteTransaction",
        json!({
            "TransactStatements": rows.iter().map(|row| row.statement.clone()).collect::<Vec<Value>>(),
            "ReturnConsumedCapacity": "TOTAL",
        }),
    )
    .await;
    let answered = responses(&read);
    assert_eq!(
        answered.len(),
        rows.len(),
        "case read transaction: one response per statement, got {read}"
    );
    for (row, response) in rows.iter().zip(&answered) {
        assert_eq!(
            response, &row.response,
            "case {}: expected the response {}, got {response}",
            row.case, row.response
        );
    }
    // "Transactional read requests require two read capacity units": each statement
    // reads one item well under the 4 KB a read unit covers.
    assert_eq!(
        capacity_units(&read),
        vec![2.0; rows.len()],
        "case read transaction: each statement consumes twice a single-item read, got {read}"
    );
    assert_eq!(
        units_by_kind(&read),
        vec![(None, Some(2.0)); rows.len()],
        "case read transaction: a read transaction bills read capacity only, got {read}"
    );

    // A read transaction changes nothing a later caller sees.
    assert_eq!(
        stored(&router, ACCOUNTS, "a1").await,
        json!({"id": {"S": "a1"}, "balance": {"N": "100"}}),
        "case read transaction: reading leaves the stored item as it was"
    );

    for row in refusal_table() {
        let (status, body) = transact(&router, row.statements.clone()).await;
        assert_eq!(
            status, 400,
            "case {}: expected HTTP 400, got {status} with {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {}: expected ValidationException, got {body}",
            row.case
        );
        assert_eq!(
            stored(&router, AUDIT, "mixed").await,
            Value::Null,
            "case {}: a refused transaction writes nothing",
            row.case
        );
        assert_eq!(
            stored(&router, ACCOUNTS, "a1").await,
            json!({"id": {"S": "a1"}, "balance": {"N": "100"}}),
            "case {}: a refused transaction leaves the stored item as it was",
            row.case
        );
    }
}

/// What one call of the idempotency table must do.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The transaction is applied: the counter moves.
    Applied,
    /// The token already stands for this transaction: the call succeeds and applies
    /// nothing.
    Replayed,
    /// The token already stands for a DIFFERENT transaction: the documented mismatch.
    Mismatch,
}

/// One row of the idempotency table: the token and statement a call carries, what the
/// call must do, and what the counter must read afterwards.
struct TokenRow {
    case: &'static str,
    token: &'static str,
    statement: &'static str,
    parameters: Option<Value>,
    outcome: Outcome,
    counter: &'static str,
}

/// The rows differ in whether the token is fresh or already used, in whether a repeat
/// carries the same statement TEXT, the same `?` PARAMETERS, or different ones, and in
/// the order those calls arrive in.
fn token_table() -> Vec<TokenRow> {
    vec![
        TokenRow {
            case: "a first call under a fresh token applies the transaction",
            token: "token-literal",
            statement: "UPDATE counters SET n = n + 1 WHERE id = 'c'",
            parameters: None,
            outcome: Outcome::Applied,
            counter: "1",
        },
        TokenRow {
            case: "a repeat of that token carrying the same statement applies nothing",
            token: "token-literal",
            statement: "UPDATE counters SET n = n + 1 WHERE id = 'c'",
            parameters: None,
            outcome: Outcome::Replayed,
            counter: "1",
        },
        TokenRow {
            case: "a repeat of that token carrying a different statement is the mismatch",
            token: "token-literal",
            statement: "UPDATE counters SET n = n + 2 WHERE id = 'c'",
            parameters: None,
            outcome: Outcome::Mismatch,
            counter: "1",
        },
        TokenRow {
            case: "a fresh token whose statement states ? markers applies the transaction",
            token: "token-marker",
            statement: "UPDATE counters SET n = n + ? WHERE id = ?",
            parameters: Some(json!([{"N": "1"}, {"S": "c"}])),
            outcome: Outcome::Applied,
            counter: "2",
        },
        TokenRow {
            case: "a repeat of that token carrying different parameter values is the mismatch",
            token: "token-marker",
            statement: "UPDATE counters SET n = n + ? WHERE id = ?",
            parameters: Some(json!([{"N": "5"}, {"S": "c"}])),
            outcome: Outcome::Mismatch,
            counter: "2",
        },
        TokenRow {
            case: "a repeat of that token carrying the same parameter values applies nothing",
            token: "token-marker",
            statement: "UPDATE counters SET n = n + ? WHERE id = ?",
            parameters: Some(json!([{"N": "1"}, {"S": "c"}])),
            outcome: Outcome::Replayed,
            counter: "2",
        },
        TokenRow {
            case: "another token applies the same transaction once more",
            token: "token-second",
            statement: "UPDATE counters SET n = n + 1 WHERE id = 'c'",
            parameters: None,
            outcome: Outcome::Applied,
            counter: "3",
        },
        TokenRow {
            case: "a call carrying no token at all applies the transaction again",
            token: "",
            statement: "UPDATE counters SET n = n + 1 WHERE id = 'c'",
            parameters: None,
            outcome: Outcome::Applied,
            counter: "4",
        },
    ]
}

#[tokio::test]
async fn a_client_request_token_makes_a_write_transaction_idempotent() {
    let router = emulator();
    create_active_table(&router, "counters", &[("id", "HASH", "S")]).await;
    ok(
        &router,
        "ExecuteTransaction",
        json!({"TransactStatements": [
            statement("INSERT INTO counters VALUE {'id': 'c', 'n': 0}"),
        ]}),
    )
    .await;

    for row in token_table() {
        let entry = match row.parameters.clone() {
            Some(parameters) => parameterized(row.statement, parameters),
            None => statement(row.statement),
        };
        let mut body = json!({
            "TransactStatements": [entry],
            "ReturnConsumedCapacity": "TOTAL",
        });
        if !row.token.is_empty() {
            body["ClientRequestToken"] = json!(row.token);
        }
        let (status, answered) = call(&router, "ExecuteTransaction", body).await;
        match row.outcome {
            Outcome::Applied => {
                assert_eq!(
                    status, 200,
                    "case {}: expected HTTP 200, got {status} with {answered}",
                    row.case
                );
                // A call that APPLIED the transaction billed the write it made.
                assert_eq!(
                    units_by_kind(&answered),
                    vec![(Some(2.0), None)],
                    "case {}: a call that applies the transaction bills write capacity, got \
                     {answered}",
                    row.case
                );
            }
            Outcome::Replayed => {
                assert_eq!(
                    status, 200,
                    "case {}: expected HTTP 200, got {status} with {answered}",
                    row.case
                );
                // A deduplicated repeat wrote nothing, so it bills no write capacity:
                // what it reports is the read of what the transaction holds.
                assert_eq!(
                    units_by_kind(&answered),
                    vec![(None, Some(2.0))],
                    "case {}: a deduplicated repeat bills no write capacity, got {answered}",
                    row.case
                );
            }
            Outcome::Mismatch => {
                assert_eq!(
                    status, 400,
                    "case {}: expected HTTP 400, got {status} with {answered}",
                    row.case
                );
                assert_eq!(
                    error_code(&answered),
                    "IdempotentParameterMismatchException",
                    "case {}: expected IdempotentParameterMismatchException, got {answered}",
                    row.case
                );
            }
        }
        let counter = stored(&router, "counters", "c").await;
        assert_eq!(
            counter["n"],
            json!({"N": row.counter}),
            "case {}: expected the counter to read {}, got {counter}",
            row.case,
            row.counter
        );
    }
}
