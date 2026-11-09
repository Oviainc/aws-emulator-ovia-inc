// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the dataset surface: `CreateDataset`, `GetDataset`,
//! `ListDatasets`, `CreateDatasetVersion` and `ListDatasetVersions`.
//!
//! The rule under test: an inline batch of examples is ONE commit, so every example in it
//! is checked against the dataset's schema before anything is persisted — one example that
//! omits a field the schema requires refuses the whole call with HTTP 400
//! `ValidationException`, naming the zero-based position of that example in the submitted
//! batch and the field it omits, and no dataset is created, not even one holding the
//! examples that would have passed.
//!
//! The second rule: a dataset's working copy round-trips — what a create ingested is what
//! a read reports, a published version is a numbered snapshot of it, and both are listed.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request in this suite signs for.
const REGION: &str = "us-east-2";

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// `DatasetSchemaType`'s values, each identifying "both the source format and the version
/// of that format's schema" (`smithy_query(query="shape", shape="DatasetSchemaType")`).
const PREDEFINED: &str = "AGENTCORE_EVALUATION_PREDEFINED_V1";
const SIMULATED: &str = "AGENTCORE_EVALUATION_SIMULATED_V1";
const THIRD_PARTY: &str = "THIRD_PARTY_EVALUATION_V1";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-agentcore-control.amazonaws.com")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        );
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn create_dataset(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "POST", "/datasets", Some(body)).await
}

async fn get_dataset(
    router: &MultiServiceRouter,
    dataset_id: &str,
    version: Option<&str>,
) -> (u16, Value) {
    let uri = match version {
        Some(version) => format!("/datasets/{dataset_id}?datasetVersion={version}"),
        None => format!("/datasets/{dataset_id}"),
    };
    send(router, "GET", &uri, None).await
}

async fn list_datasets(router: &MultiServiceRouter) -> (u16, Value) {
    send(router, "GET", "/datasets", None).await
}

async fn create_version(router: &MultiServiceRouter, dataset_id: &str) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/datasets/{dataset_id}/versions"),
        Some(&json!({})),
    )
    .await
}

async fn list_versions(router: &MultiServiceRouter, dataset_id: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!("/datasets/{dataset_id}/versions"),
        None,
    )
    .await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// An example of an AgentCore evaluation dataset: a scenario with an input turn.
fn scenario(id: &str) -> Value {
    json!({"scenario_id": id, "input": format!("ask about {id}")})
}

/// A create body over an inline batch.
fn inline_create(name: &str, schema_type: &str, examples: Vec<Value>) -> Value {
    json!({
        "datasetName": name,
        "schemaType": schema_type,
        "source": {"inlineExamples": {"examples": examples}},
    })
}

/// One row of the schema-violation table.
struct ViolationRow {
    case: &'static str,
    dataset_name: &'static str,
    schema_type: &'static str,
    examples: Vec<Value>,
    /// The zero-based position of the offending example in the submitted batch.
    index: usize,
    /// The schema field that example omits.
    field: &'static str,
}

/// A batch with one example that omits a field its schema requires refuses the whole
/// call, names that example's position and field, and creates nothing.
///
/// The rows differ along the schema type (so along which field the schema requires), the
/// position of the offending example in the batch (first, last, middle), the batch's size
/// and the shape of the offending example — including one that is not a JSON object at
/// all, which carries no field of the schema's structure.
#[tokio::test]
async fn a_batch_with_a_schema_violating_example_is_refused_wholesale() {
    let router = router();

    let rows: [ViolationRow; 6] = [
        ViolationRow {
            case: "the predefined schema, the second of two examples omits scenario_id",
            dataset_name: "PredefinedSecond",
            schema_type: PREDEFINED,
            examples: vec![scenario("s1"), json!({"input": "no scenario at all"})],
            index: 1,
            field: "scenario_id",
        },
        ViolationRow {
            case: "the predefined schema, the only example omits scenario_id",
            dataset_name: "PredefinedOnly",
            schema_type: PREDEFINED,
            examples: vec![json!({"input": "no scenario at all"})],
            index: 0,
            field: "scenario_id",
        },
        ViolationRow {
            case: "the simulated schema, the third of four examples omits scenario_id",
            dataset_name: "SimulatedThird",
            schema_type: SIMULATED,
            examples: vec![
                scenario("s1"),
                scenario("s2"),
                json!({"persona": "a shopper"}),
                scenario("s4"),
            ],
            index: 2,
            field: "scenario_id",
        },
        ViolationRow {
            case: "the third-party schema, the second of three examples omits input",
            dataset_name: "ThirdPartySecond",
            schema_type: THIRD_PARTY,
            examples: vec![
                json!({"input": "one turn", "expected": "an answer"}),
                json!({"expected": "an answer with no input"}),
                json!({"input": "another turn"}),
            ],
            index: 1,
            field: "input",
        },
        ViolationRow {
            case: "the predefined schema, an example that is not an object",
            dataset_name: "PredefinedNotAnObject",
            schema_type: PREDEFINED,
            examples: vec![scenario("s1"), json!("just a string")],
            index: 1,
            field: "scenario_id",
        },
        ViolationRow {
            case: "the predefined schema, an example carrying no member at all",
            dataset_name: "PredefinedEmptyBatchEntry",
            schema_type: PREDEFINED,
            examples: vec![json!({})],
            index: 0,
            field: "scenario_id",
        },
    ];

    for row in rows {
        let ViolationRow {
            case,
            dataset_name,
            schema_type,
            examples,
            index,
            field,
        } = row;
        let (status, refused) =
            create_dataset(&router, &inline_create(dataset_name, schema_type, examples)).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} ({refused})"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            format!(
                "Example at index {index} failed schema validation: Required field '{field}' is \
                 missing from example content"
            ),
            "case {case:?}: unexpected refusal message, got {refused}"
        );
    }

    // Nothing the refused calls carried was persisted: the account holds no dataset at
    // all, so not even the valid examples of those batches were committed.
    let (status, listed) = list_datasets(&router).await;
    assert_eq!(
        (status, listed["datasets"].clone()),
        (200, json!([])),
        "the refused creates left no dataset behind, got {status} ({listed})"
    );
}

/// A batch every example of which satisfies its schema is committed whole, and the
/// dataset round-trips: the read reports what the create ingested, the listing names it,
/// and a published version is a numbered snapshot of the same examples.
///
/// The rows differ along the schema type, the number of examples in the batch, and the
/// source — including one whose examples are an Amazon S3 object rather than an inline
/// batch, which this service holds none of its own.
#[tokio::test]
async fn an_accepted_dataset_round_trips_through_its_reads() {
    let router = router();

    // (case, dataset name, schema type, source, the example count expected)
    let rows: [(&str, &str, &str, Value, i64); 4] = [
        (
            "the predefined schema, one example",
            "PredefinedOne",
            PREDEFINED,
            json!({"inlineExamples": {"examples": [scenario("s1")]}}),
            1,
        ),
        (
            "the simulated schema, three examples",
            "SimulatedThree",
            SIMULATED,
            json!({"inlineExamples": {"examples": [
                scenario("s1"), scenario("s2"), scenario("s3"),
            ]}}),
            3,
        ),
        (
            "the third-party schema, two examples",
            "ThirdPartyTwo",
            THIRD_PARTY,
            json!({"inlineExamples": {"examples": [
                {"input": "one turn"},
                {"input": [{"role": "user", "content": "a turn"}]},
            ]}}),
            2,
        ),
        (
            "an Amazon S3 source, whose examples stay in the caller's bucket",
            "FromS3",
            PREDEFINED,
            json!({"s3Source": {"s3Uri": "s3://evaluation-batches/examples.jsonl"}}),
            0,
        ),
    ];

    let mut created_ids = Vec::new();
    for (case, name, schema_type, source, expected_count) in rows {
        let (status, created) = create_dataset(
            &router,
            &json!({
                "datasetName": name,
                "schemaType": schema_type,
                "description": format!("the {name} dataset"),
                "source": source,
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateDataset, got {status} ({created})"
        );
        // "Always CREATING immediately after this call": the transition to ACTIVE is the
        // clock's, so a caller acting straight after the call still sees it.
        assert_eq!(
            created["status"],
            json!("CREATING"),
            "case {case:?}: a fresh dataset reports CREATING, got {created}"
        );
        let dataset_id = created["datasetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create reported no datasetId"))
            .to_string();
        assert_eq!(
            created["datasetArn"],
            json!(format!(
                "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:dataset/{dataset_id}"
            )),
            "case {case:?}: the dataset's ARN names it in the request's own Region, got {created}"
        );

        let (status, read) = get_dataset(&router, &dataset_id, None).await;
        assert_eq!(
            (
                status,
                read["datasetName"].clone(),
                read["schemaType"].clone(),
                read["exampleCount"].clone(),
                read["datasetVersion"].clone(),
                read["draftStatus"].clone(),
            ),
            (
                200,
                json!(name),
                json!(schema_type),
                json!(expected_count),
                json!("DRAFT"),
                json!("MODIFIED"),
            ),
            "case {case:?}: the read reports what the create ingested, got {status} ({read})"
        );
        created_ids.push((name.to_string(), dataset_id));
    }

    // Every accepted dataset is in the account's listing, once each.
    let (status, listed) = list_datasets(&router).await;
    let mut names: Vec<String> = listed["datasets"]
        .as_array()
        .map(|datasets| {
            datasets
                .iter()
                .map(|dataset| dataset["datasetName"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut expected: Vec<String> = created_ids.iter().map(|(name, _)| name.clone()).collect();
    expected.sort();
    assert_eq!(
        (status, names),
        (200, expected),
        "the listing reports every accepted dataset once, got {listed}"
    );
}

/// Publishing a version snapshots the working copy under a number, leaves the draft in
/// place, and both are readable afterwards.
#[tokio::test]
async fn a_published_version_is_a_numbered_snapshot_of_the_working_copy() {
    let router = router();
    let (status, created) = create_dataset(
        &router,
        &inline_create(
            "Published",
            PREDEFINED,
            vec![scenario("s1"), scenario("s2")],
        ),
    )
    .await;
    assert_eq!(status, 202, "the dataset is created, got {status} ({created})");
    let dataset_id = created["datasetId"]
        .as_str()
        .expect("the create reported a datasetId")
        .to_string();

    // "All writes are blocked" while the ingestion is still in flight, so a publish right
    // after the create conflicts with the operation already running on the dataset.
    let (status, blocked) = create_version(&router, &dataset_id).await;
    assert_eq!(
        (status, error_code(&blocked).as_str()),
        (409, "ConflictException"),
        "a publish during the ingestion conflicts, got {status} ({blocked})"
    );

    // The clock owes the dataset its ACTIVE status; nothing sleeps for it in the service,
    // so the suite waits out the span the create armed.
    tokio::time::sleep(std::time::Duration::from_millis(3_200)).await;
    let (status, active) = get_dataset(&router, &dataset_id, None).await;
    assert_eq!(
        (status, active["status"].clone()),
        (200, json!("ACTIVE")),
        "the dataset reaches ACTIVE once the clock owes it, got {status} ({active})"
    );

    let (status, published) = create_version(&router, &dataset_id).await;
    assert_eq!(
        (status, published["datasetVersion"].clone()),
        (202, json!("1")),
        "versions are numbered from one, got {status} ({published})"
    );

    // The DRAFT "is preserved and remains editable after publishing", and its content now
    // matches the version just published from it.
    let (status, draft) = get_dataset(&router, &dataset_id, Some("DRAFT")).await;
    assert_eq!(
        (
            status,
            draft["exampleCount"].clone(),
            draft["draftStatus"].clone()
        ),
        (200, json!(2), json!("UNMODIFIED")),
        "the draft survives the publish, got {status} ({draft})"
    );

    let (status, snapshot) = get_dataset(&router, &dataset_id, Some("1")).await;
    assert_eq!(
        (
            status,
            snapshot["datasetVersion"].clone(),
            snapshot["exampleCount"].clone()
        ),
        (200, json!("1"), json!(2)),
        "the published version reports the examples the draft held, got {status} ({snapshot})"
    );

    let (status, versions) = list_versions(&router, &dataset_id).await;
    assert_eq!(
        (status, versions["versions"].clone()),
        (
            200,
            json!([{
                "datasetVersion": "1",
                "exampleCount": 2,
                "createdAt": published["createdAt"].clone(),
            }])
        ),
        "the version listing reports the one published version, got {status} ({versions})"
    );

    // A version number the dataset never published names nothing to read.
    for (case, version) in [("a version past the last", "2"), ("version zero", "0")] {
        let (status, answer) = get_dataset(&router, &dataset_id, Some(version)).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} ({answer})"
        );
    }
}

/// A dataset id this account never created names nothing: every dataset read and the
/// publish are refused with the modelled 404.
#[tokio::test]
async fn an_absent_dataset_id_is_refused_by_every_dataset_route() {
    let router = router();
    // A dataset really held, so the refusals below are about the id and not about the
    // account being empty.
    let (status, created) =
        create_dataset(&router, &inline_create("Present", PREDEFINED, vec![scenario("s1")])).await;
    assert_eq!(status, 202, "the dataset is created, got {status} ({created})");
    let present = created["datasetId"]
        .as_str()
        .expect("the create reported a datasetId")
        .to_string();

    // Each is a well-formed `DatasetId` (`^[a-zA-Z0-9_-]{1,110}$`), so every refusal is
    // the handler's and not the boundary's.
    let absent: [(&str, String); 3] = [
        ("an id nothing was created under", "Absent-0123456789".to_string()),
        (
            "the stored id with a character prepended",
            format!("z{present}"),
        ),
        ("a single-character id", "a".to_string()),
    ];

    for (case, dataset_id) in absent {
        for (operation, (status, answer)) in [
            ("GetDataset", get_dataset(&router, &dataset_id, None).await),
            (
                "ListDatasetVersions",
                list_versions(&router, &dataset_id).await,
            ),
            ("CreateDatasetVersion", create_version(&router, &dataset_id).await),
        ] {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?} ({operation}): expected HTTP 404 ResourceNotFoundException, got \
                 {status} ({answer})"
            );
        }
    }
}
