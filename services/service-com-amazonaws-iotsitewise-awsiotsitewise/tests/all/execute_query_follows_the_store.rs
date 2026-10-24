// SPDX-License-Identifier: CC0-1.0

//! `ExecuteQuery` answers from the store as the store is NOW, and answering changes
//! nothing in it.
//!
//! These two rules are what make the operation a reporting one over live state
//! rather than a fixture replay, and neither is pinned by a single recorded call:
//!
//! 1. The rows a statement selects are a function of what the store holds when the
//!    statement runs — create another asset and the same statement answers with it,
//!    without the statement changing.
//! 2. Running a statement — one that answers and one that is refused alike — leaves
//!    every stored resource exactly as it was, because `ExecuteQuery` reports and
//!    does not mutate.
//!
//! The harness (`router`, `call`, `create_model`, `create_asset`, `error_code`) is
//! the one [`crate::asset_lifecycle`] defines, re-used rather than copied, and the
//! statements used here are the plainest the documented `asset` view allows, so what
//! these suites assert is the operation's relationship to the store rather than any
//! corner of the query dialect.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, create_asset, create_model, error_code, router};

/// `POST /queries/execution` with `statement`.
async fn query(router: &MultiServiceRouter, statement: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        "/queries/execution",
        Some(json!({ "queryStatement": statement })),
    )
    .await
}

/// The first `scalarValue` of every row, in the order the response lists them.
fn first_column(body: &Value) -> Vec<String> {
    body["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows in {body}"))
        .iter()
        .map(|row| {
            row["data"][0]["scalarValue"]
                .as_str()
                .unwrap_or_else(|| panic!("no scalar value in {row}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_rows_a_statement_answers_follow_the_store() {
    let router = router();
    let model_id = create_model(
        &router,
        "Boiler",
        json!([{"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;

    // One statement, run again after each create: the store is the only thing that
    // changes between the rows of this table, so the expectation is what the store
    // holds at that point and nothing else.
    let statement = "SELECT asset_name FROM asset ORDER BY asset_name";
    let cases: [(&str, &str, &[&str]); 4] = [
        ("one asset", "Alpha", &["Alpha"]),
        ("a second asset", "Bravo", &["Alpha", "Bravo"]),
        ("a third asset", "Charlie", &["Alpha", "Bravo", "Charlie"]),
        // A name that sorts before the others, so the answer is ordered by the
        // statement rather than by creation order.
        ("a name that sorts first", "Aardvark", &["Aardvark", "Alpha", "Bravo", "Charlie"]),
    ];

    for (case, name, expected) in cases {
        create_asset(&router, name, &model_id).await;
        let (status, body) = query(&router, statement).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let answered = first_column(&body);
        assert_eq!(
            answered,
            expected.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {case:?}: expected {expected:?} from {statement:?}, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn running_a_statement_leaves_the_store_as_it_found_it() {
    let router = router();
    let model_id = create_model(
        &router,
        "Boiler",
        json!([{"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    let asset_id = create_asset(&router, "Boiler1", &model_id).await;

    /// What a read of the asset reports, reduced to the members a mutation would
    /// move: its identity, its name, its model and its property count.
    async fn describe(router: &MultiServiceRouter, asset_id: &str) -> Value {
        let (status, asset) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
        assert_eq!(status, 200, "describing {asset_id}: got {status} {asset}");
        json!({
            "assetId": asset["assetId"],
            "assetName": asset["assetName"],
            "assetModelId": asset["assetModelId"],
            "properties": asset["assetProperties"].as_array().map(Vec::len),
            "state": asset["assetStatus"]["state"],
        })
    }

    let before = describe(&router, &asset_id).await;

    // Rows span both outcomes a statement can have — answered and refused — because
    // a mutation on either path is the defect this pins.
    let cases: [(&str, String, u16); 4] = [
        (
            "a statement that answers rows",
            format!("SELECT asset_id, asset_name FROM asset WHERE asset_id = '{asset_id}'"),
            200,
        ),
        (
            "a statement that answers no rows",
            "SELECT asset_id FROM asset WHERE asset_name = 'nothing of this name'".to_string(),
            200,
        ),
        (
            "a statement naming a view the catalog does not hold",
            "SELECT * FROM turbines".to_string(),
            400,
        ),
        (
            "a statement that is not a query",
            format!("DELETE FROM asset WHERE asset_id = '{asset_id}'"),
            400,
        ),
    ];

    for (case, statement, expected_status) in &cases {
        let (status, body) = query(&router, statement).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status} for {statement:?}, got {status} {body}"
        );
        if *expected_status == 400 {
            assert!(
                !error_code(&body).is_empty(),
                "case {case:?}: a refusal must carry a modelled error code, got {body}"
            );
        }
        let after = describe(&router, &asset_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: the asset must be unchanged after {statement:?}, was {before}, now {after}"
        );
    }

    // And the asset is still the only one stored: nothing was created either.
    let (status, body) = query(&router, "SELECT asset_name FROM asset").await;
    assert_eq!(status, 200, "reading the view back: got {status} {body}");
    assert_eq!(
        first_column(&body),
        vec!["Boiler1".to_string()],
        "the view must still hold exactly the one stored asset, got {body}"
    );
}
