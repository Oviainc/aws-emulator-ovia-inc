// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the BODY of a refusal this service states itself.
//!
//! Athena states a request it refuses as `InvalidRequestException` (HTTP 400), and the
//! body of that refusal carries the refusal CLASS as a code — under the member the model
//! declares (`AthenaErrorCode`) and under the name the wire also gives it (`ErrorCode`),
//! both holding the same value — beside the sentence for that class. A client branches on
//! the code and reads it under either name, so neither may be absent, and the sentence is
//! the service's own wording rather than one composed around the request.
//!
//! The rule is asserted over TABLES rather than one recorded request: the rows differ
//! along the operation refused, the class of refusal (a resource the account does not
//! hold, a name already taken, an execution that is not readable yet, a value outside a
//! rule), and — where the wording is fixed text — along the name the request carried, so a
//! sentence that must not vary with it is seen not to.
//!
//! The refusal a METASTORE answers is the boundary of that rule: Athena reports it as
//! `MetadataException`, whose model declares `Message` alone, so the last test here pins
//! that such a body carries no code member at all.
//!
//! Every call goes over the wire (awsJson1_1, `x-amz-target: AmazonAthena.<Operation>`)
//! through the same router the emulator boots.

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    self, ERROR_CODE_INVALID_INPUT, ERROR_CODE_INVALID_QUERY_EXECUTION_STATE,
    ERROR_CODE_QUERY_EXECUTION_NOT_FOUND, NO_OUTPUT_LOCATION, QUERY_STATE_CANCELLED,
    WORK_GROUP_ALREADY_CREATED, WORK_GROUP_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_work_group, router};

/// The wire error code (`__type`) of a refusal body, with the shape-id prefix the awsJson
/// protocols may carry stripped.
fn error_code(body: &Value) -> String {
    let raw = body["__type"].as_str().unwrap_or("<no __type>");
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

/// The `Message` a refusal body carries.
fn message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .unwrap_or("<no Message>")
        .to_string()
}

/// Where the queries in this suite write their results. Nothing is written to it; what
/// matters is that a query HAS a location, so the refusals below are the ones each row
/// names rather than the missing-location one.
const OUTPUT_LOCATION: &str = "s3://athena-results/refusal-suite/";

/// A `StartQueryExecution` request for `sql` that names where its results go.
fn query(sql: &str) -> Value {
    json!({
        "QueryString": sql,
        "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
    })
}

/// The `State` the stored execution `id` is in.
async fn state_of(router: &MultiServiceRouter, id: &str) -> String {
    crate::harness::execution(router, id).await["Status"]["State"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert one refusal: HTTP 400, the `InvalidRequestException` envelope, the class code
/// under BOTH names, and `expected_message` word for word.
fn assert_refusal(case: &str, body: &Value, status: u16, code: &str, expected_message: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(body),
        "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got {body:?}"
    );
    assert_eq!(
        body["AthenaErrorCode"].as_str(),
        Some(code),
        "case {case:?}: expected AthenaErrorCode {code:?}, got {body:?}"
    );
    assert_eq!(
        body["ErrorCode"].as_str(),
        Some(code),
        "case {case:?}: expected ErrorCode {code:?} beside AthenaErrorCode, got {body:?}"
    );
    assert_eq!(
        message(body),
        expected_message,
        "case {case:?}: expected the message {expected_message:?}, got {:?}",
        message(body)
    );
}

/// Every refusal Athena states itself carries its class code under both names, with the
/// sentence that class is worded with.
#[tokio::test]
async fn a_refusal_carries_its_class_code_under_both_names_with_its_own_sentence() {
    let router = router();
    // One workgroup and one data catalog this account really holds, so the "already
    // taken" rows refuse a name that is taken and the "does not hold" rows refuse one
    // that is not.
    const HELD_WORK_GROUP: &str = "refusal-suite-wg";
    // The model's `WorkGroupName` is 1 to 128 characters, so the longest name a client can
    // send is a row of the duplicate class too.
    let longest_work_group = "w".repeat(128);
    create_work_group(&router, HELD_WORK_GROUP).await;
    create_work_group(&router, &longest_work_group).await;
    let (status, created) = call(
        &router,
        "CreateDataCatalog",
        &json!({
            "Name": "held_catalog",
            "Type": "GLUE",
            "Parameters": { "catalog-id": "111122223333" }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the data catalog must be registered, got {status} with {created:?}"
    );

    // A query that has not finished, and one a caller cancelled: two executions whose
    // results are not readable, for different reasons.
    let unfinished = crate::harness::start(&router, &query("SELECT 1 AS one")).await;
    let cancelled = crate::harness::start(&router, &query("SELECT 2 AS two")).await;
    let (status, stopped) = call(
        &router,
        "StopQueryExecution",
        &json!({ "QueryExecutionId": cancelled }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the query must be cancellable, got {status} with {stopped:?}"
    );
    // Read, not assumed: the state the execution reports is the one its refusal names.
    let unfinished_state = state_of(&router, &unfinished).await;

    // (the case, the operation, the request, the class code, the sentence)
    let rows: Vec<(&str, &str, Value, &str, String)> = vec![
        (
            "a query whose request and workgroup name no output location",
            "StartQueryExecution",
            json!({ "QueryString": "SELECT 1 AS one" }),
            ERROR_CODE_INVALID_INPUT,
            NO_OUTPUT_LOCATION.to_string(),
        ),
        (
            "a query whose request names an empty output location",
            "StartQueryExecution",
            json!({
                "QueryString": "SELECT 1 AS one",
                "ResultConfiguration": { "OutputLocation": "" }
            }),
            ERROR_CODE_INVALID_INPUT,
            NO_OUTPUT_LOCATION.to_string(),
        ),
        (
            "a query submitted to a workgroup this account does not hold",
            "StartQueryExecution",
            json!({
                "QueryString": "SELECT 1 AS one",
                "WorkGroup": "never-created",
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION }
            }),
            ERROR_CODE_INVALID_INPUT,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a read of a workgroup this account does not hold",
            "GetWorkGroup",
            json!({ "WorkGroup": "never-created" }),
            ERROR_CODE_INVALID_INPUT,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a read of a workgroup whose name differs only in case from one held",
            "GetWorkGroup",
            json!({ "WorkGroup": HELD_WORK_GROUP.to_uppercase() }),
            ERROR_CODE_INVALID_INPUT,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a read of a workgroup named by a single character",
            "GetWorkGroup",
            json!({ "WorkGroup": "w" }),
            ERROR_CODE_INVALID_INPUT,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "an update of a workgroup this account does not hold",
            "UpdateWorkGroup",
            json!({ "WorkGroup": "never-created", "Description": "nothing to update" }),
            ERROR_CODE_INVALID_INPUT,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a listing of the query history of a workgroup this account does not hold",
            "ListQueryExecutions",
            json!({ "WorkGroup": "never-created" }),
            ERROR_CODE_INVALID_INPUT,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a create of a workgroup under a name already taken",
            "CreateWorkGroup",
            json!({ "Name": HELD_WORK_GROUP }),
            ERROR_CODE_INVALID_INPUT,
            WORK_GROUP_ALREADY_CREATED.to_string(),
        ),
        (
            "a create of a workgroup under the longest name already taken",
            "CreateWorkGroup",
            json!({ "Name": longest_work_group }),
            ERROR_CODE_INVALID_INPUT,
            WORK_GROUP_ALREADY_CREATED.to_string(),
        ),
        (
            "a read of a data catalog nothing registered",
            "GetDataCatalog",
            json!({ "Name": "no_such_catalog" }),
            ERROR_CODE_INVALID_INPUT,
            state::data_catalog_not_found("no_such_catalog"),
        ),
        (
            "a read of a data catalog whose name differs only in case from one held",
            "GetDataCatalog",
            json!({ "Name": "HELD_CATALOG" }),
            ERROR_CODE_INVALID_INPUT,
            state::data_catalog_not_found("HELD_CATALOG"),
        ),
        (
            "a read of a data catalog named by a single character",
            "GetDataCatalog",
            json!({ "Name": "c" }),
            ERROR_CODE_INVALID_INPUT,
            state::data_catalog_not_found("c"),
        ),
        (
            "a read of a table in a data catalog nothing registered",
            "GetTableMetadata",
            json!({
                "CatalogName": "no_such_catalog",
                "DatabaseName": "some_db",
                "TableName": "some_table"
            }),
            ERROR_CODE_INVALID_INPUT,
            state::data_catalog_not_found("no_such_catalog"),
        ),
        (
            "a listing of the tables of a data catalog nothing registered",
            "ListTableMetadata",
            json!({ "CatalogName": "no_such_catalog", "DatabaseName": "some_db" }),
            ERROR_CODE_INVALID_INPUT,
            state::data_catalog_not_found("no_such_catalog"),
        ),
        (
            "a read of an execution no start minted",
            "GetQueryExecution",
            json!({ "QueryExecutionId": "does-not-exist" }),
            ERROR_CODE_QUERY_EXECUTION_NOT_FOUND,
            state::query_execution_not_found("does-not-exist"),
        ),
        (
            "a read of an execution under an id shaped like a minted one",
            "GetQueryExecution",
            json!({ "QueryExecutionId": "11111111-2222-3333-4444-555555555555" }),
            ERROR_CODE_QUERY_EXECUTION_NOT_FOUND,
            state::query_execution_not_found("11111111-2222-3333-4444-555555555555"),
        ),
        (
            "a results read of an execution no start minted",
            "GetQueryResults",
            json!({ "QueryExecutionId": "does-not-exist" }),
            ERROR_CODE_QUERY_EXECUTION_NOT_FOUND,
            state::query_execution_not_found("does-not-exist"),
        ),
        (
            "a stop of an execution no start minted",
            "StopQueryExecution",
            json!({ "QueryExecutionId": "does-not-exist" }),
            ERROR_CODE_QUERY_EXECUTION_NOT_FOUND,
            state::query_execution_not_found("does-not-exist"),
        ),
        (
            "a results read of a query that has not finished",
            "GetQueryResults",
            json!({ "QueryExecutionId": unfinished }),
            ERROR_CODE_INVALID_QUERY_EXECUTION_STATE,
            state::query_not_yet_finished(&unfinished_state),
        ),
        (
            "a results read of a query a caller cancelled",
            "GetQueryResults",
            json!({ "QueryExecutionId": cancelled }),
            ERROR_CODE_INVALID_QUERY_EXECUTION_STATE,
            state::query_did_not_succeed(QUERY_STATE_CANCELLED),
        ),
    ];

    for (case, operation, request, code, expected_message) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_refusal(case, &body, status, code, &expected_message);
    }
}

/// The code travels on every refusal this service states, including the ones whose
/// wording the emulator chooses: a bound it enforces, a token it did not mint, a result
/// form it serves for no query.
#[tokio::test]
async fn a_refusal_of_a_rule_this_service_enforces_carries_the_code_too() {
    let router = router();
    const WORK_GROUP: &str = "refusal-rules-wg";
    create_work_group(&router, WORK_GROUP).await;
    let finished = crate::harness::start(&router, &query("SELECT 1 AS one")).await;

    // (the case, the operation, the request) — each a rule this service enforces rather
    // than a resource miss, so what is pinned is the code and that a sentence is stated.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a tag key repeated in one request",
            "CreateWorkGroup",
            json!({
                "Name": "tagged-twice",
                "Tags": [
                    { "Key": "owner", "Value": "one" },
                    { "Key": "owner", "Value": "two" }
                ]
            }),
        ),
        (
            "a data catalog created under a name already taken",
            "CreateDataCatalog",
            json!({
                "Name": "twice_registered",
                "Type": "GLUE",
                "Parameters": { "catalog-id": "111122223333" }
            }),
        ),
        (
            "a page token no listing minted",
            "ListQueryExecutions",
            json!({ "WorkGroup": WORK_GROUP, "NextToken": "not-a-token" }),
        ),
        (
            "a results read asking for a manifest no query wrote",
            "GetQueryResults",
            json!({ "QueryExecutionId": finished, "QueryResultType": "DATA_MANIFEST" }),
        ),
    ];

    // The duplicate row needs the name taken first.
    let (status, created) = call(
        &router,
        "CreateDataCatalog",
        &json!({
            "Name": "twice_registered",
            "Type": "GLUE",
            "Parameters": { "catalog-id": "111122223333" }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the data catalog must be registered first, got {status} with {created:?}"
    );

    for (case, operation, request) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {body:?}"
        );
        assert_eq!(
            body["AthenaErrorCode"].as_str(),
            Some(ERROR_CODE_INVALID_INPUT),
            "case {case:?}: expected AthenaErrorCode {ERROR_CODE_INVALID_INPUT:?}, got {body:?}"
        );
        assert_eq!(
            body["ErrorCode"].as_str(),
            Some(ERROR_CODE_INVALID_INPUT),
            "case {case:?}: expected ErrorCode beside AthenaErrorCode, got {body:?}"
        );
        assert!(
            !message(&body).is_empty() && message(&body) != "<no Message>",
            "case {case:?}: expected the refusal to state why, got {body:?}"
        );
    }
}

/// What a METASTORE answered is reported as `MetadataException`, whose model declares
/// `Message` alone: no code member travels on it, and the sentence is the metastore's own
/// rather than one Athena composed.
#[tokio::test]
async fn a_metastore_refusal_carries_its_message_and_no_code_member() {
    let router = router();
    // A HIVE catalog's metadata is served by a metastore outside this emulator, so a read
    // of it is answered by the metastore layer rather than refused as a bad request.
    let (status, created) = call(
        &router,
        "CreateDataCatalog",
        &json!({
            "Name": "hive_metastore",
            "Type": "HIVE",
            "Parameters": {
                "metadata-function": "arn:aws:lambda:us-east-1:000000000000:function:hms"
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the HIVE catalog must be registered, got {status} with {created:?}"
    );

    for (case, operation, request) in [
        (
            "a single table of a catalog an external metastore serves",
            "GetTableMetadata",
            json!({
                "CatalogName": "hive_metastore",
                "DatabaseName": "some_db",
                "TableName": "some_table"
            }),
        ),
        (
            "the tables of a catalog an external metastore serves",
            "ListTableMetadata",
            json!({ "CatalogName": "hive_metastore", "DatabaseName": "some_db" }),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "MetadataException",
            "case {case:?}: expected MetadataException, got {body:?}"
        );
        assert!(
            body["AthenaErrorCode"].is_null() && body["ErrorCode"].is_null(),
            "case {case:?}: a metastore refusal declares no code member, got {body:?}"
        );
        assert_eq!(
            message(&body),
            state::metastore_unreachable("hive_metastore", "HIVE"),
            "case {case:?}: expected the metastore's own sentence, got {body:?}"
        );
    }
}
