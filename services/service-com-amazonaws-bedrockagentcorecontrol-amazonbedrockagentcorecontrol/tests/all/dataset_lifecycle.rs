// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the dataset lifecycle (`CreateDataset`, `GetDataset`).
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.
//!
//! The suites that exercise an Amazon S3 source mount object storage beside this service
//! on ONE engine — what the assembled emulator does — and store the JSONL file through
//! object storage's own front door, so the examples an ingestion counts are the ones
//! object storage really holds.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as agentcore;

/// The region the tests sign for. Not the emulator's default, so the ARNs the handlers
/// mint and the object-storage endpoint an ingestion reads at are the ones the REQUEST
/// named rather than a constant.
const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The settle span `src/state/datasets.rs` gives the transition out of `CREATING`, plus
/// slack, so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(3_300);

/// Every `DatasetSchemaType` value, from `smithy_query shape=DatasetSchemaType`.
const SCHEMA_TYPES: [&str; 3] = [
    "AGENTCORE_EVALUATION_PREDEFINED_V1",
    "AGENTCORE_EVALUATION_SIMULATED_V1",
    "THIRD_PARTY_EVALUATION_V1",
];

/// A `kmsKeyArn` inside `KmsKeyArn`'s pattern
/// `^arn:aws(|-cn|-us-gov):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// A `clientToken` inside the modelled `ClientToken` constraints (len 33..256,
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`, `smithy_query operation=CreateDataset`).
fn client_token(seed: &str) -> String {
    let token = format!("{seed}{}", "0".repeat(40));
    token[..40].to_string()
}

/// This emulator: the control plane, optionally with object storage mounted beside it on
/// the one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    object_storage: bool,
}

impl Emulator {
    /// Only the control plane: a dataset source that names no object is all this one can
    /// be driven with.
    fn new() -> Self {
        Self::over_engine(false)
    }

    /// The control plane with object storage answering, which is what the assembled
    /// emulator is.
    fn with_object_storage() -> Self {
        Self::over_engine(true)
    }

    fn over_engine(object_storage: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // `default()` registers no authenticator, so the suite exercises routing and the
        // handlers rather than this process's SigV4 keys.
        let mut router = MultiServiceRouter::default();
        router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
        if object_storage {
            router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
                Arc::clone(&engine),
            ));
        }
        Self {
            router,
            object_storage,
        }
    }

    /// One `restJson1` request to the control plane, signed for [`REGION`] — which is
    /// what the router routes on and what the handlers read the ARN Region from.
    async fn call(&self, method: &str, uri: &str, body: Option<&Value>) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
        });
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261029/{REGION}/bedrock-agentcore/\
             aws4_request, SignedHeaders=host, Signature=abc"
        );
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", authorization)
            .header("host", "bedrock-agentcore-control.amazonaws.com");
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request.body(payload).expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    /// `CreateDataset`: `POST /datasets` (`smithy_query operation=CreateDataset`).
    async fn create_dataset(&self, body: &Value) -> Answer {
        self.call("POST", "/datasets", Some(body)).await
    }

    /// `GetDataset`: `GET /datasets/{datasetId}`.
    async fn get_dataset(&self, dataset_id: &str) -> Answer {
        self.call("GET", &format!("/datasets/{dataset_id}"), None).await
    }

    /// `GetDataset` for one `datasetVersion`.
    async fn get_dataset_version(&self, dataset_id: &str, version: &str) -> Answer {
        self.call(
            "GET",
            &format!("/datasets/{dataset_id}?datasetVersion={version}"),
            None,
        )
        .await
    }

    /// One path-style request to object storage's front door. Object storage is a REST
    /// service: the bucket and the key live in the URI and the body is the object's own
    /// bytes, so this is a raw request. The credential scope names `s3`, which is what
    /// the router claims on.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> u16 {
        assert!(
            self.object_storage,
            "an emulator built without object storage cannot be seeded with objects"
        );
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261029/{REGION}/s3/aws4_request, \
             SignedHeaders=host, Signature=abc"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", authorization)
            .body(Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        self.router.dispatch(request).await.status().as_u16()
    }

    /// Store `body` at `key` in `bucket`, creating the bucket first, so an ingestion that
    /// reads that location finds the file there.
    async fn put_object(&self, bucket: &str, key: &str, body: &[u8]) {
        let created = self
            .s3(
                "PUT",
                &format!("/{bucket}"),
                format!(
                    "<CreateBucketConfiguration \
                     xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                     <LocationConstraint>{REGION}</LocationConstraint>\
                     </CreateBucketConfiguration>"
                )
                .as_bytes(),
            )
            .await;
        assert!(
            created == 200 || created == 409,
            "seeding s3://{bucket}: expected 200 or 409 from CreateBucket, got {created}"
        );
        let stored = self.s3("PUT", &format!("/{bucket}/{key}"), body).await;
        assert_eq!(
            stored, 200,
            "seeding s3://{bucket}/{key}: expected 200 from PutObject, got {stored}"
        );
    }
}

/// One answer from the emulator: its status and its JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn of(response: http::Response<Bytes>) -> Self {
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Self { status, body }
    }

    /// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// `DatasetId`'s pattern, `^[a-zA-Z0-9_-]{1,110}$` (`smithy_query shape=DatasetId`),
/// checked by hand because this crate carries no regex engine.
fn is_modelled_dataset_id(id: &str) -> bool {
    (1..=110).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// A `source` carrying `count` inline examples. Each example is a JSON object, which is
/// what `DatasetExampleList`'s `document` member admits (`smithy_query
/// shape=DatasetExampleList`).
fn inline_source(count: usize) -> Value {
    let examples: Vec<Value> = (0..count)
        .map(|index| json!({"input": format!("turn {index}"), "expected": {"score": index}}))
        .collect();
    json!({"inlineExamples": {"examples": examples}})
}

/// Assert one `Timestamp` member's wire form: an epoch-seconds NUMBER naming a real
/// instant. `createdAt` and `updatedAt` target the prelude `Timestamp` with no
/// `@timestampFormat` of their own (`smithy_query operation=GetDataset`), which
/// `restJson1` renders as epoch seconds.
fn assert_epoch_seconds(case: &str, member: &str, value: &Value) {
    let seconds = value.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an epoch-seconds number, got {value:?}")
    });
    assert!(
        seconds > 0.0,
        "case {case:?}: {member} must name a real instant, got {seconds}"
    );
}

/// One create of a dataset with an inline source: what the request carries, and what the
/// stored dataset must report.
struct InlineRow {
    case: &'static str,
    name: &'static str,
    schema_type: &'static str,
    description: Option<&'static str>,
    kms_key_arn: Option<&'static str>,
    tags: Option<Value>,
    examples: usize,
}

/// A dataset is created from the request and read back through `GetDataset`, for every
/// shape of the configuration `CreateDataset` accepts.
///
/// The rows differ along what the rule generalises over: the schema type (every
/// `DatasetSchemaType` value), the optional members the request carries (description,
/// KMS key, tags), and how many examples the inline source yields — one, several, and
/// the modelled maximum of the per-request list.
#[tokio::test]
async fn a_dataset_is_created_and_read_back_for_every_source_and_configuration() {
    let emulator = Emulator::new();

    let rows: [InlineRow; 5] = [
        InlineRow {
            case: "only the required members",
            name: "evalset",
            schema_type: SCHEMA_TYPES[0],
            description: None,
            kms_key_arn: None,
            tags: None,
            examples: 1,
        },
        InlineRow {
            case: "a described simulated dataset",
            name: "simulated_set",
            schema_type: SCHEMA_TYPES[1],
            description: Some("the shared simulation scenarios"),
            kms_key_arn: None,
            tags: None,
            examples: 3,
        },
        InlineRow {
            case: "a third-party dataset encrypted under a customer key",
            name: "third_party_set",
            schema_type: SCHEMA_TYPES[2],
            description: None,
            kms_key_arn: Some(KMS_KEY_ARN),
            tags: None,
            examples: 7,
        },
        InlineRow {
            case: "a tagged dataset carrying every optional member",
            name: "Tagged_set_9",
            schema_type: SCHEMA_TYPES[0],
            description: Some("d"),
            kms_key_arn: Some(KMS_KEY_ARN),
            tags: Some(json!({"team": "eval", "stage": "dev"})),
            examples: 2,
        },
        InlineRow {
            case: "the modelled maximum of inline examples",
            name: "full_set",
            schema_type: SCHEMA_TYPES[1],
            description: None,
            kms_key_arn: None,
            tags: None,
            // `DatasetExampleList` is `length 1..1000` (`smithy_query
            // shape=InlineExamplesSource`), so this is the largest source one
            // request can carry.
            examples: 1_000,
        },
    ];

    // The datasets the table creates, so the clock-owed transition is waited out ONCE
    // after the loop rather than per row.
    let mut settling: Vec<(&str, String, Value)> = Vec::new();

    for row in &rows {
        let case = row.case;
        let mut body = json!({
            "datasetName": row.name,
            "schemaType": row.schema_type,
            "source": inline_source(row.examples),
        });
        if let Some(description) = row.description {
            body["description"] = json!(description);
        }
        if let Some(kms_key_arn) = row.kms_key_arn {
            body["kmsKeyArn"] = json!(kms_key_arn);
        }
        if let Some(tags) = row.tags.clone() {
            body["tags"] = tags;
        }

        let created = emulator.create_dataset(&body).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected HTTP 202 from CreateDataset, got {} and body {}",
            created.status, created.body
        );
        let id = created.text("datasetId");
        assert!(
            is_modelled_dataset_id(&id),
            "case {case:?}: expected a datasetId matching ^[a-zA-Z0-9_-]{{1,110}}$, got {id:?}"
        );
        assert!(
            id.starts_with(&format!("{}-", row.name)),
            "case {case:?}: expected the datasetId to carry the name it was created under, \
             got {id:?}"
        );
        assert_eq!(
            created.text("datasetArn"),
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:dataset/{id}"),
            "case {case:?}: expected the modelled DatasetArn for {id}, got {}",
            created.text("datasetArn")
        );
        assert_eq!(
            created.text("status"),
            "CREATING",
            "case {case:?}: a dataset is always CREATING immediately after the call, got {}",
            created.text("status")
        );
        assert_epoch_seconds(case, "createdAt", &created.body["createdAt"]);

        let read = emulator.get_dataset(&id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetDataset, got {} and body {}",
            read.status, read.body
        );
        assert_eq!(
            read.text("datasetName"),
            row.name,
            "case {case:?}: the datasetName reads back as sent, got {}",
            read.text("datasetName")
        );
        assert_eq!(
            read.text("datasetId"),
            id,
            "case {case:?}: GetDataset reports the id the create minted, got {}",
            read.text("datasetId")
        );
        assert_eq!(
            read.text("datasetArn"),
            created.text("datasetArn"),
            "case {case:?}: GetDataset reports the ARN CreateDataset handed back, got {}",
            read.text("datasetArn")
        );
        assert_eq!(
            read.text("schemaType"),
            row.schema_type,
            "case {case:?}: the schemaType reads back as sent, got {}",
            read.text("schemaType")
        );
        assert_eq!(
            read.body["description"],
            row.description.map_or(Value::Null, |text| json!(text)),
            "case {case:?}: the description reads back as sent, got {}",
            read.body["description"]
        );
        assert_eq!(
            read.body["kmsKeyArn"],
            row.kms_key_arn.map_or(Value::Null, |arn| json!(arn)),
            "case {case:?}: the kmsKeyArn reads back as sent, got {}",
            read.body["kmsKeyArn"]
        );
        assert_eq!(
            read.body["tags"],
            row.tags.clone().unwrap_or(Value::Null),
            "case {case:?}: the tags read back as sent, got {}",
            read.body["tags"]
        );
        assert_eq!(
            read.body["exampleCount"],
            json!(row.examples),
            "case {case:?}: expected the {} ingested examples to be counted, got {}",
            row.examples,
            read.body["exampleCount"]
        );
        assert_eq!(
            read.text("datasetVersion"),
            "DRAFT",
            "case {case:?}: a read naming no version resolves to DRAFT, got {}",
            read.text("datasetVersion")
        );
        assert_eq!(
            read.text("draftStatus"),
            "MODIFIED",
            "case {case:?}: a DRAFT with no published version is MODIFIED, got {}",
            read.text("draftStatus")
        );
        assert_eq!(
            read.text("status"),
            "CREATING",
            "case {case:?}: the dataset is still CREATING before its settle span, got {}",
            read.text("status")
        );
        assert!(
            read.body["failureReason"].is_null(),
            "case {case:?}: a dataset that has not failed reports no failureReason, got {}",
            read.body["failureReason"]
        );
        assert!(
            read.body["downloadUrl"].is_null() && read.body["downloadUrlExpiresAt"].is_null(),
            "case {case:?}: no consolidated file exists, so no downloadUrl is reported, got {} \
             and {}",
            read.body["downloadUrl"],
            read.body["downloadUrlExpiresAt"]
        );
        assert_eq!(
            read.body["updatedAt"], read.body["createdAt"],
            "case {case:?}: expected createdAt and updatedAt to be the one create instant, got \
             {} and {}",
            read.body["createdAt"], read.body["updatedAt"]
        );
        for member in ["createdAt", "updatedAt"] {
            assert_epoch_seconds(case, member, &read.body[member]);
        }

        settling.push((case, id, read.body["createdAt"].clone()));
    }

    // The transition out of CREATING is the clock's: it is due at the settle instant
    // whether or not anybody reads the dataset.
    tokio::time::sleep(PAST_SETTLE).await;
    for (case, id, created_at) in settling {
        let read = emulator.get_dataset(&id).await;
        assert_eq!(
            read.text("status"),
            "ACTIVE",
            "case {case:?}: an ingested dataset is ACTIVE once its settle span has passed, \
             got {} and body {}",
            read.text("status"),
            read.body
        );
        assert_eq!(
            read.body["createdAt"], created_at,
            "case {case:?}: a read does not re-date the dataset, got {} against {created_at}",
            read.body["createdAt"]
        );
    }
}

/// What `CreateDataset` refuses, and with which modelled error: a name this account
/// already holds. A refused create persists nothing.
#[tokio::test]
async fn create_dataset_refuses_a_name_the_account_already_holds() {
    let emulator = Emulator::new();

    let first = emulator
        .create_dataset(&json!({
            "datasetName": "taken",
            "schemaType": SCHEMA_TYPES[0],
            "source": inline_source(2),
        }))
        .await;
    assert_eq!(
        first.status, 202,
        "the first create of a name succeeds, got {} and body {}",
        first.status, first.body
    );
    let taken_id = first.text("datasetId");

    // (case, the request body, expected status, expected error code)
    let rows: [(&str, Value, u16, &str); 4] = [
        (
            "the same name again",
            json!({
                "datasetName": "taken",
                "schemaType": SCHEMA_TYPES[0],
                "source": inline_source(1),
            }),
            409,
            "ConflictException",
        ),
        (
            "the same name with another schema type",
            json!({
                "datasetName": "taken",
                "schemaType": SCHEMA_TYPES[2],
                "source": inline_source(1),
            }),
            409,
            "ConflictException",
        ),
        (
            "a name that differs only in case is its own dataset",
            json!({
                "datasetName": "Taken",
                "schemaType": SCHEMA_TYPES[0],
                "source": inline_source(1),
            }),
            202,
            "",
        ),
        (
            "an inline source carrying no example",
            json!({
                "datasetName": "empty_source",
                "schemaType": SCHEMA_TYPES[0],
                "source": {"inlineExamples": {"examples": []}},
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, body, expected_status, expected_code) in rows {
        let answered = emulator.create_dataset(&body).await;
        assert_eq!(
            answered.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} and body {}",
            answered.status, answered.body
        );
        if expected_status == 202 {
            continue;
        }
        assert_eq!(
            answered.error_code(),
            expected_code,
            "case {case:?}: expected {expected_code}, got {} and body {}",
            answered.error_code(),
            answered.body
        );
    }

    // The refused creates persisted nothing, and the dataset the name holds is still the
    // one the first create minted.
    let read = emulator.get_dataset(&taken_id).await;
    assert_eq!(
        read.status, 200,
        "the dataset the first create minted is still readable, got {} and body {}",
        read.status, read.body
    );
    assert_eq!(
        read.body["exampleCount"],
        json!(2),
        "a refused create does not overwrite the stored dataset, got {}",
        read.body["exampleCount"]
    );
}

/// A `clientToken` replayed against `CreateDataset` answers with the dataset it first
/// produced, and an id or a version naming nothing is the modelled 404.
#[tokio::test]
async fn a_replayed_client_token_mints_one_dataset_and_unknown_identifiers_are_not_found() {
    let emulator = Emulator::new();

    let token = client_token("idem");
    let body = json!({
        "datasetName": "once",
        "schemaType": SCHEMA_TYPES[0],
        "source": inline_source(4),
        "clientToken": token,
    });
    let first = emulator.create_dataset(&body).await;
    assert_eq!(
        first.status, 202,
        "the first create succeeds, got {} and body {}",
        first.status, first.body
    );
    let id = first.text("datasetId");

    // (case, the replayed body)
    let rows: [(&str, Value); 2] = [
        ("the same request again", body.clone()),
        (
            "the same token with other members changed",
            json!({
                "datasetName": "once",
                "schemaType": SCHEMA_TYPES[0],
                "source": inline_source(9),
                "description": "changed",
                "clientToken": token,
            }),
        ),
    ];

    for (case, replayed) in rows {
        let answered = emulator.create_dataset(&replayed).await;
        assert_eq!(
            answered.status, 202,
            "case {case:?}: a replayed token is not an error, got {} and body {}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.text("datasetId"),
            id,
            "case {case:?}: a replayed token answers with the first dataset, got {}",
            answered.text("datasetId")
        );
    }
    let read = emulator.get_dataset(&id).await;
    assert_eq!(
        read.body["exampleCount"],
        json!(4),
        "a replayed token ingests nothing a second time, got {}",
        read.body["exampleCount"]
    );

    // A different token under the same name is a second create of a held name.
    let conflicted = emulator
        .create_dataset(&json!({
            "datasetName": "once",
            "schemaType": SCHEMA_TYPES[0],
            "source": inline_source(1),
            "clientToken": client_token("other"),
        }))
        .await;
    assert_eq!(
        conflicted.status, 409,
        "a fresh token does not bypass the unique-name rule, got {} and body {}",
        conflicted.status, conflicted.body
    );

    // (case, the dataset read, the version it names, expected status)
    let reads: [(&str, String, Option<&str>, u16); 5] = [
        ("the minted id", id.clone(), None, 200),
        ("the minted id at DRAFT", id.clone(), Some("DRAFT"), 200),
        (
            "a version nothing has published",
            id.clone(),
            Some("1"),
            404,
        ),
        ("version zero", id.clone(), Some("0"), 404),
        (
            "an id no dataset holds",
            "no_such_dataset".to_string(),
            None,
            404,
        ),
    ];

    for (case, dataset_id, version, expected_status) in reads {
        let answered = match version {
            Some(version) => emulator.get_dataset_version(&dataset_id, version).await,
            None => emulator.get_dataset(&dataset_id).await,
        };
        assert_eq!(
            answered.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from GetDataset, got {} and body {}",
            answered.status, answered.body
        );
        if expected_status == 404 {
            assert_eq!(
                answered.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {} and body {}",
                answered.error_code(),
                answered.body
            );
        }
    }
}

/// One create whose source is a JSONL file in object storage: what the file holds, and
/// how many examples the ingestion must count.
struct S3Row {
    case: &'static str,
    name: &'static str,
    /// The bytes stored at the key the source names, when object storage holds the file.
    file: Option<String>,
    /// The examples the ingestion is expected to yield, or `None` when it must fail.
    expected: Option<i64>,
}

/// A JSONL line nesting a scalar inside `arrays` arrays, so the value's deepest level is
/// `arrays + 1` — the depth an ingestion measures.
fn nested_line(arrays: usize) -> String {
    format!("{}{}{}\n", "[".repeat(arrays), "1", "]".repeat(arrays))
}

/// The bucket the suite's JSONL files live in, inside `S3Uri`'s pattern
/// `^s3://[a-z0-9][a-z0-9.\-]{1,61}[a-z0-9]/.{1,1024}$` (`smithy_query shape=S3Source`).
const BUCKET: &str = "agentcore-datasets";

/// An Amazon S3 source is INGESTED out of this emulator's own object storage: the
/// examples the dataset reports are the ones the file holds, and a file that yields none
/// settles the dataset in `CREATE_FAILED` with the reason it failed.
///
/// The rows differ along what the rule generalises over: how many examples the file
/// holds, whether blank lines separate them, the JSON form of a line (an object, a
/// string), a file object storage holds nothing for, a file that is not JSONL, and an
/// empty file.
#[tokio::test]
async fn an_s3_source_is_ingested_out_of_object_storage() {
    let rows: [S3Row; 9] = [
        S3Row {
            case: "one example",
            name: "s3_one",
            file: Some("{\"input\":\"a\"}\n".to_string()),
            expected: Some(1),
        },
        S3Row {
            case: "several examples",
            name: "s3_many",
            file: Some("{\"input\":\"a\"}\n{\"input\":\"b\"}\n{\"input\":\"c\"}\n".to_string()),
            expected: Some(3),
        },
        S3Row {
            case: "blank lines between examples and no trailing newline",
            name: "s3_blank_lines",
            file: Some("{\"input\":\"a\"}\n\n{\"input\":\"b\"}".to_string()),
            expected: Some(2),
        },
        S3Row {
            case: "an example that is a JSON string rather than an object",
            name: "s3_scalar",
            file: Some("\"a single turn\"\n{\"input\":\"b\"}\n".to_string()),
            expected: Some(2),
        },
        S3Row {
            case: "an example nested to the depth an ingestion reads",
            name: "s3_nested",
            // 64 levels: the bound `src/state/datasets.rs` states for one example.
            file: Some(nested_line(63)),
            expected: Some(1),
        },
        S3Row {
            case: "an example nested one level past the depth an ingestion reads",
            name: "s3_too_deep",
            file: Some(nested_line(64)),
            expected: None,
        },
        S3Row {
            case: "a file object storage holds nothing for",
            name: "s3_absent",
            file: None,
            expected: None,
        },
        S3Row {
            case: "a line that is not JSON",
            name: "s3_not_json",
            file: Some("{\"input\":\"a\"}\nnot json at all\n".to_string()),
            expected: None,
        },
        S3Row {
            case: "an empty file",
            name: "s3_empty",
            file: Some("\n  \n".to_string()),
            expected: None,
        },
    ];

    let emulator = Emulator::with_object_storage();
    // The datasets the table creates, so the clock-owed transition is waited out ONCE
    // after the loop.
    let mut settling: Vec<(&str, String, Option<i64>)> = Vec::new();

    for row in &rows {
        let case = row.case;
        let key = format!("{}/examples.jsonl", row.name);
        if let Some(file) = &row.file {
            emulator.put_object(BUCKET, &key, file.as_bytes()).await;
        }

        let created = emulator
            .create_dataset(&json!({
                "datasetName": row.name,
                "schemaType": SCHEMA_TYPES[0],
                "source": {"s3Source": {"s3Uri": format!("s3://{BUCKET}/{key}")}},
            }))
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected HTTP 202 from CreateDataset, got {} and body {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("status"),
            "CREATING",
            "case {case:?}: a dataset is always CREATING immediately after the call, got {}",
            created.text("status")
        );
        settling.push((case, created.text("datasetId"), row.expected));
    }

    tokio::time::sleep(PAST_SETTLE).await;
    for (case, id, expected) in settling {
        let read = emulator.get_dataset(&id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetDataset, got {} and body {}",
            read.status, read.body
        );
        match expected {
            Some(count) => {
                assert_eq!(
                    read.text("status"),
                    "ACTIVE",
                    "case {case:?}: an ingested dataset settles ACTIVE, got {} and body {}",
                    read.text("status"),
                    read.body
                );
                assert_eq!(
                    read.body["exampleCount"],
                    json!(count),
                    "case {case:?}: expected {count} examples read out of the file, got {}",
                    read.body["exampleCount"]
                );
                assert!(
                    read.body["failureReason"].is_null(),
                    "case {case:?}: an ingested dataset reports no failureReason, got {}",
                    read.body["failureReason"]
                );
            }
            None => {
                assert_eq!(
                    read.text("status"),
                    "CREATE_FAILED",
                    "case {case:?}: a source that yielded nothing settles CREATE_FAILED, got {} \
                     and body {}",
                    read.text("status"),
                    read.body
                );
                assert_eq!(
                    read.body["exampleCount"],
                    json!(0),
                    "case {case:?}: a failed ingestion leaves no examples, got {}",
                    read.body["exampleCount"]
                );
                assert!(
                    !read.text("failureReason").is_empty(),
                    "case {case:?}: a failed ingestion reports why, got {}",
                    read.body["failureReason"]
                );
            }
        }
    }
}

/// An emulator assembled WITHOUT object storage cannot read a caller's JSONL file, so an
/// Amazon S3 source ingests nothing: the dataset exists, holds no example and settles in
/// `CREATE_FAILED` rather than reporting a success nothing backs.
#[tokio::test]
async fn an_s3_source_ingests_nothing_without_object_storage() {
    let case = "no object storage mounted";
    let emulator = Emulator::new();

    let created = emulator
        .create_dataset(&json!({
            "datasetName": "unreadable",
            "schemaType": SCHEMA_TYPES[0],
            "source": {"s3Source": {"s3Uri": format!("s3://{BUCKET}/unreadable.jsonl")}},
        }))
        .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: expected HTTP 202 from CreateDataset, got {} and body {}",
        created.status, created.body
    );
    let id = created.text("datasetId");

    tokio::time::sleep(PAST_SETTLE).await;
    let read = emulator.get_dataset(&id).await;
    assert_eq!(
        read.text("status"),
        "CREATE_FAILED",
        "case {case:?}: a source nothing could read settles CREATE_FAILED, got {} and body {}",
        read.text("status"),
        read.body
    );
    assert_eq!(
        read.body["exampleCount"],
        json!(0),
        "case {case:?}: a failed ingestion leaves no examples, got {}",
        read.body["exampleCount"]
    );
    assert!(
        !read.text("failureReason").is_empty(),
        "case {case:?}: a failed ingestion reports why, got {}",
        read.body["failureReason"]
    );
}
