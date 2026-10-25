// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Athena's data-catalog slice: `CreateDataCatalog` registers a
//! catalog and `GetDataCatalog` reads it back, driven over the wire (awsJson1_1,
//! `x-amz-target: AmazonAthena.<Operation>`) through the same router the emulator boots,
//! so every assertion is about what an SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one
//! recorded request: the rows differ along whatever the rule generalises over — the
//! catalog type, which parameters the type requires, the name's length and character
//! set, the size of a tag list, and how much time has passed since the create call.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    self, DATA_CATALOG_STATUS_CREATE_COMPLETE, DATA_CATALOG_STATUS_CREATE_IN_PROGRESS,
    DATA_CATALOG_TYPE_FEDERATED, DATA_CATALOG_TYPE_GLUE, DATA_CATALOG_TYPE_HIVE,
    DATA_CATALOG_TYPE_LAMBDA, FEDERATED_DATA_CATALOG_CREATION, MAX_DATA_CATALOG_NAME_CHARS,
    MAX_FEDERATED_DATA_CATALOG_NAME_CHARS, MAX_TAGS_PER_RESOURCE,
};
use service_com_amazonaws_athena_amazonathena::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `x-amz-target` prefix of this service: the local name of the Smithy service
/// shape (`com.amazonaws.athena#AmazonAthena`), which is how the awsJson protocols
/// build that header.
const TARGET_PREFIX: &str = "AmazonAthena";

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `athena` (this service's `sigv4Name`). Neither the key id
/// nor the signature is a credential: the harness turns request authentication off
/// below, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/athena/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=not-verified";

/// The wire error code a refused request must carry: the client error both operations
/// model (HTTP 400).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// One service mounted on its own engine, reached the way the production router reaches
/// it, plus the engine itself for the one assertion that has no wire reader (a catalog's
/// tags: `ListTagsForResource` is still a scaffold).
fn engine_and_router() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Athena's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// A router over a fresh engine, for a test that needs no direct state access.
fn router() -> MultiServiceRouter {
    engine_and_router().1
}

/// Invoke one operation and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "athena.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code (`__type`) of an error body, with the shape-id prefix the
/// awsJson protocols may carry stripped.
fn error_code(body: &Value) -> String {
    let raw = body["__type"].as_str().unwrap_or("<no __type>");
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

/// The `DataCatalog` member of either operation's output, or `Value::Null`.
fn data_catalog(body: &Value) -> Value {
    body["DataCatalog"].clone()
}

/// `GetDataCatalog`'s view of one catalog: `(status_code, DataCatalog)`.
async fn read_catalog(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    let (status, body) = call(router, "GetDataCatalog", &json!({ "Name": name })).await;
    (status, data_catalog(&body))
}

/// A valid catalog name of `length` characters, all of them inside the documented
/// `CreateDataCatalogInput$Name` set (alphanumeric, underscore, at sign, hyphen).
fn name_of_length(length: usize) -> String {
    "aA0_@-".chars().cycle().take(length).collect()
}

/// One row of the create/read-back table.
struct CreateCase {
    case: &'static str,
    name: String,
    request: Value,
    /// The `Status` the CREATE response must report, which depends only on whether this
    /// catalog type is created synchronously.
    created_status: &'static str,
    /// The `ConnectionType` the catalog must report, if any.
    connection_type: Option<&'static str>,
}

/// The rows: a catalog differs from the next along its type, the parameters that type
/// requires, its name's shape and length, and whether a description is present.
fn create_cases() -> Vec<CreateCase> {
    let shortest = name_of_length(1);
    let longest = name_of_length(MAX_DATA_CATALOG_NAME_CHARS);
    vec![
        CreateCase {
            case: "a GLUE catalog in another account, named with one character",
            name: shortest.clone(),
            request: json!({
                "Name": shortest,
                "Type": DATA_CATALOG_TYPE_GLUE,
                "Parameters": { "catalog-id": "111122223333" }
            }),
            created_status: DATA_CATALOG_STATUS_CREATE_COMPLETE,
            connection_type: None,
        },
        CreateCase {
            case: "a HIVE catalog with a description and an optional sdk-version",
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
            created_status: DATA_CATALOG_STATUS_CREATE_COMPLETE,
            connection_type: None,
        },
        CreateCase {
            case: "a LAMBDA catalog with one composite function",
            name: "lambda-composite".to_string(),
            request: json!({
                "Name": "lambda-composite",
                "Type": DATA_CATALOG_TYPE_LAMBDA,
                "Parameters": { "function": "arn:aws:lambda:us-east-1:111122223333:function:both" }
            }),
            created_status: DATA_CATALOG_STATUS_CREATE_COMPLETE,
            connection_type: None,
        },
        CreateCase {
            case: "a LAMBDA catalog with split metadata and record functions, and the \
                   longest name allowed",
            name: longest.clone(),
            request: json!({
                "Name": longest,
                "Type": DATA_CATALOG_TYPE_LAMBDA,
                "Description": "d",
                "Parameters": {
                    "metadata-function": "arn:aws:lambda:us-east-1:111122223333:function:meta",
                    "record-function": "arn:aws:lambda:us-east-1:111122223333:function:rec"
                },
                "Tags": [{ "Key": "env", "Value": "prod" }]
            }),
            created_status: DATA_CATALOG_STATUS_CREATE_COMPLETE,
            connection_type: None,
        },
        CreateCase {
            case: "a FEDERATED catalog reusing an existing Glue connection",
            name: "federated_reused".to_string(),
            request: json!({
                "Name": "federated_reused",
                "Type": DATA_CATALOG_TYPE_FEDERATED,
                "Parameters": {
                    "connection-arn":
                        "arn:aws:glue:us-east-1:111122223333:connection/snowflake"
                }
            }),
            created_status: DATA_CATALOG_STATUS_CREATE_IN_PROGRESS,
            connection_type: None,
        },
        CreateCase {
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
            created_status: DATA_CATALOG_STATUS_CREATE_IN_PROGRESS,
            // `DataCatalog$ConnectionType` is "the type of connection for a FEDERATED
            // data catalog", which the create call states in `connection-type`.
            connection_type: Some("MYSQL"),
        },
    ]
}

/// A created catalog is the one a later read reports: every member the create call
/// carried comes back from `GetDataCatalog` unchanged, under the name it registered.
#[tokio::test]
async fn a_created_data_catalog_is_read_back_by_get_data_catalog() {
    let router = router();

    for case in create_cases() {
        let (status, created) = call(&router, "CreateDataCatalog", &case.request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateDataCatalog to answer 200, got {status} with {created:?}",
            case.case
        );
        let created = data_catalog(&created);
        assert_eq!(
            created["Status"].as_str(),
            Some(case.created_status),
            "case {:?}: expected the created catalog's Status to be {:?}, got {created:?}",
            case.case,
            case.created_status
        );

        let (status, read) = read_catalog(&router, &case.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetDataCatalog to answer 200, got {status} with {read:?}",
            case.case
        );

        // Name, Type, Description and Parameters are what the request carried; Status is
        // the one member time can move, and has its own test below.
        for member in ["Name", "Type", "Description", "Parameters"] {
            let expected = case.request.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                read[member], expected,
                "case {:?}: expected {member} to read back as {expected:?}, got {:?}",
                case.case, read[member]
            );
            assert_eq!(
                created[member], expected,
                "case {:?}: expected the create response's {member} to be {expected:?}, \
                 got {:?}",
                case.case, created[member]
            );
        }
        assert_eq!(
            read["ConnectionType"].as_str(),
            case.connection_type,
            "case {:?}: expected ConnectionType {:?}, got {:?}",
            case.case,
            case.connection_type,
            read["ConnectionType"]
        );
        // Nothing failed, so the catalog carries no error text.
        assert_eq!(
            read["Error"],
            Value::Null,
            "case {:?}: expected no Error on a catalog that was created, got {:?}",
            case.case,
            read["Error"]
        );
    }
}

/// A synchronously created catalog type is complete as the create call returns; the
/// asynchronous one is in progress until its creation is due, and complete from then on.
#[tokio::test]
async fn a_catalog_is_complete_when_its_creation_is_due() {
    let router = router();

    // The three types `DataCatalog$Status` documents as "created synchronously".
    let synchronous: [(&str, Value); 3] = [
        (
            DATA_CATALOG_TYPE_GLUE,
            json!({ "catalog-id": "111122223333" }),
        ),
        (
            DATA_CATALOG_TYPE_HIVE,
            json!({ "metadata-function": "arn:aws:lambda:us-east-1:111122223333:function:hms" }),
        ),
        (
            DATA_CATALOG_TYPE_LAMBDA,
            json!({ "function": "arn:aws:lambda:us-east-1:111122223333:function:both" }),
        ),
    ];
    for (catalog_type, parameters) in synchronous {
        let name = format!("sync_{catalog_type}");
        let (status, created) = call(
            &router,
            "CreateDataCatalog",
            &json!({ "Name": name, "Type": catalog_type, "Parameters": parameters }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {catalog_type:?}: expected CreateDataCatalog to answer 200, got {status} \
             with {created:?}"
        );
        assert_eq!(
            data_catalog(&created)["Status"].as_str(),
            Some(DATA_CATALOG_STATUS_CREATE_COMPLETE),
            "case {catalog_type:?}: expected a synchronously created catalog to be \
             {DATA_CATALOG_STATUS_CREATE_COMPLETE}, got {created:?}"
        );
        let (_, read) = read_catalog(&router, &name).await;
        assert_eq!(
            read["Status"].as_str(),
            Some(DATA_CATALOG_STATUS_CREATE_COMPLETE),
            "case {catalog_type:?}: expected the read-back status to stay \
             {DATA_CATALOG_STATUS_CREATE_COMPLETE}, got {read:?}"
        );
    }

    // The one type documented as "created asynchronously".
    let name = "federated_in_progress";
    let (status, created) = call(
        &router,
        "CreateDataCatalog",
        &json!({
            "Name": name,
            "Type": DATA_CATALOG_TYPE_FEDERATED,
            "Parameters": { "connection-type": "REDSHIFT" }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected CreateDataCatalog to answer 200, got {status} with \
         {created:?}"
    );
    assert_eq!(
        data_catalog(&created)["Status"].as_str(),
        Some(DATA_CATALOG_STATUS_CREATE_IN_PROGRESS),
        "case {name:?}: expected an asynchronously created catalog to be \
         {DATA_CATALOG_STATUS_CREATE_IN_PROGRESS}, got {created:?}"
    );
    let (_, read) = read_catalog(&router, name).await;
    assert_eq!(
        read["Status"].as_str(),
        Some(DATA_CATALOG_STATUS_CREATE_IN_PROGRESS),
        "case {name:?}: a caller reading straight back must still see \
         {DATA_CATALOG_STATUS_CREATE_IN_PROGRESS}, got {read:?}"
    );

    // Past the instant the completion is due, every later read reports it — and reading
    // is what materialises nothing: the status follows from the clock alone.
    std::thread::sleep(FEDERATED_DATA_CATALOG_CREATION + Duration::from_millis(100));
    for attempt in 0..2 {
        let (_, read) = read_catalog(&router, name).await;
        assert_eq!(
            read["Status"].as_str(),
            Some(DATA_CATALOG_STATUS_CREATE_COMPLETE),
            "case {name:?}: read {attempt} after the creation was due must report \
             {DATA_CATALOG_STATUS_CREATE_COMPLETE}, got {read:?}"
        );
    }
}

/// The parameters each catalog type documents as required are required, and the sets it
/// documents as mutually exclusive are exclusive.
#[tokio::test]
async fn the_parameters_a_catalog_type_requires_are_enforced() {
    let router = router();

    // (case, Type, Parameters, whether the request describes a catalog Athena accepts)
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        ("HIVE without a metadata function", DATA_CATALOG_TYPE_HIVE, json!({}), false),
        (
            "HIVE with a metadata function",
            DATA_CATALOG_TYPE_HIVE,
            json!({ "metadata-function": "arn:aws:lambda:us-east-1:111122223333:function:hms" }),
            true,
        ),
        (
            "LAMBDA with no parameters at all",
            DATA_CATALOG_TYPE_LAMBDA,
            json!({}),
            false,
        ),
        (
            "LAMBDA with only a metadata function",
            DATA_CATALOG_TYPE_LAMBDA,
            json!({ "metadata-function": "arn:aws:lambda:us-east-1:111122223333:function:meta" }),
            false,
        ),
        (
            "LAMBDA with both halves of the split form",
            DATA_CATALOG_TYPE_LAMBDA,
            json!({
                "metadata-function": "arn:aws:lambda:us-east-1:111122223333:function:meta",
                "record-function": "arn:aws:lambda:us-east-1:111122223333:function:rec"
            }),
            true,
        ),
        (
            "LAMBDA with the composite form",
            DATA_CATALOG_TYPE_LAMBDA,
            json!({ "function": "arn:aws:lambda:us-east-1:111122223333:function:both" }),
            true,
        ),
        (
            "LAMBDA with both forms at once, which the model documents as not both",
            DATA_CATALOG_TYPE_LAMBDA,
            json!({
                "function": "arn:aws:lambda:us-east-1:111122223333:function:both",
                "metadata-function": "arn:aws:lambda:us-east-1:111122223333:function:meta"
            }),
            false,
        ),
        ("GLUE without a catalog id", DATA_CATALOG_TYPE_GLUE, json!({}), false),
        (
            "GLUE with a catalog id",
            DATA_CATALOG_TYPE_GLUE,
            json!({ "catalog-id": "111122223333" }),
            true,
        ),
        ("FEDERATED with neither connection parameter", DATA_CATALOG_TYPE_FEDERATED, json!({}), false),
        (
            "FEDERATED reusing a connection",
            DATA_CATALOG_TYPE_FEDERATED,
            json!({ "connection-arn": "arn:aws:glue:us-east-1:111122223333:connection/c" }),
            true,
        ),
        (
            "FEDERATED describing a new connection",
            DATA_CATALOG_TYPE_FEDERATED,
            json!({ "connection-type": "SQLSERVER" }),
            true,
        ),
        (
            "FEDERATED with both connection parameters, which the model documents as \
             not both",
            DATA_CATALOG_TYPE_FEDERATED,
            json!({
                "connection-arn": "arn:aws:glue:us-east-1:111122223333:connection/c",
                "connection-type": "SQLSERVER"
            }),
            false,
        ),
        (
            "FEDERATED naming a connection type outside the modelled ConnectionType enum",
            DATA_CATALOG_TYPE_FEDERATED,
            json!({ "connection-type": "NOT_A_CONNECTION_TYPE" }),
            false,
        ),
    ];

    for (index, (case, catalog_type, parameters, accepted)) in cases.into_iter().enumerate() {
        let name = format!("params_{index}");
        let (status, body) = call(
            &router,
            "CreateDataCatalog",
            &json!({ "Name": name, "Type": catalog_type, "Parameters": parameters }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateDataCatalog to answer 200, got {status} with \
                 {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected CreateDataCatalog to refuse with 400, got {status} \
                 with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_REQUEST,
                "case {case:?}: expected {INVALID_REQUEST}, got {body:?}"
            );
            // A refused create registers nothing.
            let (read_status, read) = read_catalog(&router, &name).await;
            assert_eq!(
                read_status, 400,
                "case {case:?}: a refused create must leave no catalog behind, but \
                 GetDataCatalog answered {read_status} with {read:?}"
            );
        }
    }
}

/// A catalog name is accepted only inside the documented length and character set, which
/// is tighter than the modelled `CatalogNameString` and tighter still for a federated
/// catalog.
#[tokio::test]
async fn a_catalog_name_outside_the_documented_limits_is_refused() {
    let router = router();

    // (case, the name, the Type it is created under, whether Athena accepts it)
    let cases: Vec<(&str, String, &str, bool)> = vec![
        ("one character", name_of_length(1), DATA_CATALOG_TYPE_GLUE, true),
        (
            "every character class the documentation allows",
            "catalog_0@team-eu".to_string(),
            DATA_CATALOG_TYPE_GLUE,
            true,
        ),
        (
            "the longest documented name",
            name_of_length(MAX_DATA_CATALOG_NAME_CHARS),
            DATA_CATALOG_TYPE_GLUE,
            true,
        ),
        (
            "one character past the longest documented name",
            name_of_length(MAX_DATA_CATALOG_NAME_CHARS + 1),
            DATA_CATALOG_TYPE_GLUE,
            false,
        ),
        (
            "a dot, which the documented character set does not hold",
            "catalog.eu".to_string(),
            DATA_CATALOG_TYPE_GLUE,
            false,
        ),
        (
            "a space, which the documented character set does not hold",
            "catalog eu".to_string(),
            DATA_CATALOG_TYPE_GLUE,
            false,
        ),
        (
            "the longest federated name",
            name_of_length(MAX_FEDERATED_DATA_CATALOG_NAME_CHARS),
            DATA_CATALOG_TYPE_FEDERATED,
            true,
        ),
        (
            "one character past the longest federated name",
            name_of_length(MAX_FEDERATED_DATA_CATALOG_NAME_CHARS + 1),
            DATA_CATALOG_TYPE_FEDERATED,
            false,
        ),
    ];

    for (case, name, catalog_type, accepted) in cases {
        let parameters = if catalog_type == DATA_CATALOG_TYPE_FEDERATED {
            json!({ "connection-arn": "arn:aws:glue:us-east-1:111122223333:connection/c" })
        } else {
            json!({ "catalog-id": "111122223333" })
        };
        let (status, body) = call(
            &router,
            "CreateDataCatalog",
            &json!({ "Name": name, "Type": catalog_type, "Parameters": parameters }),
        )
        .await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected CreateDataCatalog to answer {expected} for a \
             {}-character name, got {status} with {body:?}",
            name.chars().count()
        );
        if !accepted {
            assert_eq!(
                error_code(&body),
                INVALID_REQUEST,
                "case {case:?}: expected {INVALID_REQUEST}, got {body:?}"
            );
        }
    }
}

/// A catalog name is unique per account: a second create under a name the registry
/// already holds is refused and changes nothing.
#[tokio::test]
async fn a_name_the_registry_already_holds_is_refused() {
    let router = router();

    // (case, the first create, the second create under the same name)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the same type and parameters twice",
            json!({
                "Name": "twice_glue",
                "Type": DATA_CATALOG_TYPE_GLUE,
                "Parameters": { "catalog-id": "111122223333" }
            }),
            json!({
                "Name": "twice_glue",
                "Type": DATA_CATALOG_TYPE_GLUE,
                "Parameters": { "catalog-id": "111122223333" }
            }),
        ),
        (
            "a different type under a name already taken",
            json!({
                "Name": "twice_mixed",
                "Type": DATA_CATALOG_TYPE_GLUE,
                "Parameters": { "catalog-id": "111122223333" }
            }),
            json!({
                "Name": "twice_mixed",
                "Type": DATA_CATALOG_TYPE_LAMBDA,
                "Parameters": { "function": "arn:aws:lambda:us-east-1:111122223333:function:f" }
            }),
        ),
        (
            "a different description under a name already taken",
            json!({
                "Name": "twice_described",
                "Type": DATA_CATALOG_TYPE_HIVE,
                "Description": "the first one",
                "Parameters": { "metadata-function": "arn:aws:lambda:us-east-1:1:function:a" }
            }),
            json!({
                "Name": "twice_described",
                "Type": DATA_CATALOG_TYPE_HIVE,
                "Description": "the second one",
                "Parameters": { "metadata-function": "arn:aws:lambda:us-east-1:1:function:b" }
            }),
        ),
    ];

    for (case, first, second) in cases {
        let (status, body) = call(&router, "CreateDataCatalog", &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first CreateDataCatalog to answer 200, got \
             {status} with {body:?}"
        );
        let registered = data_catalog(&body);

        let (status, body) = call(&router, "CreateDataCatalog", &second).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the second CreateDataCatalog to refuse with 400, got \
             {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {case:?}: expected {INVALID_REQUEST}, got {body:?}"
        );

        // The refused create overwrote nothing: the registry still holds the first one.
        let name = first["Name"].as_str().expect("the case names its catalog");
        let (_, read) = read_catalog(&router, name).await;
        for member in ["Name", "Type", "Description", "Parameters"] {
            assert_eq!(
                read[member], registered[member],
                "case {case:?}: expected {member} to still be {:?} after the refused \
                 create, got {:?}",
                registered[member], read[member]
            );
        }
    }
}

/// A name the registry does not hold is refused by the reader, whatever else it holds.
#[tokio::test]
async fn an_unregistered_catalog_name_is_refused_by_get_data_catalog() {
    let router = router();

    let (status, body) = call(
        &router,
        "CreateDataCatalog",
        &json!({
            "Name": "registered",
            "Type": DATA_CATALOG_TYPE_GLUE,
            "Parameters": { "catalog-id": "111122223333" }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateDataCatalog to answer 200, got {status} with {body:?}"
    );

    for case in ["unregistered", "REGISTERED", "registered2", name_of_length(1).as_str()] {
        let (status, body) = call(&router, "GetDataCatalog", &json!({ "Name": case })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected GetDataCatalog to refuse an unregistered name with \
             400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {case:?}: expected {INVALID_REQUEST}, got {body:?}"
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(case),
            "case {case:?}: expected the message to name the catalog asked for, got \
             {message:?}"
        );
    }
}

/// One row of the tag table: the case name, the `Tags` member sent (null = absent),
/// and the (key, value) pairs the stored record must hold, in order.
type TagCase = (&'static str, Value, Vec<(&'static str, &'static str)>);

/// The tags a create carried are persisted against the catalog, and a tag list that
/// names nothing taggable is refused.
#[tokio::test]
async fn tags_a_create_carried_are_persisted_against_the_catalog() {
    // `ListTagsForResource` is still a scaffold, so the tag list is asserted against the
    // record the write persisted rather than over the wire.
    let (engine, router) = engine_and_router();

    // (case, the Tags member sent (null = absent), the (key, value) pairs persisted)
    let cases: Vec<TagCase> = vec![
        ("no Tags member at all", Value::Null, Vec::new()),
        ("an empty tag list", json!([]), Vec::new()),
        (
            "one tag",
            json!([{ "Key": "env", "Value": "prod" }]),
            vec![("env", "prod")],
        ),
        (
            "keys differing only in case, which Tag$Key calls case-sensitive",
            json!([{ "Key": "env", "Value": "prod" }, { "Key": "Env", "Value": "dev" }]),
            vec![("env", "prod"), ("Env", "dev")],
        ),
        (
            "a tag whose value is the empty string the model allows",
            json!([{ "Key": "owner", "Value": "" }]),
            vec![("owner", "")],
        ),
    ];

    for (index, (case, tags, expected)) in cases.into_iter().enumerate() {
        let name = format!("tagged_{index}");
        let mut request = json!({
            "Name": name,
            "Type": DATA_CATALOG_TYPE_GLUE,
            "Parameters": { "catalog-id": "111122223333" }
        });
        if !tags.is_null() {
            request["Tags"] = tags;
        }
        let (status, body) = call(&router, "CreateDataCatalog", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDataCatalog to answer 200, got {status} with \
             {body:?}"
        );

        let store = state::service_state(&*engine).expect("this service's state");
        let stored: Vec<(String, String)> = store
            .lock()
            .data_catalogs
            .get(&name)
            .expect("the catalog the create registered")
            .tags
            .iter()
            .map(|tag| {
                (
                    tag.key.clone().unwrap_or_default(),
                    tag.value.clone().unwrap_or_default(),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the stored tags {expected:?}, got {stored:?}"
        );
    }

    // A tag list no resource could carry is refused, and registers nothing.
    let duplicate = json!([
        { "Key": "env", "Value": "prod" },
        { "Key": "env", "Value": "dev" }
    ]);
    let too_many: Vec<Value> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| json!({ "Key": format!("key-{index}"), "Value": "v" }))
        .collect();
    for (case, tags) in [
        ("one key twice, which Tag$Key calls unique per resource", duplicate),
        ("one tag more than a resource can hold", Value::Array(too_many)),
    ] {
        let (status, body) = call(
            &router,
            "CreateDataCatalog",
            &json!({
                "Name": "refused_tags",
                "Type": DATA_CATALOG_TYPE_GLUE,
                "Parameters": { "catalog-id": "111122223333" },
                "Tags": tags
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateDataCatalog to refuse with 400, got {status} \
             with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {case:?}: expected {INVALID_REQUEST}, got {body:?}"
        );
        assert!(
            !state::service_state(&*engine)
                .expect("this service's state")
                .lock()
                .data_catalogs
                .contains_key("refused_tags"),
            "case {case:?}: a refused create must register no catalog"
        );
    }
}
