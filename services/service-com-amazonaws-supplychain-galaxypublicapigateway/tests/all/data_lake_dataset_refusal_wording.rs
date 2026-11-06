// SPDX-License-Identifier: CC0-1.0

//! The AWS Supply Chain **data-lake dataset plane** driven over the wire (restJson1,
//! signing name `scn`) through the same `MultiServiceRouter` the emulator's front door
//! uses, so every assertion here is about what a real client observes.
//!
//! RULE under test: a dataset lives in any namespace its instance holds — one the
//! instance comes with or one a caller created with `CreateDataLakeNamespace` — and is
//! read back unchanged; and every refusal the dataset operations emit names the dataset
//! and the namespace the request addressed, in the service's own wording ("Dataset
//! <name> not exist in namespace <namespace>", "Dataset <name> already exist in namespace
//! <namespace>"), interpolating only those two `@httpLabel` members and never the
//! `instanceId`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_supplychain_galaxypublicapigateway::state::PREDEFINED_NAMESPACES;
use service_com_amazonaws_supplychain_galaxypublicapigateway::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these tests sign for.
const REGION: &str = "us-east-1";

/// The emulator under test: one engine, one service, driven through the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Keep the test independent of how this process was launched: no authenticator,
        // so a request routes on its credential scope alone.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send one request and return its HTTP status and JSON body.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let body_bytes = match &body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize request body")),
            None => Bytes::new(),
        };
        let credential = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/scn/aws4_request, \
             SignedHeaders=host, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://scn.{REGION}.amazonaws.com{path}"))
            .header("host", format!("scn.{REGION}.amazonaws.com"))
            .header("authorization", credential)
            .header("content-type", "application/json")
            .body(body_bytes)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "{method} {path}: body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, json)
    }

    /// A fresh instance, returning its `instanceId`.
    async fn create_instance(&self, case: &str) -> String {
        let (status, body) = self.send("POST", "/api/instance", Some(json!({}))).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateInstance expected 200, got {status} with {body}"
        );
        body.get("instance")
            .and_then(|instance| instance.get("instanceId"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: CreateInstance returned no instanceId: {body}")
            })
            .to_string()
    }

    /// `CreateDataLakeNamespace` for `name`.
    async fn create_namespace(&self, instance_id: &str, name: &str) -> (u16, Value) {
        self.send(
            "PUT",
            &format!("/api/datalake/instance/{instance_id}/namespaces/{name}"),
            Some(json!({})),
        )
        .await
    }

    /// `CreateDataLakeDataset` for one triple.
    async fn create_dataset(
        &self,
        instance_id: &str,
        namespace: &str,
        name: &str,
        request: Value,
    ) -> (u16, Value) {
        self.send("PUT", &dataset_path(instance_id, namespace, name), Some(request))
            .await
    }

    /// `GetDataLakeDataset` for one triple.
    async fn get_dataset(&self, instance_id: &str, namespace: &str, name: &str) -> (u16, Value) {
        self.send("GET", &dataset_path(instance_id, namespace, name), None)
            .await
    }

    /// `DeleteDataLakeDataset` for one triple.
    async fn delete_dataset(&self, instance_id: &str, namespace: &str, name: &str) -> (u16, Value) {
        self.send("DELETE", &dataset_path(instance_id, namespace, name), None)
            .await
    }
}

/// The URI every dataset operation binds: `.../namespaces/{namespace}/datasets/{name}`.
fn dataset_path(instance_id: &str, namespace: &str, name: &str) -> String {
    format!("/api/datalake/instance/{instance_id}/namespaces/{namespace}/datasets/{name}")
}

/// The `dataset` member of a dataset response, which the model declares `@required`.
fn dataset_of(case: &str, body: &Value) -> Value {
    body.get("dataset")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the response carries no `dataset`: {body}"))
}

/// The `message` member of a refusal, as a client reads it.
fn message_of(case: &str, body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no `message`: {body}"))
        .to_string()
}

/// The smallest schema the model admits (`DataLakeDatasetSchema$fields` is `@length`
/// 1..500), so a row is about the namespace or the wording and nothing else.
fn minimal_schema() -> Value {
    json!({
        "name": "ProbeSchema",
        "fields": [{ "name": "id", "type": "STRING", "isRequired": true }],
    })
}

/// Namespace names spanning the modelled `DataLakeNamespaceName` shape — `@length` 1..50
/// and `@pattern ^[a-z0-9_]+$` (source: the `name` member of
/// `CreateDataLakeNamespaceRequest`) — none of them starting with a prefix the service
/// reserves: a single character, digits, underscores, and the 50-character upper bound.
fn custom_namespace_rows() -> Vec<(&'static str, String)> {
    vec![
        ("single character", "n".to_string()),
        ("digits and underscores", "zone_42_raw".to_string()),
        ("trailing underscore", "orders_".to_string()),
        // 50 characters: the shape's upper length bound.
        ("name at the modelled 50-character maximum", format!("z{}", "_9".repeat(24) + "0")),
    ]
}

/// RULE: a namespace a caller created is a namespace of that instance, so a dataset
/// addressed to it is created and read back unchanged — exactly as one addressed to a
/// namespace the instance comes with.
#[tokio::test]
async fn a_dataset_lives_in_any_namespace_its_instance_holds() {
    let emulator = Emulator::new();
    let instance_id = emulator.create_instance("dataset plane").await;

    // Rows spanning the namespaces an instance can hold: the two pre-defined ones and
    // created ones across the modelled name shape.
    let mut rows: Vec<(String, String)> = PREDEFINED_NAMESPACES
        .iter()
        .map(|namespace| {
            (
                format!("pre-defined namespace {namespace}"),
                (*namespace).to_string(),
            )
        })
        .collect();
    for (case, namespace) in custom_namespace_rows() {
        let (status, body) = emulator.create_namespace(&instance_id, &namespace).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateDataLakeNamespace expected 200, got {status} with {body}"
        );
        rows.push((format!("created namespace: {case}"), namespace));
    }

    for (case, namespace) in rows {
        let name = "probe_dataset";
        let request = json!({
            "schema": minimal_schema(),
            "description": format!("dataset of {namespace}"),
        });
        let (status, body) = emulator
            .create_dataset(&instance_id, &namespace, name, request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateDataLakeDataset expected 200, got {status} with {body}"
        );
        let created = dataset_of(&case, &body);
        assert_eq!(
            created.get("namespace").and_then(Value::as_str),
            Some(namespace.as_str()),
            "case {case:?}: the dataset must report the namespace it was created in, got {created}"
        );
        assert_eq!(
            created.get("schema"),
            request.get("schema"),
            "case {case:?}: the stored schema must be the requested one, got {created}"
        );
        assert_eq!(
            created.get("arn").and_then(Value::as_str),
            Some(
                format!(
                    "arn:aws:scn:{REGION}:{}:instance/{instance_id}/namespaces/{namespace}/datasets/{name}",
                    engine::emulator_account_id()
                )
                .as_str()
            ),
            "case {case:?}: the dataset ARN must name this triple, got {created}"
        );

        let (status, body) = emulator.get_dataset(&instance_id, &namespace, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetDataLakeDataset expected 200, got {status} with {body}"
        );
        assert_eq!(
            dataset_of(&case, &body),
            created,
            "case {case:?}: the dataset read back must equal the one created, got {body}"
        );
    }
}

/// RULE: `GetDataLakeDataset` refuses a dataset the data lake does not hold with the
/// service's fixed sentence "Dataset <name> not exist in namespace <namespace>", whose
/// only variable parts are the two `@httpLabel` members of the request — so the
/// `instanceId` never appears in it, and the same absent dataset read through two
/// instances yields the same sentence.
#[tokio::test]
async fn reading_an_absent_dataset_carries_the_services_not_found_sentence() {
    let emulator = Emulator::new();
    let first = emulator.create_instance("first").await;
    let second = emulator.create_instance("second").await;
    let (status, body) = emulator.create_namespace(&first, "orders").await;
    assert_eq!(
        status, 200,
        "the namespace must be created first, got {status} with {body}"
    );

    // Rows differing along both variable parts: the namespace (pre-defined, created, and
    // one nobody created — every one of these triples names a dataset the store does not
    // hold, which is what this read refuses on) and the dataset name (single character,
    // long, underscored).
    let rows: Vec<(&str, &str, String)> = vec![
        ("pre-defined namespace", "default", "never_created".to_string()),
        ("the other pre-defined namespace", "asc", "a".to_string()),
        ("a namespace this caller created", "orders", "lines_2024".to_string()),
        ("a namespace nobody created", "shipments", "lines".to_string()),
        // 75 characters of `^[a-z0-9_]+$`: `DataLakeDatasetName`'s upper length bound.
        ("name at the modelled 75-character maximum", "default", format!("d{}", "_9".repeat(37))),
    ];

    for (case, namespace, name) in rows {
        let expected = format!("Dataset {name} not exist in namespace {namespace}");
        for (instance_case, instance_id) in [("first instance", &first), ("second instance", &second)]
        {
            let case = format!("{case} / {instance_case}");
            let (status, body) = emulator.get_dataset(instance_id, namespace, &name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: GetDataLakeDataset expected 404, got {status} with {body}"
            );
            assert!(
                body.get("__type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .contains("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException envelope, got {body}"
            );
            let message = message_of(&case, &body);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the service's not-found sentence {expected:?}, got \
                 {message:?}"
            );
            assert!(
                !message.contains(instance_id.as_str()),
                "case {case:?}: the sentence must not carry the instanceId, got {message:?}"
            );
        }
    }
}

/// RULE: `DeleteDataLakeDataset` removes the dataset its triple addresses and answers
/// with that triple, so the dataset is no longer readable afterwards — and a delete of a
/// triple the data lake does not hold, including a repeat of one that already succeeded,
/// is the same `ResourceNotFoundException` (404) sentence a read of it carries.
#[tokio::test]
async fn a_deleted_dataset_is_gone_and_both_reads_and_repeat_deletes_are_not_found() {
    let emulator = Emulator::new();
    let instance_id = emulator.create_instance("delete").await;
    let (status, body) = emulator.create_namespace(&instance_id, "orders").await;
    assert_eq!(
        status, 200,
        "the namespace must be created first, got {status} with {body}"
    );

    // Rows differing along the namespace (both pre-defined ones and a created one) and
    // the dataset name.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("pre-defined namespace", "default", "doomed_lines"),
        ("the other pre-defined namespace", "asc", "inbound_order"),
        ("a namespace this caller created", "orders", "l"),
    ];

    for (case, namespace, name) in rows {
        let (status, body) = emulator
            .create_dataset(&instance_id, namespace, name, json!({ "schema": minimal_schema() }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create expected 200, got {status} with {body}"
        );

        let (status, body) = emulator.delete_dataset(&instance_id, namespace, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteDataLakeDataset expected 200, got {status} with {body}"
        );
        assert_eq!(
            (
                body.get("instanceId").and_then(Value::as_str),
                body.get("namespace").and_then(Value::as_str),
                body.get("name").and_then(Value::as_str),
            ),
            (Some(instance_id.as_str()), Some(namespace), Some(name)),
            "case {case:?}: the delete must answer the triple it removed, got {body}"
        );

        let expected = format!("Dataset {name} not exist in namespace {namespace}");

        let (status, body) = emulator.get_dataset(&instance_id, namespace, name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a deleted dataset is no longer readable, got {status} with {body}"
        );
        let message = message_of(case, &body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the service's not-found sentence {expected:?} reading a \
             deleted dataset, got {message:?}"
        );

        let (status, body) = emulator.delete_dataset(&instance_id, namespace, name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a repeat delete expected 404, got {status} with {body}"
        );
        let message = message_of(case, &body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the service's not-found sentence {expected:?} on a repeat \
             delete, got {message:?}"
        );

        // The namespace the dataset lived in is untouched by its deletion, so the same
        // triple can be created again.
        let (status, body) = emulator
            .create_dataset(&instance_id, namespace, name, json!({ "schema": minimal_schema() }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the triple is free again after the delete, got {status} with {body}"
        );
    }
}

/// RULE: a dataset create for a triple the data lake already holds is refused
/// `ConflictException` (409) with the service's fixed sentence "Dataset <name> already
/// exist in namespace <namespace>" — again only the two `@httpLabel` members, never the
/// `instanceId`.
#[tokio::test]
async fn a_duplicate_dataset_create_carries_the_services_conflict_sentence() {
    let emulator = Emulator::new();
    let instance_id = emulator.create_instance("conflict wording").await;
    let (status, body) = emulator.create_namespace(&instance_id, "orders").await;
    assert_eq!(
        status, 200,
        "the namespace must be created first, got {status} with {body}"
    );

    // Rows differing along the namespace (pre-defined and created) and the dataset name.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("pre-defined namespace", "default", "repeat_default"),
        ("the other pre-defined namespace", "asc", "inbound_order"),
        ("a namespace this caller created", "orders", "l"),
    ];

    for (case, namespace, name) in rows {
        let (status, body) = emulator
            .create_dataset(&instance_id, namespace, name, json!({ "schema": minimal_schema() }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create expected 200, got {status} with {body}"
        );

        let (status, body) = emulator
            .create_dataset(&instance_id, namespace, name, json!({ "schema": minimal_schema() }))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a repeat create expected 409, got {status} with {body}"
        );
        let message = message_of(case, &body);
        let expected = format!("Dataset {name} already exist in namespace {namespace}");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the service's conflict sentence {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(instance_id.as_str()),
            "case {case:?}: the sentence must not carry the instanceId, got {message:?}"
        );
    }
}
