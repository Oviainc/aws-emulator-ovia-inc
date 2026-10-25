// SPDX-License-Identifier: CC0-1.0

//! Functions, end to end over HTTP: `CreateFunction` and the `GetFunction` that reads
//! one back.
//!
//! The rules asserted here are stated over CLASSES of input rather than over the one
//! request that exposed the defect:
//!
//! 1. creating a function mints an opaque `functionId`, persists the function under
//!    its GraphQL API and answers the stored record — its ARN named under the API's
//!    own path, its `dataSourceName`, its definition members (mapping templates and
//!    `functionVersion`, or `code` and `runtime`), its `syncConfig` and its
//!    `maxBatchSize` as the request set them, and a `description` only where the
//!    request carried a non-empty one — and `GetFunction` answers the same body.
//!    Asserted over a table spanning both definition forms, both published
//!    `maxBatchSize` bounds, the `description` states and the `SyncConfig`
//!    strategies;
//! 2. the function NAME is not an identifier ("The function name does not have to be
//!    unique"): two creates under one name are two functions, each with its own id
//!    and ARN, and each is read back by its own id;
//! 3. `dataSourceName` names "The Function DataSource name", so a name no data source
//!    of that API carries is a 404 `NotFoundException` and persists nothing — a name
//!    held by a DIFFERENT API included;
//! 4. a definition whose members do not pair up is a 400 `BadRequestException`:
//!    `code` without `runtime`, `runtime` without `code`, and mapping templates
//!    without `functionVersion`;
//! 5. an API holds at most the AppSync quota of functions: a create past it is
//!    refused and persists nothing, so repeating the call cannot grow the store
//!    without limit;
//! 6. an `apiId` that cannot name an API is a 400 `BadRequestException`, one that
//!    names no API this account holds a 404 `NotFoundException`, and a `functionId`
//!    the API does not hold a 404.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

// The router, the request helper and the API-creating helper are the crate's shared
// test harness, reused rather than copied.
use super::error_contract::{call, create_api, router, Answer};

/// The region the shared harness signs for; the service reads it off the credential
/// scope, so it must appear in every ARN it mints.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The number of characters an AppSync opaque id has.
const ID_LEN: usize = 26;

/// The only `functionVersion` AppSync supports: "Currently, the supported value is
/// 2018-05-29" (`CreateFunctionRequest$functionVersion`).
const FUNCTION_VERSION: &str = "2018-05-29";

/// The only runtime AppSync supports, as `AppSyncRuntime` requires both its members:
/// `name` "the only allowed value is APPSYNC_JS" and `runtimeVersion` "the only
/// allowed version is 1.0.0".
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The published bounds of `maxBatchSize` (`MaxBatchSize`, range 0..2000) plus a value
/// between them. Source: `smithy_query(query="shape", shape="MaxBatchSize")`.
const MAX_BATCH_SIZES: &[i64] = &[0, 1, 2_000];

/// One row of the round-trip table: a function to create.
#[derive(Debug, Clone)]
struct Row {
    /// What the row exercises, named in every assertion of it.
    label: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    request_template: Option<&'static str>,
    response_template: Option<&'static str>,
    function_version: Option<&'static str>,
    code: Option<&'static str>,
    runtime: Option<(&'static str, &'static str)>,
    sync_config: Option<Value>,
    max_batch_size: Option<i64>,
}

impl Row {
    /// A row with every optional member unset, which each row below fills in.
    fn bare(label: &'static str, name: &'static str) -> Self {
        Self {
            label,
            name,
            description: None,
            request_template: None,
            response_template: None,
            function_version: None,
            code: None,
            runtime: None,
            sync_config: None,
            max_batch_size: None,
        }
    }
}

/// The round-trip table: both definition forms a function can take (VTL mapping
/// templates with a `functionVersion`, and `code` with a `runtime`), crossed with the
/// `description` states, both published `maxBatchSize` bounds and a value between
/// them, and the `SyncConfig` strategies.
///
/// Source for the value sets: `smithy_query(query="shape",
/// shape="FunctionConfiguration,AppSyncRuntime,SyncConfig,ConflictHandlerType,
/// ConflictDetectionType,MaxBatchSize")`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The smallest VTL function: one template and the template version.
            request_template: Some("{\"version\": \"2018-05-29\"}"),
            function_version: Some(FUNCTION_VERSION),
            ..Row::bare("a VTL function with only a request template", "fn_vtl_request")
        },
        Row {
            request_template: Some("{\"version\": \"2018-05-29\", \"payload\": {}}"),
            response_template: Some("$util.toJson($ctx.result)"),
            function_version: Some(FUNCTION_VERSION),
            description: Some("the pipeline step"),
            max_batch_size: Some(MAX_BATCH_SIZES[0]),
            ..Row::bare("a VTL function at the minimum batch size", "fn_vtl_both")
        },
        Row {
            request_template: Some("{\"version\": \"2018-05-29\"}"),
            response_template: Some("$util.toJson($ctx.result)"),
            function_version: Some(FUNCTION_VERSION),
            // An empty description describes nothing, so none is reported.
            description: Some(""),
            max_batch_size: Some(MAX_BATCH_SIZES[2]),
            sync_config: Some(json!({
                "conflictDetection": "VERSION",
                "conflictHandler": "OPTIMISTIC_CONCURRENCY",
            })),
            ..Row::bare("a VTL function at the maximum batch size", "fn_vtl_sync")
        },
        Row {
            request_template: Some("{\"version\": \"2018-05-29\"}"),
            function_version: Some(FUNCTION_VERSION),
            max_batch_size: Some(MAX_BATCH_SIZES[1]),
            sync_config: Some(json!({
                "conflictDetection": "NONE",
                "conflictHandler": "LAMBDA",
                "lambdaConflictHandlerConfig": {
                    "lambdaConflictHandlerArn":
                        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:resolve_conflicts"),
                },
            })),
            ..Row::bare("a VTL function resolving conflicts with Lambda", "fn_vtl_lambda")
        },
        Row {
            code: Some("export function request(ctx) { return {}; }"),
            runtime: Some(RUNTIME),
            description: Some("the APPSYNC_JS step"),
            ..Row::bare("an APPSYNC_JS function", "fn_js")
        },
        Row {
            code: Some("export function response(ctx) { return ctx.result; }"),
            runtime: Some(RUNTIME),
            max_batch_size: Some(MAX_BATCH_SIZES[2]),
            sync_config: Some(json!({
                "conflictDetection": "NONE",
                "conflictHandler": "NONE",
            })),
            ..Row::bare("an APPSYNC_JS function at the maximum batch size", "fn_js_batched")
        },
    ]
}

/// `row` as a `CreateFunction` request body against `data_source`.
fn request_body(row: &Row, data_source: &str) -> Value {
    let mut body = json!({"name": row.name, "dataSourceName": data_source});
    let object = body.as_object_mut().expect("the body is an object");
    if let Some(description) = row.description {
        object.insert("description".to_string(), json!(description));
    }
    if let Some(template) = row.request_template {
        object.insert("requestMappingTemplate".to_string(), json!(template));
    }
    if let Some(template) = row.response_template {
        object.insert("responseMappingTemplate".to_string(), json!(template));
    }
    if let Some(version) = row.function_version {
        object.insert("functionVersion".to_string(), json!(version));
    }
    if let Some(code) = row.code {
        object.insert("code".to_string(), json!(code));
    }
    if let Some((name, version)) = row.runtime {
        object.insert(
            "runtime".to_string(),
            json!({"name": name, "runtimeVersion": version}),
        );
    }
    if let Some(sync_config) = row.sync_config.clone() {
        object.insert("syncConfig".to_string(), sync_config);
    }
    if let Some(max_batch_size) = row.max_batch_size {
        object.insert("maxBatchSize".to_string(), json!(max_batch_size));
    }
    body
}

/// Create one data source the functions can resolve against, and return its name.
async fn create_data_source(router: &MultiServiceRouter, api_id: &str, name: &str) -> String {
    let answer = call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": name, "type": "NONE"})),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case create the data source {name:?}: expected 200, got {answer:?}"
    );
    name.to_string()
}

/// Create one function from `row` under `api_id` and return the answer.
async fn create_function(
    router: &MultiServiceRouter,
    api_id: &str,
    data_source: &str,
    row: &Row,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/functions"),
        Some(request_body(row, data_source)),
    )
    .await
}

/// Read one function back by its id.
async fn get_function(router: &MultiServiceRouter, api_id: &str, function_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/functions/{function_id}"),
        None,
    )
    .await
}

/// Assert `function` reports exactly what `row` asked for, under `api_id`.
fn assert_projects(row: &Row, api_id: &str, data_source: &str, function: &Value, source: &str) {
    let case = format!("{} ({source})", row.label);

    // The id is opaque and addressable: it is one of this service's own ids, and
    // every operation that addresses a function takes it as a `ResourceName`, whose
    // pattern rules out a leading digit.
    let function_id = function["functionId"].as_str().unwrap_or_default();
    assert_eq!(
        function_id.chars().count(),
        ID_LEN,
        "case {case:?}: expected a {ID_LEN}-character functionId, got {:?}",
        function["functionId"]
    );
    assert!(
        function_id.starts_with(|first: char| first.is_ascii_alphabetic()),
        "case {case:?}: expected a functionId a ResourceName can carry, got {function_id:?}"
    );

    let expected_arn = format!(
        "arn:aws:appsync:{REGION}:{ACCOUNT}:{}",
        state::function_resource(api_id, function_id)
    );
    assert_eq!(
        function["functionArn"],
        json!(expected_arn),
        "case {case:?}: expected functionArn {expected_arn}, got {:?}",
        function["functionArn"]
    );
    assert_eq!(
        function["name"],
        json!(row.name),
        "case {case:?}: expected name {:?}, got {:?}",
        row.name,
        function["name"]
    );
    assert_eq!(
        function["dataSourceName"],
        json!(data_source),
        "case {case:?}: expected dataSourceName {data_source:?}, got {:?}",
        function["dataSourceName"]
    );

    // A description is reported only where the request carried a non-empty one: an
    // empty description describes nothing, so the member is reported absent just like
    // an omitted one.
    match row.description.filter(|description| !description.is_empty()) {
        Some(description) => assert_eq!(
            function["description"],
            json!(description),
            "case {case:?}: expected description {description:?}, got {:?}",
            function["description"]
        ),
        None => assert!(
            function["description"].is_null(),
            "case {case:?}: expected no description, got {:?}",
            function["description"]
        ),
    }

    // Every definition member the request carried is reported with the value it was
    // given, and no member it left out is reported at all.
    for (member, sent) in [
        ("requestMappingTemplate", row.request_template),
        ("responseMappingTemplate", row.response_template),
        ("functionVersion", row.function_version),
        ("code", row.code),
    ] {
        match sent {
            Some(value) => assert_eq!(
                function[member],
                json!(value),
                "case {case:?}: expected {member} {value:?}, got {:?}",
                function[member]
            ),
            None => assert!(
                function[member].is_null(),
                "case {case:?}: expected no {member}, got {:?}",
                function[member]
            ),
        }
    }
    match row.runtime {
        Some((name, version)) => assert_eq!(
            function["runtime"],
            json!({"name": name, "runtimeVersion": version}),
            "case {case:?}: expected runtime {name}/{version}, got {:?}",
            function["runtime"]
        ),
        None => assert!(
            function["runtime"].is_null(),
            "case {case:?}: expected no runtime, got {:?}",
            function["runtime"]
        ),
    }
    match row.sync_config.as_ref() {
        Some(sync_config) => {
            for (key, sent) in sync_config.as_object().expect("the syncConfig is an object") {
                assert_eq!(
                    &function["syncConfig"][key], sent,
                    "case {case:?}: expected syncConfig.{key} {sent:?}, got {:?}",
                    function["syncConfig"][key]
                );
            }
        }
        None => assert!(
            function["syncConfig"].is_null(),
            "case {case:?}: expected no syncConfig, got {:?}",
            function["syncConfig"]
        ),
    }
    // `maxBatchSize` is modelled with a default of 0, so a request that names none
    // leaves the function at that default rather than at no value.
    let expected_batch_size = row.max_batch_size.unwrap_or(0);
    assert_eq!(
        function["maxBatchSize"],
        json!(expected_batch_size),
        "case {case:?}: expected maxBatchSize {expected_batch_size}, got {:?}",
        function["maxBatchSize"]
    );
}

#[tokio::test]
async fn create_persists_every_function_shape_and_get_reads_it_back() {
    let router = router();
    let api_id = create_api(&router, "Api_function_round_trip").await;
    let data_source = create_data_source(&router, &api_id, "fn_round_trip_source").await;

    for row in rows() {
        let created = create_function(&router, &api_id, &data_source, &row).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected CreateFunction 200, got {created:?}",
            row.label
        );
        let function = &created.body["functionConfiguration"];
        assert_projects(&row, &api_id, &data_source, function, "create");

        let function_id = function["functionId"].as_str().unwrap_or_default();
        let read = get_function(&router, &api_id, function_id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected GetFunction 200, got {read:?}",
            row.label
        );
        assert_projects(
            &row,
            &api_id,
            &data_source,
            &read.body["functionConfiguration"],
            "get",
        );
        // The read answers the SAME record the create persisted, member for member.
        assert_eq!(
            read.body["functionConfiguration"], created.body["functionConfiguration"],
            "case {}: expected GetFunction to answer the created function {:?}, got {:?}",
            row.label, created.body["functionConfiguration"], read.body["functionConfiguration"]
        );
    }
}

#[tokio::test]
async fn a_function_name_need_not_be_unique_and_the_id_identifies_one() {
    let router = router();
    let api_id = create_api(&router, "Api_function_names").await;
    let data_source = create_data_source(&router, &api_id, "fn_name_source").await;
    let other_api = create_api(&router, "Api_function_names_other").await;

    for row in rows() {
        // The same request twice: two functions, not one replaced.
        let first = create_function(&router, &api_id, &data_source, &row).await;
        let second = create_function(&router, &api_id, &data_source, &row).await;
        for (answer, which) in [(&first, "first"), (&second, "second")] {
            assert_eq!(
                answer.status, 200,
                "case {} ({which} create under one name): expected 200, got {answer:?}",
                row.label
            );
        }
        let first_id = first.body["functionConfiguration"]["functionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let second_id = second.body["functionConfiguration"]["functionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            first_id, second_id,
            "case {}: expected two ids for two creates under one name, got {first_id:?} twice",
            row.label
        );
        assert_ne!(
            first.body["functionConfiguration"]["functionArn"],
            second.body["functionConfiguration"]["functionArn"],
            "case {}: expected two ARNs for two functions, got {:?} twice",
            row.label,
            first.body["functionConfiguration"]["functionArn"]
        );

        // Each is read back by its OWN id, and both are still there.
        for id in [&first_id, &second_id] {
            let read = get_function(&router, &api_id, id).await;
            assert_eq!(
                read.status, 200,
                "case {} (reading {id:?}): expected 200, got {read:?}",
                row.label
            );
            assert_eq!(
                read.body["functionConfiguration"]["functionId"],
                json!(id),
                "case {} (reading {id:?}): expected that function, got {read:?}",
                row.label
            );

            // A function belongs to the API it was created under, so another API does
            // not hold it.
            let elsewhere = get_function(&router, &other_api, id).await;
            assert_eq!(
                elsewhere.status, 404,
                "case {} (reading {id:?} under another API): expected 404, got {elsewhere:?}",
                row.label
            );
            assert_eq!(
                elsewhere.error_type, "NotFoundException",
                "case {} (reading {id:?} under another API): expected NotFoundException, \
                 got {elsewhere:?}",
                row.label
            );
        }
    }
}

#[tokio::test]
async fn a_data_source_the_api_does_not_hold_is_a_not_found() {
    let router = router();
    let api_id = create_api(&router, "Api_function_data_source").await;
    create_data_source(&router, &api_id, "fn_held_source").await;
    // A data source of ANOTHER API: held by this account, but not by this API.
    let other_api = create_api(&router, "Api_function_data_source_other").await;
    let elsewhere = create_data_source(&router, &other_api, "fn_other_source").await;

    let row = &rows()[0];
    for data_source in ["fn_never_created", elsewhere.as_str()] {
        let answer = create_function(&router, &api_id, data_source, row).await;
        assert_eq!(
            answer.status, 404,
            "case a function on the data source {data_source:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "NotFoundException",
            "case a function on the data source {data_source:?}: expected NotFoundException, \
             got {answer:?}"
        );
        assert!(
            answer.body["functionConfiguration"].is_null(),
            "case a function on the data source {data_source:?}: a refused create persists \
             nothing, got {answer:?}"
        );
    }

    // The same request against a data source the API DOES hold is created, so the
    // refusal is about the data source rather than about the function.
    let served = create_function(&router, &api_id, "fn_held_source", row).await;
    assert_eq!(
        served.status, 200,
        "case a function on a data source the API holds: expected 200, got {served:?}"
    );
}

#[tokio::test]
async fn a_definition_whose_members_do_not_pair_up_is_refused() {
    let router = router();
    let api_id = create_api(&router, "Api_function_definitions").await;
    let data_source = create_data_source(&router, &api_id, "fn_definition_source").await;

    // (label, the row, whether AppSync can run the definition it describes). The rows
    // differ along which member of a pair the request carries: `code` needs a
    // `runtime` and a `runtime` needs `code`, while a function defined with mapping
    // templates needs the template version.
    let code = "export function request(ctx) { return {}; }";
    let template = "{\"version\": \"2018-05-29\"}";
    let cases: Vec<(Row, bool)> = vec![
        (
            Row {
                code: Some(code),
                ..Row::bare("code without a runtime", "fn_code_only")
            },
            false,
        ),
        (
            Row {
                runtime: Some(RUNTIME),
                ..Row::bare("a runtime without code", "fn_runtime_only")
            },
            false,
        ),
        (
            Row {
                request_template: Some(template),
                ..Row::bare("mapping templates without a functionVersion", "fn_no_version")
            },
            false,
        ),
        (
            Row {
                response_template: Some(template),
                function_version: Some(""),
                ..Row::bare("an empty functionVersion", "fn_empty_version")
            },
            false,
        ),
        (
            Row {
                ..Row::bare("neither a definition nor a version", "fn_nothing")
            },
            false,
        ),
        (
            Row {
                code: Some(code),
                runtime: Some(RUNTIME),
                ..Row::bare("code with its runtime", "fn_code_and_runtime")
            },
            true,
        ),
        (
            Row {
                request_template: Some(template),
                function_version: Some(FUNCTION_VERSION),
                ..Row::bare("templates with their version", "fn_template_and_version")
            },
            true,
        ),
    ];

    for (row, runnable) in cases {
        let answer = create_function(&router, &api_id, &data_source, &row).await;
        if runnable {
            assert_eq!(
                answer.status, 200,
                "case {}: expected a runnable definition to be created, got {answer:?}",
                row.label
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {}: expected 400, got {answer:?}",
            row.label
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case {}: expected BadRequestException, got {answer:?}",
            row.label
        );
        assert!(
            answer.body["functionConfiguration"].is_null(),
            "case {}: a refused create persists nothing, got {answer:?}",
            row.label
        );
    }
}

#[tokio::test]
async fn an_api_holds_at_most_the_quota_of_functions() {
    /// The AppSync service quota for functions on ONE GraphQL API, which
    /// `CreateFunction` refuses a create past, so repeating the call cannot grow the
    /// store without limit.
    const QUOTA: usize = 1_000;

    let router = router();
    let api_id = create_api(&router, "Api_function_quota").await;
    let data_source = create_data_source(&router, &api_id, "fn_quota_source").await;
    let row = rows().swap_remove(0);

    for index in 0..QUOTA {
        let answer = create_function(&router, &api_id, &data_source, &row).await;
        assert_eq!(
            answer.status, 200,
            "case function {index} of the quota: expected 200, got {answer:?}"
        );
    }

    let past = create_function(&router, &api_id, &data_source, &row).await;
    assert_eq!(
        past.status, 400,
        "case one function past the quota of {QUOTA}: expected 400, got {past:?}"
    );
    assert_eq!(
        past.error_type, "BadRequestException",
        "case one function past the quota of {QUOTA}: expected BadRequestException, got {past:?}"
    );
    assert!(
        past.body["functionConfiguration"].is_null(),
        "case one function past the quota of {QUOTA}: a refused create persists nothing, \
         got {past:?}"
    );
}

#[tokio::test]
async fn an_unnameable_api_is_a_bad_request_and_an_absent_one_a_not_found() {
    let router = router();
    let api_id = create_api(&router, "Api_function_refusals").await;
    let data_source = create_data_source(&router, &api_id, "fn_refusal_source").await;
    let row = &rows()[0];

    // An `apiId` longer than an AppSync API id holds more characters than one has, so
    // it names no API at all; an id of the right shape this account simply does not
    // hold is a missing resource.
    let unnameable = "z".repeat(state::API_ID_LEN + 1);
    let absent = "z".repeat(state::API_ID_LEN);
    let created = create_function(&router, &api_id, &data_source, row).await;
    let function_id = created.body["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        created.status, 200,
        "case seed one function: expected 200, got {created:?}"
    );

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
        let create = create_function(&router, api, &data_source, row).await;
        assert_eq!(
            create.status, status,
            "case CreateFunction under {api:?}: expected {status}, got {create:?}"
        );
        assert_eq!(
            create.error_type, error_type,
            "case CreateFunction under {api:?}: expected {error_type}, got {create:?}"
        );
        assert_eq!(
            create.message, message,
            "case CreateFunction under {api:?}: expected {message:?}, got {create:?}"
        );

        let read = get_function(&router, api, &function_id).await;
        assert_eq!(
            read.status, status,
            "case GetFunction under {api:?}: expected {status}, got {read:?}"
        );
        assert_eq!(
            read.error_type, error_type,
            "case GetFunction under {api:?}: expected {error_type}, got {read:?}"
        );
        assert_eq!(
            read.message, message,
            "case GetFunction under {api:?}: expected {message:?}, got {read:?}"
        );
    }

    // A `functionId` the API does not hold is a not-found, not an empty answer.
    let unheld_id = "z".repeat(ID_LEN);
    for unheld in ["fn_never_created", unheld_id.as_str()] {
        let read = get_function(&router, &api_id, unheld).await;
        assert_eq!(
            read.status, 404,
            "case GetFunction on the unheld id {unheld:?}: expected 404, got {read:?}"
        );
        assert_eq!(
            read.error_type, "NotFoundException",
            "case GetFunction on the unheld id {unheld:?}: expected NotFoundException, \
             got {read:?}"
        );
    }
}
