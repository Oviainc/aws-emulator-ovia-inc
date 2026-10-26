// SPDX-License-Identifier: CC0-1.0

//! `UpdateFunction`, end to end over HTTP: updating a function a `CreateFunction` left
//! behind and reading it back through `GetFunction`.
//!
//! The rules asserted here are stated over CLASSES of input rather than over the one
//! request that exposed the defect:
//!
//! 1. every settable member of the request is the function's NEW value, so an update
//!    REPLACES the stored function's settable members rather than merging into them — a
//!    member the request omits is gone from the function — while the function's
//!    identity (its `functionId` and its ARN) is not settable and survives the update
//!    unchanged, and `GetFunction` answers the updated record. Asserted over a table of
//!    TRANSITIONS spanning both definition forms in both directions, the `description`
//!    states, the published `maxBatchSize` bounds and the `SyncConfig` strategies,
//!    including a transition that drops every optional member and one that takes them
//!    all back up;
//! 2. `dataSourceName` names "The Function DataSource name", so an update may point a
//!    function at any data source its own API holds, and a name no data source of that
//!    API carries is a 404 `NotFoundException` that leaves the function as it was — a
//!    name held by a DIFFERENT API included;
//! 3. a candidate whose definition members do not pair up is a 400
//!    `BadRequestException` and leaves the function as it was, exactly as on the
//!    create;
//! 4. the function the URI addresses is resolved before the body is judged: an `apiId`
//!    that cannot name an API is a 400 `BadRequestException`, one that names no API
//!    this account holds a 404 `NotFoundException`, and a `functionId` the addressed
//!    API does not hold a 404 — a function held by another API included.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together.

use serde_json::json;
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

// The router, the request helper and the API-creating helper are the crate's shared
// test harness, and the function table, request body and projection assertions are the
// `CreateFunction` suite's: `UpdateFunction` takes the same member set and answers the
// same `FunctionConfiguration`, so both are reused rather than copied.
use super::error_contract::{call, create_api, router, Answer};
use super::functions::{
    assert_projects, create_data_source, create_function, get_function, request_body, rows, Row,
    FUNCTION_VERSION, MAX_BATCH_SIZES, RUNTIME,
};

/// Update the function `function_id` of `api_id` to what `row` describes, against
/// `data_source`.
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

/// A function carrying none of the optional members beyond the definition a VTL
/// function needs, used as both the target and the source of a transition that drops
/// every optional member and of one that takes them all back up.
fn minimal() -> Row {
    Row {
        request_template: Some("{\"version\": \"2018-05-29\"}"),
        function_version: Some(FUNCTION_VERSION),
        ..Row::bare(
            "a VTL function with no description, syncConfig or batch size",
            "fn_minimal",
        )
    }
}

/// The transition table: the stored function to create, and the function the update
/// asks for. The pairs differ along what the rule generalises over — the definition
/// form (both directions), the `description` states, the `maxBatchSize` bounds and the
/// `SyncConfig` strategies — rather than along the name alone.
fn transitions() -> Vec<(Row, Row)> {
    let catalog = rows();
    vec![
        // A VTL function becomes an APPSYNC_JS one: the mapping templates and the
        // template version are gone, and `code` and `runtime` take their place.
        (catalog[0].clone(), catalog[3].clone()),
        // ... and back the other way.
        (catalog[3].clone(), catalog[0].clone()),
        // One conflict-resolution strategy for another, at a different batch size.
        (catalog[1].clone(), catalog[2].clone()),
        // Every optional member dropped.
        (catalog[4].clone(), minimal()),
        // ... and every optional member taken back up.
        (minimal(), catalog[4].clone()),
        // Only the batch size moves, from the published minimum to the published
        // maximum.
        (
            catalog[0].clone(),
            Row {
                label: "a VTL function moved to the maximum batch size",
                max_batch_size: Some(MAX_BATCH_SIZES[2]),
                ..catalog[0].clone()
            },
        ),
    ]
}

#[tokio::test]
async fn update_replaces_every_settable_member_and_keeps_the_function_s_identity() {
    let router = router();
    let api_id = create_api(&router, "Api_function_update").await;
    let data_source = create_data_source(&router, &api_id, "fn_update_source").await;

    for (from, to) in transitions() {
        let case = format!("{} -> {}", from.label, to.label);
        let created = create_function(&router, &api_id, &data_source, &from).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateFunction 200, got {created:?}"
        );
        let stored = &created.body["functionConfiguration"];
        let function_id = stored["functionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let arn = stored["functionArn"].clone();

        let updated = update_function(&router, &api_id, &function_id, &data_source, &to).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateFunction 200, got {updated:?}"
        );
        let function = &updated.body["functionConfiguration"];
        assert_projects(&to, &api_id, &data_source, function, "update");

        // The id and the ARN are not settable: they identify the function, so the
        // update reports the ones the create minted.
        assert_eq!(
            function["functionId"],
            json!(function_id),
            "case {case:?}: expected the stored functionId {function_id:?}, got {:?}",
            function["functionId"]
        );
        assert_eq!(
            function["functionArn"], arn,
            "case {case:?}: expected the stored functionArn {arn:?}, got {:?}",
            function["functionArn"]
        );

        // The update REPLACED the stored record, so the reader answers the updated
        // function member for member — including the members the update dropped.
        let read = get_function(&router, &api_id, &function_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetFunction 200, got {read:?}"
        );
        assert_eq!(
            read.body["functionConfiguration"], *function,
            "case {case:?}: expected GetFunction to answer the updated function {function:?}, \
             got {:?}",
            read.body["functionConfiguration"]
        );
    }
}

#[tokio::test]
async fn an_update_points_a_function_at_a_data_source_its_own_api_holds() {
    let router = router();
    let api_id = create_api(&router, "Api_function_update_data_source").await;
    let first = create_data_source(&router, &api_id, "fn_update_first_source").await;
    let second = create_data_source(&router, &api_id, "fn_update_second_source").await;
    // A data source of ANOTHER API: held by this account, but not by this API.
    let other_api = create_api(&router, "Api_function_update_other").await;
    let elsewhere = create_data_source(&router, &other_api, "fn_update_other_source").await;

    let row = &rows()[0];
    let created = create_function(&router, &api_id, &first, row).await;
    assert_eq!(
        created.status, 200,
        "case seed one function: expected 200, got {created:?}"
    );
    let function_id = created.body["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A data source the API holds: the function resolves against it from now on.
    let moved = update_function(&router, &api_id, &function_id, &second, row).await;
    assert_eq!(
        moved.status, 200,
        "case move the function to {second:?}: expected 200, got {moved:?}"
    );
    assert_eq!(
        moved.body["functionConfiguration"]["dataSourceName"],
        json!(second),
        "case move the function to {second:?}: expected that dataSourceName, got {moved:?}"
    );
    let read = get_function(&router, &api_id, &function_id).await;
    assert_eq!(
        read.body["functionConfiguration"]["dataSourceName"],
        json!(second),
        "case read the moved function: expected dataSourceName {second:?}, got {read:?}"
    );

    // A name no data source of THIS API carries names no resource, so the update is
    // refused and the function keeps the data source it had.
    for name in ["fn_never_created", elsewhere.as_str()] {
        let answer = update_function(&router, &api_id, &function_id, name, row).await;
        assert_eq!(
            answer.status, 404,
            "case move the function to {name:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "NotFoundException",
            "case move the function to {name:?}: expected NotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.message,
            format!("Data source {name} not found"),
            "case move the function to {name:?}: expected the data-source not-found text, \
             got {answer:?}"
        );
        let after = get_function(&router, &api_id, &function_id).await;
        assert_eq!(
            after.body["functionConfiguration"], read.body["functionConfiguration"],
            "case move the function to {name:?}: a refused update changes nothing, expected \
             {:?}, got {:?}",
            read.body["functionConfiguration"], after.body["functionConfiguration"]
        );
    }
}

#[tokio::test]
async fn an_update_whose_definition_does_not_pair_up_is_refused_and_changes_nothing() {
    let router = router();
    let api_id = create_api(&router, "Api_function_update_definitions").await;
    let data_source = create_data_source(&router, &api_id, "fn_update_definition_source").await;

    let stored_row = &rows()[0];
    let created = create_function(&router, &api_id, &data_source, stored_row).await;
    assert_eq!(
        created.status, 200,
        "case seed one function: expected 200, got {created:?}"
    );
    let function_id = created.body["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The rows differ along which member of a pair the candidate carries: `code` needs
    // a `runtime`, a `runtime` needs `code`, and a function defined with mapping
    // templates needs the template version.
    let code = "export function request(ctx) { return {}; }";
    let template = "{\"version\": \"2018-05-29\"}";
    let refused = vec![
        Row {
            code: Some(code),
            ..Row::bare("code without a runtime", "fn_update_code_only")
        },
        Row {
            runtime: Some(RUNTIME),
            ..Row::bare("a runtime without code", "fn_update_runtime_only")
        },
        Row {
            request_template: Some(template),
            ..Row::bare(
                "mapping templates without a functionVersion",
                "fn_update_no_version",
            )
        },
        Row {
            response_template: Some(template),
            function_version: Some(""),
            ..Row::bare("an empty functionVersion", "fn_update_empty_version")
        },
        Row {
            ..Row::bare(
                "neither a definition nor a version",
                "fn_update_nothing",
            )
        },
    ];

    for row in refused {
        let answer = update_function(&router, &api_id, &function_id, &data_source, &row).await;
        assert_eq!(
            answer.status, 400,
            "case update to {}: expected 400, got {answer:?}",
            row.label
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case update to {}: expected BadRequestException, got {answer:?}",
            row.label
        );
        assert!(
            answer.body["functionConfiguration"].is_null(),
            "case update to {}: a refused update answers no function, got {answer:?}",
            row.label
        );

        // The stored function is still the one the create left behind.
        let read = get_function(&router, &api_id, &function_id).await;
        assert_eq!(
            read.status, 200,
            "case update to {}: expected the function to survive the refusal, got {read:?}",
            row.label
        );
        assert_projects(
            stored_row,
            &api_id,
            &data_source,
            &read.body["functionConfiguration"],
            "after a refused update",
        );
    }
}

#[tokio::test]
async fn the_addressed_api_and_function_are_resolved_before_the_body() {
    let router = router();
    let api_id = create_api(&router, "Api_function_update_refusals").await;
    let data_source = create_data_source(&router, &api_id, "fn_update_refusal_source").await;
    let other_api = create_api(&router, "Api_function_update_refusals_other").await;
    create_data_source(&router, &other_api, "fn_update_refusal_source").await;

    let row = &rows()[0];
    let created = create_function(&router, &api_id, &data_source, row).await;
    assert_eq!(
        created.status, 200,
        "case seed one function: expected 200, got {created:?}"
    );
    let function_id = created.body["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // An `apiId` longer than an AppSync API id holds more characters than one has, so
    // it names no API at all; an id of the right shape this account simply does not
    // hold is a missing resource.
    let unnameable = "z".repeat(state::API_ID_LEN + 1);
    let absent = "z".repeat(state::API_ID_LEN);
    for (api, status, error_type, message) in [
        (
            unnameable.as_str(),
            400,
            "BadRequestException",
            format!("Invalid API ID {unnameable}"),
        ),
        (
            absent.as_str(),
            404,
            "NotFoundException",
            "API not found.".to_string(),
        ),
    ] {
        let answer = update_function(&router, api, &function_id, &data_source, row).await;
        assert_eq!(
            answer.status, status,
            "case UpdateFunction under {api:?}: expected {status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, error_type,
            "case UpdateFunction under {api:?}: expected {error_type}, got {answer:?}"
        );
        assert_eq!(
            answer.message, message,
            "case UpdateFunction under {api:?}: expected {message:?}, got {answer:?}"
        );
    }

    // A `functionId` the addressed API does not hold names no function — never
    // created, or held by another API — and the refusal names the id the caller asked
    // for. The candidate body is a runnable definition against a data source both APIs
    // hold, so the refusal is about the addressed function alone.
    let unheld_id = "z".repeat(state::API_ID_LEN);
    let other_function = create_function(&router, &other_api, "fn_update_refusal_source", row).await;
    let other_id = other_function.body["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for unheld in ["fn_never_created", unheld_id.as_str(), other_id.as_str()] {
        let answer = update_function(&router, &api_id, unheld, &data_source, row).await;
        assert_eq!(
            answer.status, 404,
            "case UpdateFunction on the unheld id {unheld:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "NotFoundException",
            "case UpdateFunction on the unheld id {unheld:?}: expected NotFoundException, \
             got {answer:?}"
        );
        assert_eq!(
            answer.message,
            format!("Function {unheld} not found"),
            "case UpdateFunction on the unheld id {unheld:?}: expected the function not-found \
             text, got {answer:?}"
        );
        // A refused update creates nothing under the id it was aimed at.
        let read = get_function(&router, &api_id, unheld).await;
        assert_eq!(
            read.status, 404,
            "case UpdateFunction on the unheld id {unheld:?}: expected the id to stay unheld, \
             got {read:?}"
        );
    }

    // The function of the OTHER API is untouched by the update that named its id under
    // this API.
    let elsewhere = get_function(&router, &other_api, &other_id).await;
    assert_eq!(
        elsewhere.status, 200,
        "case the other API's function: expected it to survive, got {elsewhere:?}"
    );
    assert_eq!(
        elsewhere.body["functionConfiguration"], other_function.body["functionConfiguration"],
        "case the other API's function: expected {:?}, got {:?}",
        other_function.body["functionConfiguration"], elsewhere.body["functionConfiguration"]
    );
}
