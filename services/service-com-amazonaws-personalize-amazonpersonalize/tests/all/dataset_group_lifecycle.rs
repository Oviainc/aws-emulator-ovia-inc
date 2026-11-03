// SPDX-License-Identifier: CC0-1.0

//! The Personalize dataset-group lifecycle, end to end over the wire.
//!
//! Every case drives the production `MultiServiceRouter` with a buffered awsJson1_1
//! request (`POST /`, `X-Amz-Target: AmazonPersonalize.<Operation>`), so what is asserted
//! is what a signing AWS client would see — routing, deserialization, constraint
//! validation, the handler and serialization included.
//!
//! The suites are tables rather than single calls: each row varies the thing the rule
//! under test generalises over (the domain, the key/role pairing, the dataset type and its
//! casing, the schema document, the page size), and the case the bootstrap issue cited is
//! one row among them.

use std::time::{Duration, Instant};

use crate::refusal_wording::SCHEMA_IN_USE_MESSAGE;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names; every minted ARN is scoped to it.
pub(crate) const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// The statuses a dataset group or dataset reports before its create completes. Source:
/// the model's own documentation for `CreateDatasetGroup` — "CREATE PENDING > CREATE
/// IN_PROGRESS > ACTIVE".
const NON_TERMINAL_CREATE_STATUSES: &[&str] = &["CREATE PENDING", "CREATE IN_PROGRESS"];

/// How long a case waits for a create to complete before calling it stuck. Far above the
/// emulator's own create span, so a slow machine does not fail the suite.
const ACTIVE_DEADLINE: Duration = Duration::from_secs(10);

/// One emulator over one shared engine, driven through the same `dispatch` the served
/// socket path uses.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: std::sync::Arc<engine::InMemoryEngine> =
            std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite drives unsigned requests, so signature verification is off; the
        // `Authorization` header below is still what routes the request and names the
        // region, exactly as a signed client's would.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_personalize_amazonpersonalize::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, returning the HTTP status and the parsed response.
    pub(crate) async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("AmazonPersonalize.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}\
                     /personalize/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call an ARN-addressed operation, naming the ARN in `member`.
    pub(crate) async fn call_with_arn(
        &self,
        operation: &str,
        member: &str,
        arn: &str,
    ) -> (u16, Value) {
        let mut body = Map::new();
        body.insert(member.to_string(), Value::String(arn.to_string()));
        self.call(operation, Value::Object(body)).await
    }
}

/// The `__type` discriminator an awsJson error body carries, or `""`.
pub(crate) fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// Assert `operation` on `arn` fails with `status` and `code`.
async fn assert_fails(
    emulator: &Emulator,
    operation: &str,
    member: &str,
    arn: &str,
    expected_status: u16,
    expected_code: &str,
    case: &str,
) {
    let (status, body) = emulator.call_with_arn(operation, member, arn).await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected {operation} to answer HTTP {expected_status}, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains(expected_code),
        "case {case:?}: expected {operation} to answer {expected_code}, got {body:?}"
    );
}

/// Poll `operation` until the resource it reports is `ACTIVE`, and return that resource.
///
/// A freshly created dataset group or dataset is NOT ready — the model documents the
/// progression "CREATE PENDING > CREATE IN_PROGRESS > ACTIVE" — so every status seen on
/// the way has to be one of the documented transient ones.
async fn await_active(
    emulator: &Emulator,
    operation: &str,
    arn_member: &str,
    resource_member: &str,
    arn: &str,
    case: &str,
) -> Value {
    let deadline = Instant::now() + ACTIVE_DEADLINE;
    loop {
        let (status, body) = emulator.call_with_arn(operation, arn_member, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {body:?}"
        );
        let resource = body[resource_member].clone();
        let reported = resource["status"].as_str().unwrap_or("").to_string();
        if reported == "ACTIVE" {
            return resource;
        }
        assert!(
            NON_TERMINAL_CREATE_STATUSES.contains(&reported.as_str()),
            "case {case:?}: expected {operation} to report ACTIVE or one of \
             {NON_TERMINAL_CREATE_STATUSES:?}, got {reported:?} in {body:?}"
        );
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {operation} never reported ACTIVE, last status {reported:?}"
        );
        // The runtime this test target uses carries no timer, so the wait is a blocking
        // sleep; nothing else is in flight on it.
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A minimal Avro record schema for `dataset_type`, in the Avro JSON format
/// `CreateSchema` documents.
pub(crate) fn avro_schema(dataset_type: &str) -> String {
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

/// Create a schema and return its ARN.
pub(crate) async fn create_schema(
    emulator: &Emulator,
    name: &str,
    schema: &str,
    case: &str,
) -> String {
    let (status, body) = emulator
        .call("CreateSchema", json!({"name": name, "schema": schema}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateSchema to answer 200, got {status} {body:?}"
    );
    body["schemaArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a schemaArn, got {body:?}"))
        .to_string()
}

/// Create a dataset group, wait for it to become `ACTIVE`, and return its ARN.
pub(crate) async fn create_active_group(emulator: &Emulator, name: &str, case: &str) -> String {
    let (status, body) = emulator
        .call("CreateDatasetGroup", json!({"name": name}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateDatasetGroup to answer 200, got {status} {body:?}"
    );
    let arn = body["datasetGroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a datasetGroupArn, got {body:?}"))
        .to_string();
    await_active(
        emulator,
        "DescribeDatasetGroup",
        "datasetGroupArn",
        "datasetGroup",
        &arn,
        case,
    )
    .await;
    arn
}

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_schema_is_described_back_exactly_as_created() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: the document's shape and
    // size, and whether the schema belongs to a Domain dataset group. The `domain` values
    // are the model's own `Domain` enum set (`DOMAIN_VALUES` in
    // `src/ops/types/shared_types.rs`); the first row is the case the bootstrap issue
    // cited.
    let cases: &[(&str, String, Option<&str>)] = &[
        ("my-interactions", avro_schema("Interactions"), None),
        ("items9", avro_schema("Items"), Some("ECOMMERCE")),
        (
            "vod_users",
            json!({"type": "record", "name": "Users", "fields": []}).to_string(),
            Some("VIDEO_ON_DEMAND"),
        ),
        (
            "A",
            json!({
                "type": "record",
                "name": "Action_Interactions",
                "fields": (0..64)
                    .map(|index| json!({"name": format!("FIELD_{index}"), "type": "string"}))
                    .collect::<Vec<_>>(),
            })
            .to_string(),
            None,
        ),
    ];

    for (name, schema, domain) in cases {
        let case = format!("schema {name}");
        let mut request = json!({"name": name, "schema": schema});
        if let Some(domain) = domain {
            request["domain"] = json!(domain);
        }

        let (status, body) = emulator.call("CreateSchema", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateSchema to answer 200, got {status} {body:?}"
        );
        let arn = body["schemaArn"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            arn,
            format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/{name}"),
            "case {case:?}: expected the minted schema ARN, got {body:?}"
        );

        let (status, body) = emulator
            .call_with_arn("DescribeSchema", "schemaArn", &arn)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeSchema to answer 200, got {status} {body:?}"
        );
        let described = &body["schema"];
        assert_eq!(
            described["name"].as_str(),
            Some(*name),
            "case {case:?}: expected the created name, got {described:?}"
        );
        assert_eq!(
            described["schemaArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the created ARN, got {described:?}"
        );
        assert_eq!(
            described["schema"].as_str(),
            Some(schema.as_str()),
            "case {case:?}: expected the document stored verbatim, got {described:?}"
        );
        assert_eq!(
            described["domain"].as_str(),
            *domain,
            "case {case:?}: expected the created domain, got {described:?}"
        );
        assert!(
            described["creationDateTime"].is_number()
                && described["lastUpdatedDateTime"].is_number(),
            "case {case:?}: expected both timestamps populated, got {described:?}"
        );

        // A second create under the same name is a clash, not an update.
        let (status, body) = emulator
            .call("CreateSchema", json!({"name": name, "schema": schema}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a duplicate CreateSchema to answer HTTP 400, got \
             {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceAlreadyExistsException"),
            "case {case:?}: expected ResourceAlreadyExistsException, got {body:?}"
        );
    }

    assert_fails(
        &emulator,
        "DescribeSchema",
        "schemaArn",
        &format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/never-created"),
        400,
        "ResourceNotFoundException",
        "unknown schema",
    )
    .await;
}

#[tokio::test]
async fn create_schema_refuses_a_document_that_is_not_an_avro_record() {
    let emulator = Emulator::new();

    // "The schema you create must be in Avro JSON format", and a Personalize dataset
    // schema is an Avro RECORD declaration.
    let refused: &[(&str, &str)] = &[
        ("not JSON at all", "USER_ID string, ITEM_ID string"),
        ("truncated JSON", "{\"type\": \"record\","),
        ("a JSON array", "[{\"name\": \"USER_ID\"}]"),
        ("a JSON string", "\"Interactions\""),
        (
            "an enum rather than a record",
            "{\"type\": \"enum\", \"name\": \"X\", \"symbols\": [\"A\"]}",
        ),
        (
            "a record with no fields list",
            "{\"type\": \"record\", \"name\": \"Interactions\"}",
        ),
        (
            "a record whose fields are not a list",
            "{\"type\": \"record\", \"name\": \"X\", \"fields\": {\"USER_ID\": \"string\"}}",
        ),
    ];

    for (case, schema) in refused {
        let (status, body) = emulator
            .call(
                "CreateSchema",
                json!({"name": "refused-schema", "schema": schema}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body:?}"
        );
    }

    // And nothing was stored by any of them.
    let arn = format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/refused-schema");
    assert_fails(
        &emulator,
        "DescribeSchema",
        "schemaArn",
        &arn,
        400,
        "ResourceNotFoundException",
        "after every refusal",
    )
    .await;

    // The same name is still free for a well-formed document.
    let created = create_schema(
        &emulator,
        "refused-schema",
        &avro_schema("Interactions"),
        "after every refusal",
    )
    .await;
    assert_eq!(
        created, arn,
        "case \"after every refusal\": expected the name to still be free, got {created}"
    );
}

/// A row of the schema-delete table: a schema, and the datasets that reference it when
/// the delete is attempted.
struct SchemaDeleteRow {
    schema: &'static str,
    /// The dataset types created against this schema inside one group. A group holds at
    /// most one dataset of each type, so the rows name distinct types; the spellings are
    /// the model's own documented set for `CreateDatasetRequest$datasetType`
    /// (`DATASET_TYPE_VALUES` in `src/state.rs`).
    referencing_types: &'static [&'static str],
}

const SCHEMA_DELETE_CATALOG: &[SchemaDeleteRow] = &[
    // Nothing ever referenced it, so the delete succeeds on the first call.
    SchemaDeleteRow {
        schema: "orphan-schema",
        referencing_types: &[],
    },
    // The case the recorded unit cited: exactly one dataset references the schema.
    SchemaDeleteRow {
        schema: "my-interactions",
        referencing_types: &["Interactions"],
    },
    // Several datasets reference one schema, so the refusal has to hold until the LAST
    // reference goes, not just until the first is deleted.
    SchemaDeleteRow {
        schema: "shared_schema_3",
        referencing_types: &["Items", "Users", "Actions"],
    },
];

/// The `message` an awsJson error body carries, or `""`.
pub(crate) fn error_message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Create a dataset of `dataset_type` against `schema_arn` in `group_arn`, and return its
/// ARN.
pub(crate) async fn create_dataset(
    emulator: &Emulator,
    group_arn: &str,
    schema_arn: &str,
    dataset_type: &str,
    case: &str,
) -> String {
    let (status, body) = emulator
        .call(
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
        status, 200,
        "case {case:?}: expected CreateDataset to answer 200, got {status} {body:?}"
    );
    body["datasetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a datasetArn, got {body:?}"))
        .to_string()
}

/// Assert `operation` on `arn` succeeds.
pub(crate) async fn assert_deleted(
    emulator: &Emulator,
    operation: &str,
    member: &str,
    arn: &str,
    case: &str,
) {
    let (status, body) = emulator.call_with_arn(operation, member, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {body:?}"
    );
}

#[tokio::test]
async fn a_schema_is_deleted_only_once_no_dataset_references_it() {
    let emulator = Emulator::new();

    for row in SCHEMA_DELETE_CATALOG {
        let case = format!(
            "schema {} referenced by {:?}",
            row.schema, row.referencing_types
        );
        let group_arn = create_active_group(&emulator, &format!("{}-dsg", row.schema), &case).await;
        let schema_arn =
            create_schema(&emulator, row.schema, &avro_schema("Interactions"), &case).await;

        let mut outstanding: Vec<String> = Vec::new();
        for dataset_type in row.referencing_types {
            outstanding
                .push(create_dataset(&emulator, &group_arn, &schema_arn, dataset_type, &case).await);
        }

        // Every reference has to go before the schema may: while any dataset names it,
        // the delete is refused and changes nothing.
        while let Some(dataset_arn) = outstanding.pop() {
            let (status, body) = emulator
                .call_with_arn("DeleteSchema", "schemaArn", &schema_arn)
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected DeleteSchema to answer HTTP 400 while {} dataset(s) \
                 reference it, got {status} {body:?}",
                outstanding.len() + 1
            );
            assert!(
                error_code(&body).contains("ResourceInUseException"),
                "case {case:?}: expected ResourceInUseException, got {body:?}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, SCHEMA_IN_USE_MESSAGE,
                "case {case:?}: expected the service's fixed in-use wording, got {message:?}"
            );

            // The refused delete left the schema exactly where it was.
            let (status, body) = emulator
                .call_with_arn("DescribeSchema", "schemaArn", &schema_arn)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the refused DeleteSchema to leave the schema \
                 describable, got {status} {body:?}"
            );

            assert_deleted(&emulator, "DeleteDataset", "datasetArn", &dataset_arn, &case).await;
        }

        // With the last reference gone the delete succeeds, and the schema is really gone:
        // a read is refused as not found and a second delete has nothing to find.
        assert_deleted(&emulator, "DeleteSchema", "schemaArn", &schema_arn, &case).await;
        assert_fails(
            &emulator,
            "DescribeSchema",
            "schemaArn",
            &schema_arn,
            400,
            "ResourceNotFoundException",
            &case,
        )
        .await;
        assert_fails(
            &emulator,
            "DeleteSchema",
            "schemaArn",
            &schema_arn,
            400,
            "ResourceNotFoundException",
            &case,
        )
        .await;
    }

    // A dataset blocks only the schema IT references: another schema in the same group,
    // referenced by nothing, still deletes.
    let case = "another schema's dataset";
    let group_arn = create_active_group(&emulator, "cross-reference-dsg", case).await;
    let referenced =
        create_schema(&emulator, "referenced-schema", &avro_schema("Items"), case).await;
    let unreferenced = create_schema(
        &emulator,
        "unreferenced-schema",
        &avro_schema("Items"),
        case,
    )
    .await;
    let dataset_arn = create_dataset(&emulator, &group_arn, &referenced, "Items", case).await;

    assert_deleted(&emulator, "DeleteSchema", "schemaArn", &unreferenced, case).await;
    assert_fails(
        &emulator,
        "DeleteSchema",
        "schemaArn",
        &referenced,
        400,
        "ResourceInUseException",
        case,
    )
    .await;

    // And a schema nobody ever created is not found, whatever datasets exist.
    assert_fails(
        &emulator,
        "DeleteSchema",
        "schemaArn",
        &format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/never-created"),
        400,
        "ResourceNotFoundException",
        case,
    )
    .await;

    assert_deleted(&emulator, "DeleteDataset", "datasetArn", &dataset_arn, case).await;
    assert_deleted(&emulator, "DeleteSchema", "schemaArn", &referenced, case).await;
}

// ---------------------------------------------------------------------------
// Dataset groups
// ---------------------------------------------------------------------------

/// A row of the dataset-group catalog.
struct GroupRow {
    name: &'static str,
    /// From the model's own `Domain` enum set, or `None` for a Custom dataset group.
    domain: Option<&'static str>,
    /// "Supplying an IAM role is only valid when also specifying a KMS key", so the two
    /// are supplied together or not at all.
    encryption: Option<(&'static str, &'static str)>,
}

const GROUP_CATALOG: &[GroupRow] = &[
    // The case the bootstrap issue cited.
    GroupRow {
        name: "my-dsg",
        domain: None,
        encryption: None,
    },
    GroupRow {
        name: "ecommerce-dsg",
        domain: Some("ECOMMERCE"),
        encryption: None,
    },
    GroupRow {
        name: "vod_dsg_2",
        domain: Some("VIDEO_ON_DEMAND"),
        encryption: None,
    },
    GroupRow {
        name: "E",
        domain: Some("ECOMMERCE"),
        encryption: Some((
            "arn:aws:iam::000000000000:role/PersonalizeKeyAccess",
            "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555",
        )),
    },
];

/// The request body a catalog row creates its group with.
fn group_request(row: &GroupRow) -> Value {
    let mut request = json!({"name": row.name});
    if let Some(domain) = row.domain {
        request["domain"] = json!(domain);
    }
    if let Some((role_arn, kms_key_arn)) = row.encryption {
        request["roleArn"] = json!(role_arn);
        request["kmsKeyArn"] = json!(kms_key_arn);
    }
    request
}

#[tokio::test]
async fn a_dataset_group_is_created_pending_becomes_active_and_is_deleted() {
    let emulator = Emulator::new();

    for row in GROUP_CATALOG {
        let case = format!("dataset group {}", row.name);
        let expected_arn =
            format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-group/{}", row.name);

        let (status, body) = emulator
            .call("CreateDatasetGroup", group_request(row))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDatasetGroup to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            body["datasetGroupArn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected the minted dataset-group ARN, got {body:?}"
        );
        assert_eq!(
            body["domain"].as_str(),
            row.domain,
            "case {case:?}: expected the stored domain echoed back, got {body:?}"
        );

        // The create does not hand back a ready group: the first Describe still sees a
        // transient status.
        let (status, body) = emulator
            .call_with_arn("DescribeDatasetGroup", "datasetGroupArn", &expected_arn)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeDatasetGroup to answer 200, got {status} {body:?}"
        );
        let reported = body["datasetGroup"]["status"].as_str().unwrap_or_default();
        assert!(
            NON_TERMINAL_CREATE_STATUSES.contains(&reported),
            "case {case:?}: expected a create still in one of \
             {NON_TERMINAL_CREATE_STATUSES:?} immediately after the create, got {body:?}"
        );

        let group = await_active(
            &emulator,
            "DescribeDatasetGroup",
            "datasetGroupArn",
            "datasetGroup",
            &expected_arn,
            &case,
        )
        .await;
        assert_eq!(
            group["name"].as_str(),
            Some(row.name),
            "case {case:?}: expected the created name, got {group:?}"
        );
        assert_eq!(
            group["domain"].as_str(),
            row.domain,
            "case {case:?}: expected the created domain, got {group:?}"
        );
        assert_eq!(
            group["roleArn"].as_str(),
            row.encryption.map(|(role_arn, _)| role_arn),
            "case {case:?}: expected the created roleArn, got {group:?}"
        );
        assert_eq!(
            group["kmsKeyArn"].as_str(),
            row.encryption.map(|(_, kms_key_arn)| kms_key_arn),
            "case {case:?}: expected the created kmsKeyArn, got {group:?}"
        );
        assert!(
            group["creationDateTime"].is_number() && group["lastUpdatedDateTime"].is_number(),
            "case {case:?}: expected both timestamps populated, got {group:?}"
        );
        assert!(
            group.get("failureReason").is_none_or(Value::is_null),
            "case {case:?}: a successful create has no failureReason, got {group:?}"
        );

        // A second create under the same name is a clash, not an update.
        let (status, body) = emulator
            .call("CreateDatasetGroup", group_request(row))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a duplicate create to answer HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceAlreadyExistsException"),
            "case {case:?}: expected ResourceAlreadyExistsException, got {body:?}"
        );
    }

    // Every group the catalog created is discoverable, and only those.
    let listed = list_all(&emulator, "ListDatasetGroups", "datasetGroups", None, None).await;
    let mut listed_arns: Vec<String> = listed
        .iter()
        .map(|summary| summary["datasetGroupArn"].as_str().unwrap_or("").to_string())
        .collect();
    listed_arns.sort();
    let mut expected: Vec<String> = GROUP_CATALOG
        .iter()
        .map(|row| format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-group/{}", row.name))
        .collect();
    expected.sort();
    assert_eq!(
        listed_arns, expected,
        "case \"ListDatasetGroups\": expected exactly the created groups"
    );

    // Teardown: every group deletes, and is gone from both the Describe and the List.
    for row in GROUP_CATALOG {
        let case = format!("delete dataset group {}", row.name);
        let arn = format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-group/{}", row.name);
        let (status, body) = emulator
            .call_with_arn("DeleteDatasetGroup", "datasetGroupArn", &arn)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteDatasetGroup to answer 200, got {status} {body:?}"
        );
        assert_fails(
            &emulator,
            "DescribeDatasetGroup",
            "datasetGroupArn",
            &arn,
            400,
            "ResourceNotFoundException",
            &case,
        )
        .await;
        assert_fails(
            &emulator,
            "DeleteDatasetGroup",
            "datasetGroupArn",
            &arn,
            400,
            "ResourceNotFoundException",
            &format!("{case} twice"),
        )
        .await;
    }

    let listed = list_all(&emulator, "ListDatasetGroups", "datasetGroups", None, None).await;
    assert!(
        listed.is_empty(),
        "case \"ListDatasetGroups after teardown\": expected no groups, got {listed:?}"
    );
}

#[tokio::test]
async fn create_dataset_group_refuses_a_key_without_a_role_and_a_role_without_a_key() {
    let emulator = Emulator::new();

    let role_arn = "arn:aws:iam::000000000000:role/PersonalizeKeyAccess";
    let kms_key_arn = "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555";

    // The model documents the two members as a pair, in both directions.
    let refused: &[(&str, Value)] = &[
        (
            "roleArn without kmsKeyArn",
            json!({"name": "paired", "roleArn": role_arn}),
        ),
        (
            "kmsKeyArn without roleArn",
            json!({"name": "paired", "kmsKeyArn": kms_key_arn}),
        ),
    ];

    for (case, request) in refused {
        let (status, body) = emulator.call("CreateDatasetGroup", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body:?}"
        );
    }

    // `domain` carries the model's own enum constraint, so a value outside the documented
    // set is refused at the boundary with a `ValidationException` rather than reaching
    // the handler.
    let (status, body) = emulator
        .call("CreateDatasetGroup", json!({"name": "paired", "domain": "MUSIC"}))
        .await;
    assert_eq!(
        status, 400,
        "case \"a domain outside the enum\": expected HTTP 400, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case \"a domain outside the enum\": expected a ValidationException, got {body:?}"
    );

    // The matching pair still creates, and neither refusal stored anything.
    let (status, body) = emulator
        .call(
            "CreateDatasetGroup",
            json!({"name": "paired", "roleArn": role_arn, "kmsKeyArn": kms_key_arn}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the matching pair\": expected 200, got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Datasets
// ---------------------------------------------------------------------------

/// The dataset types Personalize accepts, in the spelling it reports them back in, each
/// paired with a differently cased request value.
///
/// Source: the model's own documentation for `CreateDatasetRequest$datasetType` — "One of
/// the following (case insensitive) values: Interactions, Items, Users, Actions,
/// Action_Interactions". The first row is the case the bootstrap issue cited; the rest are
/// entries it did not, including the lower- and upper-cased spellings the documented
/// case-insensitivity has to accept.
const DATASET_TYPE_CASES: &[(&str, &str)] = &[
    ("Interactions", "Interactions"),
    ("items", "Items"),
    ("USERS", "Users"),
    ("Actions", "Actions"),
    ("action_interactions", "Action_Interactions"),
];

#[tokio::test]
async fn a_dataset_of_every_documented_type_round_trips_through_its_group() {
    let emulator = Emulator::new();

    let group_arn = create_active_group(&emulator, "dataset-types-dsg", "dataset types").await;
    let schema_arn = create_schema(
        &emulator,
        "dataset-types-schema",
        &avro_schema("Interactions"),
        "dataset types",
    )
    .await;

    for (requested, canonical) in DATASET_TYPE_CASES {
        let case = format!("datasetType {requested:?}");
        let (status, body) = emulator
            .call(
                "CreateDataset",
                json!({
                    "name": format!("ds-{}", canonical.to_ascii_lowercase()),
                    "schemaArn": schema_arn,
                    "datasetGroupArn": group_arn,
                    "datasetType": requested,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDataset to answer 200, got {status} {body:?}"
        );
        let arn = body["datasetArn"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            arn,
            format!(
                "arn:aws:personalize:{REGION}:{ACCOUNT}:dataset/dataset-types-dsg/{}",
                canonical.to_ascii_uppercase()
            ),
            "case {case:?}: expected the minted dataset ARN, got {body:?}"
        );

        let dataset = await_active(
            &emulator,
            "DescribeDataset",
            "datasetArn",
            "dataset",
            &arn,
            &case,
        )
        .await;
        assert_eq!(
            dataset["name"].as_str(),
            Some(format!("ds-{}", canonical.to_ascii_lowercase()).as_str()),
            "case {case:?}: expected the created name, got {dataset:?}"
        );
        assert_eq!(
            dataset["datasetType"].as_str(),
            Some(*canonical),
            "case {case:?}: expected the canonical dataset type, got {dataset:?}"
        );
        assert_eq!(
            dataset["datasetGroupArn"].as_str(),
            Some(group_arn.as_str()),
            "case {case:?}: expected the created datasetGroupArn, got {dataset:?}"
        );
        assert_eq!(
            dataset["schemaArn"].as_str(),
            Some(schema_arn.as_str()),
            "case {case:?}: expected the created schemaArn, got {dataset:?}"
        );
        assert!(
            dataset["creationDateTime"].is_number() && dataset["lastUpdatedDateTime"].is_number(),
            "case {case:?}: expected both timestamps populated, got {dataset:?}"
        );

        // A group holds at most one dataset per type, whatever the new dataset is named
        // and however the type is cased.
        let (status, body) = emulator
            .call(
                "CreateDataset",
                json!({
                    "name": "a-different-name",
                    "schemaArn": schema_arn,
                    "datasetGroupArn": group_arn,
                    "datasetType": canonical.to_ascii_uppercase(),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a second dataset of the type to answer HTTP 400, got \
             {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceAlreadyExistsException"),
            "case {case:?}: expected ResourceAlreadyExistsException, got {body:?}"
        );
    }

    // The group now holds one dataset per documented type, and it cannot be deleted while
    // it does.
    let (status, body) = emulator
        .call_with_arn("DeleteDatasetGroup", "datasetGroupArn", &group_arn)
        .await;
    assert_eq!(
        status, 400,
        "case \"delete a group that still holds datasets\": expected HTTP 400, got \
         {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ResourceInUseException"),
        "case \"delete a group that still holds datasets\": expected ResourceInUseException, \
         got {body:?}"
    );

    // Emptying it makes the group deletable, and each dataset is gone for good.
    for (_, canonical) in DATASET_TYPE_CASES {
        let case = format!("delete dataset {canonical}");
        let arn = format!(
            "arn:aws:personalize:{REGION}:{ACCOUNT}:dataset/dataset-types-dsg/{}",
            canonical.to_ascii_uppercase()
        );
        let (status, body) = emulator
            .call_with_arn("DeleteDataset", "datasetArn", &arn)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteDataset to answer 200, got {status} {body:?}"
        );
        assert_fails(
            &emulator,
            "DescribeDataset",
            "datasetArn",
            &arn,
            400,
            "ResourceNotFoundException",
            &case,
        )
        .await;
        assert_fails(
            &emulator,
            "DeleteDataset",
            "datasetArn",
            &arn,
            400,
            "ResourceNotFoundException",
            &format!("{case} twice"),
        )
        .await;
    }

    let (status, body) = emulator
        .call_with_arn("DeleteDatasetGroup", "datasetGroupArn", &group_arn)
        .await;
    assert_eq!(
        status, 200,
        "case \"delete the emptied group\": expected HTTP 200, got {status} {body:?}"
    );
}

#[tokio::test]
async fn create_dataset_refuses_an_unknown_reference_an_unknown_type_and_a_pending_group() {
    let emulator = Emulator::new();

    let group_arn = create_active_group(&emulator, "refusals-dsg", "refusals").await;
    let schema_arn = create_schema(
        &emulator,
        "refusals-schema",
        &avro_schema("Interactions"),
        "refusals",
    )
    .await;
    let missing_group = format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-group/absent");
    let missing_schema = format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/absent");

    let refused: &[(&str, Value, u16, &str)] = &[
        (
            "an unknown datasetGroupArn",
            json!({
                "name": "ds", "datasetType": "Interactions",
                "schemaArn": schema_arn, "datasetGroupArn": missing_group,
            }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "an unknown schemaArn",
            json!({
                "name": "ds", "datasetType": "Interactions",
                "schemaArn": missing_schema, "datasetGroupArn": group_arn,
            }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a dataset type outside the documented set",
            json!({
                "name": "ds", "datasetType": "Clicks",
                "schemaArn": schema_arn, "datasetGroupArn": group_arn,
            }),
            400,
            "InvalidInputException",
        ),
        (
            "a dataset type outside the documented set, upper-cased",
            json!({
                "name": "ds", "datasetType": "INTERACTION",
                "schemaArn": schema_arn, "datasetGroupArn": group_arn,
            }),
            400,
            "InvalidInputException",
        ),
    ];

    for (case, request, expected_status, expected_code) in refused {
        let (status, body) = emulator.call("CreateDataset", request.clone()).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains(expected_code),
            "case {case:?}: expected {expected_code}, got {body:?}"
        );
    }

    // "You must wait until the status of the dataset group is ACTIVE before adding a
    // dataset to the group": a group whose create is still pending refuses one.
    let (status, body) = emulator
        .call("CreateDatasetGroup", json!({"name": "pending-dsg"}))
        .await;
    assert_eq!(
        status, 200,
        "case \"a pending group\": expected CreateDatasetGroup to answer 200, got \
         {status} {body:?}"
    );
    let pending_arn = body["datasetGroupArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, body) = emulator
        .call(
            "CreateDataset",
            json!({
                "name": "too-early", "datasetType": "Interactions",
                "schemaArn": schema_arn, "datasetGroupArn": pending_arn,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"a pending group\": expected HTTP 400, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ResourceInUseException"),
        "case \"a pending group\": expected ResourceInUseException, got {body:?}"
    );

    // Nothing the refusals touched was stored.
    let listed = list_all(&emulator, "ListDatasets", "datasets", None, None).await;
    assert!(
        listed.is_empty(),
        "case \"after every refusal\": expected no datasets, got {listed:?}"
    );

    // Once the group reaches ACTIVE the same request succeeds, so the refusal was about
    // the status and nothing else.
    await_active(
        &emulator,
        "DescribeDatasetGroup",
        "datasetGroupArn",
        "datasetGroup",
        &pending_arn,
        "a pending group",
    )
    .await;
    let (status, body) = emulator
        .call(
            "CreateDataset",
            json!({
                "name": "not-too-early", "datasetType": "Interactions",
                "schemaArn": schema_arn, "datasetGroupArn": pending_arn,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"an active group\": expected HTTP 200, got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Page through `operation` with `max_results`, returning every item it reported.
///
/// Asserts the pagination invariants as it goes: a page never exceeds the requested size,
/// the final page carries no token, and no item is reported twice.
///
/// `pub(crate)` so the `ListSchemas` suite pages through its listing with the SAME
/// invariants rather than re-implementing them.
pub(crate) async fn list_all(
    emulator: &Emulator,
    operation: &str,
    items_member: &str,
    filter: Option<(&str, &str)>,
    max_results: Option<i64>,
) -> Vec<Value> {
    let case = format!("{operation} with maxResults {max_results:?}");
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    for page in 0..100 {
        let mut body = Map::new();
        if let Some((member, value)) = filter {
            body.insert(member.to_string(), Value::String(value.to_string()));
        }
        if let Some(max_results) = max_results {
            body.insert("maxResults".to_string(), json!(max_results));
        }
        if let Some(token) = &next_token {
            body.insert("nextToken".to_string(), Value::String(token.clone()));
        }
        let (status, response) = emulator.call(operation, Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected page {page} to answer 200, got {status} {response:?}"
        );
        let items = response[items_member]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a {items_member} list, got {response:?}"))
            .clone();
        if let Some(max_results) = max_results {
            assert!(
                items.len() as i64 <= max_results,
                "case {case:?}: page {page} returned {} items, more than the requested \
                 {max_results}",
                items.len()
            );
        }
        collected.extend(items);
        next_token = response["nextToken"].as_str().map(ToString::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: pagination did not terminate");
}

#[tokio::test]
async fn list_datasets_filters_by_group_and_pages_at_every_size() {
    let emulator = Emulator::new();

    // Two groups, so the filter has something to exclude: the first holds every
    // documented type, the second holds one.
    let first = create_active_group(&emulator, "paging-dsg-1", "paging").await;
    let second = create_active_group(&emulator, "paging-dsg-2", "paging").await;
    let schema_arn = create_schema(
        &emulator,
        "paging-schema",
        &avro_schema("Interactions"),
        "paging",
    )
    .await;

    let mut expected_first: Vec<String> = Vec::new();
    for (_, canonical) in DATASET_TYPE_CASES {
        let (status, body) = emulator
            .call(
                "CreateDataset",
                json!({
                    "name": format!("first-{}", canonical.to_ascii_lowercase()),
                    "schemaArn": schema_arn,
                    "datasetGroupArn": first,
                    "datasetType": canonical,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case \"paging\": expected CreateDataset to answer 200, got {status} {body:?}"
        );
        expected_first.push(body["datasetArn"].as_str().unwrap_or("").to_string());
    }
    expected_first.sort();

    let (status, body) = emulator
        .call(
            "CreateDataset",
            json!({
                "name": "second-interactions",
                "schemaArn": schema_arn,
                "datasetGroupArn": second,
                "datasetType": "Interactions",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"paging\": expected CreateDataset to answer 200, got {status} {body:?}"
    );
    let expected_second = body["datasetArn"].as_str().unwrap_or("").to_string();

    // `maxResults` is modelled 1..100: every page size inside the range reports exactly
    // the group's datasets, as does an absent `maxResults`, and both published bounds are
    // accepted.
    for max_results in [None, Some(1), Some(2), Some(3), Some(5), Some(100)] {
        let case = format!("maxResults {max_results:?}");
        let listed = list_all(
            &emulator,
            "ListDatasets",
            "datasets",
            Some(("datasetGroupArn", &first)),
            max_results,
        )
        .await;
        let mut arns: Vec<String> = listed
            .iter()
            .map(|summary| summary["datasetArn"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(
            arns.len(),
            expected_first.len(),
            "case {case:?}: expected {} datasets without duplicates, got {arns:?}",
            expected_first.len()
        );
        arns.sort();
        assert_eq!(
            arns, expected_first,
            "case {case:?}: expected exactly the first group's datasets"
        );
        for summary in &listed {
            assert!(
                summary["status"].as_str().is_some_and(|status| {
                    status == "ACTIVE" || NON_TERMINAL_CREATE_STATUSES.contains(&status)
                }),
                "case {case:?}: expected a documented status on every summary, got {summary:?}"
            );
            assert!(
                summary["name"].as_str().is_some_and(|name| name.starts_with("first-")),
                "case {case:?}: expected only the first group's datasets, got {summary:?}"
            );
        }

        // The sibling filter reports the other group's single dataset, and an absent
        // filter reports both groups' datasets.
        let listed = list_all(
            &emulator,
            "ListDatasets",
            "datasets",
            Some(("datasetGroupArn", &second)),
            max_results,
        )
        .await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected the second group's single dataset, got {listed:?}"
        );
        assert_eq!(
            listed[0]["datasetArn"].as_str(),
            Some(expected_second.as_str()),
            "case {case:?}: expected the second group's dataset, got {listed:?}"
        );

        let listed = list_all(&emulator, "ListDatasets", "datasets", None, max_results).await;
        assert_eq!(
            listed.len(),
            expected_first.len() + 1,
            "case {case:?}: expected every dataset when no group filters, got {listed:?}"
        );

        // And the same page sizes drive the sibling listing of the groups themselves.
        let listed = list_all(
            &emulator,
            "ListDatasetGroups",
            "datasetGroups",
            None,
            max_results,
        )
        .await;
        assert_eq!(
            listed.len(),
            2,
            "case {case:?}: expected both dataset groups, got {listed:?}"
        );
    }

    // A `maxResults` outside the modelled 1..100 range is refused on both listings.
    for operation in ["ListDatasets", "ListDatasetGroups"] {
        for max_results in [0, 101, -1] {
            let case = format!("{operation} with maxResults {max_results}");
            let (status, body) = emulator
                .call(operation, json!({"maxResults": max_results}))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();

    let group = create_active_group(&emulator, "token-dsg", "tokens").await;
    let schema_arn = create_schema(
        &emulator,
        "token-schema",
        &avro_schema("Interactions"),
        "tokens",
    )
    .await;
    for (_, canonical) in DATASET_TYPE_CASES {
        let (status, body) = emulator
            .call(
                "CreateDataset",
                json!({
                    "name": format!("tok-{}", canonical.to_ascii_lowercase()),
                    "schemaArn": schema_arn,
                    "datasetGroupArn": group,
                    "datasetType": canonical,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case \"tokens\": expected CreateDataset to answer 200, got {status} {body:?}"
        );
    }

    // The token a real page hands back is the only thing either listing resumes from.
    let (status, body) = emulator
        .call("ListDatasets", json!({"maxResults": 1}))
        .await;
    assert_eq!(
        status, 200,
        "case \"a real page\": expected HTTP 200, got {status} {body:?}"
    );
    let real_token = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"a real page\": expected a nextToken, got {body:?}"))
        .to_string();
    let (status, body) = emulator
        .call("ListDatasetGroups", json!({"maxResults": 1}))
        .await;
    assert_eq!(
        status, 200,
        "case \"a real group page\": expected HTTP 200, got {status} {body:?}"
    );
    let group_token = body["nextToken"].as_str().map(ToString::to_string);

    let refused: &[(&str, &str)] = &[
        ("an arbitrary string", "bogus"),
        ("an ARN pasted in raw", "arn:aws:personalize:us-east-1:000000000000:dataset/token-dsg/ITEMS"),
        ("hex that decodes to no tagged key", "6e6f742d612d746f6b656e"),
        ("the empty string", ""),
        // A token this operation issued, offered to the SIBLING listing.
        ("another listing's token", real_token.as_str()),
    ];

    for (case, token) in refused {
        // `real_token` is valid for ListDatasets, so only the sibling rejects it.
        let operation = if *case == "another listing's token" {
            "ListDatasetGroups"
        } else {
            "ListDatasets"
        };
        let (status, body) = emulator
            .call(operation, json!({"nextToken": token}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} on {operation}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("InvalidNextTokenException"),
            "case {case:?} on {operation}: expected InvalidNextTokenException, got {body:?}"
        );
    }

    // And the real tokens still resume their own listing.
    let (status, body) = emulator
        .call("ListDatasets", json!({"nextToken": real_token}))
        .await;
    assert_eq!(
        status, 200,
        "case \"the real token\": expected HTTP 200, got {status} {body:?}"
    );
    assert_eq!(
        body["datasets"].as_array().map(Vec::len),
        Some(DATASET_TYPE_CASES.len() - 1),
        "case \"the real token\": expected the remaining datasets, got {body:?}"
    );
    if let Some(group_token) = group_token {
        let (status, body) = emulator
            .call("ListDatasets", json!({"nextToken": group_token}))
            .await;
        assert_eq!(
            status, 400,
            "case \"a group token on ListDatasets\": expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("InvalidNextTokenException"),
            "case \"a group token on ListDatasets\": expected InvalidNextTokenException, got \
             {body:?}"
        );
    }
}
