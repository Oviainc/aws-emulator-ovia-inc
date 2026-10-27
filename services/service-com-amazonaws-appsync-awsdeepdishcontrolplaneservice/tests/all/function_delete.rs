// SPDX-License-Identifier: CC0-1.0

//! `DeleteFunction`, end to end over HTTP: what a delete removes, and which deletes it
//! refuses.
//!
//! The rule asserted here is stated over CLASSES of input rather than over the one
//! request that exposed the defect: a delete removes exactly the function the request
//! addresses by its id from the API that holds it and answers an empty 200, so
//! afterwards every operation that addresses that function answers as if it had never
//! been created (`GetFunction` and `UpdateFunction` a 404 `NotFoundException` naming the
//! id) while every other function — of that API and of any other — is untouched; a
//! request that addresses no function removes nothing and is refused: an id the API does
//! not hold and an `apiId` this account does not hold are 404s, and an `apiId` that
//! cannot name an API at all is a 400.
//!
//! The rows span:
//!
//! 1. every definition form the function table holds (VTL mapping templates with a
//!    `functionVersion`, `code` with a `runtime`, the `description` states, the
//!    published `maxBatchSize` bounds and the `SyncConfig` strategies), so the delete
//!    cannot be keyed to one kind of function;
//! 2. the position of the deleted function in its API's creation order (first, last,
//!    middle), and the count of functions the API holds, from several down to none;
//! 3. the forms of a request that addresses nothing: an id never minted, an id an
//!    earlier delete already removed, an id another API holds, an `apiId` no API has and
//!    an `apiId` too long to be one.
//!
//! The harness (`call`, `create_api`, `router`, `Answer`) is the error-contract suite's;
//! the function table, the request body, the creating helper, the reader and the
//! projection assertion are the `CreateFunction` suite's — reused rather than copied.

use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::error_contract::{call, create_api, router, Answer};
use super::functions::{
    assert_projects, create_data_source, create_function, get_function, request_body, rows, Row,
};

/// Send one `DeleteFunction`.
async fn delete_function(
    router: &MultiServiceRouter,
    api_id: &str,
    function_id: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/apis/{api_id}/functions/{function_id}"),
        None,
    )
    .await
}

/// Send one `UpdateFunction`, to show a write addressed at a deleted function is
/// refused as missing too.
async fn update_function(
    router: &MultiServiceRouter,
    api_id: &str,
    function_id: &str,
    data_source: &str,
    row: &Row,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/functions/{function_id}"),
        Some(request_body(row, data_source)),
    )
    .await
}

/// Assert `answer` is the empty 200 the modelled output (no members) serializes to.
fn assert_empty_success(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DeleteFunction 200, got {answer:?}"
    );
    assert!(
        answer
            .body
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "case {case}: expected an empty body, got {answer:?}"
    );
}

/// The text a `NotFoundException` carries for a function the addressed API does not
/// hold: AppSync's own wording, a fixed template whose only variable part is the
/// requested id.
fn not_found_message(function_id: &str) -> String {
    format!("function id {function_id} not found")
}

/// Assert the API holds no function under `function_id` any more: the read is a 404
/// naming it, and a write addressed at it is refused the same way.
async fn assert_gone(
    router: &MultiServiceRouter,
    api_id: &str,
    function_id: &str,
    data_source: &str,
    row: &Row,
    case: &str,
) {
    let read = get_function(router, api_id, function_id).await;
    assert_eq!(
        read.status, 404,
        "case {case}: expected GetFunction 404 after the delete, got {read:?}"
    );
    assert_eq!(
        read.error_type, "NotFoundException",
        "case {case}: expected NotFoundException after the delete, got {read:?}"
    );
    assert_eq!(
        read.message,
        not_found_message(function_id),
        "case {case}: expected the refusal to name the absent function, got {read:?}"
    );

    let written = update_function(router, api_id, function_id, data_source, row).await;
    assert_eq!(
        written.status, 404,
        "case {case}: expected UpdateFunction 404 after the delete, got {written:?}"
    );
    assert_eq!(
        written.error_type, "NotFoundException",
        "case {case}: expected UpdateFunction to refuse the deleted id as missing, \
         got {written:?}"
    );
    assert_eq!(
        written.message,
        not_found_message(function_id),
        "case {case}: expected the refusal to name the deleted function, got {written:?}"
    );
}

/// A delete removes exactly the addressed function, whatever its definition form,
/// whatever its position in the API's creation order and however many functions the API
/// holds — down to the last one.
#[tokio::test]
async fn a_delete_removes_exactly_the_addressed_function_of_every_definition_form() {
    let router = router();
    let api_id = create_api(&router, "Api_function_delete").await;
    let data_source = create_data_source(&router, &api_id, "fn_delete_source").await;

    // One function per row of the shared table, so the functions left after each delete
    // span every definition form the table holds.
    let catalog = rows();
    let mut remaining: Vec<(Row, String)> = Vec::new();
    for row in &catalog {
        let created = create_function(&router, &api_id, &data_source, row).await;
        assert_eq!(
            created.status, 200,
            "case create {}: expected 200, got {created:?}",
            row.label
        );
        let function_id = created.body["functionConfiguration"]["functionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        remaining.push((row.clone(), function_id));
    }

    // The deletion order walks the first function created, then the last, then one from
    // the middle each time, so the removed function's position varies across the rows
    // and the API is driven from full down to empty.
    let mut step = 0;
    while !remaining.is_empty() {
        let position = match step % 3 {
            0 => 0,
            1 => remaining.len() - 1,
            _ => remaining.len() / 2,
        };
        let (row, function_id) = remaining.remove(position);
        let case = format!(
            "delete {} at position {position} of {}",
            row.label,
            remaining.len() + 1
        );

        // The function is readable before the delete, so the 404 after it is the
        // delete's own effect.
        let before = get_function(&router, &api_id, &function_id).await;
        assert_eq!(
            before.status, 200,
            "case {case}: expected the function to exist before the delete, got {before:?}"
        );

        let deleted = delete_function(&router, &api_id, &function_id).await;
        assert_empty_success(&case, &deleted);
        assert_gone(&router, &api_id, &function_id, &data_source, &row, &case).await;

        // Every function the delete did not address is still there, reporting the same
        // body it reported before — the delete removed one record, not a class.
        for (survivor, survivor_id) in &remaining {
            let read = get_function(&router, &api_id, survivor_id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected {} to survive, got {read:?}",
                survivor.label
            );
            assert_projects(
                survivor,
                &api_id,
                &data_source,
                &read.body["functionConfiguration"],
                "GetFunction after a delete of another function",
            );
        }
        step += 1;
    }
}

/// A deleted function reserves nothing: the identical create is accepted afterwards and
/// reads back as a NEW function, under a fresh id that is not the deleted one.
#[tokio::test]
async fn a_deleted_function_can_be_created_again_under_a_fresh_id() {
    let router = router();
    let api_id = create_api(&router, "Api_function_delete_recreate").await;
    let data_source = create_data_source(&router, &api_id, "fn_delete_recreate_source").await;

    for row in &rows() {
        let case = format!("recreate {} after a delete", row.label);
        let created = create_function(&router, &api_id, &data_source, row).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected the first create to be accepted, got {created:?}"
        );
        let first_id = created.body["functionConfiguration"]["functionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let deleted = delete_function(&router, &api_id, &first_id).await;
        assert_empty_success(&case, &deleted);
        assert_gone(&router, &api_id, &first_id, &data_source, row, &case).await;

        let again = create_function(&router, &api_id, &data_source, row).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected the recreate to be accepted, got {again:?}"
        );
        let second_id = again.body["functionConfiguration"]["functionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            second_id, first_id,
            "case {case}: expected a fresh id for the recreated function, got {second_id:?}"
        );

        let read = get_function(&router, &api_id, &second_id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the recreated function to read back, got {read:?}"
        );
        assert_projects(
            row,
            &api_id,
            &data_source,
            &read.body["functionConfiguration"],
            "GetFunction after a recreate",
        );

        // Tidy up so the next row starts from an API holding only its own function.
        let cleanup = delete_function(&router, &api_id, &second_id).await;
        assert_empty_success(&format!("{case} cleanup"), &cleanup);
    }
}

/// A request that addresses no function removes nothing and says which refusal it is:
/// an id nothing holds (never minted, already deleted, or held by another API) is a
/// 404, an `apiId` this account does not hold is a 404, and an `apiId` too long to be
/// one is a 400.
#[tokio::test]
async fn a_delete_that_addresses_nothing_is_refused_and_removes_nothing() {
    let router = router();
    let api_id = create_api(&router, "Api_function_delete_refusals").await;
    let data_source = create_data_source(&router, &api_id, "fn_delete_refusal_source").await;
    let other_api = create_api(&router, "Api_function_delete_refusals_other").await;
    create_data_source(&router, &other_api, "fn_delete_refusal_source").await;

    let row = &rows()[0];
    let kept = create_function(&router, &api_id, &data_source, row).await;
    assert_eq!(
        kept.status, 200,
        "case seed the surviving function: expected 200, got {kept:?}"
    );
    let kept_id = kept.body["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // An id an earlier delete already removed: the second delete has nothing left to
    // remove, so it takes the same refusal as an id never minted.
    let doomed = create_function(&router, &api_id, &data_source, row).await;
    let doomed_id = doomed.body["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let first = delete_function(&router, &api_id, &doomed_id).await;
    assert_empty_success("the first delete of an id", &first);

    // An id ANOTHER API holds: a function is addressed within its own API, so naming it
    // here addresses nothing.
    let elsewhere = create_function(&router, &other_api, "fn_delete_refusal_source", row).await;
    let elsewhere_id = elsewhere.body["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (case, unheld) in [
        ("an id never minted", "fn_never_created".to_string()),
        ("an id an earlier delete removed", doomed_id.clone()),
        ("an id another API holds", elsewhere_id.clone()),
    ] {
        let answer = delete_function(&router, &api_id, &unheld).await;
        assert_eq!(
            answer.status, 404,
            "case delete {case}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "NotFoundException",
            "case delete {case}: expected NotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.message,
            not_found_message(&unheld),
            "case delete {case}: expected the refusal to name the id, got {answer:?}"
        );
    }

    // An `apiId` of the right shape that this account does not hold, and one longer than
    // an AppSync API id, which cannot name an API at all. Source for the length:
    // `state::API_ID_LEN`.
    let absent_api = "z".repeat(state::API_ID_LEN);
    let unnameable_api = "z".repeat(state::API_ID_LEN + 1);
    for (case, api, status, error_type, message) in [
        (
            "an apiId no API has",
            absent_api.as_str(),
            404,
            "NotFoundException",
            "API not found.".to_string(),
        ),
        (
            "an apiId too long to be one",
            unnameable_api.as_str(),
            400,
            "BadRequestException",
            format!("Invalid API ID {unnameable_api}"),
        ),
    ] {
        let answer = delete_function(&router, api, &kept_id).await;
        assert_eq!(
            answer.status, status,
            "case delete under {case}: expected {status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, error_type,
            "case delete under {case}: expected {error_type}, got {answer:?}"
        );
        assert_eq!(
            answer.message, message,
            "case delete under {case}: expected {message:?}, got {answer:?}"
        );
    }

    // None of those refusals touched the function that does exist, nor the one the other
    // API holds.
    let read = get_function(&router, &api_id, &kept_id).await;
    assert_eq!(
        read.status, 200,
        "case the surviving function: expected a refused delete to remove nothing, got {read:?}"
    );
    assert_eq!(
        read.body["functionConfiguration"], kept.body["functionConfiguration"],
        "case the surviving function: expected {:?}, got {:?}",
        kept.body["functionConfiguration"], read.body["functionConfiguration"]
    );

    let read_other = get_function(&router, &other_api, &elsewhere_id).await;
    assert_eq!(
        read_other.status, 200,
        "case the other API's function: expected it to survive, got {read_other:?}"
    );
    assert_eq!(
        read_other.body["functionConfiguration"], elsewhere.body["functionConfiguration"],
        "case the other API's function: expected {:?}, got {:?}",
        elsewhere.body["functionConfiguration"], read_other.body["functionConfiguration"]
    );
}
