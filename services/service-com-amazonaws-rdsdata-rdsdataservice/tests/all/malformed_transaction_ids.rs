// SPDX-License-Identifier: CC0-1.0

//! What a `transactionId` the resource could NEVER have issued is answered with, driven end
//! to end over the real wire path.
//!
//! The rule under test, asserted over a TABLE of ids and on every operation that takes one
//! ([`an_id_the_resource_could_not_have_issued_is_a_validation_fault`]): a `transactionId`
//! whose value cannot be a transaction id of the resource the call names is a fault of the
//! REQUEST — HTTP 400 `ValidationException` "Invalid transaction ID", decided before any
//! transaction is looked up — while an id that resource COULD have issued and no
//! transaction is running under is a lookup miss, HTTP 404
//! `TransactionNotFoundException`. The two classes must not collapse onto one code: a
//! caller that corrupted or truncated its id has to learn that its request is malformed,
//! not that a previous call ended its transaction.

use serde_json::{json, Value};

use crate::sql_data_api::{a_sibling_id, error_code, parameter, DataApi};

/// The fixed text AWS words a `transactionId` the resource could not have issued with.
const INVALID_TRANSACTION_ID: &str = "Invalid transaction ID";

/// The modelled maximum length of `transactionId`: the member targets `Id`, which carries
/// `@length(max: 192)` (`smithy_query(query="shape",shape="Id")`).
const MAX_ID_LENGTH: usize = 192;

/// The table the rows below would write into, if any refused call ever reached the engine.
async fn given_notes(api: &DataApi) {
    api.run(json!({"sql": "CREATE TABLE notes (id INT PRIMARY KEY)"}))
        .await;
}

/// Call `transaction_id` on every operation that takes one, with members that are otherwise
/// valid, and report what each answered.
async fn every_operation_taking_an_id(
    api: &DataApi,
    transaction_id: &str,
) -> Vec<(&'static str, (u16, Value))> {
    vec![
        (
            "ExecuteStatement",
            api.execute(json!({
                "sql": "INSERT INTO notes (id) VALUES (7)",
                "transactionId": transaction_id,
            }))
            .await,
        ),
        (
            "BatchExecuteStatement",
            api.batch(json!({
                "sql": "INSERT INTO notes (id) VALUES (:id)",
                "parameterSets": [[parameter("id", json!({"longValue": 8}))]],
                "transactionId": transaction_id,
            }))
            .await,
        ),
        ("CommitTransaction", api.commit(transaction_id).await),
        ("RollbackTransaction", api.rollback(transaction_id).await),
    ]
}

/// How many rows the table holds, read outside any transaction.
async fn note_count(api: &DataApi) -> i64 {
    let rows = api
        .run(json!({"sql": "SELECT count(*) AS rows FROM notes"}))
        .await;
    rows["records"][0][0]["longValue"]
        .as_i64()
        .unwrap_or_else(|| panic!("the count must come back as a long: {rows}"))
}

/// RULE: a `transactionId` whose value cannot be a transaction id of the resource the call
/// names is refused on the request's own shape — HTTP 400 `ValidationException` with the
/// fixed text "Invalid transaction ID", before any transaction is looked up — on every
/// operation that takes one; an id that resource COULD have issued, which no transaction of
/// it is running under, is the lookup miss instead (HTTP 404
/// `TransactionNotFoundException`), so the two draw different codes. Nothing a refused call
/// carried runs.
#[tokio::test]
async fn an_id_the_resource_could_not_have_issued_is_a_validation_fault() {
    let api = DataApi::new();
    given_notes(&api).await;

    // One id this resource really issued, which the rows below deform. A transaction id is
    // opaque, so the forms that are NOT one are stated relative to one that is.
    let issued = api.begin_transaction(None).await;
    let truncated = issued[..issued.len() - 1].to_string();
    let lengthened = format!("{issued}0");
    let mut not_hexadecimal = issued.clone();
    not_hexadecimal.replace_range(3..4, "z");

    let malformed = [
        ("an id that is empty", String::new()),
        (
            "a string that is no id at all",
            "no-such-transaction-id".to_string(),
        ),
        ("an issued id with its last digit cut off", truncated),
        ("an issued id with one digit appended", lengthened),
        (
            "an issued id with a digit that is not hexadecimal",
            not_hexadecimal,
        ),
        ("an issued id in upper case", issued.to_uppercase()),
        (
            "an issued id with a space in front of it",
            format!(" {issued}"),
        ),
        (
            "a UUID in its dashed form",
            "123e4567-e89b-12d3-a456-426614174000".to_string(),
        ),
        (
            "the longest id the model allows",
            "a".repeat(MAX_ID_LENGTH),
        ),
    ];

    for (case, transaction_id) in &malformed {
        for (operation, (status, body)) in every_operation_taking_an_id(&api, transaction_id).await
        {
            assert_eq!(
                status, 400,
                "case {case} on {operation}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case} on {operation}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(INVALID_TRANSACTION_ID),
                "case {case} on {operation}: expected the invalid-argument text, got {body}"
            );
        }
    }

    // One past the modelled maximum: the model's own length constraint refuses it, which is
    // the same class and status and still never reaches the transaction store.
    let past_the_bound = "a".repeat(MAX_ID_LENGTH + 1);
    for (operation, (status, body)) in every_operation_taking_an_id(&api, &past_the_bound).await {
        assert_eq!(
            status, 400,
            "case an id past the modelled maximum on {operation}: expected HTTP 400, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case an id past the modelled maximum on {operation}: expected ValidationException, \
             got {body}"
        );
    }

    // The ids this resource COULD have issued and no transaction is running under draw the
    // OTHER class, so a client can tell them apart: one whose transaction a rollback ended,
    // and one no transaction was ever opened under.
    let rolled_back = api.begin_transaction(None).await;
    let (status, body) = api.rollback(&rolled_back).await;
    assert_eq!(status, 200, "the transaction rolls back: {body}");
    let never_opened = a_sibling_id(&issued);
    for (case, transaction_id) in [
        ("a transaction a rollback ended", rolled_back.as_str()),
        (
            "an id this resource could have issued and never did",
            never_opened.as_str(),
        ),
    ] {
        for (operation, (status, body)) in every_operation_taking_an_id(&api, transaction_id).await
        {
            assert_eq!(
                status, 404,
                "case {case} on {operation}: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "TransactionNotFoundException",
                "case {case} on {operation}: expected TransactionNotFoundException, got {body}"
            );
        }
    }

    // The transaction this resource opened is untouched by every refusal above: it is still
    // running, and its own statement still reaches its database.
    let (status, body) = api
        .execute(json!({
            "sql": "INSERT INTO notes (id) VALUES (1)",
            "transactionId": issued.clone(),
        }))
        .await;
    assert_eq!(
        status, 200,
        "the issued transaction must still be running: {status} {body}"
    );
    assert_eq!(
        note_count(&api).await,
        0,
        "case before the commit: a refused call and an uncommitted write must both be absent"
    );
    let (status, body) = api.commit(&issued).await;
    assert_eq!(status, 200, "the issued transaction commits: {body}");
    assert_eq!(
        note_count(&api).await,
        1,
        "case after the commit: only the transaction's own row may be there"
    );
}
