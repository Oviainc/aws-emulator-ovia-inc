// SPDX-License-Identifier: CC0-1.0

//! The ingestion-job slice, end to end over HTTP: `StartIngestionJob` persists
//! a job for one data source of one knowledge base and `GetIngestionJob`
//! reports that stored record, with whatever status the clock has made due.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather
//! than calling a handler directly.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    INGESTION_JOB_RUN_SPAN, INGESTION_JOB_START_SPAN,
};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// Every member of the modelled `IngestionJobStatistics` structure, each of
/// which the model defaults to 0.
///
/// Source: the `IngestionJobStatistics` shape of this service's Smithy model
/// (`smithy_query(query="shape",shape="IngestionJobStatistics")`).
const STATISTICS_MEMBERS: [&str; 8] = [
    "numberOfDocumentsScanned",
    "numberOfMetadataDocumentsScanned",
    "numberOfNewDocumentsIndexed",
    "numberOfModifiedDocumentsIndexed",
    "numberOfMetadataDocumentsModified",
    "numberOfDocumentsDeleted",
    "numberOfDocumentsFailed",
    "numberOfDocumentsSkipped",
];

/// A margin over a span the clock owes, so a test that waits for a transition
/// is not racing the instant it comes due.
const MARGIN: Duration = Duration::from_millis(100);

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    /// A member of the response's `ingestionJob`.
    fn job(&self, member: &str) -> &Value {
        &self.body["ingestionJob"][member]
    }

    fn job_string(&self, member: &str) -> String {
        self.job(member)
            .as_str()
            .unwrap_or_else(|| {
                panic!("ingestionJob.{member} is not a string in {:?}", self.body)
            })
            .to_string()
    }

    /// A timestamp member, as the epoch seconds the restJson1 `date-time`
    /// binding puts on the wire.
    fn job_instant(&self, member: &str) -> f64 {
        self.job(member)
            .as_f64()
            .unwrap_or_else(|| panic!("ingestionJob.{member} is not a number in {:?}", self.body))
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // The knowledge bases here bind to an S3 Vectors index, which
        // `CreateKnowledgeBase` resolves in the service that owns it — so the
        // index is created first, as a caller creates it first against AWS.
        crate::vector_store_fixture::seed_index(
            &engine,
            "arn:aws:s3vectors:::bucket/ingestion-demo-vectors",
            "ingestion-demo-index",
            crate::vector_store_fixture::EMBEDDING_DIMENSION,
        );
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let bytes = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bedrock-agent.eu-west-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|e| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        let error_type = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// Create a knowledge base and answer its minted `knowledgeBaseId`.
    async fn knowledge_base(&self, name: &str) -> String {
        let request = json!({
            "name": name,
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": {
                "type": "VECTOR",
                "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
            },
            "storageConfiguration": {
                "type": "S3_VECTORS",
                "s3VectorsConfiguration": {
                    "vectorBucketArn": "arn:aws:s3vectors:::bucket/ingestion-demo-vectors",
                    "indexName": "ingestion-demo-index",
                },
            },
        });
        let created = self.call("PUT", "/knowledgebases/", Some(request)).await;
        assert_eq!(
            created.status, 202,
            "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.body["knowledgeBase"]["knowledgeBaseId"]
            .as_str()
            .expect("a minted knowledgeBaseId")
            .to_string()
    }

    /// Attach a data source to `knowledge_base_id` and answer its minted
    /// `dataSourceId`.
    async fn data_source(&self, knowledge_base_id: &str, request: Value) -> String {
        let created = self
            .call(
                "PUT",
                &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
                Some(request),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case setup: expected CreateDataSource -> 200, got {created:?}"
        );
        created.body["dataSource"]["dataSourceId"]
            .as_str()
            .expect("a minted dataSourceId")
            .to_string()
    }

    async fn start_ingestion_job(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        request: Value,
    ) -> Answer {
        self.call(
            "PUT",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/ingestionjobs/"
            ),
            Some(request),
        )
        .await
    }

    async fn get_ingestion_job(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        ingestion_job_id: &str,
    ) -> Answer {
        self.call(
            "GET",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                 /ingestionjobs/{ingestion_job_id}"
            ),
            None,
        )
        .await
    }
}

/// An S3 data source request: the kind the retrieve-and-generate scenario
/// ingests from.
fn s3_data_source(name: &str) -> Value {
    json!({
        "name": name,
        "dataSourceConfiguration": {
            "type": "S3",
            "s3Configuration": { "bucketArn": "arn:aws:s3:::ingestion-demo-docs" },
        },
    })
}

/// A CUSTOM data source request: a kind whose documents are pushed in rather
/// than crawled.
fn custom_data_source(name: &str) -> Value {
    json!({
        "name": name,
        "dataSourceConfiguration": { "type": "CUSTOM" },
    })
}

/// Assert `id` is shaped as the model's `Id` demands (`^[0-9a-zA-Z]{10}$`).
fn assert_well_formed_id(case: &str, member: &str, id: &str) {
    assert_eq!(
        id.len(),
        10,
        "case {case}: expected a 10-character {member}, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case}: expected {member} to match ^[0-9a-zA-Z]{{10}}$, got {id:?}"
    );
}

/// `StartIngestionJob` persists a job for the data source the URI names and
/// `GetIngestionJob` reports that record back — over a table of requests that
/// differ in the kind of data source being ingested, which optional members the
/// request carries and the form of the description.
#[tokio::test]
async fn started_ingestion_jobs_round_trip_through_get_ingestion_job() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: the kind of the data
    // source being ingested, and which optional members the start request
    // carries (`description`, the `@idempotencyToken`, both, neither).
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "an S3 data source, started with only its required members",
            s3_data_source("s3-documents"),
            json!({}),
        ),
        (
            "an S3 data source, started with a description",
            s3_data_source("S3_documents1"),
            json!({ "description": "the nightly crawl of the demo bucket" }),
        ),
        (
            "a CUSTOM data source, started with a description and a client token",
            custom_data_source("custom_documents"),
            json!({
                "description": "d",
                "clientToken": "ingestion-token-for-the-custom-source",
            }),
        ),
        (
            "a CUSTOM data source, started with a client token only",
            custom_data_source("custom-documents-2"),
            json!({ "clientToken": "IngestionTokenForTheSecondSource000" }),
        ),
        (
            "a REDSHIFT_METADATA data source, started with a 200-character description",
            json!({
                "name": "redshift-metadata-0",
                "dataSourceConfiguration": { "type": "REDSHIFT_METADATA" },
            }),
            json!({ "description": "d".repeat(200) }),
        ),
    ];

    for (index, (case, data_source_request, request)) in cases.into_iter().enumerate() {
        let knowledge_base_id = emulator.knowledge_base(&format!("ingestion-kb-{index}")).await;
        let data_source_id = emulator
            .data_source(&knowledge_base_id, data_source_request)
            .await;

        let started = emulator
            .start_ingestion_job(&knowledge_base_id, &data_source_id, request.clone())
            .await;
        assert_eq!(
            started.status, 202,
            "case {case}: expected StartIngestionJob -> 202, got {started:?}"
        );

        let ingestion_job_id = started.job_string("ingestionJobId");
        assert_well_formed_id(case, "ingestionJobId", &ingestion_job_id);
        assert_eq!(
            started.job_string("knowledgeBaseId"),
            knowledge_base_id,
            "case {case}: the job belongs to the knowledge base the URI named, got {:?}",
            started.job("knowledgeBaseId")
        );
        assert_eq!(
            started.job_string("dataSourceId"),
            data_source_id,
            "case {case}: the job belongs to the data source the URI named, got {:?}",
            started.job("dataSourceId")
        );
        assert_eq!(
            started.job_string("status"),
            "STARTING",
            "case {case}: a job this call accepted has only started, got {started:?}"
        );
        assert_eq!(
            started.job("description"),
            request.get("description").unwrap_or(&Value::Null),
            "case {case}: expected the description the request supplied, got {:?}",
            started.job("description")
        );
        assert!(
            started.job("startedAt").is_number() && started.job("updatedAt").is_number(),
            "case {case}: startedAt/updatedAt must be populated, got {:?}",
            started.body
        );
        assert!(
            started.job("textReadyAt").is_null(),
            "case {case}: no text is queryable before the job completes, got {:?}",
            started.job("textReadyAt")
        );
        assert!(
            started.job("failureReasons").is_null(),
            "case {case}: nothing has failed on a job just started, got {:?}",
            started.job("failureReasons")
        );
        // The store holds no documents for a data source, so every count of
        // what was ingested is the 0 the model defaults it to.
        for member in STATISTICS_MEMBERS {
            assert_eq!(
                started.body["ingestionJob"]["statistics"][member].as_i64(),
                Some(0),
                "case {case}: expected statistics.{member} to be 0, got {:?}",
                started.body["ingestionJob"]["statistics"][member]
            );
        }

        // The round trip: the stored record, read back through
        // GetIngestionJob.
        let fetched = emulator
            .get_ingestion_job(&knowledge_base_id, &data_source_id, &ingestion_job_id)
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetIngestionJob -> 200, got {fetched:?}"
        );
        assert_eq!(
            fetched.body["ingestionJob"], started.body["ingestionJob"],
            "case {case}: GetIngestionJob must report the job exactly as \
             StartIngestionJob persisted it, got {:?}",
            fetched.body
        );

        // A read changes nothing a later caller sees.
        let again = emulator
            .get_ingestion_job(&knowledge_base_id, &data_source_id, &ingestion_job_id)
            .await;
        assert_eq!(
            again.body, fetched.body,
            "case {case}: a read must not change what a later caller sees"
        );
    }
}

/// A job's status is owed by the CLOCK: `StartIngestionJob` reports `STARTING`,
/// and the stored record advances to `IN_PROGRESS` and then to `COMPLETE` —
/// whose instant is also when its text became queryable — with no further call
/// moving it.
#[tokio::test]
async fn an_ingestion_job_advances_on_the_clock_to_complete() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("clock-kb").await;
    let data_source_id = emulator
        .data_source(&knowledge_base_id, s3_data_source("clock-source"))
        .await;

    let started = emulator
        .start_ingestion_job(&knowledge_base_id, &data_source_id, json!({}))
        .await;
    assert_eq!(
        started.status, 202,
        "case start: expected StartIngestionJob -> 202, got {started:?}"
    );
    let ingestion_job_id = started.job_string("ingestionJobId");
    let started_at = started.job_instant("startedAt");

    // The transient status is observable: a caller reading back immediately
    // sees the job still starting.
    let starting = emulator
        .get_ingestion_job(&knowledge_base_id, &data_source_id, &ingestion_job_id)
        .await;
    assert_eq!(
        starting.job_string("status"),
        "STARTING",
        "case read during start-up: expected STARTING, got {starting:?}"
    );

    tokio::time::sleep(INGESTION_JOB_START_SPAN + MARGIN).await;
    let running = emulator
        .get_ingestion_job(&knowledge_base_id, &data_source_id, &ingestion_job_id)
        .await;
    assert_eq!(
        running.job_string("status"),
        "IN_PROGRESS",
        "case read after the start span: expected IN_PROGRESS, got {running:?}"
    );
    assert_eq!(
        running.job_instant("startedAt"),
        started_at,
        "case read after the start span: startedAt is the instant the job started, \
         got {running:?}"
    );
    assert!(
        running.job_instant("updatedAt") > started_at,
        "case read after the start span: expected updatedAt past startedAt {started_at}, \
         got {running:?}"
    );
    assert!(
        running.job("textReadyAt").is_null(),
        "case read after the start span: no text is queryable before the job completes, \
         got {running:?}"
    );

    tokio::time::sleep(INGESTION_JOB_RUN_SPAN + MARGIN).await;
    let mut previous: Option<Value> = None;
    for read in ["first", "second"] {
        let complete = emulator
            .get_ingestion_job(&knowledge_base_id, &data_source_id, &ingestion_job_id)
            .await;
        assert_eq!(
            complete.job_string("status"),
            "COMPLETE",
            "case read {read} after the run span: expected COMPLETE, got {complete:?}"
        );
        assert_eq!(
            complete.job("textReadyAt"),
            complete.job("updatedAt"),
            "case read {read} after the run span: the job's text became queryable when it \
             completed, got {complete:?}"
        );
        if let Some(previous) = previous.as_ref() {
            assert_eq!(
                &complete.body["ingestionJob"], previous,
                "case read {read} after the run span: a terminal job is stable across reads"
            );
        }
        previous = Some(complete.body["ingestionJob"].clone());
    }
}

/// An ingestion job is identified by the knowledge base and data source it ran
/// for as well as its own id: `StartIngestionJob` answers
/// `ResourceNotFoundException` (404) for a knowledge base or data source the
/// store does not hold, and `GetIngestionJob` does the same for a job id
/// nothing holds or one that ran for another data source.
#[tokio::test]
async fn ingestion_jobs_are_scoped_to_the_data_source_that_ran_them() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("scope-kb").await;
    let first_source = emulator
        .data_source(&knowledge_base_id, s3_data_source("scope-source-1"))
        .await;
    let second_source = emulator
        .data_source(&knowledge_base_id, custom_data_source("scope-source-2"))
        .await;

    // Rows: a well-formed knowledgeBaseId the store does not hold, and a
    // well-formed dataSourceId the knowledge base does not hold.
    let refusals: Vec<(&str, &str, &str)> = vec![
        (
            "a knowledge base nothing holds",
            "0123456789",
            first_source.as_str(),
        ),
        (
            "a data source the knowledge base does not hold",
            knowledge_base_id.as_str(),
            "aZ09aZ09aZ",
        ),
    ];
    for (case, knowledge_base, data_source) in refusals {
        let refused = emulator
            .start_ingestion_job(knowledge_base, data_source, json!({}))
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case}: expected StartIngestionJob -> 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {refused:?}"
        );
    }

    let started = emulator
        .start_ingestion_job(&knowledge_base_id, &first_source, json!({}))
        .await;
    assert_eq!(
        started.status, 202,
        "case setup: expected StartIngestionJob -> 202, got {started:?}"
    );
    let ingestion_job_id = started.job_string("ingestionJobId");

    // Rows: an id nothing holds, one differing from the stored id in its last
    // character only, and the stored id read through the OTHER data source.
    let mut nearly = ingestion_job_id.clone();
    nearly.pop();
    nearly.push(if ingestion_job_id.ends_with('X') { 'Y' } else { 'X' });
    let reads: Vec<(&str, &str, &str)> = vec![
        ("an id nothing holds", first_source.as_str(), "0123456789"),
        (
            "an id differing in its last character",
            first_source.as_str(),
            nearly.as_str(),
        ),
        (
            "the stored id read through another data source",
            second_source.as_str(),
            ingestion_job_id.as_str(),
        ),
    ];
    for (case, data_source, requested) in reads {
        let fetched = emulator
            .get_ingestion_job(&knowledge_base_id, data_source, requested)
            .await;
        assert_eq!(
            fetched.status, 404,
            "case {case}: expected GetIngestionJob -> 404, got {fetched:?}"
        );
        assert_eq!(
            fetched.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {fetched:?}"
        );
    }

    // The job that does exist is still readable through its own data source.
    let fetched = emulator
        .get_ingestion_job(&knowledge_base_id, &first_source, &ingestion_job_id)
        .await;
    assert_eq!(
        fetched.status, 200,
        "case the stored job: expected GetIngestionJob -> 200, got {fetched:?}"
    );
}

/// A data source is ingested by one job at a time: a repeated
/// `@idempotencyToken` returns the job the first call started, a second start
/// while that job is running is refused with `ConflictException` (409), and
/// once the clock has carried the job to a terminal status a further start is
/// served as its own job.
#[tokio::test]
async fn start_ingestion_job_replays_its_token_and_refuses_a_concurrent_job() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("conflict-kb").await;

    // Rows: a data source started with a token, and one started without — the
    // concurrency rule holds either way, and the token only decides whether the
    // repeat is a replay or a conflict.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a start carrying an idempotency token",
            s3_data_source("token-source"),
            Some("ingestion-token-replayed-by-this-case"),
        ),
        (
            "a start carrying no idempotency token",
            custom_data_source("tokenless-source"),
            None,
        ),
    ];

    for (case, data_source_request, token) in cases {
        let data_source_id = emulator
            .data_source(&knowledge_base_id, data_source_request)
            .await;
        let request = match token {
            Some(token) => json!({ "clientToken": token }),
            None => json!({}),
        };

        let once = emulator
            .start_ingestion_job(&knowledge_base_id, &data_source_id, request.clone())
            .await;
        assert_eq!(
            once.status, 202,
            "case {case}: expected StartIngestionJob -> 202, got {once:?}"
        );

        let again = emulator
            .start_ingestion_job(&knowledge_base_id, &data_source_id, request)
            .await;
        if token.is_some() {
            assert_eq!(
                again.status, 202,
                "case {case}: a replayed token is served, got {again:?}"
            );
            assert_eq!(
                again.body["ingestionJob"], once.body["ingestionJob"],
                "case {case}: a replayed idempotency token must return the job the first \
                 call started, got {:?}",
                again.body
            );
        } else {
            assert_eq!(
                again.status, 409,
                "case {case}: a second job while one is running is refused, got {again:?}"
            );
            assert_eq!(
                again.error_type.as_deref(),
                Some("ConflictException"),
                "case {case}: expected ConflictException, got {again:?}"
            );
        }

        // Once the clock has carried the first job to a terminal status, the
        // data source may be ingested again — and that is a job of its own.
        tokio::time::sleep(INGESTION_JOB_START_SPAN + INGESTION_JOB_RUN_SPAN + MARGIN).await;
        let later = emulator
            .start_ingestion_job(&knowledge_base_id, &data_source_id, json!({}))
            .await;
        assert_eq!(
            later.status, 202,
            "case {case}: a start after the previous job completed is served, got {later:?}"
        );
        assert_ne!(
            later.job_string("ingestionJobId"),
            once.job_string("ingestionJobId"),
            "case {case}: each accepted start mints its own ingestionJobId"
        );
        assert_eq!(
            later.job_string("status"),
            "STARTING",
            "case {case}: the new job has only started, got {later:?}"
        );

        // The first job is still readable, and still terminal.
        let first = emulator
            .get_ingestion_job(
                &knowledge_base_id,
                &data_source_id,
                &once.job_string("ingestionJobId"),
            )
            .await;
        assert_eq!(
            first.job_string("status"),
            "COMPLETE",
            "case {case}: the completed job keeps its terminal status, got {first:?}"
        );
    }
}

/// An `@idempotencyToken` names the job it started within the request that
/// started it: the same token presented for another data source starts that
/// data source's own job, and the first job is still what a retry of the first
/// request replays.
#[tokio::test]
async fn an_idempotency_token_is_scoped_to_the_data_source_it_started() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base("token-scope-kb").await;
    let first_source = emulator
        .data_source(&knowledge_base_id, s3_data_source("token-scope-source-1"))
        .await;
    let second_source = emulator
        .data_source(&knowledge_base_id, custom_data_source("token-scope-source-2"))
        .await;

    let request = json!({ "clientToken": "ingestion-token-shared-by-two-sources" });
    let first = emulator
        .start_ingestion_job(&knowledge_base_id, &first_source, request.clone())
        .await;
    let second = emulator
        .start_ingestion_job(&knowledge_base_id, &second_source, request.clone())
        .await;
    assert_eq!(
        second.status, 202,
        "case the shared token on another data source: expected 202, got {second:?}"
    );
    assert_ne!(
        first.job_string("ingestionJobId"),
        second.job_string("ingestionJobId"),
        "case the shared token on another data source: each data source gets its own job"
    );

    // Each request still replays to the job IT started.
    for (case, data_source, started) in [
        ("the first data source", first_source.as_str(), &first),
        ("the second data source", second_source.as_str(), &second),
    ] {
        let replay = emulator
            .start_ingestion_job(&knowledge_base_id, data_source, request.clone())
            .await;
        assert_eq!(
            replay.body["ingestionJob"], started.body["ingestionJob"],
            "case {case}: a retry must replay the job this request started, got {replay:?}"
        );
    }
}
