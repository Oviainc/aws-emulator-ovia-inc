// SPDX-License-Identifier: CC0-1.0

//! The Personalize ingestion hand-off: the CONTROL plane (`personalize`) mints the
//! targets, the DATA plane (`personalize-events`) ingests into them.
//!
//! RULE: a target the control plane mints over the wire is a target the data plane
//! resolves, because the create RECORDS it in the registry the ingesting service reads —
//! the dataset ARN `CreateDataset` answers with, and the tracking id it provides with an
//! Action interactions dataset ("The ID of the event tracker for an Action interactions
//! dataset. You specify the tracker's ID in the `PutActionInteractions` API operation").
//! A delete un-records it, and a target no create ever minted is refused as the input
//! fault it is: `InvalidInputException` (400), never a not-found.
//!
//! The ingestion API carries no dataset definition — a `PutItems` names an ARN and
//! nothing else — so the two services can only agree because the control plane recorded
//! what it minted. This suite drives both halves through ONE router over ONE engine, the
//! way the emulator mounts them, which is the only place that agreement is observable:
//! with the control plane absent, nothing creates a dataset at all.
//!
//! The rows span the class the rule covers: every metadata dataset type and its put, the
//! Action interactions dataset addressed by its tracking id, a dataset of the wrong type
//! for the put, an ARN never created, and a dataset that was deleted.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_personalizeevents_amazonpersonalizeevents::state::{
    service_state as ingestion_state, MetadataKind, ACTION_INTERACTIONS_NOT_SUPPORTED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the signing name the whole Personalize family
/// shares. The signature is never checked: the router here runs with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/personalize/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// How long a case waits for a create to complete before calling it stuck. Far above the
/// control plane's own create span, so a slow machine does not fail the suite.
const ACTIVE_DEADLINE: Duration = Duration::from_secs(10);

/// One answer: the status and the parsed JSON body (`Null` for the empty 200 every
/// ingestion operation answers with).
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code: both protocols carry it as the body's `__type`.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    /// The `message` an error body carries, or `""`.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// The control plane and the ingestion data plane, mounted on one engine as
/// `core/router/src/lib.rs` mounts them.
fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_personalize_amazonpersonalize::EmulatedService::new(Arc::clone(
            engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_personalizeevents_amazonpersonalizeevents::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// One request through the whole router.
async fn dispatch(
    router: &MultiServiceRouter,
    path: &str,
    target: Option<&str>,
    body: &Value,
) -> Answer {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://emulator.local{path}"))
        .header("host", "emulator.local")
        .header("authorization", AUTHORIZATION);
    builder = match target {
        Some(target) => builder
            .header("x-amz-target", format!("AmazonPersonalize.{target}"))
            .header("content-type", "application/x-amz-json-1.1"),
        None => builder.header("content-type", "application/json"),
    };
    let request = builder
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null),
    }
}

/// An awsJson1_1 control-plane call.
async fn control(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    dispatch(router, "/", Some(operation), &body).await
}

/// A restJson1 ingestion call.
async fn ingest(router: &MultiServiceRouter, path: &str, body: Value) -> Answer {
    dispatch(router, path, None, &body).await
}

/// A minimal Avro record schema for `dataset_type`, in the format `CreateSchema` takes.
fn avro_schema(dataset_type: &str) -> String {
    json!({
        "type": "record",
        "name": dataset_type,
        "namespace": "com.amazonaws.personalize.schema",
        "fields": [
            {"name": "USER_ID", "type": "string"},
            {"name": "ITEM_ID", "type": "string"},
            {"name": "TIMESTAMP", "type": "long"},
        ],
        "version": "1.0",
    })
    .to_string()
}

/// Poll `operation` until the resource it describes reports `ACTIVE`, and return the last
/// description. A create is not instant: "CREATE PENDING > CREATE IN_PROGRESS > ACTIVE".
async fn await_active(
    router: &MultiServiceRouter,
    operation: &str,
    member: &str,
    resource: &str,
    arn: &str,
    case: &str,
) -> Value {
    let deadline = Instant::now() + ACTIVE_DEADLINE;
    loop {
        let described = control(router, operation, json!({ member: arn })).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected {operation} to answer 200, got {} with body {}",
            described.status, described.body
        );
        let status = described.body[resource]["status"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if status == "ACTIVE" {
            return described.body[resource].clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {operation} never reported ACTIVE (last status {status:?})"
        );
        // The wait is the create clock's, so it is awaited rather than slept through: a
        // blocking sleep would hold the runtime worker this test's dispatches run on.
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Create a dataset group, wait for it to be `ACTIVE`, and return its ARN.
async fn create_active_group(router: &MultiServiceRouter, name: &str) -> String {
    let created = control(router, "CreateDatasetGroup", json!({"name": name})).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected CreateDatasetGroup to answer 200, got {} with body {}",
        created.status, created.body
    );
    let arn = created.body["datasetGroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: expected a datasetGroupArn, got {}", created.body))
        .to_owned();
    await_active(
        router,
        "DescribeDatasetGroup",
        "datasetGroupArn",
        "datasetGroup",
        &arn,
        name,
    )
    .await;
    arn
}

/// Create a dataset of `dataset_type`, wait for it to be `ACTIVE`, and return its ARN
/// together with the `trackingId` the describe reports for it (absent for every type but
/// Action interactions).
async fn create_active_dataset(
    router: &MultiServiceRouter,
    group_arn: &str,
    schema_arn: &str,
    dataset_type: &str,
) -> (String, Option<String>) {
    let created = control(
        router,
        "CreateDataset",
        json!({
            "name": format!("ds-{}", dataset_type.to_ascii_lowercase()),
            "schemaArn": schema_arn,
            "datasetGroupArn": group_arn,
            "datasetType": dataset_type,
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {dataset_type:?}: expected CreateDataset to answer 200, got {} with body {}",
        created.status, created.body
    );
    let arn = created.body["datasetArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {dataset_type:?}: expected a datasetArn, got {}",
                created.body
            )
        })
        .to_owned();
    let described = await_active(
        router,
        "DescribeDataset",
        "datasetArn",
        "dataset",
        &arn,
        dataset_type,
    )
    .await;
    let tracking_id = described["trackingId"].as_str().map(ToOwned::to_owned);
    (arn, tracking_id)
}

/// The metadata puts, as (dataset type, route, the member carrying the batch, the member
/// carrying one record's id).
const METADATA_PUTS: [(&str, &str, &str, &str); 3] = [
    ("Items", "/items", "items", "itemId"),
    ("Users", "/users", "users", "userId"),
    ("Actions", "/actions", "actions", "actionId"),
];

/// The metadata collection a route writes into, for reading the store back.
fn kind_of(route: &str) -> MetadataKind {
    match route {
        "/items" => MetadataKind::Items,
        "/users" => MetadataKind::Users,
        _ => MetadataKind::Actions,
    }
}

/// One batch of metadata records for `route`.
fn metadata_body(dataset_arn: &str, batch_member: &str, id_member: &str, ids: &[&str]) -> Value {
    json!({
        "datasetArn": dataset_arn,
        batch_member: ids
            .iter()
            .map(|id| json!({ id_member: id, "properties": r#"{"price":9.99}"# }))
            .collect::<Vec<_>>(),
    })
}

#[tokio::test]
async fn the_data_plane_ingests_into_the_targets_the_control_plane_minted_and_refuses_the_rest() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    let group = create_active_group(&router, "wire-ingestion-dsg").await;
    let schema = control(
        &router,
        "CreateSchema",
        json!({"name": "wire-ingestion-schema", "schema": avro_schema("Interactions")}),
    )
    .await;
    assert_eq!(
        schema.status, 200,
        "case \"CreateSchema\": expected HTTP 200, got {} with body {}",
        schema.status, schema.body
    );
    let schema_arn = schema.body["schemaArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a schemaArn, got {}", schema.body))
        .to_owned();

    // Every metadata dataset type: created over the wire, then ingested into over the
    // wire — and the records are in the store the service keeps, which is the whole
    // result of a call whose response body is empty.
    let mut created: Vec<(&str, String)> = Vec::new();
    for (dataset_type, route, batch_member, id_member) in METADATA_PUTS {
        let (dataset_arn, tracking_id) =
            create_active_dataset(&router, &group, &schema_arn, dataset_type).await;
        assert_eq!(
            tracking_id, None,
            "case {dataset_type:?}: only an Action interactions dataset carries a trackingId, \
             got {tracking_id:?}"
        );

        let put = ingest(
            &router,
            route,
            metadata_body(&dataset_arn, batch_member, id_member, &["record-1", "record-2"]),
        )
        .await;
        assert_eq!(
            (put.status, put.error_code().as_str()),
            (200, ""),
            "case {dataset_type:?}: expected {route} into the dataset the control plane just \
             created to answer HTTP 200, got {} {:?} with body {}",
            put.status,
            put.error_code(),
            put.body
        );

        let stored: Vec<String> = ingestion_state(&*engine)
            .expect("the ingestion store")
            .lock()
            .dataset_mut(&dataset_arn)
            .map(|dataset| dataset.records(kind_of(route)).keys().cloned().collect())
            .unwrap_or_default();
        assert_eq!(
            stored,
            vec!["record-1".to_string(), "record-2".to_string()],
            "case {dataset_type:?}: expected both records ingested into {dataset_arn}, got \
             {stored:?}"
        );
        created.push((dataset_type, dataset_arn));
    }

    // A put into a dataset of ANOTHER type is refused for the type, naming the type the
    // operation serves.
    for (dataset_type, dataset_arn) in &created {
        for (_, route, batch_member, id_member) in METADATA_PUTS {
            let required = route.trim_start_matches('/').to_ascii_uppercase();
            if required.eq_ignore_ascii_case(dataset_type) {
                continue;
            }
            let case = format!("{route} into the {dataset_type} dataset");
            let put = ingest(
                &router,
                route,
                metadata_body(dataset_arn, batch_member, id_member, &["record-x"]),
            )
            .await;
            assert_eq!(
                (put.status, put.error_code().as_str()),
                (400, "InvalidInputException"),
                "case {case:?}: expected HTTP 400 InvalidInputException, got {} {:?} with body {}",
                put.status,
                put.error_code(),
                put.body
            );
            let expected = format!("{dataset_arn} is not a valid {required} dataset.");
            assert_eq!(
                put.message(),
                expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                put.message()
            );
        }
    }

    // An Action interactions dataset is addressed on the data plane by the tracking id the
    // control plane provides with it, and that tracker ingests.
    let (action_interactions_arn, tracking_id) =
        create_active_dataset(&router, &group, &schema_arn, "Action_Interactions").await;
    let tracking_id = tracking_id.unwrap_or_else(|| {
        panic!("expected DescribeDataset to report a trackingId for {action_interactions_arn}")
    });
    let interactions = ingest(
        &router,
        "/action-interactions",
        json!({
            "trackingId": tracking_id,
            "actionInteractions": [{
                "actionId": "action-1",
                "sessionId": "session-1",
                "timestamp": 1_764_000_000,
                "eventType": "Taken",
            }],
        }),
    )
    .await;
    assert_eq!(
        (interactions.status, interactions.error_code().as_str()),
        (200, ""),
        "case \"the tracking id the dataset was created with\": expected HTTP 200, got {} {:?} \
         with body {}",
        interactions.status,
        interactions.error_code(),
        interactions.body
    );
    let ingested = ingestion_state(&*engine)
        .expect("the ingestion store")
        .lock()
        .event_tracker_mut(&tracking_id)
        .map(|tracker| tracker.action_interactions.len())
        .unwrap_or_default();
    assert_eq!(
        ingested, 1,
        "case \"the tracking id the dataset was created with\": expected the interaction \
         ingested, got {ingested} record(s)"
    );

    // A dataset ARN no create ever minted resolves to nothing: an input fault that names
    // the member and does not echo the ARN.
    let never_created = format!("{}-nope/ITEMS", created[0].1);
    let unknown = ingest(
        &router,
        "/items",
        metadata_body(&never_created, "items", "itemId", &["record-1"]),
    )
    .await;
    assert_eq!(
        (unknown.status, unknown.error_code().as_str(), unknown.message()),
        (400, "InvalidInputException", "Invalid datasetArn"),
        "case \"an ARN never created\": expected HTTP 400 InvalidInputException \"Invalid \
         datasetArn\", got {} {:?} with body {}",
        unknown.status,
        unknown.error_code(),
        unknown.body
    );

    // Deleting a target un-records it: the ARN the control plane no longer holds is
    // refused exactly as one that never existed, and the Action interactions dataset's
    // tracking id stops resolving with it.
    for (dataset_type, dataset_arn) in &created {
        let deleted = control(
            &router,
            "DeleteDataset",
            json!({"datasetArn": dataset_arn}),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {dataset_type:?}: expected DeleteDataset to answer 200, got {} with body {}",
            deleted.status, deleted.body
        );
        let route = METADATA_PUTS
            .iter()
            .find(|(candidate, _, _, _)| candidate == dataset_type)
            .expect("the row this dataset was created from");
        let put = ingest(
            &router,
            route.1,
            metadata_body(dataset_arn, route.2, route.3, &["record-3"]),
        )
        .await;
        assert_eq!(
            (put.status, put.error_code().as_str(), put.message()),
            (400, "InvalidInputException", "Invalid datasetArn"),
            "case {dataset_type:?}: expected a put into a DELETED dataset to be refused as an \
             unresolvable datasetArn, got {} {:?} with body {}",
            put.status,
            put.error_code(),
            put.body
        );
    }

    let deleted = control(
        &router,
        "DeleteDataset",
        json!({"datasetArn": action_interactions_arn}),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete the Action interactions dataset\": expected HTTP 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let interactions = ingest(
        &router,
        "/action-interactions",
        json!({
            "trackingId": tracking_id,
            "actionInteractions": [{
                "actionId": "action-2",
                "sessionId": "session-1",
                "timestamp": 1_764_000_001,
                "eventType": "Taken",
            }],
        }),
    )
    .await;
    // A tracking id the data plane can ingest no action-interaction stream through is the
    // same input fault as an unresolvable `datasetArn`, worded the way that put words it.
    assert_eq!(
        (
            interactions.status,
            interactions.error_code().as_str(),
            interactions.message()
        ),
        (
            400,
            "InvalidInputException",
            ACTION_INTERACTIONS_NOT_SUPPORTED_MESSAGE
        ),
        "case \"the tracking id of a deleted dataset\": expected the tracker to stop resolving \
         and the put to be refused as an input fault, got {} {:?} with body {}",
        interactions.status,
        interactions.error_code(),
        interactions.body
    );
}
