// SPDX-License-Identifier: CC0-1.0

//! What one transaction READS while another is writing, over the real wire.
//!
//! Every call goes through the router, so routing, restJson1 deserialization, the
//! handler and serialization are all exercised; the client harness is the one
//! `sql_data_api` builds.

use serde_json::json;

use crate::sql_data_api::{parameter, DataApi};

/// One emulator holding one table with the baseline row every case starts from.
async fn ledger() -> DataApi {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE rows_of (id INT PRIMARY KEY, note VARCHAR(16))"}))
        .await;
    api.run(json!({"sql": "INSERT INTO rows_of (id, note) VALUES (1, 'baseline')"}))
        .await;
    api
}

/// Insert one row, in a transaction when `transaction` names one.
async fn insert(api: &DataApi, transaction: Option<&str>, id: i64, note: &str) {
    let mut members = json!({
        "sql": "INSERT INTO rows_of (id, note) VALUES (:id, :note)",
        "parameters": [
            parameter("id", json!({"longValue": id})),
            parameter("note", json!({"stringValue": note})),
        ],
    });
    if let Some(transaction) = transaction {
        members["transactionId"] = json!(transaction);
    }
    api.run(members).await;
}

/// The rows a reader sees, as `(id, note)` pairs, read in a transaction when
/// `transaction` names one.
async fn rows_seen(api: &DataApi, transaction: Option<&str>) -> Vec<(i64, String)> {
    let mut members = json!({"sql": "SELECT id, note FROM rows_of ORDER BY id"});
    if let Some(transaction) = transaction {
        members["transactionId"] = json!(transaction);
    }
    let body = api.run(members).await;
    body["records"]
        .as_array()
        .unwrap_or_else(|| panic!("a result set: {body}"))
        .iter()
        .map(|row| {
            (
                row[0]["longValue"].as_i64().unwrap_or_default(),
                row[1]["stringValue"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `(id, note)` written the way the assertions below compare them.
fn rows(pairs: &[(i64, &str)]) -> Vec<(i64, String)> {
    pairs
        .iter()
        .map(|(id, note)| (*id, (*note).to_string()))
        .collect()
}

/// RULE: a transaction reads at READ COMMITTED, so every statement of it sees the rows
/// every OTHER writer had committed when that statement ran, alongside the rows the
/// transaction has written itself and nobody else can see yet.
///
/// The rows differ along WHEN the open transaction wrote its own row — before the other
/// writer committed and after it — and along who the other writer is (another
/// transaction, and a call that is in none and so commits at once), because the rule is
/// about what is COMMITTED and not about who committed it.
#[tokio::test]
async fn an_open_transaction_reads_what_another_writer_committed() {
    for writes_first in [true, false] {
        for other_writer_is_a_transaction in [true, false] {
            let case = format!(
                "the open transaction writes {} the other writer, which is {}",
                if writes_first { "before" } else { "after" },
                if other_writer_is_a_transaction {
                    "a transaction"
                } else {
                    "a call in none"
                }
            );
            let api = ledger().await;
            let b = api.begin_transaction(None).await;

            // What B reads before anything else happens: the committed baseline.
            let seen = rows_seen(&api, Some(&b)).await;
            assert_eq!(
                seen,
                rows(&[(1, "baseline")]),
                "case {case}: a transaction starts from what was committed, got {seen:?}"
            );

            if writes_first {
                insert(&api, Some(&b), 20, "txn-b-20").await;
            }

            // The other writer stores two rows and commits them.
            let a = if other_writer_is_a_transaction {
                Some(api.begin_transaction(None).await)
            } else {
                None
            };
            insert(&api, a.as_deref(), 10, "txn-a-10").await;
            insert(&api, a.as_deref(), 11, "txn-a-11").await;
            if let Some(a) = &a {
                // Until it commits, B sees nothing of it.
                let during = rows_seen(&api, Some(&b)).await;
                let expected_during = if writes_first {
                    rows(&[(1, "baseline"), (20, "txn-b-20")])
                } else {
                    rows(&[(1, "baseline")])
                };
                assert_eq!(
                    during, expected_during,
                    "case {case}: an uncommitted write of another transaction is \
                     invisible, expected {expected_during:?}, got {during:?}"
                );
                let (status, body) = api.commit(a).await;
                assert_eq!(status, 200, "case {case}: committing failed with {body}");
            }

            if !writes_first {
                insert(&api, Some(&b), 20, "txn-b-20").await;
            }

            // B is still open, so it reads everything committed plus its own row.
            let seen = rows_seen(&api, Some(&b)).await;
            assert_eq!(
                seen,
                rows(&[
                    (1, "baseline"),
                    (10, "txn-a-10"),
                    (11, "txn-a-11"),
                    (20, "txn-b-20"),
                ]),
                "case {case}: B must see every committed row alongside its own \
                 uncommitted one, got {seen:?}"
            );

            // And nobody else sees B's row until B commits.
            let outside = rows_seen(&api, None).await;
            assert_eq!(
                outside,
                rows(&[(1, "baseline"), (10, "txn-a-10"), (11, "txn-a-11")]),
                "case {case}: B's uncommitted row stays its own, got {outside:?}"
            );
        }
    }
}

/// RULE: a commit ADDS the transaction's writes to what is committed, so two writers on
/// one database both keep their rows — and a transaction that rolls back adds nothing,
/// which leaves the other writer's rows exactly as they were.
///
/// The rows differ along how the open transaction finishes.
#[tokio::test]
async fn two_transactions_on_one_database_both_keep_their_writes() {
    for (case, commits) in [("the transaction commits", true), ("it rolls back", false)] {
        let api = ledger().await;
        let open = api.begin_transaction(None).await;
        insert(&api, Some(&open), 20, "txn").await;

        // A second writer commits to the same database while that transaction is still
        // open, so it finishes FIRST.
        insert(&api, None, 10, "outsider").await;

        let (status, body) = if commits {
            api.commit(&open).await
        } else {
            api.rollback(&open).await
        };
        assert_eq!(status, 200, "case {case}: finishing failed with {body}");

        let after = rows_seen(&api, None).await;
        let expected = if commits {
            rows(&[(1, "baseline"), (10, "outsider"), (20, "txn")])
        } else {
            rows(&[(1, "baseline"), (10, "outsider")])
        };
        assert_eq!(
            after, expected,
            "case {case}: expected {expected:?}, got {after:?}"
        );
    }
}

/// RULE (the same visibility, from the batch operation): a batch inside a transaction
/// runs against the database that transaction reads, so its statements see what is
/// committed, and what it wrote is installed by the transaction's commit alongside the
/// rows another writer committed in the meantime.
#[tokio::test]
async fn a_batch_in_a_transaction_reads_and_writes_what_the_transaction_does() {
    let api = ledger().await;
    let open = api.begin_transaction(None).await;

    // Another writer commits while the transaction is open.
    insert(&api, None, 10, "outsider").await;

    let (status, batch) = api
        .batch(json!({
            "sql": "INSERT INTO rows_of (id, note) VALUES (:id, :note)",
            "transactionId": open,
            "parameterSets": [
                [
                    parameter("id", json!({"longValue": 20})),
                    parameter("note", json!({"stringValue": "batch-20"})),
                ],
                [
                    parameter("id", json!({"longValue": 21})),
                    parameter("note", json!({"stringValue": "batch-21"})),
                ],
            ],
        }))
        .await;
    assert_eq!(status, 200, "the batch failed with {batch}");

    let inside = rows_seen(&api, Some(&open)).await;
    assert_eq!(
        inside,
        rows(&[
            (1, "baseline"),
            (10, "outsider"),
            (20, "batch-20"),
            (21, "batch-21"),
        ]),
        "case \"a batch inside a transaction\": the transaction reads the committed \
         row and its own batched rows, got {inside:?}"
    );

    let (status, body) = api.commit(&open).await;
    assert_eq!(status, 200, "committing failed with {body}");
    let after = rows_seen(&api, None).await;
    assert_eq!(
        after,
        rows(&[
            (1, "baseline"),
            (10, "outsider"),
            (20, "batch-20"),
            (21, "batch-21"),
        ]),
        "case \"a batch inside a transaction\": the commit keeps both writers' rows, \
         got {after:?}"
    );
}
