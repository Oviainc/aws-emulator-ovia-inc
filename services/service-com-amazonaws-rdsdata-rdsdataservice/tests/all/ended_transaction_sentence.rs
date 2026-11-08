// SPDX-License-Identifier: CC0-1.0

//! What a `transactionId` whose transaction a previous call ENDED is answered with, driven
//! end to end over the real wire path.
//!
//! The rule under test
//! ([`a_transaction_a_previous_call_ended_is_reported_as_terminated`]): a transaction that
//! stopped running is REMEMBERED as one that ended, so every operation taking a
//! `transactionId` answers its id with HTTP 404 `TransactionNotFoundException` carrying
//! AWS's termination sentence — whatever ended it, and however many calls later. The rows
//! differ along HOW the transaction stopped running.
//!
//! That the sentence stays apart from the answer a value the resource could never have
//! issued gets is pinned beside this suite (`transaction_id_classes`), and the forms such a
//! value can take by `malformed_transaction_ids`.

use serde_json::{json, Value};

use crate::sql_data_api::{a_sibling_id, error_code, parameter, DataApi};

/// AWS's own sentence for a `transactionId` that names no running transaction, written out
/// here so a change of wording in the service's own constant fails this suite.
const EXPECTED_NOT_RUNNING_MESSAGE: &str =
    "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of \
     your transaction";

/// The table every row below writes into, with the row a duplicate key collides with.
async fn given_entries(api: &DataApi) {
    api.run(json!({
        "sql": "CREATE TABLE entries (id INT PRIMARY KEY, note TEXT NOT NULL)",
    }))
    .await;
    api.run(json!({"sql": "INSERT INTO entries (id, note) VALUES (1, 'first')"}))
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
                "sql": "INSERT INTO entries (id, note) VALUES (9, 'late')",
                "transactionId": transaction_id,
            }))
            .await,
        ),
        (
            "BatchExecuteStatement",
            api.batch(json!({
                "sql": "INSERT INTO entries (id, note) VALUES (:id, :note)",
                "parameterSets": [[
                    parameter("id", json!({"longValue": 10})),
                    parameter("note", json!({"stringValue": "batched"})),
                ]],
                "transactionId": transaction_id,
            }))
            .await,
        ),
        ("CommitTransaction", api.commit(transaction_id).await),
        ("RollbackTransaction", api.rollback(transaction_id).await),
    ]
}

/// Every id in the committed table, in id order.
async fn entry_ids(api: &DataApi) -> Vec<i64> {
    let rows = api
        .run(json!({"sql": "SELECT id FROM entries ORDER BY id"}))
        .await;
    rows["records"]
        .as_array()
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    record[0]["longValue"]
                        .as_i64()
                        .unwrap_or_else(|| panic!("an id comes back as a long: {record}"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: a transaction that is no longer RUNNING is remembered as one that ended, so its id
/// draws HTTP 404 `TransactionNotFoundException` with AWS's termination sentence on every
/// operation that takes a `transactionId` — however the transaction stopped (a commit
/// installed it, a rollback discarded it, a statement of its own failed and a later call
/// ended it) and for an id the resource could have issued and never did. Refusing changes
/// nothing: the committed writes stay and the discarded ones stay away.
#[tokio::test]
async fn a_transaction_a_previous_call_ended_is_reported_as_terminated() {
    assert_eq!(
        EXPECTED_NOT_RUNNING_MESSAGE,
        service_com_amazonaws_rdsdata_rdsdataservice::state::TRANSACTION_NOT_RUNNING_MESSAGE,
        "the sentence this service states must be AWS's own"
    );

    let api = DataApi::new();
    given_entries(&api).await;

    // A transaction a COMMIT ended, whose write is installed.
    let committed = api.begin_transaction(None).await;
    api.run(json!({
        "sql": "INSERT INTO entries (id, note) VALUES (2, 'committed')",
        "transactionId": committed.clone(),
    }))
    .await;
    let (status, body) = api.commit(&committed).await;
    assert_eq!(status, 200, "case the commit: got {status} {body}");

    // A transaction a ROLLBACK ended, whose write is discarded.
    let rolled_back = api.begin_transaction(None).await;
    api.run(json!({
        "sql": "INSERT INTO entries (id, note) VALUES (3, 'discarded')",
        "transactionId": rolled_back.clone(),
    }))
    .await;
    let (status, body) = api.rollback(&rolled_back).await;
    assert_eq!(status, 200, "case the rollback: got {status} {body}");
    assert_eq!(
        body["transactionStatus"],
        json!("Rollback complete"),
        "case the rollback: expected the rollback status, got {body}"
    );

    // A transaction a statement of its OWN failed in, which a rollback then ended.
    let aborted_then_rolled_back = api.begin_transaction(None).await;
    let (status, body) = api
        .execute(json!({
            "sql": "INSERT INTO entries (id, note) VALUES (1, 'duplicate')",
            "transactionId": aborted_then_rolled_back.clone(),
        }))
        .await;
    assert_eq!(status, 400, "case the failing statement: got {status} {body}");
    let (status, body) = api.rollback(&aborted_then_rolled_back).await;
    assert_eq!(
        status, 200,
        "case the rollback of an aborted transaction: got {status} {body}"
    );

    // A transaction a statement of its own failed in, which a COMMIT then ended.
    let aborted_then_committed = api.begin_transaction(None).await;
    let (status, body) = api
        .execute(json!({
            "sql": "INSERT INTO entries (id, note) VALUES (1, 'duplicate')",
            "transactionId": aborted_then_committed.clone(),
        }))
        .await;
    assert_eq!(status, 400, "case the failing statement: got {status} {body}");
    let (status, body) = api.commit(&aborted_then_committed).await;
    assert_eq!(
        status, 200,
        "case the commit of an aborted transaction: got {status} {body}"
    );

    // An id this resource could have issued and never did: not a transaction that ended,
    // but the same answer — the service tells a caller only that no transaction of that id
    // is running.
    let never_opened = a_sibling_id(&committed);

    let ended = [
        ("a transaction a commit ended", committed.as_str()),
        ("a transaction a rollback ended", rolled_back.as_str()),
        (
            "a transaction its own statement failed in, ended by a rollback",
            aborted_then_rolled_back.as_str(),
        ),
        (
            "a transaction its own statement failed in, ended by a commit",
            aborted_then_committed.as_str(),
        ),
        (
            "an id this resource could have issued and never did",
            never_opened.as_str(),
        ),
    ];
    for (case, transaction_id) in ended {
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
            assert_eq!(
                body["message"].as_str(),
                Some(EXPECTED_NOT_RUNNING_MESSAGE),
                "case {case} on {operation}: expected the termination sentence, got {body}"
            );
        }
    }

    // The id of a transaction that ended is remembered as ended however many calls later:
    // asking again, after every refusal above, draws the same sentence rather than
    // degrading into another answer.
    for (operation, (status, body)) in every_operation_taking_an_id(&api, &rolled_back).await {
        assert_eq!(
            body["message"].as_str(),
            Some(EXPECTED_NOT_RUNNING_MESSAGE),
            "case asking again about an ended transaction on {operation}: expected the \
             termination sentence, got {status} {body}"
        );
    }

    // Nothing any refused call carried reached the data, and the ended transactions' own
    // outcomes stand: the committed write is there, and neither the rolled-back write nor
    // the write of a transaction a failed statement aborted is.
    let ids = entry_ids(&api).await;
    assert_eq!(
        ids,
        vec![1, 2],
        "case after every refusal: expected the committed rows only, got {ids:?}"
    );
}
