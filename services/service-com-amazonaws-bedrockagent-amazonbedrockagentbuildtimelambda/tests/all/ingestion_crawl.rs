// SPDX-License-Identifier: CC0-1.0

//! What an ingestion job over an S3 data source does, end to end over HTTP.
//!
//! `IngestionJobStatistics` is the service's own accounting of a run:
//! `numberOfDocumentsScanned` is "the total number of source documents that were
//! scanned. Includes new, updated, and unchanged documents",
//! `numberOfNewDocumentsIndexed` "the number of new source documents in the data
//! source that were successfully indexed", beside the modified, deleted, failed
//! and skipped counts
//! (`smithy_query(query="shape",shape="IngestionJobStatistics")`). The rule this
//! suite pins is that each counter MEASURES the run: a job that completes over a
//! data source holding N previously-unseen source documents reports N new, those
//! documents become documents of the data source, and the knowledge base's bound
//! vector store really holds a vector for each of them.
//!
//! The rows differ along what the rule generalises over: how many source
//! documents the data source's bucket holds, whether an inclusion prefix narrows
//! them, whether a metadata file sits beside them, and whether a document is new,
//! unchanged, modified or gone since the last run.
//!
//! A document is only reported indexed where the bound store really took its
//! vector, so a knowledge base whose index has since been deleted reports its
//! documents FAILED rather than indexed — driven here too.
//!
//! The data source's `dataDeletionPolicy` then decides what becomes of those
//! vectors when the data source is deleted — `DELETE` removes them, `RETAIN`
//! leaves them — and both arms are driven here.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    INGESTION_JOB_RUN_SPAN, INGESTION_JOB_START_SPAN,
};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use service_com_amazonaws_s3_amazons3::state::{service_state as s3_state, Object};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_store_fixture::{
    drop_index, indexed_vector, index_vector_keys, seed_index, EMBEDDING_DIMENSION,
};

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The region the data source's bucket is held in.
const REGION: &str = "eu-west-1";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// The vector bucket every knowledge base here binds to.
pub(crate) const VECTOR_BUCKET: &str = "arn:aws:s3vectors:::bucket/crawl-vectors";

/// The index of [`VECTOR_BUCKET`] the vectors are written into.
pub(crate) const INDEX: &str = "crawl-index";

/// The S3 bucket the data sources here are crawled from.
const SOURCE_BUCKET: &str = "crawl-source-documents";

/// Slack on every wait, so a span that has only just come due is observed as
/// due.
const MARGIN: Duration = Duration::from_millis(100);

/// Every member of the modelled `IngestionJobStatistics`.
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

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
pub(crate) struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn member(&self, path: &[&str]) -> &Value {
        let mut at = &self.body;
        for key in path {
            at = &at[*key];
        }
        at
    }

    fn string(&self, path: &[&str]) -> String {
        self.member(path)
            .as_str()
            .unwrap_or_else(|| panic!("{path:?} is not a string in {:?}", self.body))
            .to_string()
    }

    /// One counter of the job's statistics.
    fn counter(&self, member: &str) -> i64 {
        self.member(&["ingestionJob", "statistics", member])
            .as_i64()
            .unwrap_or_else(|| panic!("statistics.{member} is not a number in {:?}", self.body))
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
///
/// `pub(crate)`, with the seams a sibling suite drives it through, so the suite that
/// asserts what an ingestion RECORDS beside each vector
/// (`tests/all/ingestion_index.rs`) drives the emulator exactly the way this one does
/// instead of growing a second copy of the harness.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // The vector store the knowledge base binds to, and the bucket the data
        // source is crawled from: both set up before anything names them, as a
        // caller sets them up against AWS.
        seed_index(&engine, VECTOR_BUCKET, INDEX, EMBEDDING_DIMENSION);
        {
            let state = s3_state(&*engine).expect("the object store on this engine");
            state.lock().create_bucket(SOURCE_BUCKET, REGION);
        }
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
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
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({error}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        Answer { status, body }
    }

    /// Store `body` at `key` of the source bucket, through Amazon S3's own
    /// store — the one map its `PutObject` writes and its `GetObject` reads.
    pub(crate) fn hold_object(&self, key: &str, body: &str) {
        let state = s3_state(&*self.engine).expect("the object store on this engine");
        let mut store = state.lock();
        let written = store.put_object(
            SOURCE_BUCKET,
            key,
            Object::stored(body.as_bytes().to_vec(), Some("text/plain".to_string())),
        );
        assert!(
            written.is_some(),
            "the harness must be able to store the source document at {key:?}"
        );
    }

    /// Take the object at `key` out of the source bucket again.
    pub(crate) fn drop_object(&self, key: &str) {
        let state = s3_state(&*self.engine).expect("the object store on this engine");
        let mut store = state.lock();
        let removed = store.remove_object(SOURCE_BUCKET, key);
        assert!(
            removed.is_some(),
            "the harness must be able to delete the source document at {key:?}"
        );
    }

    /// Create a knowledge base bound to [`INDEX`] and answer its minted id.
    pub(crate) async fn knowledge_base(&self, name: &str) -> String {
        let created = self
            .call(
                "PUT",
                "/knowledgebases/",
                Some(json!({
                    "name": name,
                    "roleArn": KB_ROLE,
                    "knowledgeBaseConfiguration": {
                        "type": "VECTOR",
                        "vectorKnowledgeBaseConfiguration": {
                            "embeddingModelArn": EMBEDDING_MODEL,
                        },
                    },
                    "storageConfiguration": {
                        "type": "S3_VECTORS",
                        "s3VectorsConfiguration": {
                            "vectorBucketArn": VECTOR_BUCKET,
                            "indexName": INDEX,
                        },
                    },
                })),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.string(&["knowledgeBase", "knowledgeBaseId"])
    }

    /// Attach an S3 data source over `prefixes` and answer its minted id.
    pub(crate) async fn data_source(
        &self,
        knowledge_base_id: &str,
        name: &str,
        prefixes: Option<Vec<&str>>,
        data_deletion_policy: Option<&str>,
    ) -> String {
        let mut s3 = json!({ "bucketArn": format!("arn:aws:s3:::{SOURCE_BUCKET}") });
        if let Some(prefixes) = prefixes {
            s3["inclusionPrefixes"] = json!(prefixes);
        }
        let mut request = json!({
            "name": name,
            "dataSourceConfiguration": { "type": "S3", "s3Configuration": s3 },
        });
        if let Some(policy) = data_deletion_policy {
            request["dataDeletionPolicy"] = json!(policy);
        }
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
        created.string(&["dataSource", "dataSourceId"])
    }

    /// Start an ingestion job and wait for the clock to complete it, answering
    /// the completed job as `GetIngestionJob` reports it.
    pub(crate) async fn run_ingestion_job(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
    ) -> Answer {
        let started = self
            .call(
                "PUT",
                &format!(
                    "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                     /ingestionjobs/"
                ),
                Some(json!({})),
            )
            .await;
        assert_eq!(
            started.status, 202,
            "case setup: expected StartIngestionJob -> 202, got {started:?}"
        );
        // A run that has not finished has accounted for nothing yet.
        for member in STATISTICS_MEMBERS {
            assert_eq!(
                started.counter(member),
                0,
                "case setup: a job that has only started reports no {member} yet, got {:?}",
                started.body
            );
        }
        let ingestion_job_id = started.string(&["ingestionJob", "ingestionJobId"]);
        tokio::time::sleep(INGESTION_JOB_START_SPAN + INGESTION_JOB_RUN_SPAN + MARGIN).await;
        let completed = self
            .call(
                "GET",
                &format!(
                    "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                     /ingestionjobs/{ingestion_job_id}"
                ),
                None,
            )
            .await;
        assert_eq!(
            completed.status, 200,
            "case setup: expected GetIngestionJob -> 200, got {completed:?}"
        );
        assert_eq!(
            completed.string(&["ingestionJob", "status"]),
            "COMPLETE",
            "case setup: the clock must complete the job, got {completed:?}"
        );
        completed
    }

    /// The source URIs `ListKnowledgeBaseDocuments` reports for the data source,
    /// in URI order.
    async fn document_uris(&self, knowledge_base_id: &str, data_source_id: &str) -> Vec<String> {
        let listed = self
            .call(
                "POST",
                &format!(
                    "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/documents"
                ),
                Some(json!({})),
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case setup: expected ListKnowledgeBaseDocuments -> 200, got {listed:?}"
        );
        let mut uris: Vec<String> = listed
            .member(&["documentDetails"])
            .as_array()
            .unwrap_or_else(|| panic!("documentDetails is a list in {:?}", listed.body))
            .iter()
            .map(|detail| detail["identifier"]["s3"]["uri"].as_str().unwrap_or("").to_string())
            .collect();
        uris.sort();
        uris
    }

    /// Delete the data source and let the clock take it out of the store.
    async fn delete_data_source(&self, knowledge_base_id: &str, data_source_id: &str) {
        let deleted = self
            .call(
                "DELETE",
                &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}"),
                None,
            )
            .await;
        assert_eq!(
            deleted.status, 202,
            "case setup: expected DeleteDataSource -> 202, got {deleted:?}"
        );
    }

    /// The keys of the vectors the bound index holds.
    fn index_keys(&self) -> Vec<String> {
        index_vector_keys(&self.engine, VECTOR_BUCKET, INDEX)
    }

    /// Delete the bound index in the vector store, so the knowledge base is
    /// bound to an index that is no longer there.
    fn drop_bound_index(&self) {
        drop_index(&self.engine, VECTOR_BUCKET, INDEX);
    }

    /// The data of the vector the bound index holds under `key`.
    fn vector_of(&self, key: &str) -> Vec<f32> {
        indexed_vector(&self.engine, VECTOR_BUCKET, INDEX, key)
            .unwrap_or_else(|| panic!("the bound index holds a vector for {key}"))
    }
}

/// The `s3://` URI of one source document, as a knowledge-base document names it.
pub(crate) fn uri(key: &str) -> String {
    format!("s3://{SOURCE_BUCKET}/{key}")
}

/// One row of the crawl table.
struct CrawlRow {
    case: &'static str,
    /// The objects the data source's bucket holds, as `(key, body)`.
    objects: Vec<(&'static str, &'static str)>,
    /// The `inclusionPrefixes` the data source declares, if any.
    prefixes: Option<Vec<&'static str>>,
    /// The source documents the job must report scanned and newly indexed, in
    /// URI order.
    expected: Vec<&'static str>,
    /// The metadata files the job must report scanned.
    expected_metadata: i64,
}

/// A completed ingestion job accounts for every source document of its data
/// source, and the documents and their vectors are really there afterwards.
#[tokio::test]
async fn a_completed_ingestion_job_accounts_for_the_source_documents_it_indexed() {
    let rows = [
        CrawlRow {
            case: "a bucket holding nothing",
            objects: Vec::new(),
            prefixes: None,
            expected: Vec::new(),
            expected_metadata: 0,
        },
        CrawlRow {
            case: "one source document",
            objects: vec![("docs/one.txt", "the first document")],
            prefixes: None,
            expected: vec!["docs/one.txt"],
            expected_metadata: 0,
        },
        CrawlRow {
            case: "two source documents under one prefix",
            objects: vec![
                ("docs/one.txt", "the first document"),
                ("docs/two.txt", "the second document"),
            ],
            prefixes: Some(vec!["docs/"]),
            expected: vec!["docs/one.txt", "docs/two.txt"],
            expected_metadata: 0,
        },
        CrawlRow {
            case: "five source documents, no prefix",
            objects: vec![
                ("a.txt", "a"),
                ("b.txt", "bb"),
                ("c.txt", "ccc"),
                ("d.txt", "dddd"),
                ("e.txt", "ddddd"),
            ],
            prefixes: None,
            expected: vec!["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"],
            expected_metadata: 0,
        },
        CrawlRow {
            case: "an inclusion prefix that leaves objects out",
            objects: vec![
                ("included/one.txt", "inside the prefix"),
                ("included/two.txt", "also inside"),
                ("excluded/three.txt", "outside the prefix"),
            ],
            prefixes: Some(vec!["included/"]),
            expected: vec!["included/one.txt", "included/two.txt"],
            expected_metadata: 0,
        },
        CrawlRow {
            case: "a metadata file beside the source document",
            objects: vec![
                ("docs/one.txt", "the first document"),
                (
                    "docs/one.txt.metadata.json",
                    r#"{"metadataAttributes":{"tier":"gold"}}"#,
                ),
            ],
            prefixes: Some(vec!["docs/"]),
            expected: vec!["docs/one.txt"],
            expected_metadata: 1,
        },
    ];

    for (position, row) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        for (key, body) in &row.objects {
            emulator.hold_object(key, body);
        }
        let knowledge_base_id = emulator
            .knowledge_base(&format!("crawl-kb-{position}"))
            .await;
        let data_source_id = emulator
            .data_source(
                &knowledge_base_id,
                &format!("crawl-ds-{position}"),
                row.prefixes.clone(),
                None,
            )
            .await;

        let job = emulator
            .run_ingestion_job(&knowledge_base_id, &data_source_id)
            .await;
        let expected: Vec<String> = row.expected.iter().map(|key| uri(key)).collect();
        let counted = i64::try_from(expected.len()).expect("a small count");
        assert_eq!(
            job.counter("numberOfDocumentsScanned"),
            counted,
            "case {}: expected {counted} source documents scanned, got {:?}",
            row.case,
            job.member(&["ingestionJob", "statistics"])
        );
        assert_eq!(
            job.counter("numberOfNewDocumentsIndexed"),
            counted,
            "case {}: expected {counted} previously-unseen documents indexed as new, got {:?}",
            row.case,
            job.member(&["ingestionJob", "statistics"])
        );
        assert_eq!(
            job.counter("numberOfMetadataDocumentsScanned"),
            row.expected_metadata,
            "case {}: expected {} metadata files scanned, got {:?}",
            row.case,
            row.expected_metadata,
            job.member(&["ingestionJob", "statistics"])
        );
        for member in [
            "numberOfModifiedDocumentsIndexed",
            "numberOfDocumentsDeleted",
            "numberOfDocumentsFailed",
            "numberOfDocumentsSkipped",
        ] {
            assert_eq!(
                job.counter(member),
                0,
                "case {}: a first run over a new source leaves {member} at 0, got {:?}",
                row.case,
                job.member(&["ingestionJob", "statistics"])
            );
        }

        // The documents the job accounted for are documents of the data source.
        assert_eq!(
            emulator
                .document_uris(&knowledge_base_id, &data_source_id)
                .await,
            expected,
            "case {}: expected the crawled documents to be the data source's own",
            row.case
        );
        // And the vector store the knowledge base is bound to really holds one
        // vector for each of them.
        assert_eq!(
            emulator.index_keys(),
            expected,
            "case {}: expected the bound index to hold one vector per indexed document",
            row.case
        );
    }
}

/// A document is reported indexed only where the bound vector store really took
/// its vector: with the index gone, the job accounts for every source document
/// as FAILED, indexes none of them, and leaves the data source holding none.
#[tokio::test]
async fn a_document_the_vector_store_refused_is_reported_failed_not_indexed() {
    let emulator = Emulator::new();
    emulator.hold_object("docs/one.txt", "the first document");
    emulator.hold_object("docs/two.txt", "the second document");
    let knowledge_base_id = emulator.knowledge_base("crawl-refused-kb").await;
    let data_source_id = emulator
        .data_source(
            &knowledge_base_id,
            "crawl-refused-ds",
            Some(vec!["docs/"]),
            None,
        )
        .await;
    emulator.drop_bound_index();

    let job = emulator
        .run_ingestion_job(&knowledge_base_id, &data_source_id)
        .await;
    for (member, expected) in [
        ("numberOfDocumentsScanned", 2),
        ("numberOfDocumentsFailed", 2),
        ("numberOfNewDocumentsIndexed", 0),
        ("numberOfModifiedDocumentsIndexed", 0),
        ("numberOfDocumentsSkipped", 0),
    ] {
        assert_eq!(
            job.counter(member),
            expected,
            "case the refused index: expected {member} to be {expected}, got {:?}",
            job.member(&["ingestionJob", "statistics"])
        );
    }
    assert!(
        emulator
            .document_uris(&knowledge_base_id, &data_source_id)
            .await
            .is_empty(),
        "case the refused index: a document nothing indexed is not a document of the data source"
    );
}

/// A second run over the same source accounts for what CHANGED since the first:
/// an untouched document is scanned and nothing more, a rewritten one is
/// modified, a fresh one is new, and one that is gone from the source is
/// deleted.
#[tokio::test]
async fn a_second_run_accounts_for_what_changed_since_the_first() {
    let emulator = Emulator::new();
    emulator.hold_object("docs/kept.txt", "unchanged between the two runs");
    emulator.hold_object("docs/rewritten.txt", "the first revision");
    emulator.hold_object("docs/removed.txt", "here for the first run only");
    let knowledge_base_id = emulator.knowledge_base("crawl-rerun-kb").await;
    let data_source_id = emulator
        .data_source(&knowledge_base_id, "crawl-rerun-ds", Some(vec!["docs/"]), None)
        .await;

    let first = emulator
        .run_ingestion_job(&knowledge_base_id, &data_source_id)
        .await;
    assert_eq!(
        first.counter("numberOfNewDocumentsIndexed"),
        3,
        "case the first run: expected all three documents indexed as new, got {:?}",
        first.member(&["ingestionJob", "statistics"])
    );

    emulator.hold_object("docs/rewritten.txt", "the second revision, longer");
    emulator.hold_object("docs/added.txt", "written between the two runs");
    emulator.drop_object("docs/removed.txt");

    let second = emulator
        .run_ingestion_job(&knowledge_base_id, &data_source_id)
        .await;
    for (member, expected) in [
        ("numberOfDocumentsScanned", 3),
        ("numberOfNewDocumentsIndexed", 1),
        ("numberOfModifiedDocumentsIndexed", 1),
        ("numberOfDocumentsDeleted", 1),
        ("numberOfDocumentsFailed", 0),
        ("numberOfDocumentsSkipped", 0),
    ] {
        assert_eq!(
            second.counter(member),
            expected,
            "case the second run: expected {member} to be {expected}, got {:?}",
            second.member(&["ingestionJob", "statistics"])
        );
    }
    assert_eq!(
        emulator
            .document_uris(&knowledge_base_id, &data_source_id)
            .await,
        vec![
            uri("docs/added.txt"),
            uri("docs/kept.txt"),
            uri("docs/rewritten.txt")
        ],
        "case the second run: the data source holds exactly what its source now offers"
    );
    assert_eq!(
        emulator.index_keys(),
        vec![
            uri("docs/added.txt"),
            uri("docs/kept.txt"),
            uri("docs/rewritten.txt")
        ],
        "case the second run: the bound index holds a vector for each document that is still \
         there"
    );
}

/// The data source's `dataDeletionPolicy` decides what becomes of its documents'
/// vectors when it is deleted: `DELETE` takes them out of the vector store,
/// `RETAIN` leaves them there.
#[tokio::test]
async fn the_data_deletion_policy_governs_vector_removal_at_the_store() {
    for (policy, retained) in [("RETAIN", true), ("DELETE", false)] {
        let emulator = Emulator::new();
        emulator.hold_object("docs/one.txt", "the first document");
        emulator.hold_object("docs/two.txt", "the second document");
        let knowledge_base_id = emulator
            .knowledge_base(&format!("crawl-policy-kb-{}", policy.to_lowercase()))
            .await;
        let data_source_id = emulator
            .data_source(
                &knowledge_base_id,
                &format!("crawl-policy-ds-{}", policy.to_lowercase()),
                Some(vec!["docs/"]),
                Some(policy),
            )
            .await;
        let job = emulator
            .run_ingestion_job(&knowledge_base_id, &data_source_id)
            .await;
        assert_eq!(
            job.counter("numberOfNewDocumentsIndexed"),
            2,
            "case {policy}: expected both documents indexed as new, got {:?}",
            job.member(&["ingestionJob", "statistics"])
        );
        assert_eq!(
            emulator.index_keys(),
            vec![uri("docs/one.txt"), uri("docs/two.txt")],
            "case {policy}: the ingestion must leave a vector per document in the store"
        );

        emulator
            .delete_data_source(&knowledge_base_id, &data_source_id)
            .await;
        let expected: Vec<String> = if retained {
            vec![uri("docs/one.txt"), uri("docs/two.txt")]
        } else {
            Vec::new()
        };
        assert_eq!(
            emulator.index_keys(),
            expected,
            "case {policy}: expected the vectors to be {} after the data source was deleted",
            if retained { "retained" } else { "removed" }
        );
    }
}

/// The vector a document is indexed under is a vector of its TEXT: two
/// documents with the same text are indexed at the same point, two that share
/// no term are orthogonal under the index's cosine metric, and a document that
/// is rewritten moves.
#[tokio::test]
async fn a_document_s_vector_is_computed_from_its_text() {
    let emulator = Emulator::new();
    emulator.hold_object("docs/one.txt", "alpha beta gamma");
    emulator.hold_object("docs/same.txt", "alpha beta gamma");
    emulator.hold_object("docs/other.txt", "delta epsilon zeta");
    let knowledge_base_id = emulator.knowledge_base("crawl-vector-kb").await;
    let data_source_id = emulator
        .data_source(
            &knowledge_base_id,
            "crawl-vector-ds",
            Some(vec!["docs/"]),
            None,
        )
        .await;
    emulator
        .run_ingestion_job(&knowledge_base_id, &data_source_id)
        .await;

    let first = emulator.vector_of(&uri("docs/one.txt"));
    let same = emulator.vector_of(&uri("docs/same.txt"));
    let other = emulator.vector_of(&uri("docs/other.txt"));
    assert_eq!(
        first, same,
        "case the same text: two documents of the same text are indexed at the same point"
    );
    assert_eq!(
        dot(&first, &other),
        0.0,
        "case no shared term: documents sharing no term must be orthogonal, got {:?}",
        dot(&first, &other)
    );
    assert!(
        (dot(&first, &first) - 1.0).abs() < 1e-5,
        "case the norm: an indexed vector is normalised, got {:?}",
        dot(&first, &first)
    );

    // A rewritten document is re-indexed at the point its new text names.
    emulator.hold_object("docs/one.txt", "delta epsilon zeta");
    emulator
        .run_ingestion_job(&knowledge_base_id, &data_source_id)
        .await;
    assert_eq!(
        emulator.vector_of(&uri("docs/one.txt")),
        other,
        "case the rewritten document: its vector follows its text"
    );
}

/// The dot product of two vectors, which under the index's cosine metric is
/// their similarity (both are normalised).
fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}
