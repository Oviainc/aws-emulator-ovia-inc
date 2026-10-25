// SPDX-License-Identifier: CC0-1.0

//! `DeleteDataSource`, end to end over HTTP: what a delete removes, and which deletes
//! it refuses.
//!
//! The rule asserted here is stated over CLASSES of input rather than over the one
//! request that exposed the defect: a delete removes exactly the data source the
//! request addresses by name from the API that holds it and answers an empty 200, so
//! afterwards every reader of that API answers without it (`GetDataSource` a 404
//! `NotFoundException`, `ListDataSources` a page it is absent from) while every other
//! data source — of that API and of any other — is untouched; a request that addresses
//! no data source removes nothing and is refused: a name the API does not hold and an
//! API this account does not hold are 404s, and an `apiId` that cannot name an API at
//! all is a 400.
//!
//! The rows span:
//!
//! 1. every `DataSourceType` the model declares, so the delete cannot be keyed to one
//!    kind of data source;
//! 2. the position of the deleted name in its API's listing (first, middle, last),
//!    since the listing's order is what a cursor resumes from;
//! 3. the count of data sources the API holds, from several down to none;
//! 4. the forms of a request that addresses nothing: a never-created name, a name an
//!    earlier delete already removed, a name another API holds, an `apiId` no API has
//!    and an `apiId` too long to be one.
//!
//! The harness (`call`, `create_api`, `router`, `Answer`) is the error-contract
//! suite's; the type catalog, the body builder, the reader and the paging listing are
//! the service-role suite's — reused rather than copied.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_source_service_role::{
    get_data_source, list_names, request_body, DATA_SOURCE_TYPES, ROLE_ARN,
};
use crate::error_contract::{call, create_api, router, Answer};

/// The number of characters an AppSync API id has, so an id of one more cannot name an
/// API at all. Source: `state::API_ID_LEN`, the service's own copy.
const API_ID_LEN: usize = 26;

/// Create one data source of `data_source_type` named `name` under `api_id`, carrying
/// the per-type configuration and the service role that type needs, and assert it was
/// created.
async fn create_data_source(
    router: &MultiServiceRouter,
    api_id: &str,
    name: &str,
    data_source_type: &str,
    needs_role: bool,
) {
    let created = call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(request_body(
            name,
            data_source_type,
            needs_role.then_some(ROLE_ARN),
            json!({}),
        )),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create {name:?} of type {data_source_type}: expected 200, got {created:?}"
    );
}

/// Send one `DeleteDataSource`.
async fn delete_data_source(router: &MultiServiceRouter, api_id: &str, name: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/apis/{api_id}/datasources/{name}"),
        None,
    )
    .await
}

/// Assert `answer` is the empty 200 the modelled output (no members) serializes to.
fn assert_empty_success(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DeleteDataSource 200, got {answer:?}"
    );
    assert!(
        answer
            .body
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "case {case}: expected an empty body, got {answer:?}"
    );
}

/// The text a `NotFoundException` carries for a data source the API does not hold: a
/// fixed template over the requested name.
fn not_found_message(name: &str) -> String {
    format!("Data source {name} not found")
}

/// Assert the API holds no data source called `name` any more: the read is a 404
/// naming it, and the listing does not report it.
async fn assert_gone(router: &MultiServiceRouter, api_id: &str, name: &str, case: &str) {
    let read = get_data_source(router, api_id, name).await;
    assert_eq!(
        read.status, 404,
        "case {case}: expected GetDataSource 404 after the delete, got {read:?}"
    );
    assert_eq!(
        read.error_type, "NotFoundException",
        "case {case}: expected NotFoundException after the delete, got {read:?}"
    );
    assert_eq!(
        read.message,
        not_found_message(name),
        "case {case}: expected the refusal to name the absent data source, got {read:?}"
    );
    let listed = list_names(router, api_id).await;
    assert!(
        !listed.contains(&name.to_string()),
        "case {case}: expected the listing not to hold the deleted name, got {listed:?}"
    );
}

/// A delete removes exactly the named data source, whatever its type, whatever its
/// position in the listing and however many the API holds — down to the last one.
#[tokio::test]
async fn a_delete_removes_exactly_the_named_data_source_of_every_type() {
    let router = router();
    let api_id = create_api(&router, "Api_data_source_delete").await;

    // One data source per `DataSourceType`, so the names left after each delete span
    // every kind of data source the model declares.
    let names: Vec<String> = DATA_SOURCE_TYPES
        .iter()
        .enumerate()
        .map(|(index, (data_source_type, _))| format!("ds_{index}_{data_source_type}"))
        .collect();
    for (name, (data_source_type, needs_role)) in names.iter().zip(DATA_SOURCE_TYPES) {
        create_data_source(&router, &api_id, name, data_source_type, *needs_role).await;
    }

    // The deletion order walks the listing's first name, its last, and then one from
    // the middle each time, so the removed name's position varies across the rows and
    // the API is driven from full down to empty.
    let mut remaining = list_names(&router, &api_id).await;
    assert_eq!(
        remaining.len(),
        names.len(),
        "case the whole listing before any delete: expected {} data sources, got {remaining:?}",
        names.len()
    );

    let mut step = 0;
    while !remaining.is_empty() {
        let position = match step % 3 {
            0 => 0,
            1 => remaining.len() - 1,
            _ => remaining.len() / 2,
        };
        let name = remaining.remove(position);
        let case = format!("delete {name:?} at position {position} of {}", remaining.len() + 1);

        // The data source is readable before the delete, so the 404 after it is the
        // delete's own effect.
        let before = get_data_source(&router, &api_id, &name).await;
        assert_eq!(
            before.status, 200,
            "case {case}: expected the data source to exist before the delete, got {before:?}"
        );

        let deleted = delete_data_source(&router, &api_id, &name).await;
        assert_empty_success(&case, &deleted);
        assert_gone(&router, &api_id, &name, &case).await;

        // Every data source the delete did not name is still there, reporting the
        // same body it reported before — the delete removed one record, not a class.
        let listed = list_names(&router, &api_id).await;
        assert_eq!(
            listed, remaining,
            "case {case}: expected the listing to hold exactly {remaining:?}, got {listed:?}"
        );
        for survivor in &remaining {
            let read = get_data_source(&router, &api_id, survivor).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected {survivor:?} to survive, got {read:?}"
            );
            assert_eq!(
                read.body["dataSource"]["name"].as_str(),
                Some(survivor.as_str()),
                "case {case}: expected {survivor:?} to report its own name, got {read:?}"
            );
        }
        step += 1;
    }

    // The last delete leaves an API that holds no data source at all, which still
    // lists (as an empty page) rather than failing.
    let listed = list_names(&router, &api_id).await;
    assert!(
        listed.is_empty(),
        "case the whole listing after deleting every data source: expected none, got {listed:?}"
    );
}

/// A deleted name is free again: the identical create is accepted after the delete and
/// reads back as a new data source, so the delete reserved nothing.
#[tokio::test]
async fn a_deleted_name_can_be_created_again() {
    let router = router();
    let api_id = create_api(&router, "Api_data_source_delete_recreate").await;

    for (index, (data_source_type, needs_role)) in DATA_SOURCE_TYPES.iter().enumerate() {
        let name = format!("recreated_{index}");
        let case = format!("recreate {name:?} of type {data_source_type} after a delete");
        create_data_source(&router, &api_id, &name, data_source_type, *needs_role).await;

        let deleted = delete_data_source(&router, &api_id, &name).await;
        assert_empty_success(&case, &deleted);
        assert_gone(&router, &api_id, &name, &case).await;

        create_data_source(&router, &api_id, &name, data_source_type, *needs_role).await;
        let read = get_data_source(&router, &api_id, &name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the recreated data source to read back, got {read:?}"
        );
        assert_eq!(
            read.body["dataSource"]["type"].as_str(),
            Some(*data_source_type),
            "case {case}: expected the recreated type, got {read:?}"
        );
    }
}

/// A request that addresses no data source removes nothing and says which refusal it
/// is: a name nothing holds (including one an earlier delete removed) is a 404, an
/// `apiId` this account does not hold is a 404, and an `apiId` too long to be one is a
/// 400.
#[tokio::test]
async fn a_delete_that_addresses_nothing_is_refused_and_removes_nothing() {
    let router = router();
    let api_id = create_api(&router, "Api_data_source_delete_refusals").await;
    let kept = "kept";
    create_data_source(&router, &api_id, kept, "NONE", false).await;

    // A name the API never held.
    let absent = delete_data_source(&router, &api_id, "never_created").await;
    assert_eq!(
        absent.status, 404,
        "case delete a never-created name: expected 404, got {absent:?}"
    );
    assert_eq!(
        absent.error_type, "NotFoundException",
        "case delete a never-created name: expected NotFoundException, got {absent:?}"
    );
    assert_eq!(
        absent.message,
        not_found_message("never_created"),
        "case delete a never-created name: expected the refusal to name it, got {absent:?}"
    );

    // A name an earlier delete already removed: the second delete has nothing to
    // remove, so it takes the same refusal.
    let doomed = "deleted_twice";
    create_data_source(&router, &api_id, doomed, "NONE", false).await;
    let first = delete_data_source(&router, &api_id, doomed).await;
    assert_empty_success("the first delete of a name", &first);
    let second = delete_data_source(&router, &api_id, doomed).await;
    assert_eq!(
        second.status, 404,
        "case delete the same name twice: expected 404, got {second:?}"
    );
    assert_eq!(
        second.error_type, "NotFoundException",
        "case delete the same name twice: expected NotFoundException, got {second:?}"
    );
    assert_eq!(
        second.message,
        not_found_message(doomed),
        "case delete the same name twice: expected the refusal to name it, got {second:?}"
    );

    // An `apiId` of the right shape that this account does not hold, and one longer
    // than an API id, which cannot name an API at all.
    for (case, api, expected_status, expected_error) in [
        (
            "an apiId no API has",
            "z".repeat(API_ID_LEN),
            404,
            "NotFoundException",
        ),
        (
            "an apiId too long to be one",
            "z".repeat(API_ID_LEN + 1),
            400,
            "BadRequestException",
        ),
    ] {
        let answer = delete_data_source(&router, &api, kept).await;
        assert_eq!(
            answer.status, expected_status,
            "case delete under {case}: expected {expected_status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, expected_error,
            "case delete under {case}: expected {expected_error}, got {answer:?}"
        );
    }

    // None of those refusals touched the data source that does exist.
    let read = get_data_source(&router, &api_id, kept).await;
    assert_eq!(
        read.status, 200,
        "case the surviving data source: expected a refused delete to remove nothing, \
         got {read:?}"
    );
    let listed = list_names(&router, &api_id).await;
    assert_eq!(
        listed,
        vec![kept.to_string()],
        "case the whole listing after the refusals: expected only {kept:?}, got {listed:?}"
    );
}

/// A data source belongs to the API it was created under, so deleting one by name
/// under another API removes nothing: the name is addressed within its own API.
#[tokio::test]
async fn a_delete_under_another_api_removes_nothing() {
    let router = router();
    let first = create_api(&router, "Api_data_source_delete_scope_first").await;
    let second = create_api(&router, "Api_data_source_delete_scope_second").await;

    for (index, (data_source_type, needs_role)) in DATA_SOURCE_TYPES.iter().enumerate() {
        let name = format!("shared_{index}");
        let case = format!("delete {name:?} of type {data_source_type} under the other API");
        // The SAME name under both APIs: two different data sources.
        create_data_source(&router, &first, &name, data_source_type, *needs_role).await;

        // While the other API does not hold it, the delete is a 404 there ...
        let missing = delete_data_source(&router, &second, &name).await;
        assert_eq!(
            missing.status, 404,
            "case {case} before it holds one: expected 404, got {missing:?}"
        );
        let read = get_data_source(&router, &first, &name).await;
        assert_eq!(
            read.status, 200,
            "case {case} before it holds one: expected the first API to keep it, got {read:?}"
        );

        // ... and once it holds one of its own, deleting that one leaves the first
        // API's data source exactly where it was.
        create_data_source(&router, &second, &name, data_source_type, *needs_role).await;
        let deleted = delete_data_source(&router, &second, &name).await;
        assert_empty_success(&case, &deleted);
        assert_gone(&router, &second, &name, &case).await;

        let read = get_data_source(&router, &first, &name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the first API's data source to survive, got {read:?}"
        );
        assert_eq!(
            read.body["dataSource"]["name"].as_str(),
            Some(name.as_str()),
            "case {case}: expected it to report its own name, got {read:?}"
        );
    }
}
