// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon ML `DataSource` -> `MLModel` lifecycle slice.
//!
//! Every test drives the service the way a client does: an `awsJson1_1` request
//! (`POST /`, `X-Amz-Target: AmazonML_20141212.<Operation>`) through the same
//! `MultiServiceRouter` the production router assembles, so the deserialize ->
//! validate -> handle -> serialize walk, the modelled constraints and the error
//! envelope are all exercised.
//!
//! The suites are tables: each row is one input in the class the rule under test
//! covers, and the recorded/quoted case from the bootstrap issue is only one row.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_machinelearning_amazonml_20141212::state::{
    self, ComAmazonawsMachinelearningAmazonml20141212State, DataSource, MlModel,
};
use smithy_server_runtime::MultiServiceRouter;

/// Comfortably longer than the `PENDING` -> `COMPLETED` span
/// (`state::SETTLE_AFTER`), so a test that waits this long always observes a
/// settled entity.
const LONGER_THAN_THE_SETTLE_SPAN: Duration = Duration::from_millis(1_200);

/// The `MLModelType` values the model declares, as
/// `smithy_query(query="shape", shape="MLModelType")` reports them.
const ML_MODEL_TYPES: [&str; 3] = ["BINARY", "MULTICLASS", "REGRESSION"];

/// An emulator with just this service mounted, over one shared engine.
fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// The same, over an engine the caller keeps a handle to, so a test can seed the store
/// directly instead of driving thousands of requests to reach a cap.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_machinelearning_amazonml_20141212::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call, answering the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        // The router resolves the target service from the SigV4 credential scope,
        // whose fourth field is the signing name.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDTEST/20141212/us-east-1/machinelearning/aws4_request, \
             SignedHeaders=host, Signature=00",
        )
        .header("x-amz-target", format!("AmazonML_20141212.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, body)
}

/// The wire error code of a failure body: the last segment of its `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `CreateDataSourceFromS3` with the fields a row supplies, omitting the rest.
fn create_data_source_request(
    id: &str,
    name: Option<&str>,
    location: &str,
    rearrangement: Option<&str>,
    schema: Option<&str>,
    schema_location: Option<&str>,
    compute_statistics: bool,
) -> Value {
    let mut spec = json!({ "DataLocationS3": location });
    if let Some(rearrangement) = rearrangement {
        spec["DataRearrangement"] = json!(rearrangement);
    }
    if let Some(schema) = schema {
        spec["DataSchema"] = json!(schema);
    }
    if let Some(schema_location) = schema_location {
        spec["DataSchemaLocationS3"] = json!(schema_location);
    }
    let mut request = json!({
        "DataSourceId": id,
        "DataSpec": spec,
        "ComputeStatistics": compute_statistics,
    });
    if let Some(name) = name {
        request["DataSourceName"] = json!(name);
    }
    request
}

/// Create one settled-or-pending `DataSource`, asserting the call succeeded.
async fn seed_data_source(router: &MultiServiceRouter, id: &str, location: &str) {
    let (status, body) = call(
        router,
        "CreateDataSourceFromS3",
        create_data_source_request(id, None, location, None, None, None, true),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding DataSource {id}: expected 200, got {status} with {body:?}"
    );
}

/// Create one `MLModel`, asserting the call succeeded.
async fn seed_model(
    router: &MultiServiceRouter,
    id: &str,
    name: &str,
    model_type: &str,
    training_data_source_id: &str,
) {
    let (status, body) = call(
        router,
        "CreateMLModel",
        json!({
            "MLModelId": id,
            "MLModelName": name,
            "MLModelType": model_type,
            "TrainingDataSourceId": training_data_source_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding MLModel {id}: expected 200, got {status} with {body:?}"
    );
}

/// The `MLModelId`s of a `DescribeMLModels` body, in the order returned.
fn described_ids(body: &Value) -> Vec<String> {
    body["Results"]
        .as_array()
        .map(|results| {
            results
                .iter()
                .map(|model| model["MLModelId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn create_data_source_from_s3_round_trips_every_stored_field() {
    // The rows differ along what the rule generalises over: which optional members
    // are present, the shape of the S3 location (bucket, key, nested prefix), and
    // `ComputeStatistics`.
    struct Case {
        id: &'static str,
        name: Option<&'static str>,
        location: &'static str,
        rearrangement: Option<&'static str>,
        schema: Option<&'static str>,
        schema_location: Option<&'static str>,
        compute_statistics: bool,
    }
    let cases = [
        Case {
            id: "ds-1",
            name: Some("training data"),
            location: "s3://bucket/train.csv",
            rearrangement: None,
            schema: None,
            schema_location: None,
            compute_statistics: true,
        },
        Case {
            id: "bare.bucket_only-99",
            name: None,
            location: "s3://bucket-only",
            rearrangement: None,
            schema: None,
            schema_location: None,
            compute_statistics: false,
        },
        Case {
            id: "ds_with_everything",
            name: Some("every optional member"),
            location: "s3://bucket/nested/prefix/part-0001.csv",
            rearrangement: Some("{\"splitting\":{\"percentBegin\":10,\"percentEnd\":60}}"),
            schema: Some("{\"version\":\"1.0\",\"dataFormat\":\"CSV\"}"),
            schema_location: Some("s3://bucket/schema.json"),
            compute_statistics: true,
        },
        Case {
            id: "ds-schema-by-uri",
            name: Some("schema by uri"),
            location: "s3://bucket/data/",
            rearrangement: None,
            schema: None,
            schema_location: Some("s3://other-bucket/schema.json"),
            compute_statistics: false,
        },
    ];

    let router = emulator();
    let expected_iam_user = engine::emulator_arn("iam", "", "root");

    for case in &cases {
        let (status, body) = call(
            &router,
            "CreateDataSourceFromS3",
            create_data_source_request(
                case.id,
                case.name,
                case.location,
                case.rearrangement,
                case.schema,
                case.schema_location,
                case.compute_statistics,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from CreateDataSourceFromS3, got {status} with {body:?}",
            case.id
        );
        assert_eq!(
            body["DataSourceId"],
            json!(case.id),
            "case {:?}: expected the DataSourceId echoed as created, got {:?}",
            case.id,
            body["DataSourceId"]
        );

        let (status, got) = call(
            &router,
            "GetDataSource",
            json!({ "DataSourceId": case.id, "Verbose": true }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetDataSource, got {status} with {got:?}",
            case.id
        );
        assert_eq!(
            got["DataSourceId"],
            json!(case.id),
            "case {:?}: expected DataSourceId {:?}, got {:?}",
            case.id,
            case.id,
            got["DataSourceId"]
        );
        assert_eq!(
            got["DataLocationS3"],
            json!(case.location),
            "case {:?}: expected DataLocationS3 {:?}, got {:?}",
            case.id,
            case.location,
            got["DataLocationS3"]
        );
        assert_eq!(
            got["Name"],
            case.name.map_or(Value::Null, |name| json!(name)),
            "case {:?}: expected Name {:?}, got {:?}",
            case.id,
            case.name,
            got["Name"]
        );
        assert_eq!(
            got["DataRearrangement"],
            case.rearrangement.map_or(Value::Null, |v| json!(v)),
            "case {:?}: expected DataRearrangement {:?}, got {:?}",
            case.id,
            case.rearrangement,
            got["DataRearrangement"]
        );
        assert_eq!(
            got["ComputeStatistics"],
            json!(case.compute_statistics),
            "case {:?}: expected ComputeStatistics {}, got {:?}",
            case.id,
            case.compute_statistics,
            got["ComputeStatistics"]
        );
        assert_eq!(
            got["CreatedByIamUser"],
            json!(expected_iam_user),
            "case {:?}: expected CreatedByIamUser {expected_iam_user:?}, got {:?}",
            case.id,
            got["CreatedByIamUser"]
        );
        assert!(
            got["CreatedAt"].is_number() && got["LastUpdatedAt"].is_number(),
            "case {:?}: expected numeric EpochTime CreatedAt/LastUpdatedAt, got {:?} / {:?}",
            case.id,
            got["CreatedAt"],
            got["LastUpdatedAt"]
        );
        // `DataSourceSchema` is returned only on a verbose read.
        assert_eq!(
            got["DataSourceSchema"],
            case.schema.map_or(Value::Null, |schema| json!(schema)),
            "case {:?}: expected the verbose read to carry DataSourceSchema {:?}, got {:?}",
            case.id,
            case.schema,
            got["DataSourceSchema"]
        );

        let (status, terse) = call(
            &router,
            "GetDataSource",
            json!({ "DataSourceId": case.id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from a non-verbose GetDataSource, got {status}",
            case.id
        );
        assert_eq!(
            terse["DataSourceSchema"],
            Value::Null,
            "case {:?}: expected no DataSourceSchema without Verbose, got {:?}",
            case.id,
            terse["DataSourceSchema"]
        );
    }
}

#[tokio::test]
async fn a_pending_entity_settles_to_completed_without_any_other_call() {
    let router = emulator();
    seed_data_source(&router, "ds-settle", "s3://bucket/train.csv").await;
    seed_model(&router, "ml-settle", "settling", "BINARY", "ds-settle").await;

    for (operation, key, id) in [
        ("GetDataSource", "DataSourceId", "ds-settle"),
        ("GetMLModel", "MLModelId", "ml-settle"),
    ] {
        let (status, body) = call(&router, operation, json!({ key: id })).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with {body:?}"
        );
        assert_eq!(
            body["Status"],
            json!("PENDING"),
            "case {operation}: a create is asynchronous, so a read straight after it must \
             still see PENDING, got {:?}",
            body["Status"]
        );
        assert_eq!(
            body["StartedAt"],
            Value::Null,
            "case {operation}: StartedAt is unavailable while the entity is PENDING, got {:?}",
            body["StartedAt"]
        );
        assert_eq!(
            body["FinishedAt"],
            Value::Null,
            "case {operation}: expected no FinishedAt while PENDING, got {:?}",
            body["FinishedAt"]
        );
    }

    // Nothing else is called in between: the move to COMPLETED is owed by the clock.
    std::thread::sleep(LONGER_THAN_THE_SETTLE_SPAN);

    for (operation, key, id) in [
        ("GetDataSource", "DataSourceId", "ds-settle"),
        ("GetMLModel", "MLModelId", "ml-settle"),
    ] {
        let (status, body) = call(&router, operation, json!({ key: id })).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with {body:?}"
        );
        assert_eq!(
            body["Status"],
            json!("COMPLETED"),
            "case {operation}: expected COMPLETED once the span elapsed, got {:?}",
            body["Status"]
        );
        assert!(
            body["StartedAt"].is_number() && body["FinishedAt"].is_number(),
            "case {operation}: expected StartedAt/FinishedAt once settled, got {:?} / {:?}",
            body["StartedAt"],
            body["FinishedAt"]
        );
        let compute_time = body["ComputeTime"].as_i64();
        assert!(
            compute_time.is_some_and(|millis| millis > 0),
            "case {operation}: expected a positive ComputeTime once settled, got {:?}",
            body["ComputeTime"]
        );
    }
}

#[tokio::test]
async fn an_unknown_id_is_resource_not_found() {
    let router = emulator();
    seed_data_source(&router, "ds-known", "s3://bucket/train.csv").await;
    seed_model(&router, "ml-known", "known", "BINARY", "ds-known").await;

    // One row per operation that reads or deletes by id, each with an id that names
    // no record and one that names a record of the OTHER kind.
    let cases = [
        ("GetDataSource", "DataSourceId", "ds-missing"),
        ("GetDataSource", "DataSourceId", "ml-known"),
        ("GetMLModel", "MLModelId", "ml-missing"),
        ("GetMLModel", "MLModelId", "ds-known"),
        ("DeleteMLModel", "MLModelId", "ml-missing"),
        ("DeleteMLModel", "MLModelId", "ds-known"),
    ];

    for (operation, key, id) in cases {
        let (status, body) = call(&router, operation, json!({ key: id })).await;
        assert_eq!(
            status, 404,
            "case {operation}({id}): expected 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {operation}({id}): expected ResourceNotFoundException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn repeating_a_create_is_idempotent_only_when_the_parameters_match() {
    let router = emulator();
    let original = create_data_source_request(
        "ds-idem",
        Some("training data"),
        "s3://bucket/train.csv",
        Some("{\"splitting\":{\"percentBegin\":10,\"percentEnd\":60}}"),
        Some("{\"version\":\"1.0\"}"),
        None,
        true,
    );
    let (status, _) = call(&router, "CreateDataSourceFromS3", original.clone()).await;
    assert_eq!(status, 200, "the first create must succeed, got {status}");

    // An identical retry is the same call.
    let (status, body) = call(&router, "CreateDataSourceFromS3", original.clone()).await;
    assert_eq!(
        status, 200,
        "case identical-retry: expected 200, got {status} with {body:?}"
    );
    assert_eq!(
        body["DataSourceId"],
        json!("ds-idem"),
        "case identical-retry: expected the same DataSourceId, got {:?}",
        body["DataSourceId"]
    );

    // One row per member of the create: changing any single one makes it a different
    // request for an id that is already taken.
    let mismatches = [
        (
            "DataSourceName",
            create_data_source_request(
                "ds-idem",
                Some("a different name"),
                "s3://bucket/train.csv",
                Some("{\"splitting\":{\"percentBegin\":10,\"percentEnd\":60}}"),
                Some("{\"version\":\"1.0\"}"),
                None,
                true,
            ),
        ),
        (
            "DataLocationS3",
            create_data_source_request(
                "ds-idem",
                Some("training data"),
                "s3://bucket/other.csv",
                Some("{\"splitting\":{\"percentBegin\":10,\"percentEnd\":60}}"),
                Some("{\"version\":\"1.0\"}"),
                None,
                true,
            ),
        ),
        (
            "DataRearrangement",
            create_data_source_request(
                "ds-idem",
                Some("training data"),
                "s3://bucket/train.csv",
                None,
                Some("{\"version\":\"1.0\"}"),
                None,
                true,
            ),
        ),
        (
            "DataSchema",
            create_data_source_request(
                "ds-idem",
                Some("training data"),
                "s3://bucket/train.csv",
                Some("{\"splitting\":{\"percentBegin\":10,\"percentEnd\":60}}"),
                Some("{\"version\":\"2.0\"}"),
                None,
                true,
            ),
        ),
        (
            "DataSchemaLocationS3",
            create_data_source_request(
                "ds-idem",
                Some("training data"),
                "s3://bucket/train.csv",
                Some("{\"splitting\":{\"percentBegin\":10,\"percentEnd\":60}}"),
                Some("{\"version\":\"1.0\"}"),
                Some("s3://bucket/schema.json"),
                true,
            ),
        ),
        (
            "ComputeStatistics",
            create_data_source_request(
                "ds-idem",
                Some("training data"),
                "s3://bucket/train.csv",
                Some("{\"splitting\":{\"percentBegin\":10,\"percentEnd\":60}}"),
                Some("{\"version\":\"1.0\"}"),
                None,
                false,
            ),
        ),
    ];

    for (member, request) in mismatches {
        let (status, body) = call(&router, "CreateDataSourceFromS3", request).await;
        assert_eq!(
            status, 400,
            "case changed-{member}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatchException",
            "case changed-{member}: expected IdempotentParameterMismatchException, got {body:?}"
        );
    }

    // The stored record is still the one the first call created.
    let (_, got) = call(
        &router,
        "GetDataSource",
        json!({ "DataSourceId": "ds-idem", "Verbose": true }),
    )
    .await;
    assert_eq!(
        got["Name"],
        json!("training data"),
        "a rejected retry must not change the stored record, got {:?}",
        got["Name"]
    );

    // The same rule holds for the model create.
    seed_model(&router, "ml-idem", "demo", "BINARY", "ds-idem").await;
    let model_mismatches = [
        ("MLModelName", json!("other")),
        ("MLModelType", json!("REGRESSION")),
        ("TrainingDataSourceId", json!("ds-idem-2")),
        ("Recipe", json!("{\"groups\":{}}")),
    ];
    seed_data_source(&router, "ds-idem-2", "s3://bucket/second.csv").await;
    for (member, value) in model_mismatches {
        let mut request = json!({
            "MLModelId": "ml-idem",
            "MLModelName": "demo",
            "MLModelType": "BINARY",
            "TrainingDataSourceId": "ds-idem",
        });
        request[member] = value.clone();
        let (status, body) = call(&router, "CreateMLModel", request).await;
        assert_eq!(
            status, 400,
            "case model-changed-{member}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatchException",
            "case model-changed-{member}: expected IdempotentParameterMismatchException, \
             got {body:?}"
        );
    }
}

#[tokio::test]
async fn create_ml_model_refuses_a_training_data_source_it_cannot_train_on() {
    let router = emulator();
    seed_data_source(&router, "ds-ok", "s3://bucket/train.csv").await;
    seed_model(&router, "ml-to-delete", "doomed", "BINARY", "ds-ok").await;
    let (status, _) = call(
        &router,
        "DeleteMLModel",
        json!({ "MLModelId": "ml-to-delete" }),
    )
    .await;
    assert_eq!(status, 200, "the delete must succeed, got {status}");

    // Rows: an id no DataSource answers to, an id that names an MLModel rather than a
    // DataSource, and a request naming both Recipe and RecipeUri.
    let cases: [(&str, Value); 3] = [
        (
            "unknown-training-data-source",
            json!({
                "MLModelId": "ml-bad-1",
                "MLModelType": "BINARY",
                "TrainingDataSourceId": "ds-missing",
            }),
        ),
        (
            "training-data-source-id-naming-a-model",
            json!({
                "MLModelId": "ml-bad-2",
                "MLModelType": "BINARY",
                "TrainingDataSourceId": "ml-to-delete",
            }),
        ),
        (
            "both-recipe-and-recipe-uri",
            json!({
                "MLModelId": "ml-bad-3",
                "MLModelType": "BINARY",
                "TrainingDataSourceId": "ds-ok",
                "Recipe": "{\"groups\":{}}",
                "RecipeUri": "s3://bucket/recipe.json",
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = call(&router, "CreateMLModel", request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {body:?}"
        );
    }

    // And nothing was stored for any of them.
    for id in ["ml-bad-1", "ml-bad-2", "ml-bad-3"] {
        let (status, _) = call(&router, "GetMLModel", json!({ "MLModelId": id })).await;
        assert_eq!(
            status, 404,
            "case {id}: a refused create must store nothing, got {status}"
        );
    }
}

#[tokio::test]
async fn get_ml_model_reports_the_model_as_created() {
    let router = emulator();
    seed_data_source(&router, "ds-train", "s3://bucket/train.csv").await;

    // One row per modelled `MLModelType`, differing also in whether a name, training
    // parameters and a recipe were supplied.
    for (index, model_type) in ML_MODEL_TYPES.iter().enumerate() {
        let id = format!("ml-{model_type}");
        let name = format!("model {index}");
        let with_extras = index % 2 == 0;
        let mut request = json!({
            "MLModelId": id,
            "MLModelName": name,
            "MLModelType": model_type,
            "TrainingDataSourceId": "ds-train",
        });
        if with_extras {
            request["Parameters"] = json!({ "sgd.maxPasses": "10", "sgd.l2RegularizationAmount": "1e-6" });
            request["Recipe"] = json!("{\"groups\":{},\"assignments\":{}}");
        }

        let (status, body) = call(&router, "CreateMLModel", request).await;
        assert_eq!(
            status, 200,
            "case {model_type}: expected 200 from CreateMLModel, got {status} with {body:?}"
        );
        assert_eq!(
            body["MLModelId"],
            json!(id),
            "case {model_type}: expected the MLModelId echoed as created, got {:?}",
            body["MLModelId"]
        );

        let (status, got) = call(
            &router,
            "GetMLModel",
            json!({ "MLModelId": id, "Verbose": true }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {model_type}: expected 200 from GetMLModel, got {status} with {got:?}"
        );
        assert_eq!(
            got["MLModelType"],
            json!(model_type),
            "case {model_type}: expected MLModelType {model_type}, got {:?}",
            got["MLModelType"]
        );
        assert_eq!(
            got["Name"],
            json!(name),
            "case {model_type}: expected Name {name:?}, got {:?}",
            got["Name"]
        );
        assert_eq!(
            got["TrainingDataSourceId"],
            json!("ds-train"),
            "case {model_type}: expected TrainingDataSourceId ds-train, got {:?}",
            got["TrainingDataSourceId"]
        );
        assert_eq!(
            got["InputDataLocationS3"],
            json!("s3://bucket/train.csv"),
            "case {model_type}: InputDataLocationS3 must resolve from the training \
             DataSource, got {:?}",
            got["InputDataLocationS3"]
        );
        assert_eq!(
            got["EndpointInfo"]["EndpointStatus"],
            json!("NONE"),
            "case {model_type}: a model with no realtime endpoint reports NONE, got {:?}",
            got["EndpointInfo"]
        );
        // Only a binary classification model has a scoring threshold.
        if *model_type == "BINARY" {
            assert_eq!(
                got["ScoreThreshold"].as_f64(),
                Some(0.5),
                "case {model_type}: expected a scoring threshold, got {:?}",
                got["ScoreThreshold"]
            );
        } else {
            assert_eq!(
                got["ScoreThreshold"],
                Value::Null,
                "case {model_type}: only a binary model has a ScoreThreshold, got {:?}",
                got["ScoreThreshold"]
            );
        }

        let expected_parameters = if with_extras {
            json!({ "sgd.maxPasses": "10", "sgd.l2RegularizationAmount": "1e-6" })
        } else {
            json!({})
        };
        assert_eq!(
            got["TrainingParameters"],
            expected_parameters,
            "case {model_type}: expected TrainingParameters {expected_parameters:?}, got {:?}",
            got["TrainingParameters"]
        );
        let expected_recipe = if with_extras {
            json!("{\"groups\":{},\"assignments\":{}}")
        } else {
            Value::Null
        };
        assert_eq!(
            got["Recipe"],
            expected_recipe,
            "case {model_type}: expected the verbose read to carry Recipe \
             {expected_recipe:?}, got {:?}",
            got["Recipe"]
        );

        let (_, terse) = call(&router, "GetMLModel", json!({ "MLModelId": id })).await;
        assert_eq!(
            terse["Recipe"],
            Value::Null,
            "case {model_type}: expected no Recipe without Verbose, got {:?}",
            terse["Recipe"]
        );
        assert_eq!(
            terse["Schema"],
            Value::Null,
            "case {model_type}: expected no Schema without Verbose, got {:?}",
            terse["Schema"]
        );
    }
}

#[tokio::test]
async fn describe_ml_models_pages_in_order_and_resumes_where_it_stopped() {
    let router = emulator();
    seed_data_source(&router, "ds-page", "s3://bucket/train.csv").await;
    let created: Vec<String> = (0..7).map(|n| format!("ml-page-{n}")).collect();
    for (n, id) in created.iter().enumerate() {
        seed_model(&router, id, &format!("page {n}"), "BINARY", "ds-page").await;
    }

    // Rows: the page size, spanning 1, a divisor, a non-divisor, the whole set and
    // the published upper bound of `Limit`.
    for limit in [1_usize, 2, 3, 7, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "Limit": limit });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "DescribeMLModels", request).await;
            assert_eq!(
                status, 200,
                "case limit={limit}: expected 200, got {status} with {body:?}"
            );
            let ids = described_ids(&body);
            assert!(
                ids.len() <= limit,
                "case limit={limit}: a page must hold at most Limit results, got {}",
                ids.len()
            );
            seen.extend(ids);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case limit={limit}: paging did not terminate, saw {seen:?}"
            );
            match body["NextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, created,
            "case limit={limit}: expected every model exactly once in creation order, got {seen:?}"
        );
    }

    // Descending order is the same set, reversed.
    let (status, body) = call(
        &router,
        "DescribeMLModels",
        json!({ "SortOrder": "dsc", "Limit": 3 }),
    )
    .await;
    assert_eq!(status, 200, "case dsc: expected 200, got {status}");
    let mut expected_descending: Vec<String> = created.clone();
    expected_descending.reverse();
    assert_eq!(
        described_ids(&body),
        expected_descending[..3].to_vec(),
        "case dsc: expected the newest three first, got {:?}",
        described_ids(&body)
    );
    let token = body["NextToken"]
        .as_str()
        .expect("case dsc: a first page of 3 of 7 must carry a NextToken")
        .to_string();
    let (status, body) = call(
        &router,
        "DescribeMLModels",
        json!({ "SortOrder": "dsc", "Limit": 3, "NextToken": token }),
    )
    .await;
    assert_eq!(status, 200, "case dsc-page-2: expected 200, got {status}");
    assert_eq!(
        described_ids(&body),
        expected_descending[3..6].to_vec(),
        "case dsc-page-2: expected the next three, got {:?}",
        described_ids(&body)
    );

    // `Limit` carries a modelled range of 1..100, so a value past either bound is
    // refused before the handler runs.
    for limit in [0, 101] {
        let (status, body) = call(&router, "DescribeMLModels", json!({ "Limit": limit })).await;
        assert_eq!(
            status, 400,
            "case limit={limit}: a Limit outside 1..100 must be refused, got {status} \
             with {body:?}"
        );
    }

    // A token this service never issued is a client error, not a panic or a reset.
    let over_long = "6d".repeat(20_000);
    for token in ["not-hex", "00", "6e6f742d612d746f6b656e", over_long.as_str()] {
        let (status, body) = call(
            &router,
            "DescribeMLModels",
            json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case token of {} chars: expected 400, got {status} with {body:?}",
            token.len()
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case token of {} chars: expected InvalidInputException, got {body:?}",
            token.len()
        );
    }
}

#[tokio::test]
async fn describe_ml_models_filters_on_the_named_filter_variable() {
    let router = emulator();
    seed_data_source(&router, "ds-alpha", "s3://alpha-bucket/train.csv").await;
    seed_data_source(&router, "ds-beta", "s3://beta-bucket/train.csv").await;
    seed_model(&router, "ml-a", "alpha model", "BINARY", "ds-alpha").await;
    seed_model(&router, "ml-b", "beta model", "MULTICLASS", "ds-beta").await;
    seed_model(&router, "ml-c", "beta runner", "REGRESSION", "ds-beta").await;

    // Every model has settled, so a Status filter has one deterministic answer.
    std::thread::sleep(LONGER_THAN_THE_SETTLE_SPAN);

    // One row per `MLModelFilterVariable` the model declares, across the comparators.
    let cases: [(&str, &str, &str, Vec<&str>); 12] = [
        ("Name", "EQ", "beta model", vec!["ml-b"]),
        ("Name", "Prefix", "beta", vec!["ml-b", "ml-c"]),
        ("Name", "NE", "alpha model", vec!["ml-b", "ml-c"]),
        ("Name", "LT", "beta model", vec!["ml-a"]),
        ("Name", "GE", "beta model", vec!["ml-b", "ml-c"]),
        ("Status", "EQ", "COMPLETED", vec!["ml-a", "ml-b", "ml-c"]),
        ("Status", "EQ", "PENDING", vec![]),
        ("TrainingDataSourceId", "EQ", "ds-beta", vec!["ml-b", "ml-c"]),
        ("MLModelType", "EQ", "REGRESSION", vec!["ml-c"]),
        ("Algorithm", "EQ", "sgd", vec!["ml-a", "ml-b", "ml-c"]),
        ("TrainingDataURI", "Prefix", "s3://alpha", vec!["ml-a"]),
        ("RealtimeEndpointStatus", "EQ", "NONE", vec!["ml-a", "ml-b", "ml-c"]),
    ];

    for (variable, comparator, value, expected) in cases {
        let mut request = json!({ "FilterVariable": variable, "SortOrder": "asc" });
        request[comparator] = json!(value);
        let (status, body) = call(&router, "DescribeMLModels", request).await;
        assert_eq!(
            status, 200,
            "case {variable} {comparator} {value}: expected 200, got {status} with {body:?}"
        );
        let mut got = described_ids(&body);
        got.sort();
        let mut expected: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
        expected.sort();
        assert_eq!(
            got, expected,
            "case {variable} {comparator} {value}: expected {expected:?}, got {got:?}"
        );
    }

    // A comparator with nothing to compare against is a client error.
    for comparator in ["EQ", "GT", "LT", "GE", "LE", "NE", "Prefix"] {
        let mut request = json!({});
        request[comparator] = json!("anything");
        let (status, body) = call(&router, "DescribeMLModels", request).await;
        assert_eq!(
            status, 400,
            "case bare-{comparator}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case bare-{comparator}: expected InvalidInputException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn delete_ml_model_soft_deletes_the_record() {
    let router = emulator();
    seed_data_source(&router, "ds-del", "s3://bucket/train.csv").await;
    for (id, name) in [("ml-keep", "kept"), ("ml-gone", "deleted")] {
        seed_model(&router, id, name, "BINARY", "ds-del").await;
    }

    let (status, body) = call(&router, "DeleteMLModel", json!({ "MLModelId": "ml-gone" })).await;
    assert_eq!(
        status, 200,
        "expected 200 from DeleteMLModel, got {status} with {body:?}"
    );
    assert_eq!(
        body["MLModelId"],
        json!("ml-gone"),
        "expected the deleted MLModelId echoed, got {:?}",
        body["MLModelId"]
    );

    // The record stays readable and reports DELETED.
    let (status, got) = call(&router, "GetMLModel", json!({ "MLModelId": "ml-gone" })).await;
    assert_eq!(
        status, 200,
        "case read-after-delete: a soft-deleted model stays readable, got {status} with {got:?}"
    );
    assert_eq!(
        got["Status"],
        json!("DELETED"),
        "case read-after-delete: expected Status DELETED, got {:?}",
        got["Status"]
    );

    // Deleting again is the same answer: the status is already DELETED.
    let (status, _) = call(&router, "DeleteMLModel", json!({ "MLModelId": "ml-gone" })).await;
    assert_eq!(
        status, 200,
        "case delete-twice: expected 200 for an already deleted model, got {status}"
    );

    // It drops out of the DEFAULT, unfiltered list.
    let (_, body) = call(&router, "DescribeMLModels", json!({})).await;
    assert_eq!(
        described_ids(&body),
        vec!["ml-keep".to_string()],
        "case default-list: a deleted model must not be listed, got {:?}",
        described_ids(&body)
    );

    // A request that names a FilterVariable is asking a specific question, and gets
    // the full answer — whichever variable it names.
    let filtered: [(&str, &str, &str, Vec<&str>); 3] = [
        ("Status", "EQ", "DELETED", vec!["ml-gone"]),
        ("Name", "EQ", "deleted", vec!["ml-gone"]),
        ("MLModelType", "EQ", "BINARY", vec!["ml-gone", "ml-keep"]),
    ];
    for (variable, comparator, value, expected) in filtered {
        let mut request = json!({ "FilterVariable": variable });
        request[comparator] = json!(value);
        let (_, body) = call(&router, "DescribeMLModels", request).await;
        let mut got = described_ids(&body);
        got.sort();
        let mut expected: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
        expected.sort();
        assert_eq!(
            got, expected,
            "case {variable} {comparator} {value}: expected {expected:?}, got {got:?}"
        );
    }

    // And a model whose training DataSource is fine still trains; a deleted MODEL is
    // not a DataSource, so it cannot be trained on.
    let (status, body) = call(
        &router,
        "CreateMLModel",
        json!({
            "MLModelId": "ml-after-delete",
            "MLModelType": "BINARY",
            "TrainingDataSourceId": "ds-del",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case train-after-delete: expected 200, got {status} with {body:?}"
    );
}

/// A settled `DataSource` record, for seeding the store directly.
fn seeded_data_source(id: &str) -> DataSource {
    let now = SystemTime::now();
    DataSource {
        id: id.to_string(),
        name: None,
        data_location_s3: "s3://seed-bucket/train.csv".to_string(),
        data_rearrangement: None,
        data_schema: None,
        data_schema_location_s3: None,
        compute_statistics: false,
        role_arn: None,
        status: state::entity_status::COMPLETED.to_string(),
        created_at: now,
        last_updated_at: now,
        started_at: now,
        finished_at: Some(now),
        compute_time_ms: Some(0),
        data_size_in_bytes: None,
        number_of_files: None,
        message: None,
        created_by_iam_user: engine::emulator_arn("iam", "", "root"),
        settles_at: now,
        request_fingerprint: format!("seed-{id}"),
    }
}

/// A settled `MLModel` record, for seeding the store directly.
fn seeded_model(id: &str, training_data_source_id: &str) -> MlModel {
    let now = SystemTime::now();
    MlModel {
        id: id.to_string(),
        name: None,
        model_type: "BINARY".to_string(),
        training_data_source_id: training_data_source_id.to_string(),
        training_parameters: std::collections::BTreeMap::new(),
        recipe: None,
        recipe_uri: None,
        schema: None,
        score_threshold: Some(0.5),
        score_threshold_last_updated_at: None,
        status: state::entity_status::COMPLETED.to_string(),
        size_in_bytes: None,
        created_at: now,
        last_updated_at: now,
        started_at: now,
        finished_at: Some(now),
        compute_time_ms: Some(0),
        message: None,
        endpoint_info: None,
        created_by_iam_user: engine::emulator_arn("iam", "", "root"),
        settles_at: now,
        request_fingerprint: format!("seed-{id}"),
    }
}

#[tokio::test]
async fn neither_store_grows_past_its_documented_cap() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // Seed both stores to one short of their caps directly, so the boundary is
    // exercised without issuing thousands of requests.
    {
        let handle = engine.state::<ComAmazonawsMachinelearningAmazonml20141212State>();
        let mut store = handle.lock();
        for n in 0..(state::MAX_DATA_SOURCES - 1) {
            let id = format!("seed-ds-{n}");
            store.data_sources.insert(id.clone(), seeded_data_source(&id));
        }
        for n in 0..(state::MAX_ML_MODELS - 1) {
            let id = format!("seed-ml-{n}");
            store
                .ml_models
                .insert(id.clone(), seeded_model(&id, "seed-ds-0"));
        }
    }

    // One row per store: the create that lands exactly on the cap succeeds, and the
    // next one is refused rather than inserted.
    let at_cap = create_data_source_request(
        "ds-at-cap",
        None,
        "s3://bucket/train.csv",
        None,
        None,
        None,
        false,
    );
    let (status, body) = call(&router, "CreateDataSourceFromS3", at_cap).await;
    assert_eq!(
        status, 200,
        "case datasource-at-cap: the last create under the cap must succeed, got \
         {status} with {body:?}"
    );

    let past_cap = create_data_source_request(
        "ds-past-cap",
        None,
        "s3://bucket/train.csv",
        None,
        None,
        None,
        false,
    );
    let (status, body) = call(&router, "CreateDataSourceFromS3", past_cap).await;
    assert_eq!(
        status, 500,
        "case datasource-past-cap: expected a refusal once the store is full, got \
         {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InternalServerException",
        "case datasource-past-cap: expected InternalServerException, got {body:?}"
    );
    let (status, _) = call(
        &router,
        "GetDataSource",
        json!({ "DataSourceId": "ds-past-cap" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case datasource-past-cap: a refused create must store nothing, got {status}"
    );

    let (status, body) = call(
        &router,
        "CreateMLModel",
        json!({
            "MLModelId": "ml-at-cap",
            "MLModelType": "BINARY",
            "TrainingDataSourceId": "seed-ds-0",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case model-at-cap: the last create under the cap must succeed, got {status} \
         with {body:?}"
    );

    let (status, body) = call(
        &router,
        "CreateMLModel",
        json!({
            "MLModelId": "ml-past-cap",
            "MLModelType": "BINARY",
            "TrainingDataSourceId": "seed-ds-0",
        }),
    )
    .await;
    assert_eq!(
        status, 500,
        "case model-past-cap: expected a refusal once the store is full, got {status} \
         with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InternalServerException",
        "case model-past-cap: expected InternalServerException, got {body:?}"
    );
    let (status, _) = call(&router, "GetMLModel", json!({ "MLModelId": "ml-past-cap" })).await;
    assert_eq!(
        status, 404,
        "case model-past-cap: a refused create must store nothing, got {status}"
    );

    // A repeat of an id that already exists is still a no-op at the cap, not a refusal.
    let (status, body) = call(
        &router,
        "CreateMLModel",
        json!({
            "MLModelId": "ml-at-cap",
            "MLModelType": "BINARY",
            "TrainingDataSourceId": "seed-ds-0",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case retry-at-cap: an identical retry must stay idempotent at the cap, got \
         {status} with {body:?}"
    );
}
