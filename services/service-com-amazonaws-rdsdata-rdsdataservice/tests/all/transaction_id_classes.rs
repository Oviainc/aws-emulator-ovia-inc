// SPDX-License-Identifier: CC0-1.0

//! That the two refusals a `transactionId` can draw stay APART, over the real wire path.
//!
//! The rule under test ([`an_ended_id_and_a_deformed_one_draw_different_refusals`]): the id
//! of a transaction a previous call ENDED and a value that could never have been a
//! transaction id of the resource are different conditions and keep different answers — the
//! first is the lookup's own HTTP 404 `TransactionNotFoundException` with the termination
//! sentence, the second a fault of the REQUEST, HTTP 400 `ValidationException` "Invalid
//! transaction ID". A client that retries or re-begins on the first but must fix its call
//! on the second can only branch while the two differ, so each row here takes ONE ended
//! transaction's id and deforms it, asking both questions about the same transaction.
//!
//! The classes' own tables live beside this suite: `ended_transaction_sentence` spans how a
//! transaction can stop running, and `malformed_transaction_ids` spans the forms a value
//! that is no id of the resource can take.

use serde_json::{json, Value};

use crate::sql_data_api::{error_code, DataApi};

/// AWS's own sentence for a `transactionId` that names no running transaction, stated here
/// as this suite's expectation rather than read from the service.
const EXPECTED_NOT_RUNNING_MESSAGE: &str =
    "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of \
     your transaction";

/// The fixed text a value that could not be an id of the resource is refused with.
const INVALID_TRANSACTION_ID: &str = "Invalid transaction ID";

/// `ExecuteStatement` carrying `transaction_id`, which is the operation a client runs the
/// next statement of its transaction through.
async fn statement_in(api: &DataApi, transaction_id: &str) -> (u16, Value) {
    api.execute(json!({
        "sql": "INSERT INTO ledger (id, note) VALUES (5, 'late')",
        "transactionId": transaction_id,
    }))
    .await
}

/// RULE: for one and the same transaction that a previous call ended, its OWN id is a
/// lookup miss (404 `TransactionNotFoundException`, the termination sentence) while every
/// deformation of that id is a fault of the request (400 `ValidationException`, "Invalid
/// transaction ID"). The rows differ along HOW the id is deformed, and each row asks both
/// questions, so the two answers can never collapse onto one.
#[tokio::test]
async fn an_ended_id_and_a_deformed_one_draw_different_refusals() {
    let api = DataApi::new();
    api.run(json!({
        "sql": "CREATE TABLE ledger (id INT PRIMARY KEY, note TEXT NOT NULL)",
    }))
    .await;

    let ended = api.begin_transaction(None).await;
    let (status, body) = api.rollback(&ended).await;
    assert_eq!(status, 200, "case the rollback: got {status} {body}");

    // The transaction's own id: the service remembers that this transaction ended.
    let (status, body) = statement_in(&api, &ended).await;
    assert_eq!(status, 404, "case the ended id: got {status} {body}");
    assert_eq!(
        error_code(&body),
        "TransactionNotFoundException",
        "case the ended id: expected TransactionNotFoundException, got {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(EXPECTED_NOT_RUNNING_MESSAGE),
        "case the ended id: expected the termination sentence, got {body}"
    );

    // The same id, deformed: a value the resource could never have issued, whatever the
    // transaction it was built from did.
    let mut not_hexadecimal = ended.clone();
    not_hexadecimal.replace_range(2..3, "x");
    let deformations = [
        ("its last character cut off", ended[..ended.len() - 1].to_string()),
        ("one character appended", format!("{ended}f")),
        ("a character that is not hexadecimal", not_hexadecimal),
        ("written in upper case", ended.to_uppercase()),
        ("wrapped in whitespace", format!(" {ended} ")),
    ];
    for (case, deformed) in deformations {
        let (status, body) = statement_in(&api, &deformed).await;
        assert_eq!(
            status, 400,
            "case the ended id with {case}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case the ended id with {case}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(INVALID_TRANSACTION_ID),
            "case the ended id with {case}: expected the invalid-argument text, got {body}"
        );
    }

    // Asking once more about the transaction itself still draws its own answer: the
    // deformed calls changed nothing about what the service remembers.
    let (status, body) = statement_in(&api, &ended).await;
    assert_eq!(
        status, 404,
        "case the ended id after every deformation: got {status} {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(EXPECTED_NOT_RUNNING_MESSAGE),
        "case the ended id after every deformation: expected the termination sentence, got \
         {body}"
    );

    // Nothing any refused call carried reached the table.
    let rows = api
        .run(json!({"sql": "SELECT count(*) AS rows FROM ledger"}))
        .await;
    assert_eq!(
        rows["records"][0][0]["longValue"].as_i64(),
        Some(0),
        "case after every refusal: no refused statement may have written, got {rows}"
    );
}
