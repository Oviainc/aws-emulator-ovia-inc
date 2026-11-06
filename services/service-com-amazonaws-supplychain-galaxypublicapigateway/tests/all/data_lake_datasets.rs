// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the AWS Supply Chain **data-lake dataset**, driven over the
//! wire (restJson1, signing name `scn`) through the same `MultiServiceRouter` the
//! emulator's front door uses — so every assertion here is about what a real client
//! observes, not about a handler's internals.
//!
//! RULE under test: `CreateDataLakeDataset` persists one dataset per
//! (`instanceId`, `namespace`, `name`) triple of an existing instance and one of its
//! namespaces, and every member of the dataset it answers with (`arn`, `schema`,
//! `description`, `partitionSpec`, the timestamps) is read back unchanged by
//! `GetDataLakeDataset`; a triple that is already taken is a conflict, an instance or
//! namespace that does not exist is a not-found, and a create carrying no `schema` is a
//! validation failure that persists nothing.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_supplychain_galaxypublicapigateway::ops::create_data_lake_dataset::SCHEMA_REQUIRED_MESSAGE;
use service_com_amazonaws_supplychain_galaxypublicapigateway::ops::types::{
    DATA_LAKE_DATASET_PARTITION_TRANSFORM_TYPE_VALUES, DATA_LAKE_DATASET_SCHEMA_FIELD_TYPE_VALUES,
};
use service_com_amazonaws_supplychain_galaxypublicapigateway::state::{
    MAX_DATASETS_PER_INSTANCE, PREDEFINED_NAMESPACES,
};
use service_com_amazonaws_supplychain_galaxypublicapigateway::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these tests sign for unless a case names another one.
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

    /// Send one request signed for `region` and return its HTTP status and JSON body.
    async fn send_in(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let body_bytes = match &body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize request body")),
            None => Bytes::new(),
        };
        // A SigV4 `Authorization` header whose credential scope names this service
        // (`scn`), which is how the router picks the service that claims the request.
        // Signature verification is off for the test router, so the signature itself is
        // never read.
        let credential = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/scn/aws4_request, \
             SignedHeaders=host, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://scn.{region}.amazonaws.com{path}"))
            .header("host", format!("scn.{region}.amazonaws.com"))
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

    /// Send one request signed for [`REGION`].
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.send_in(REGION, method, path, body).await
    }

    /// A fresh instance, returning its `instanceId`.
    async fn create_instance(&self, case: &str) -> String {
        let (status, body) = self.send("POST", "/api/instance", Some(json!({}))).await;
        assert_eq!(
            status, 200,
            "case {case}: CreateInstance expected 200, got {status} with {body}"
        );
        body.get("instance")
            .and_then(|instance| instance.get("instanceId"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: CreateInstance returned no instanceId: {body}"))
            .to_string()
    }

    /// `CreateDataLakeDataset` for one triple, with `request` as its body.
    async fn create_dataset(
        &self,
        region: &str,
        instance_id: &str,
        namespace: &str,
        name: &str,
        request: Value,
    ) -> (u16, Value) {
        self.send_in(
            region,
            "PUT",
            &dataset_path(instance_id, namespace, name),
            Some(request),
        )
        .await
    }

    /// `GetDataLakeDataset` for one triple.
    async fn get_dataset(&self, instance_id: &str, namespace: &str, name: &str) -> (u16, Value) {
        self.send("GET", &dataset_path(instance_id, namespace, name), None)
            .await
    }
}

/// The URI every dataset operation is addressed at, as the model's `http` trait spells
/// it: `/api/datalake/instance/{instanceId}/namespaces/{namespace}/datasets/{name}`.
fn dataset_path(instance_id: &str, namespace: &str, name: &str) -> String {
    format!("/api/datalake/instance/{instance_id}/namespaces/{namespace}/datasets/{name}")
}

/// The `dataset` member of a response body.
fn dataset_of(case: &str, body: &Value) -> Value {
    body.get("dataset")
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: response carries no `dataset` member: {body}"))
}

/// A string member of a `dataset` payload.
fn string_of(case: &str, dataset: &Value, member: &str) -> String {
    dataset
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: dataset carries no `{member}`: {dataset}"))
        .to_string()
}

/// The ARN a dataset of this triple must carry, built independently of the handler from
/// `AscResourceArn`'s modelled form (`arn:aws:scn:<region>:<account>:instance/<id>` plus
/// free `[-_./A-Za-z0-9]*`) and the emulator's canonical account id.
fn expected_arn(region: &str, instance_id: &str, namespace: &str, name: &str) -> String {
    format!(
        "arn:aws:scn:{region}:{}:instance/{instance_id}/namespaces/{namespace}/datasets/{name}",
        engine::emulator_account_id()
    )
}

/// A one-field schema, the smallest the model allows (`fields` is `@length` 1..500).
fn minimal_schema() -> Value {
    json!({
        "name": "MinimalSchema",
        "fields": [{ "name": "id", "type": "STRING", "isRequired": true }],
    })
}

/// A schema carrying one field of EVERY modelled `DataLakeDatasetSchemaFieldType`
/// (source: that enum, re-exported as `DATA_LAKE_DATASET_SCHEMA_FIELD_TYPE_VALUES`), so
/// no field type is special-cased.
fn schema_over_every_field_type() -> Value {
    let fields: Vec<Value> = DATA_LAKE_DATASET_SCHEMA_FIELD_TYPE_VALUES
        .iter()
        .enumerate()
        .map(|(index, field_type)| {
            json!({
                "name": format!("field_{}", field_type.to_lowercase()),
                "type": field_type,
                "isRequired": index % 2 == 0,
            })
        })
        .collect();
    json!({
        "name": "EveryFieldType",
        "fields": fields,
        "primaryKeys": [{ "name": format!("field_{}", DATA_LAKE_DATASET_SCHEMA_FIELD_TYPE_VALUES[0].to_lowercase()) }],
    })
}

/// A partition spec carrying one field of every modelled
/// `DataLakeDatasetPartitionTransformType` (source: that enum, re-exported as
/// `DATA_LAKE_DATASET_PARTITION_TRANSFORM_TYPE_VALUES`); the list's modelled maximum is
/// 10 fields, so every transform fits.
fn partition_spec_over_every_transform() -> Value {
    let fields: Vec<Value> = DATA_LAKE_DATASET_PARTITION_TRANSFORM_TYPE_VALUES
        .iter()
        .map(|transform| {
            json!({
                "name": format!("part_{}", transform.to_lowercase()),
                "transform": { "type": transform },
            })
        })
        .collect();
    json!({ "fields": fields })
}

/// One row of the create/read-back table: the dataset's namespace, name and request body.
struct Row {
    case: &'static str,
    namespace: &'static str,
    name: String,
    request: Value,
}

/// The rows spanning the class the rule generalises over: both pre-defined namespaces,
/// names at the ends of the modelled `^[a-z0-9_]+$` / 1..75 shape, every modelled field
/// type and partition transform, and the optional members (`description`,
/// `partitionSpec`, `tags`) both present and absent.
fn create_rows() -> Vec<Row> {
    vec![
        Row {
            case: "default namespace, nothing optional",
            namespace: "default",
            name: "orders".to_string(),
            request: json!({ "schema": minimal_schema() }),
        },
        Row {
            case: "default namespace, every optional member",
            namespace: "default",
            name: "shipment_lines_2024".to_string(),
            request: json!({
                "schema": schema_over_every_field_type(),
                "description": "every modelled field type, partitioned and tagged",
                "partitionSpec": partition_spec_over_every_transform(),
                "tags": { "team": "supply", "env": "" },
            }),
        },
        Row {
            case: "asc namespace with an explicit schema",
            namespace: "asc",
            name: "inbound_order".to_string(),
            request: json!({
                "schema": minimal_schema(),
                "description": "a dataset in the pre-defined asc namespace",
            }),
        },
        Row {
            case: "single-character name",
            namespace: "default",
            name: "a".to_string(),
            request: json!({ "schema": minimal_schema() }),
        },
        Row {
            case: "name at the modelled 75-character maximum",
            namespace: "default",
            // 75 characters of `^[a-z0-9_]+$`, the shape's upper length bound.
            name: format!("d{}", "_9".repeat(37)),
            request: json!({
                "schema": minimal_schema(),
                "partitionSpec": json!({
                    "fields": [{ "name": "id", "transform": { "type": "IDENTITY" } }],
                }),
            }),
        },
    ]
}

/// RULE: a created dataset is persisted under its (instance, namespace, name) triple and
/// `GetDataLakeDataset` answers with exactly the dataset the create returned — every
/// member projected from the stored record, the optional ones absent when the request
/// named none — and reading it again changes nothing.
#[tokio::test]
async fn a_dataset_round_trips_through_create_and_get() {
    let emulator = Emulator::new();
    let instance_id = emulator.create_instance("round trip").await;

    for row in create_rows() {
        let case = row.case;
        let (status, body) = emulator
            .create_dataset(REGION, &instance_id, row.namespace, &row.name, row.request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case}: CreateDataLakeDataset expected 200, got {status} with {body}"
        );
        let created = dataset_of(case, &body);

        assert_eq!(
            string_of(case, &created, "instanceId"),
            instance_id,
            "case {case}: the dataset must report the instance it was created under, got {created}"
        );
        assert_eq!(
            string_of(case, &created, "namespace"),
            row.namespace,
            "case {case}: the dataset must report the namespace it was created in, got {created}"
        );
        assert_eq!(
            string_of(case, &created, "name"),
            row.name,
            "case {case}: the dataset must report the name it was created with, got {created}"
        );
        assert_eq!(
            string_of(case, &created, "arn"),
            expected_arn(REGION, &instance_id, row.namespace, &row.name),
            "case {case}: the dataset ARN must name this triple, got {created}"
        );
        assert_eq!(
            created.get("schema"),
            row.request.get("schema"),
            "case {case}: the stored schema must be the requested one, got {created}"
        );
        assert_eq!(
            created.get("description"),
            row.request.get("description"),
            "case {case}: the stored description must be the requested one, got {created}"
        );
        assert_eq!(
            created.get("partitionSpec"),
            row.request.get("partitionSpec"),
            "case {case}: the stored partitionSpec must be the requested one, got {created}"
        );
        assert_eq!(
            created.get("createdTime"),
            created.get("lastModifiedTime"),
            "case {case}: an untouched dataset was last modified when it was created, got {created}"
        );
        assert!(
            created.get("createdTime").and_then(Value::as_f64).is_some(),
            "case {case}: createdTime must be a timestamp, got {created}"
        );
        assert!(
            created.get("tags").is_none(),
            "case {case}: `tags` is not a member of the DataLakeDataset shape, got {created}"
        );

        let (status, body) = emulator
            .get_dataset(&instance_id, row.namespace, &row.name)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: GetDataLakeDataset expected 200, got {status} with {body}"
        );
        let read = dataset_of(case, &body);
        assert_eq!(
            read, created,
            "case {case}: the dataset read back must equal the one created, got {read}"
        );

        let (status, body) = emulator
            .get_dataset(&instance_id, row.namespace, &row.name)
            .await;
        let read_again = dataset_of(case, &body);
        assert_eq!(status, 200, "case {case}: a second read expected 200, got {status}");
        assert_eq!(
            read_again, created,
            "case {case}: reading a dataset must not change it, got {read_again}"
        );
    }
}

/// RULE: a dataset is addressed by the whole (instance, namespace, name) triple, so one
/// name may be held once per namespace of each instance and reading another triple of
/// the same name is a not-found.
#[tokio::test]
async fn datasets_are_scoped_to_their_instance_and_namespace() {
    let emulator = Emulator::new();
    let first = emulator.create_instance("first instance").await;
    let second = emulator.create_instance("second instance").await;
    let name = "shared_name";

    // One dataset of this name per namespace of each instance: four distinct triples.
    let mut arns = Vec::new();
    for instance_id in [&first, &second] {
        for namespace in PREDEFINED_NAMESPACES {
            let case = format!("{instance_id}/{namespace}");
            let (status, body) = emulator
                .create_dataset(
                    REGION,
                    instance_id,
                    namespace,
                    name,
                    json!({ "schema": minimal_schema(), "description": case.clone() }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: each triple is its own dataset, got {status} with {body}"
            );
            arns.push(string_of(&case, &dataset_of(&case, &body), "arn"));
        }
    }
    let distinct: std::collections::BTreeSet<&String> = arns.iter().collect();
    assert_eq!(
        distinct.len(),
        arns.len(),
        "each triple must carry its own ARN, got {arns:?}"
    );

    // Each stored dataset answers with the description its own create carried, so no
    // triple is reading another's record.
    for instance_id in [&first, &second] {
        for namespace in PREDEFINED_NAMESPACES {
            let case = format!("read {instance_id}/{namespace}");
            let (status, body) = emulator.get_dataset(instance_id, namespace, name).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} with {body}"
            );
            let dataset = dataset_of(&case, &body);
            assert_eq!(
                string_of(&case, &dataset, "description"),
                format!("{instance_id}/{namespace}"),
                "case {case}: each triple keeps its own record, got {dataset}"
            );
        }
    }

    // A name no triple of this instance holds is still a not-found.
    let (status, body) = emulator.get_dataset(&first, "default", "never_created").await;
    assert_eq!(
        status, 404,
        "case absent name: expected 404, got {status} with {body}"
    );
}

/// RULE: a (instance, namespace, name) triple names one dataset, so a create for a
/// triple that already holds one is refused `ConflictException` (409) and the stored
/// dataset is left exactly as it was.
#[tokio::test]
async fn a_second_create_of_one_triple_is_a_conflict() {
    let emulator = Emulator::new();
    let instance_id = emulator.create_instance("conflict").await;

    // Rows differing in what the SECOND create changes: nothing, the schema, the
    // description, the partition spec.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("identical request", "repeat_identical", json!({ "schema": minimal_schema() })),
        (
            "a different schema",
            "repeat_other_schema",
            json!({ "schema": schema_over_every_field_type() }),
        ),
        (
            "a different description",
            "repeat_other_description",
            json!({ "schema": minimal_schema(), "description": "second attempt" }),
        ),
        (
            "an added partition spec",
            "repeat_added_partition",
            json!({
                "schema": minimal_schema(),
                "partitionSpec": partition_spec_over_every_transform(),
            }),
        ),
    ];

    for (case, name, second_request) in rows {
        let (status, body) = emulator
            .create_dataset(
                REGION,
                &instance_id,
                "default",
                name,
                json!({ "schema": minimal_schema(), "description": "first" }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: the first create expected 200, got {status} with {body}"
        );
        let first = dataset_of(case, &body);

        let (status, body) = emulator
            .create_dataset(REGION, &instance_id, "default", name, second_request)
            .await;
        assert_eq!(
            status, 409,
            "case {case}: a second create of one triple must be refused, got {status} with {body}"
        );
        assert!(
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("ConflictException"),
            "case {case}: expected a ConflictException envelope, got {body}"
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            message,
            format!("Dataset {name} already exist in namespace default"),
            "case {case}: the refusal must carry the service's conflict sentence for this \
             dataset and namespace, got {message:?}"
        );

        let (status, body) = emulator.get_dataset(&instance_id, "default", name).await;
        assert_eq!(
            status, 200,
            "case {case}: the stored dataset survives the refusal, got {status} with {body}"
        );
        assert_eq!(
            dataset_of(case, &body),
            first,
            "case {case}: a refused create must change nothing, got {body}"
        );
    }
}

/// RULE: a dataset exists only under an instance the store holds and a namespace that
/// instance has, so a create or read naming either of them wrongly is
/// `ResourceNotFoundException` (404) — and an `instanceId` outside the modelled `UUID`
/// shape is refused `ValidationException` (400) before any handler runs.
#[tokio::test]
async fn an_unknown_instance_or_namespace_is_not_found() {
    let emulator = Emulator::new();
    let instance_id = emulator.create_instance("not found").await;

    // Rows: (case, instanceId, namespace, expected status).
    let rows: Vec<(&str, String, &str, u16)> = vec![
        (
            "well-formed instance id no instance holds",
            "00000000-0000-0000-0000-000000000000".to_string(),
            "default",
            404,
        ),
        (
            "a namespace that was never created",
            instance_id.clone(),
            "custom_namespace",
            404,
        ),
        (
            "a namespace one character off a pre-defined one",
            instance_id.clone(),
            "defaults",
            404,
        ),
        (
            "an instance id outside the modelled UUID shape",
            "not-a-uuid".to_string(),
            "default",
            400,
        ),
        ("a live instance and a pre-defined namespace", instance_id.clone(), "default", 200),
    ];

    for (case, instance, namespace, expected) in rows {
        let (status, body) = emulator
            .create_dataset(
                REGION,
                &instance,
                namespace,
                "probe",
                json!({ "schema": minimal_schema() }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {case}: CreateDataLakeDataset expected {expected}, got {status} with {body}"
        );
        if expected != 200 {
            let kind = if expected == 404 {
                "ResourceNotFoundException"
            } else {
                "ValidationException"
            };
            assert!(
                body.get("__type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .contains(kind),
                "case {case}: expected a {kind} envelope, got {body}"
            );
        }

        // The same triple reads back exactly as the create left it: present only where
        // the create was served.
        let (status, body) = emulator.get_dataset(&instance, namespace, "probe").await;
        assert_eq!(
            status, expected,
            "case {case}: GetDataLakeDataset expected {expected}, got {status} with {body}"
        );
    }
}

/// RULE: a dataset's schema is part of the stored resource and the pre-defined schemas of
/// the `asc` namespace are not part of this service's model, so a create naming no
/// `schema` is refused `ValidationException` (400) and persists nothing.
#[tokio::test]
async fn a_create_without_a_schema_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    let instance_id = emulator.create_instance("no schema").await;

    // Rows: every namespace an instance comes with, and a body that names other members
    // but no schema.
    for namespace in PREDEFINED_NAMESPACES {
        for (case, request) in [
            ("empty body", json!({})),
            (
                "description and tags but no schema",
                json!({ "description": "no schema here", "tags": { "team": "supply" } }),
            ),
            (
                "a partition spec but no schema",
                json!({ "partitionSpec": partition_spec_over_every_transform() }),
            ),
        ] {
            let case = format!("{namespace}: {case}");
            let (status, body) = emulator
                .create_dataset(REGION, &instance_id, namespace, "schemaless", request)
                .await;
            assert_eq!(
                status, 400,
                "case {case}: a schemaless create must be refused, got {status} with {body}"
            );
            assert!(
                body.get("__type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .contains("ValidationException"),
                "case {case}: expected a ValidationException envelope, got {body}"
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(SCHEMA_REQUIRED_MESSAGE),
                "case {case}: expected the schema refusal sentence, got {body}"
            );

            let (status, body) = emulator
                .get_dataset(&instance_id, namespace, "schemaless")
                .await;
            assert_eq!(
                status, 404,
                "case {case}: a refused create must persist nothing, got {status} with {body}"
            );
        }
    }
}

/// RULE: a dataset's ARN carries the region the call that created it was signed for, and
/// the canonical emulator account — the stored value, so a read reports the same ARN.
#[tokio::test]
async fn a_dataset_arn_names_the_region_the_create_was_signed_for() {
    let emulator = Emulator::new();

    for region in ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"] {
        let instance_id = emulator.create_instance(region).await;
        let (status, body) = emulator
            .create_dataset(
                region,
                &instance_id,
                "default",
                "regional",
                json!({ "schema": minimal_schema() }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {region}: CreateDataLakeDataset expected 200, got {status} with {body}"
        );
        let created = dataset_of(region, &body);
        assert_eq!(
            string_of(region, &created, "arn"),
            expected_arn(region, &instance_id, "default", "regional"),
            "case {region}: the ARN must name the region the create was signed for, got {created}"
        );

        // Read through the default region: the ARN is stored, not rebuilt per request.
        let (status, body) = emulator.get_dataset(&instance_id, "default", "regional").await;
        assert_eq!(
            status, 200,
            "case {region}: GetDataLakeDataset expected 200, got {status} with {body}"
        );
        assert_eq!(
            dataset_of(region, &body),
            created,
            "case {region}: a read reports the stored ARN, got {body}"
        );
    }
}

/// RULE: a dataset is addressed by its instance, so deleting the instance takes its
/// datasets with it and leaves every other instance's datasets reachable.
#[tokio::test]
async fn deleting_an_instance_drops_its_datasets() {
    let emulator = Emulator::new();
    let doomed = emulator.create_instance("doomed").await;
    let survivor = emulator.create_instance("survivor").await;

    for instance_id in [&doomed, &survivor] {
        for namespace in PREDEFINED_NAMESPACES {
            let (status, body) = emulator
                .create_dataset(
                    REGION,
                    instance_id,
                    namespace,
                    "kept",
                    json!({ "schema": minimal_schema() }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {instance_id}/{namespace}: expected 200, got {status} with {body}"
            );
        }
    }

    let (status, body) = emulator
        .send("DELETE", &format!("/api/instance/{doomed}"), None)
        .await;
    assert_eq!(
        status, 200,
        "case delete: DeleteInstance expected 200, got {status} with {body}"
    );

    for namespace in PREDEFINED_NAMESPACES {
        let (status, body) = emulator.get_dataset(&doomed, namespace, "kept").await;
        assert_eq!(
            status, 404,
            "case {namespace}: a deleted instance's dataset is gone, got {status} with {body}"
        );
        let (status, body) = emulator.get_dataset(&survivor, namespace, "kept").await;
        assert_eq!(
            status, 200,
            "case {namespace}: another instance's dataset survives, got {status} with {body}"
        );
    }
}

/// RULE: the number of datasets one instance holds is capped, and a create past the cap
/// is refused with the modelled `ServiceQuotaExceededException` (402) while every stored
/// dataset stays readable.
#[tokio::test]
async fn a_create_past_the_dataset_cap_is_a_quota_refusal() {
    let emulator = Emulator::new();
    let instance_id = emulator.create_instance("quota").await;

    for index in 0..MAX_DATASETS_PER_INSTANCE {
        let name = format!("dataset_{index}");
        let (status, body) = emulator
            .create_dataset(
                REGION,
                &instance_id,
                "default",
                &name,
                json!({ "schema": minimal_schema() }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {name}: a create below the cap expected 200, got {status} with {body}"
        );
    }

    let (status, body) = emulator
        .create_dataset(
            REGION,
            &instance_id,
            "asc",
            "one_too_many",
            json!({ "schema": minimal_schema() }),
        )
        .await;
    assert_eq!(
        status, 402,
        "case past the cap: expected the modelled quota refusal, got {status} with {body}"
    );
    assert!(
        body.get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("ServiceQuotaExceededException"),
        "case past the cap: expected a ServiceQuotaExceededException envelope, got {body}"
    );

    // The cap is per instance, so another instance can still create.
    let other = emulator.create_instance("other instance").await;
    let (status, body) = emulator
        .create_dataset(
            REGION,
            &other,
            "default",
            "first_here",
            json!({ "schema": minimal_schema() }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case other instance: the cap is per instance, got {status} with {body}"
    );

    let (status, body) = emulator.get_dataset(&instance_id, "default", "dataset_0").await;
    assert_eq!(
        status, 200,
        "case first stored dataset: still readable at the cap, got {status} with {body}"
    );
}
