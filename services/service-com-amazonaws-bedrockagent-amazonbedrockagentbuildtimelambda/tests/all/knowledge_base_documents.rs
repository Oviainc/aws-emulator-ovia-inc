// SPDX-License-Identifier: CC0-1.0

//! Documents pushed straight into a data source, end to end over HTTP:
//! `IngestKnowledgeBaseDocuments` persists each document of the request against
//! the data source the URI names, and `GetKnowledgeBaseDocuments` reports those
//! stored documents back — with the status the clock has made due, or with the
//! status the model gives an identifier nothing is stored for.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather
//! than calling a handler directly.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    DOCUMENT_INDEXING_SPAN, DOCUMENT_INGESTION_SPAN, DOCUMENT_QUOTA,
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

/// A margin over a span the clock owes, so a test that waits for a transition
/// is not racing the instant it comes due.
const MARGIN: Duration = Duration::from_millis(100);

/// Every status a document this service stores passes through, in the order the
/// clock owes them.
///
/// Source: the `DocumentStatus` member of the `KnowledgeBaseDocumentDetail`
/// shape of this service's Smithy model
/// (`smithy_query(query="doc",shape="KnowledgeBaseDocumentDetail",member="status")`),
/// whose `STARTING`, `IN_PROGRESS` and `INDEXED` values it documents as the
/// submitted, running and successfully indexed states of a document.
const INGESTION_STATUSES: [&str; 3] = ["STARTING", "IN_PROGRESS", "INDEXED"];

/// The status that same member documents for a document that "wasn't found".
const NOT_FOUND: &str = "NOT_FOUND";

/// One row of the round-trip table: the case name, the knowledge base and the
/// data source to create, the documents to ingest and the identifiers those
/// documents are stored under.
type RoundTripCase = (&'static str, Value, Value, Vec<Value>, Vec<Value>);

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
    /// The response's `documentDetails`, which every document operation of this
    /// service answers with.
    fn details(&self) -> &Vec<Value> {
        self.body["documentDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("documentDetails is not a list in {:?}", self.body))
    }

    /// One detail of the response, by its position in the request.
    fn detail(&self, index: usize) -> &Value {
        self.details().get(index).unwrap_or_else(|| {
            panic!(
                "documentDetails has no entry {index} in {:?}",
                self.body
            )
        })
    }

    /// A string member of one detail.
    fn detail_string(&self, index: usize, member: &str) -> String {
        let value = &self.detail(index)[member];
        value
            .as_str()
            .unwrap_or_else(|| {
                panic!("documentDetails[{index}].{member} is not a string in {value:?}")
            })
            .to_string()
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
            "arn:aws:s3vectors:::bucket/documents-demo-vectors",
            "documents-demo-index",
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
    async fn knowledge_base(&self, request: Value) -> String {
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

    async fn ingest(&self, knowledge_base_id: &str, data_source_id: &str, body: Value) -> Answer {
        self.call(
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/documents"),
            Some(body),
        )
        .await
    }

    async fn get_documents(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        identifiers: Vec<Value>,
    ) -> Answer {
        self.call(
            "POST",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                 /documents/getDocuments"
            ),
            Some(json!({ "documentIdentifiers": identifiers })),
        )
        .await
    }
}

/// A vector knowledge base: the kind whose data sources are crawled or pushed
/// into directly.
fn vector_knowledge_base(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": KB_ROLE,
        "knowledgeBaseConfiguration": {
            "type": "VECTOR",
            "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
        },
        "storageConfiguration": {
            "type": "S3_VECTORS",
            "s3VectorsConfiguration": {
                "vectorBucketArn": "arn:aws:s3vectors:::bucket/documents-demo-vectors",
                "indexName": "documents-demo-index",
            },
        },
    })
}

/// A CUSTOM data source: documents are pushed into it rather than crawled.
fn custom_data_source(name: &str) -> Value {
    json!({
        "name": name,
        "dataSourceConfiguration": { "type": "CUSTOM" },
    })
}

/// An S3 data source.
fn s3_data_source(name: &str) -> Value {
    json!({
        "name": name,
        "dataSourceConfiguration": {
            "type": "S3",
            "s3Configuration": { "bucketArn": "arn:aws:s3:::documents-demo-docs" },
        },
    })
}

/// A custom document whose content is defined inline as text.
fn custom_text_document(id: &str, text: &str) -> Value {
    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "IN_LINE",
                "inlineContent": {
                    "type": "TEXT",
                    "textContent": { "data": text },
                },
            },
        },
    })
}

/// A custom document whose content is defined inline in bytes, with metadata
/// attached to it.
fn custom_byte_document(id: &str) -> Value {
    json!({
        "metadata": {
            "type": "IN_LINE_ATTRIBUTE",
            "inlineAttributes": [
                { "key": "department", "value": { "type": "STRING", "stringValue": "finance" } },
                { "key": "year", "value": { "type": "NUMBER", "numberValue": 2026 } },
            ],
        },
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "IN_LINE",
                "inlineContent": {
                    "type": "BYTE",
                    "byteContent": { "mimeType": "text/plain", "data": "aGVsbG8sIGRvY3VtZW50" },
                },
            },
        },
    })
}

/// A custom document whose content is read from an S3 object.
fn custom_s3_document(id: &str, uri: &str) -> Value {
    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "S3_LOCATION",
                "s3Location": { "uri": uri },
            },
        },
    })
}

/// A document of an S3 data source, which the object's URI identifies.
fn s3_document(uri: &str) -> Value {
    json!({
        "content": {
            "dataSourceType": "S3",
            "s3": { "s3Location": { "uri": uri } },
        },
    })
}

/// The identifier of a custom document.
fn custom_identifier(id: &str) -> Value {
    json!({ "dataSourceType": "CUSTOM", "custom": { "id": id } })
}

/// The identifier of a document of an S3 data source.
fn s3_identifier(uri: &str) -> Value {
    json!({ "dataSourceType": "S3", "s3": { "uri": uri } })
}

/// Assert `detail` reports the document of this knowledge base and data source
/// that `identifier` names, with a status drawn from the ingestion progression.
fn assert_ingested_detail(
    case: &str,
    detail: &Value,
    knowledge_base_id: &str,
    data_source_id: &str,
    identifier: &Value,
) {
    assert_eq!(
        detail["knowledgeBaseId"].as_str(),
        Some(knowledge_base_id),
        "case {case}: the document belongs to the knowledge base the URI named, got {detail:?}"
    );
    assert_eq!(
        detail["dataSourceId"].as_str(),
        Some(data_source_id),
        "case {case}: the document belongs to the data source the URI named, got {detail:?}"
    );
    assert_eq!(
        &detail["identifier"], identifier,
        "case {case}: expected the identifier of the document ingested, got {detail:?}"
    );
    let status = detail["status"].as_str().unwrap_or_default();
    assert!(
        INGESTION_STATUSES.contains(&status),
        "case {case}: expected one of {INGESTION_STATUSES:?}, got {detail:?}"
    );
    let updated_at = detail["updatedAt"].as_str().unwrap_or_default();
    assert!(
        updated_at.contains('T') && updated_at.ends_with('Z'),
        "case {case}: expected updatedAt in the modelled date-time wire form, got {detail:?}"
    );
}

/// Every document a request ingests is persisted against the data source the
/// URI names and read back by `GetKnowledgeBaseDocuments` — over a table of
/// requests that differ in the kind of data source, the kind and source of the
/// document content, whether metadata is attached and how many documents one
/// request carries.
#[tokio::test]
async fn ingested_documents_round_trip_through_get_knowledge_base_documents() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: the kind of data
    // source, the kind of content (each value of the modelled
    // `ContentDataSourceType`), where that content comes from (inline text,
    // inline bytes, an S3 object), whether metadata rides along, and the number
    // of documents in one request — one, several and the modelled maximum of
    // ten.
    let ten_documents: Vec<Value> = (0..10)
        .map(|index| custom_text_document(&format!("bulk-document-{index}"), "bulk text"))
        .collect();
    let ten_identifiers: Vec<Value> = (0..10)
        .map(|index| custom_identifier(&format!("bulk-document-{index}")))
        .collect();

    let cases: Vec<RoundTripCase> = vec![
        (
            "one inline-text document of a CUSTOM data source",
            vector_knowledge_base("documents-kb-text"),
            custom_data_source("custom-text"),
            vec![custom_text_document("the-text-document", "a line of text")],
            vec![custom_identifier("the-text-document")],
        ),
        (
            "one inline-byte document carrying metadata",
            vector_knowledge_base("documents-kb-bytes"),
            custom_data_source("custom-bytes"),
            vec![custom_byte_document("the-byte-document")],
            vec![custom_identifier("the-byte-document")],
        ),
        (
            "one custom document read from an S3 object",
            vector_knowledge_base("documents-kb-custom-s3"),
            custom_data_source("custom-s3"),
            vec![custom_s3_document(
                "the-s3-backed-document",
                "s3://documents-demo-docs/reports/q1.txt",
            )],
            vec![custom_identifier("the-s3-backed-document")],
        ),
        (
            "three documents of mixed content sources in one request",
            vector_knowledge_base("documents-kb-mixed"),
            custom_data_source("custom-mixed"),
            vec![
                custom_text_document("mixed-1", "first"),
                custom_byte_document("mixed-2"),
                custom_s3_document("mixed-3", "s3://documents-demo-docs/reports/q2.txt"),
            ],
            vec![
                custom_identifier("mixed-1"),
                custom_identifier("mixed-2"),
                custom_identifier("mixed-3"),
            ],
        ),
        (
            "ten documents, the modelled maximum for one request",
            vector_knowledge_base("documents-kb-bulk"),
            custom_data_source("custom-bulk"),
            ten_documents,
            ten_identifiers,
        ),
        (
            "one document of an S3 data source",
            vector_knowledge_base("documents-kb-s3"),
            s3_data_source("s3-one"),
            vec![s3_document("s3://documents-demo-docs/handbook.pdf")],
            vec![s3_identifier("s3://documents-demo-docs/handbook.pdf")],
        ),
        (
            "two documents of an S3 data source",
            vector_knowledge_base("documents-kb-s3-two"),
            s3_data_source("s3-two"),
            vec![
                s3_document("s3://documents-demo-docs/a.txt"),
                s3_document("s3://documents-demo-docs/b.txt"),
            ],
            vec![
                s3_identifier("s3://documents-demo-docs/a.txt"),
                s3_identifier("s3://documents-demo-docs/b.txt"),
            ],
        ),
    ];

    for (case, knowledge_base_request, data_source_request, documents, identifiers) in cases {
        let knowledge_base_id = emulator.knowledge_base(knowledge_base_request).await;
        let data_source_id = emulator
            .data_source(&knowledge_base_id, data_source_request)
            .await;

        let ingested = emulator
            .ingest(
                &knowledge_base_id,
                &data_source_id,
                json!({ "documents": documents }),
            )
            .await;
        assert_eq!(
            ingested.status, 202,
            "case {case}: expected IngestKnowledgeBaseDocuments -> 202, got {ingested:?}"
        );
        assert_eq!(
            ingested.details().len(),
            identifiers.len(),
            "case {case}: expected one detail per document ingested, got {:?}",
            ingested.body
        );
        for (index, identifier) in identifiers.iter().enumerate() {
            assert_eq!(
                ingested.detail_string(index, "status"),
                "STARTING",
                "case {case}: a document just submitted has only started, got {:?}",
                ingested.detail(index)
            );
            assert_ingested_detail(
                case,
                ingested.detail(index),
                &knowledge_base_id,
                &data_source_id,
                identifier,
            );
        }

        let fetched = emulator
            .get_documents(&knowledge_base_id, &data_source_id, identifiers.clone())
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetKnowledgeBaseDocuments -> 200, got {fetched:?}"
        );
        assert_eq!(
            fetched.details().len(),
            identifiers.len(),
            "case {case}: expected one detail per identifier asked about, got {:?}",
            fetched.body
        );
        for (index, identifier) in identifiers.iter().enumerate() {
            assert_ingested_detail(
                case,
                fetched.detail(index),
                &knowledge_base_id,
                &data_source_id,
                identifier,
            );
        }
    }
}

/// A document is identified within the data source it was ingested into: an
/// identifier that data source holds nothing for is reported with the status
/// the model gives a document that was not found, and in the order asked,
/// alongside the ones it does hold.
#[tokio::test]
async fn an_identifier_this_data_source_holds_nothing_for_is_reported_not_found() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("not-found-kb"))
        .await;
    let custom_source = emulator
        .data_source(&knowledge_base_id, custom_data_source("not-found-custom"))
        .await;
    let other_source = emulator
        .data_source(&knowledge_base_id, custom_data_source("not-found-other"))
        .await;

    let ingested = emulator
        .ingest(
            &knowledge_base_id,
            &custom_source,
            json!({ "documents": [custom_text_document("stored-document", "text")] }),
        )
        .await;
    assert_eq!(
        ingested.status, 202,
        "case setup: expected IngestKnowledgeBaseDocuments -> 202, got {ingested:?}"
    );
    let other = emulator
        .ingest(
            &knowledge_base_id,
            &other_source,
            json!({ "documents": [custom_text_document("other-source-document", "text")] }),
        )
        .await;
    assert_eq!(
        other.status, 202,
        "case setup: expected IngestKnowledgeBaseDocuments -> 202, got {other:?}"
    );

    // Rows: an id nothing holds, an id differing from the stored one in its
    // last character, an id stored for ANOTHER data source of the same
    // knowledge base, an identifier of a kind the data source holds no document
    // of at all, and — as the control — the document that is stored.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a custom id nothing holds",
            custom_identifier("never-ingested"),
            NOT_FOUND,
        ),
        (
            "a custom id differing in its last character",
            custom_identifier("stored-documenu"),
            NOT_FOUND,
        ),
        (
            "a custom id stored for another data source",
            custom_identifier("other-source-document"),
            NOT_FOUND,
        ),
        (
            "an S3 identifier on a data source holding no S3 document",
            s3_identifier("s3://documents-demo-docs/never-ingested.txt"),
            NOT_FOUND,
        ),
        (
            "the document this data source holds",
            custom_identifier("stored-document"),
            "STARTING",
        ),
    ];

    for (case, identifier, expected) in &cases {
        let fetched = emulator
            .get_documents(&knowledge_base_id, &custom_source, vec![identifier.clone()])
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: a document nothing holds is a status, not a refusal, got {fetched:?}"
        );
        assert_eq!(
            fetched.detail_string(0, "status"),
            *expected,
            "case {case}: expected status {expected}, got {:?}",
            fetched.detail(0)
        );
        assert_eq!(
            &fetched.detail(0)["identifier"],
            identifier,
            "case {case}: expected the identifier asked about, got {:?}",
            fetched.detail(0)
        );
        let has_updated_at = !fetched.detail(0)["updatedAt"].is_null();
        assert_eq!(
            has_updated_at,
            *expected != NOT_FOUND,
            "case {case}: only a document the store holds has an instant it changed at, \
             got {:?}",
            fetched.detail(0)
        );
    }

    // One request mixing the two: each identifier is answered in the order it
    // was asked about.
    let identifiers: Vec<Value> = cases
        .iter()
        .map(|(_, identifier, _)| identifier.clone())
        .collect();
    let mixed = emulator
        .get_documents(&knowledge_base_id, &custom_source, identifiers.clone())
        .await;
    assert_eq!(
        mixed.status, 200,
        "case a request mixing stored and unknown identifiers: expected 200, got {mixed:?}"
    );
    for (index, (case, identifier, expected)) in cases.iter().enumerate() {
        assert_eq!(
            &mixed.detail(index)["identifier"],
            identifier,
            "case a request mixing identifiers, entry {index} ({case}): details answer in the \
             order asked, got {:?}",
            mixed.body
        );
        assert_eq!(
            mixed.detail_string(index, "status"),
            *expected,
            "case a request mixing identifiers, entry {index} ({case}): expected {expected}, \
             got {:?}",
            mixed.detail(index)
        );
    }
}

/// A document's status is owed by the CLOCK: the ingestion reports `STARTING`,
/// and the stored document advances to `IN_PROGRESS` and then to `INDEXED` with
/// no further call moving it.
#[tokio::test]
async fn an_ingested_document_advances_on_the_clock_to_indexed() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("clock-documents-kb"))
        .await;
    let data_source_id = emulator
        .data_source(&knowledge_base_id, custom_data_source("clock-documents"))
        .await;
    let identifier = custom_identifier("clock-document");

    let ingested = emulator
        .ingest(
            &knowledge_base_id,
            &data_source_id,
            json!({ "documents": [custom_text_document("clock-document", "text")] }),
        )
        .await;
    assert_eq!(
        ingested.status, 202,
        "case setup: expected IngestKnowledgeBaseDocuments -> 202, got {ingested:?}"
    );

    // The transient status is observable: a caller reading back immediately
    // still sees the document starting.
    let starting = emulator
        .get_documents(&knowledge_base_id, &data_source_id, vec![identifier.clone()])
        .await;
    assert_eq!(
        starting.detail_string(0, "status"),
        "STARTING",
        "case read during submission: expected STARTING, got {:?}",
        starting.detail(0)
    );

    tokio::time::sleep(DOCUMENT_INGESTION_SPAN + MARGIN).await;
    let running = emulator
        .get_documents(&knowledge_base_id, &data_source_id, vec![identifier.clone()])
        .await;
    assert_eq!(
        running.detail_string(0, "status"),
        "IN_PROGRESS",
        "case read after the ingestion span: expected IN_PROGRESS, got {:?}",
        running.detail(0)
    );

    tokio::time::sleep(DOCUMENT_INDEXING_SPAN + MARGIN).await;
    let mut previous: Option<Value> = None;
    for read in ["first", "second"] {
        let indexed = emulator
            .get_documents(&knowledge_base_id, &data_source_id, vec![identifier.clone()])
            .await;
        assert_eq!(
            indexed.detail_string(0, "status"),
            "INDEXED",
            "case read {read} after the indexing span: expected INDEXED, got {:?}",
            indexed.detail(0)
        );
        if let Some(previous) = previous.as_ref() {
            assert_eq!(
                indexed.detail(0),
                previous,
                "case read {read} after the indexing span: an indexed document is stable \
                 across reads"
            );
        }
        previous = Some(indexed.detail(0).clone());
    }
}

/// Documents live under one data source of one knowledge base: both document
/// operations answer `ResourceNotFoundException` (404) for a knowledge base the
/// store does not hold, and for a data source that knowledge base does not
/// hold.
#[tokio::test]
async fn document_operations_are_scoped_to_the_knowledge_base_and_data_source_named() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("scope-documents-kb"))
        .await;
    let data_source_id = emulator
        .data_source(&knowledge_base_id, custom_data_source("scope-documents"))
        .await;

    // Rows: a well-formed knowledgeBaseId the store does not hold, and a
    // well-formed dataSourceId the knowledge base does not hold.
    let refusals: Vec<(&str, &str, &str)> = vec![
        (
            "a knowledge base nothing holds",
            "0123456789",
            data_source_id.as_str(),
        ),
        (
            "a data source the knowledge base does not hold",
            knowledge_base_id.as_str(),
            "aZ09aZ09aZ",
        ),
    ];

    for (case, knowledge_base, data_source) in refusals {
        let ingested = emulator
            .ingest(
                knowledge_base,
                data_source,
                json!({ "documents": [custom_text_document("scoped-document", "text")] }),
            )
            .await;
        assert_eq!(
            ingested.status, 404,
            "case {case}: expected IngestKnowledgeBaseDocuments -> 404, got {ingested:?}"
        );
        assert_eq!(
            ingested.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {ingested:?}"
        );

        let fetched = emulator
            .get_documents(
                knowledge_base,
                data_source,
                vec![custom_identifier("scoped-document")],
            )
            .await;
        assert_eq!(
            fetched.status, 404,
            "case {case}: expected GetKnowledgeBaseDocuments -> 404, got {fetched:?}"
        );
        assert_eq!(
            fetched.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {fetched:?}"
        );
    }
}

/// A document whose content names the kind of data source the addressed data
/// source itself is, is the only content that data source takes: anything else
/// is refused with `ValidationException` (400) and nothing of the request is
/// written.
#[tokio::test]
async fn a_document_naming_another_kind_of_data_source_is_refused() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("mismatch-kb"))
        .await;
    let custom_source = emulator
        .data_source(&knowledge_base_id, custom_data_source("mismatch-custom"))
        .await;
    let s3_source = emulator
        .data_source(&knowledge_base_id, s3_data_source("mismatch-s3"))
        .await;

    // Rows: each kind of content presented to a data source of the OTHER kind,
    // alone and alongside a document the data source would take.
    let cases: Vec<(&str, &str, Vec<Value>, Value)> = vec![
        (
            "S3 content pushed into a CUSTOM data source",
            custom_source.as_str(),
            vec![s3_document("s3://documents-demo-docs/mismatched.txt")],
            s3_identifier("s3://documents-demo-docs/mismatched.txt"),
        ),
        (
            "custom content ingested into an S3 data source",
            s3_source.as_str(),
            vec![custom_text_document("mismatched-document", "text")],
            custom_identifier("mismatched-document"),
        ),
        (
            "a mismatched document alongside one the data source would take",
            custom_source.as_str(),
            vec![
                custom_text_document("would-be-taken", "text"),
                s3_document("s3://documents-demo-docs/mismatched-too.txt"),
            ],
            custom_identifier("would-be-taken"),
        ),
    ];

    for (case, data_source_id, documents, unwritten) in cases {
        let refused = emulator
            .ingest(
                &knowledge_base_id,
                data_source_id,
                json!({ "documents": documents }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected IngestKnowledgeBaseDocuments -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {refused:?}"
        );

        // Nothing of a refused request is written, including the document the
        // data source would otherwise have taken.
        let fetched = emulator
            .get_documents(&knowledge_base_id, data_source_id, vec![unwritten.clone()])
            .await;
        assert_eq!(
            fetched.detail_string(0, "status"),
            NOT_FOUND,
            "case {case}: a refused request persists no document, got {:?}",
            fetched.detail(0)
        );
    }
}

/// `clientToken` is an `@idempotencyToken`: a repeat of the request reports the
/// documents the first call ingested rather than ingesting a second set, and
/// the token names those documents only within the data source it was presented
/// for.
#[tokio::test]
async fn a_repeated_ingestion_token_reports_the_documents_it_already_ingested() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("token-documents-kb"))
        .await;
    let first_source = emulator
        .data_source(&knowledge_base_id, custom_data_source("token-documents-1"))
        .await;
    let second_source = emulator
        .data_source(&knowledge_base_id, custom_data_source("token-documents-2"))
        .await;

    // The modelled `ClientToken` is `length 33..256`, so the token is at least
    // 33 characters.
    let token = "ingestion-token-shared-by-two-data-sources";
    let first = emulator
        .ingest(
            &knowledge_base_id,
            &first_source,
            json!({
                "clientToken": token,
                "documents": [custom_text_document("token-document-1", "text")],
            }),
        )
        .await;
    assert_eq!(
        first.status, 202,
        "case the first ingestion: expected 202, got {first:?}"
    );

    // The same token, carrying a DIFFERENT document: the retry is served with
    // the documents the first call ingested, and the second list is not stored.
    let replay = emulator
        .ingest(
            &knowledge_base_id,
            &first_source,
            json!({
                "clientToken": token,
                "documents": [custom_text_document("token-document-retry", "text")],
            }),
        )
        .await;
    assert_eq!(
        replay.status, 202,
        "case a replayed token: a retry is served, got {replay:?}"
    );
    assert_eq!(
        &replay.detail(0)["identifier"],
        &first.detail(0)["identifier"],
        "case a replayed token: expected the document the first call ingested, got {:?}",
        replay.body
    );
    let retried = emulator
        .get_documents(
            &knowledge_base_id,
            &first_source,
            vec![custom_identifier("token-document-retry")],
        )
        .await;
    assert_eq!(
        retried.detail_string(0, "status"),
        NOT_FOUND,
        "case a replayed token: a retry ingests nothing new, got {:?}",
        retried.detail(0)
    );

    // The same token presented for another data source ingests that data
    // source's own documents.
    let second = emulator
        .ingest(
            &knowledge_base_id,
            &second_source,
            json!({
                "clientToken": token,
                "documents": [custom_text_document("token-document-2", "text")],
            }),
        )
        .await;
    assert_eq!(
        second.status, 202,
        "case the shared token on another data source: expected 202, got {second:?}"
    );
    assert_eq!(
        &second.detail(0)["identifier"],
        &custom_identifier("token-document-2"),
        "case the shared token on another data source: each data source takes its own \
         documents, got {:?}",
        second.body
    );
    assert_eq!(
        second.detail_string(0, "dataSourceId"),
        second_source,
        "case the shared token on another data source: the document belongs to that data \
         source, got {:?}",
        second.detail(0)
    );
}

/// Re-ingesting a document the data source already holds replaces it rather
/// than adding a second one, and its status starts over.
#[tokio::test]
async fn re_ingesting_a_document_replaces_the_one_the_data_source_holds() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("replace-documents-kb"))
        .await;
    let data_source_id = emulator
        .data_source(&knowledge_base_id, custom_data_source("replace-documents"))
        .await;
    let identifier = custom_identifier("replaced-document");

    let first = emulator
        .ingest(
            &knowledge_base_id,
            &data_source_id,
            json!({ "documents": [custom_text_document("replaced-document", "first text")] }),
        )
        .await;
    assert_eq!(
        first.status, 202,
        "case the first ingestion: expected 202, got {first:?}"
    );

    tokio::time::sleep(DOCUMENT_INGESTION_SPAN + DOCUMENT_INDEXING_SPAN + MARGIN).await;
    let indexed = emulator
        .get_documents(&knowledge_base_id, &data_source_id, vec![identifier.clone()])
        .await;
    assert_eq!(
        indexed.detail_string(0, "status"),
        "INDEXED",
        "case the indexed document: expected INDEXED, got {:?}",
        indexed.detail(0)
    );

    let again = emulator
        .ingest(
            &knowledge_base_id,
            &data_source_id,
            json!({ "documents": [custom_text_document("replaced-document", "second text")] }),
        )
        .await;
    assert_eq!(
        again.status, 202,
        "case the second ingestion: expected 202, got {again:?}"
    );
    assert_eq!(
        again.detail_string(0, "status"),
        "STARTING",
        "case the second ingestion: the document is being ingested again, got {:?}",
        again.detail(0)
    );
    assert_eq!(
        again.details().len(),
        1,
        "case the second ingestion: one document was ingested, got {:?}",
        again.body
    );

    let fetched = emulator
        .get_documents(&knowledge_base_id, &data_source_id, vec![identifier.clone()])
        .await;
    assert_eq!(
        fetched.details().len(),
        1,
        "case the re-ingested document: the data source holds one document for the \
         identifier, got {:?}",
        fetched.body
    );
    assert_eq!(
        fetched.detail_string(0, "status"),
        "STARTING",
        "case the re-ingested document: the stored document is the one just ingested, \
         got {:?}",
        fetched.detail(0)
    );
}

/// A document, or an identifier, that names a kind of data source but carries
/// none of that kind's own details identifies no document: both operations
/// refuse it with `ValidationException` (400), which is what the shapes mean by
/// "include the field that corresponds to it".
#[tokio::test]
async fn a_document_carrying_none_of_its_kinds_details_is_refused() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("incomplete-kb"))
        .await;
    let custom_source = emulator
        .data_source(&knowledge_base_id, custom_data_source("incomplete-custom"))
        .await;
    let s3_source = emulator
        .data_source(&knowledge_base_id, s3_data_source("incomplete-s3"))
        .await;

    // Rows: each kind named with its own member missing, on the data source of
    // that same kind, alone and alongside a complete document.
    let ingestions: Vec<(&str, &str, Vec<Value>)> = vec![
        (
            "CUSTOM content with no custom member",
            custom_source.as_str(),
            vec![json!({ "content": { "dataSourceType": "CUSTOM" } })],
        ),
        (
            "S3 content with no s3 member",
            s3_source.as_str(),
            vec![json!({ "content": { "dataSourceType": "S3" } })],
        ),
        (
            "an incomplete document alongside a complete one",
            custom_source.as_str(),
            vec![
                custom_text_document("complete-document", "text"),
                json!({ "content": { "dataSourceType": "CUSTOM" } }),
            ],
        ),
    ];
    for (case, data_source_id, documents) in ingestions {
        let refused = emulator
            .ingest(
                &knowledge_base_id,
                data_source_id,
                json!({ "documents": documents }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected IngestKnowledgeBaseDocuments -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {refused:?}"
        );
    }

    // Nothing of a refused request is written, including the complete document
    // that shared it.
    let fetched = emulator
        .get_documents(
            &knowledge_base_id,
            &custom_source,
            vec![custom_identifier("complete-document")],
        )
        .await;
    assert_eq!(
        fetched.detail_string(0, "status"),
        NOT_FOUND,
        "case a refused ingestion: nothing of it is persisted, got {:?}",
        fetched.detail(0)
    );

    // The same rule on the read: an identifier naming a kind but carrying no id
    // for it names no document.
    let reads: Vec<(&str, Vec<Value>)> = vec![
        (
            "a CUSTOM identifier with no custom member",
            vec![json!({ "dataSourceType": "CUSTOM" })],
        ),
        (
            "an S3 identifier with no s3 member",
            vec![json!({ "dataSourceType": "S3" })],
        ),
        (
            "an incomplete identifier alongside a well-formed one",
            vec![
                custom_identifier("complete-document"),
                json!({ "dataSourceType": "CUSTOM" }),
            ],
        ),
    ];
    for (case, identifiers) in reads {
        let refused = emulator
            .get_documents(&knowledge_base_id, &custom_source, identifiers)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected GetKnowledgeBaseDocuments -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {refused:?}"
        );
    }
}

/// The store this operation writes is bounded per data source: once it holds
/// `DOCUMENT_QUOTA` documents a request that would add another is refused with
/// the modelled `ServiceQuotaExceededException` (402) and nothing is written,
/// while a request that adds no new document is still served. The index of
/// remembered idempotency tokens takes the same bound, because it grows once
/// per accepted request even when the documents do not.
#[tokio::test]
async fn the_document_store_refuses_to_grow_past_its_per_data_source_bound() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("bound-kb"))
        .await;
    let document_source = emulator
        .data_source(&knowledge_base_id, custom_data_source("bound-documents"))
        .await;

    // Fill the store, ten documents at a time — the modelled maximum for one
    // request.
    let per_request = 10;
    for chunk in 0..DOCUMENT_QUOTA / per_request {
        let documents: Vec<Value> = (0..per_request)
            .map(|index| {
                custom_text_document(&format!("bound-document-{chunk}-{index}"), "text")
            })
            .collect();
        let accepted = emulator
            .ingest(
                &knowledge_base_id,
                &document_source,
                json!({ "documents": documents }),
            )
            .await;
        assert_eq!(
            accepted.status, 202,
            "case filling the store, chunk {chunk}: expected 202, got {accepted:?}"
        );
    }

    // One more document is refused, and is not written.
    let refused = emulator
        .ingest(
            &knowledge_base_id,
            &document_source,
            json!({ "documents": [custom_text_document("one-document-too-many", "text")] }),
        )
        .await;
    assert_eq!(
        refused.status, 402,
        "case one document past the bound: expected ServiceQuotaExceededException's 402, \
         got {refused:?}"
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case one document past the bound: expected ServiceQuotaExceededException, \
         got {refused:?}"
    );
    let fetched = emulator
        .get_documents(
            &knowledge_base_id,
            &document_source,
            vec![custom_identifier("one-document-too-many")],
        )
        .await;
    assert_eq!(
        fetched.detail_string(0, "status"),
        NOT_FOUND,
        "case one document past the bound: the refused document is not written, got {:?}",
        fetched.detail(0)
    );

    // A request that adds NO new document is still served at the bound: only
    // what the request adds counts against it.
    let replaced = emulator
        .ingest(
            &knowledge_base_id,
            &document_source,
            json!({ "documents": [custom_text_document("bound-document-0-0", "text again")] }),
        )
        .await;
    assert_eq!(
        replaced.status, 202,
        "case re-ingesting a document the store already holds: expected 202, got {replaced:?}"
    );

    // The token index takes the same bound on its own data source: every one of
    // these requests ingests the SAME document under a fresh token, so the
    // documents do not grow and the tokens do.
    let token_source = emulator
        .data_source(&knowledge_base_id, custom_data_source("bound-tokens"))
        .await;
    for index in 0..DOCUMENT_QUOTA {
        let accepted = emulator
            .ingest(
                &knowledge_base_id,
                &token_source,
                json!({
                    "clientToken": format!("ingestion-token-number-{index:06}-of-this-case"),
                    "documents": [custom_text_document("the-one-document", "text")],
                }),
            )
            .await;
        assert_eq!(
            accepted.status, 202,
            "case filling the token index, request {index}: expected 202, got {accepted:?}"
        );
    }
    let refused = emulator
        .ingest(
            &knowledge_base_id,
            &token_source,
            json!({
                "clientToken": "ingestion-token-one-past-the-bound-of-this-case",
                "documents": [custom_text_document("the-one-document", "text")],
            }),
        )
        .await;
    assert_eq!(
        refused.status, 402,
        "case one token past the bound: expected ServiceQuotaExceededException's 402, \
         got {refused:?}"
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case one token past the bound: expected ServiceQuotaExceededException, got {refused:?}"
    );

    // A request carrying no token is still served there: it adds no entry to
    // the index that is full.
    let tokenless = emulator
        .ingest(
            &knowledge_base_id,
            &token_source,
            json!({ "documents": [custom_text_document("the-one-document", "text")] }),
        )
        .await;
    assert_eq!(
        tokenless.status, 202,
        "case a tokenless ingestion at the token bound: expected 202, got {tokenless:?}"
    );
}
