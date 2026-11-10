// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteDataCatalog`, driven over the wire (awsJson1_1,
//! `x-amz-target: AmazonAthena.<Operation>`) through the same router the emulator boots,
//! so every assertion is about what an SDK client observes.
//!
//! The rule under test: a delete removes the ONE data catalog its `Name` addresses from
//! the account's registry and answers with the record it removed, so the name no longer
//! resolves for any later call, and a `Name` the registry does not hold removes nothing
//! and is refused.
//!
//! Each test asserts that rule over a TABLE of inputs rather than one request: the rows
//! differ along the catalog type, the parameters that type requires, the name's length and
//! character set, whether `DeleteCatalogOnly` is set, and how many callers delete the same
//! catalog at once.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    self, DATA_CATALOG_STATUS_DELETE_COMPLETE, DATA_CATALOG_TYPE_FEDERATED,
    DATA_CATALOG_TYPE_GLUE, DATA_CATALOG_TYPE_HIVE, DATA_CATALOG_TYPE_LAMBDA,
    ERROR_CODE_INVALID_INPUT, MAX_DATA_CATALOG_NAME_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, error_code, message, router};

/// The wire error code a refused request must carry: the client error this operation
/// models (HTTP 400).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The refusal class of an error body, read under BOTH names Athena serialises it as:
/// the modelled `InvalidRequestException$AthenaErrorCode` and the `ErrorCode` alias a
/// client may branch on instead. Returns `(AthenaErrorCode, ErrorCode)` so a test asserts
/// that neither name is left out.
fn refusal_codes(body: &Value) -> (String, String) {
    let under = |member: &str| {
        body[member]
            .as_str()
            .unwrap_or("<absent>")
            .to_string()
    };
    (under("AthenaErrorCode"), under("ErrorCode"))
}

/// The `DataCatalog` member of an operation's output, or `Value::Null`.
fn data_catalog(body: &Value) -> Value {
    body["DataCatalog"].clone()
}

/// A valid catalog name of `length` characters, all of them inside the documented
/// `CreateDataCatalogInput$Name` set (alphanumeric, underscore, at sign, hyphen).
fn name_of_length(length: usize) -> String {
    "aA0_@-".chars().cycle().take(length).collect()
}

/// Create one catalog and return the `DataCatalog` the create answered with.
async fn create(router: &MultiServiceRouter, case: &str, request: &Value) -> Value {
    let (status, body) = call(router, "CreateDataCatalog", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateDataCatalog to answer 200, got {status} with {body:?}"
    );
    data_catalog(&body)
}

/// One row of the delete table: a catalog that differs from the next along its type, the
/// parameters that type requires, and its name's shape.
struct DeleteCase {
    case: &'static str,
    name: String,
    request: Value,
    /// The `ConnectionType` the removed catalog must still report, if any.
    connection_type: Option<&'static str>,
}

/// The rows. Every `DataCatalogType` the model holds is represented, each with a
/// parameter set its type documents, and the names span one character to the longest a
/// catalog name may use.
fn delete_cases() -> Vec<DeleteCase> {
    let shortest = name_of_length(1);
    let longest = name_of_length(MAX_DATA_CATALOG_NAME_CHARS);
    vec![
        DeleteCase {
            case: "a GLUE catalog in another account, named with one character",
            name: shortest.clone(),
            request: json!({
                "Name": shortest,
                "Type": DATA_CATALOG_TYPE_GLUE,
                "Parameters": { "catalog-id": "111122223333" }
            }),
            connection_type: None,
        },
        DeleteCase {
            case: "a HIVE catalog with a description",
            name: "hive_metastore@eu-1".to_string(),
            request: json!({
                "Name": "hive_metastore@eu-1",
                "Type": DATA_CATALOG_TYPE_HIVE,
                "Description": "an external Apache Hive metastore",
                "Parameters": {
                    "metadata-function": "arn:aws:lambda:us-east-1:111122223333:function:hms",
                    "sdk-version": "1.0"
                }
            }),
            connection_type: None,
        },
        DeleteCase {
            case: "a LAMBDA catalog with one composite function and the longest name allowed",
            name: longest.clone(),
            request: json!({
                "Name": longest,
                "Type": DATA_CATALOG_TYPE_LAMBDA,
                "Parameters": { "function": "arn:aws:lambda:us-east-1:111122223333:function:both" },
                "Tags": [{ "Key": "env", "Value": "prod" }]
            }),
            connection_type: None,
        },
        DeleteCase {
            case: "a LAMBDA catalog with split metadata and record functions",
            name: "lambda-split".to_string(),
            request: json!({
                "Name": "lambda-split",
                "Type": DATA_CATALOG_TYPE_LAMBDA,
                "Parameters": {
                    "metadata-function": "arn:aws:lambda:us-east-1:111122223333:function:meta",
                    "record-function": "arn:aws:lambda:us-east-1:111122223333:function:rec"
                }
            }),
            connection_type: None,
        },
        DeleteCase {
            case: "a FEDERATED catalog reusing an existing Glue connection",
            name: "federated_reused".to_string(),
            request: json!({
                "Name": "federated_reused",
                "Type": DATA_CATALOG_TYPE_FEDERATED,
                "Parameters": {
                    "connection-arn": "arn:aws:glue:us-east-1:111122223333:connection/snowflake"
                }
            }),
            connection_type: None,
        },
        DeleteCase {
            case: "a FEDERATED catalog describing a new connection",
            name: "federated_new".to_string(),
            request: json!({
                "Name": "federated_new",
                "Type": DATA_CATALOG_TYPE_FEDERATED,
                "Parameters": {
                    "connection-type": "MYSQL",
                    "connection-properties": "{\"spill_bucket\":\"my_spill\"}"
                }
            }),
            connection_type: Some("MYSQL"),
        },
    ]
}

/// A delete removes the catalog it names and nothing else: the response is the record the
/// registry held, the name no longer reads back, a second delete of it finds nothing, and
/// every other catalog in the registry is untouched.
#[tokio::test]
async fn a_delete_removes_the_named_catalog_and_reports_the_record_it_removed() {
    let router = router();
    let cases = delete_cases();

    // One registry holding every row at once, so each delete below is also a test that
    // the OTHER catalogs survive it.
    let mut created = Vec::new();
    for case in &cases {
        created.push(create(&router, case.case, &case.request).await);
    }

    for (position, case) in cases.iter().enumerate() {
        let (status, body) = call(&router, "DeleteDataCatalog", &json!({ "Name": case.name })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DeleteDataCatalog to answer 200, got {status} with {body:?}",
            case.case
        );
        let removed = data_catalog(&body);

        // The response is the record the registry held, member for member, as the create
        // reported it.
        for member in ["Name", "Type", "Description", "Parameters", "ConnectionType"] {
            assert_eq!(
                removed[member], created[position][member],
                "case {:?}: expected the deleted catalog's {member} to be the registered \
                 {:?}, got {:?}",
                case.case, created[position][member], removed[member]
            );
        }
        assert_eq!(
            removed["ConnectionType"].as_str(),
            case.connection_type,
            "case {:?}: expected ConnectionType {:?}, got {:?}",
            case.case,
            case.connection_type,
            removed["ConnectionType"]
        );
        // `DataCatalog$Status` is "the status of the creation or deletion of the data
        // catalog", and this call's deletion is done when it returns.
        assert_eq!(
            removed["Status"].as_str(),
            Some(DATA_CATALOG_STATUS_DELETE_COMPLETE),
            "case {:?}: expected the deleted catalog's Status to be \
             {DATA_CATALOG_STATUS_DELETE_COMPLETE}, got {removed:?}",
            case.case
        );
        assert_eq!(
            removed["Error"],
            Value::Null,
            "case {:?}: expected no Error on a catalog that was deleted, got {:?}",
            case.case,
            removed["Error"]
        );

        // The name resolves for nothing afterwards: neither a read nor a second delete.
        let (read_status, read_body) =
            call(&router, "GetDataCatalog", &json!({ "Name": case.name })).await;
        assert_eq!(
            read_status, 400,
            "case {:?}: expected GetDataCatalog to refuse a deleted catalog with 400, got \
             {read_status} with {read_body:?}",
            case.case
        );
        assert_eq!(
            message(&read_body),
            state::data_catalog_not_found(&case.name),
            "case {:?}: expected the read of a deleted catalog to answer the catalog-miss \
             wording, got {read_body:?}",
            case.case
        );

        let (again_status, again_body) =
            call(&router, "DeleteDataCatalog", &json!({ "Name": case.name })).await;
        assert_eq!(
            again_status, 400,
            "case {:?}: expected a second DeleteDataCatalog to refuse with 400, got \
             {again_status} with {again_body:?}",
            case.case
        );
        assert_eq!(
            message(&again_body),
            state::data_catalog_not_found_on_delete(&case.name),
            "case {:?}: expected the second delete to answer the delete path's catalog-miss \
             wording, got {again_body:?}",
            case.case
        );

        // Every row not yet deleted is still exactly where it was.
        for later in cases.iter().skip(position + 1) {
            let (status, body) =
                call(&router, "GetDataCatalog", &json!({ "Name": later.name })).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected {:?} to survive the delete of {:?}, got {status} with \
                 {body:?}",
                case.case, later.name, case.name
            );
        }
    }
}

/// A `Name` the registry does not hold deletes nothing and is refused as a bad request,
/// whatever else the registry holds.
#[tokio::test]
async fn a_name_the_registry_does_not_hold_is_refused() {
    let router = router();
    let held = "registered-lambda";
    create(
        &router,
        "the catalog the registry does hold",
        &json!({
            "Name": held,
            "Type": DATA_CATALOG_TYPE_LAMBDA,
            "Parameters": { "function": "arn:aws:lambda:us-east-1:111122223333:function:both" }
        }),
    )
    .await;

    // A name never registered; one differing from a held name only in case (a catalog
    // name is the registry's exact key); one that extends a held name; the shortest and
    // the longest name a catalog may carry.
    let cases = [
        "unregistered".to_string(),
        "REGISTERED-LAMBDA".to_string(),
        format!("{held}2"),
        name_of_length(1),
        name_of_length(MAX_DATA_CATALOG_NAME_CHARS),
    ];
    for case in &cases {
        let (status, body) = call(&router, "DeleteDataCatalog", &json!({ "Name": case })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected DeleteDataCatalog to refuse an unheld name with 400, \
             got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {case:?}: expected {INVALID_REQUEST}, got {body:?}"
        );
        assert_eq!(
            refusal_codes(&body),
            (
                ERROR_CODE_INVALID_INPUT.to_string(),
                ERROR_CODE_INVALID_INPUT.to_string()
            ),
            "case {case:?}: expected the refusal to carry {ERROR_CODE_INVALID_INPUT} under \
             both AthenaErrorCode and ErrorCode, got {body:?}"
        );
        assert_eq!(
            message(&body),
            state::data_catalog_not_found_on_delete(case),
            "case {case:?}: expected the catalog-miss wording naming the requested name, \
             got {body:?}"
        );
    }

    // None of those refusals touched the catalog the registry does hold.
    let (status, body) = call(&router, "GetDataCatalog", &json!({ "Name": held })).await;
    assert_eq!(
        status, 200,
        "expected {held:?} to survive every refused delete, got {status} with {body:?}"
    );
}

/// One row of the `DeleteCatalogOnly` table: the catalog's type, the parameters that type
/// requires, and whether the flag is one that type takes.
struct CatalogOnlyCase {
    catalog_type: &'static str,
    name: &'static str,
    parameters: Value,
}

/// `DeleteCatalogOnly` is documented usable only with a FEDERATED catalog: the flag is
/// accepted for that type and refused for every other, and a refused delete removes
/// nothing.
#[tokio::test]
async fn delete_catalog_only_is_accepted_only_for_a_federated_catalog() {
    let router = router();

    // Every `DataCatalogType` the model holds, so the one type that takes the flag and
    // the three that do not are both covered.
    let cases = [
        CatalogOnlyCase {
            catalog_type: DATA_CATALOG_TYPE_FEDERATED,
            name: "catalog-only-federated",
            parameters: json!({
                "connection-arn": "arn:aws:glue:us-east-1:111122223333:connection/snowflake"
            }),
        },
        CatalogOnlyCase {
            catalog_type: DATA_CATALOG_TYPE_GLUE,
            name: "catalog-only-glue",
            parameters: json!({ "catalog-id": "111122223333" }),
        },
        CatalogOnlyCase {
            catalog_type: DATA_CATALOG_TYPE_HIVE,
            name: "catalog-only-hive",
            parameters: json!({
                "metadata-function": "arn:aws:lambda:us-east-1:111122223333:function:hms"
            }),
        },
        CatalogOnlyCase {
            catalog_type: DATA_CATALOG_TYPE_LAMBDA,
            name: "catalog-only-lambda",
            parameters: json!({
                "function": "arn:aws:lambda:us-east-1:111122223333:function:both"
            }),
        },
    ];

    for case in &cases {
        let federated = case.catalog_type == DATA_CATALOG_TYPE_FEDERATED;
        create(
            &router,
            case.catalog_type,
            &json!({
                "Name": case.name,
                "Type": case.catalog_type,
                "Parameters": case.parameters
            }),
        )
        .await;

        let (status, body) = call(
            &router,
            "DeleteDataCatalog",
            &json!({ "Name": case.name, "DeleteCatalogOnly": true }),
        )
        .await;

        if federated {
            assert_eq!(
                status, 200,
                "case {:?}: expected DeleteCatalogOnly to be accepted, got {status} with \
                 {body:?}",
                case.catalog_type
            );
            let (read_status, read_body) =
                call(&router, "GetDataCatalog", &json!({ "Name": case.name })).await;
            assert_eq!(
                read_status, 400,
                "case {:?}: expected the catalog to be gone after DeleteCatalogOnly, got \
                 {read_status} with {read_body:?}",
                case.catalog_type
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {:?}: expected DeleteCatalogOnly to be refused with 400, got {status} \
             with {body:?}",
            case.catalog_type
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {:?}: expected {INVALID_REQUEST}, got {body:?}",
            case.catalog_type
        );
        assert_eq!(
            message(&body),
            state::delete_catalog_only_unsupported(case.name, case.catalog_type),
            "case {:?}: expected the refusal to name the member, the catalog and its type, \
             got {body:?}",
            case.catalog_type
        );

        // The refused delete removed nothing, and the same catalog goes away once the
        // flag the type does not take is left off.
        let (read_status, read_body) =
            call(&router, "GetDataCatalog", &json!({ "Name": case.name })).await;
        assert_eq!(
            read_status, 200,
            "case {:?}: expected the catalog to survive the refused delete, got \
             {read_status} with {read_body:?}",
            case.catalog_type
        );
        let (status, body) = call(
            &router,
            "DeleteDataCatalog",
            &json!({ "Name": case.name, "DeleteCatalogOnly": false }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the delete without the flag to answer 200, got {status} \
             with {body:?}",
            case.catalog_type
        );
    }
}

/// However many callers delete one catalog at once, exactly one of them removes it: the
/// rest find the registry no longer holding that name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_deletes_of_one_catalog_leave_exactly_one_succeeding() {
    let router = Arc::new(router());

    // The rows differ in how many callers race: two, and two larger counts.
    for callers in [2usize, 4, 9] {
        let name = format!("raced-{callers}");
        create(
            &router,
            &name,
            &json!({
                "Name": name,
                "Type": DATA_CATALOG_TYPE_GLUE,
                "Parameters": { "catalog-id": "111122223333" }
            }),
        )
        .await;

        let mut racers = Vec::with_capacity(callers);
        for _ in 0..callers {
            let router = Arc::clone(&router);
            let request = json!({ "Name": name });
            racers.push(tokio::spawn(async move {
                call(&router, "DeleteDataCatalog", &request).await
            }));
        }

        let mut answers = Vec::with_capacity(callers);
        for racer in racers {
            answers.push(racer.await.expect("the delete task runs to completion"));
        }

        let succeeded: Vec<&(u16, Value)> =
            answers.iter().filter(|(status, _)| *status == 200).collect();
        assert_eq!(
            succeeded.len(),
            1,
            "case {callers} callers: expected exactly one delete to succeed, got \
             {answers:?}"
        );
        assert_eq!(
            data_catalog(&succeeded[0].1)["Name"].as_str(),
            Some(name.as_str()),
            "case {callers} callers: expected the one success to report the catalog it \
             removed, got {:?}",
            succeeded[0].1
        );
        for (status, body) in answers.iter().filter(|(status, _)| *status != 200) {
            assert_eq!(
                *status, 400,
                "case {callers} callers: expected every other delete to be refused with \
                 400, got {status} with {body:?}"
            );
            assert_eq!(
                message(body),
                state::data_catalog_not_found_on_delete(&name),
                "case {callers} callers: expected the losing deletes to answer the \
                 catalog-miss wording, got {body:?}"
            );
        }

        let (status, body) = call(&router, "GetDataCatalog", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {callers} callers: expected the raced catalog to be gone, got {status} \
             with {body:?}"
        );
    }
}
