// SPDX-License-Identifier: CC0-1.0

//! `ListKnowledgeBaseDocuments`, end to end over HTTP: every document this
//! service holds for one data source is reported, in a stable document order
//! and in pages, with each detail read back from the stored document record.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather
//! than calling a handler directly. The documents are placed by
//! `IngestKnowledgeBaseDocuments`, so what the listing reports is state the
//! public API wrote.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    DOCUMENT_INDEXING_SPAN, DOCUMENT_INGESTION_SPAN, MAX_DOCUMENTS_PER_PAGE,
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
/// Source: the `status` member of the `KnowledgeBaseDocumentDetail` shape of
/// this service's Smithy model
/// (`smithy_query(query="doc",shape="KnowledgeBaseDocumentDetail",member="status")`),
/// whose `STARTING`, `IN_PROGRESS` and `INDEXED` values it documents as the
/// submitted, running and successfully indexed states of a document.
const INGESTION_STATUSES: [&str; 3] = ["STARTING", "IN_PROGRESS", "INDEXED"];

/// The terminal status of that progression.
const INDEXED: &str = "INDEXED";

/// Both values of the modelled `ContentDataSourceType`
/// (`smithy_query(query="shape",shape="ContentDataSourceType")`), which is what
/// a document's identity is discriminated by.
const CONTENT_TYPES: [&str; 2] = ["CUSTOM", "S3"];

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
    fn details(&self) -> Vec<Value> {
        self.body["documentDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("documentDetails is not a list in {:?}", self.body))
            .clone()
    }

    /// The identity of every document the response reported, in the order it
    /// reported them.
    fn identifiers(&self) -> Vec<Value> {
        self.details()
            .iter()
            .map(|detail| detail["identifier"].clone())
            .collect()
    }

    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(ToString::to_string)
    }

    fn message(&self) -> String {
        self.body["message"].as_str().unwrap_or_default().to_string()
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

    /// `IngestKnowledgeBaseDocuments`, in batches of the modelled maximum of
    /// ten documents per request, so a data source can be filled with more
    /// documents than one request may carry.
    async fn ingest_all(&self, knowledge_base_id: &str, data_source_id: &str, documents: &[Value]) {
        for batch in documents.chunks(10) {
            let ingested = self
                .call(
                    "PUT",
                    &format!(
                        "/knowledgebases/{knowledge_base_id}/datasources/\
                         {data_source_id}/documents"
                    ),
                    Some(json!({ "documents": batch })),
                )
                .await;
            assert_eq!(
                ingested.status, 202,
                "case setup: expected IngestKnowledgeBaseDocuments -> 202, got {ingested:?}"
            );
        }
    }

    /// `ListKnowledgeBaseDocuments`, with or without the paging members.
    async fn list(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        max_results: Option<i64>,
        next_token: Option<&str>,
    ) -> Answer {
        let mut body = json!({});
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        if let Some(next_token) = next_token {
            body["nextToken"] = json!(next_token);
        }
        self.call(
            "POST",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/documents"),
            Some(body),
        )
        .await
    }
}

/// A vector knowledge base: the kind whose data sources are pushed into
/// directly.
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
                "vectorBucketArn": "arn:aws:s3vectors:::bucket/listing-vectors",
                "indexName": "listing-index",
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
            "s3Configuration": { "bucketArn": "arn:aws:s3:::listing-docs" },
        },
    })
}

/// A managed-connector data source: a kind no document content can name, so it
/// takes documents of either modelled `ContentDataSourceType` and one data
/// source can hold documents of both kinds.
fn managed_connector_data_source(name: &str) -> Value {
    json!({
        "name": name,
        "dataSourceConfiguration": {
            "type": "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
            "managedKnowledgeBaseConnectorConfiguration": {},
        },
    })
}

/// A custom document whose content is defined inline as text.
fn custom_document(id: &str) -> Value {
    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "IN_LINE",
                "inlineContent": {
                    "type": "TEXT",
                    "textContent": { "data": format!("the text of {id}") },
                },
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

/// The identifier the store holds a custom document under.
fn custom_identifier(id: &str) -> Value {
    json!({ "dataSourceType": "CUSTOM", "custom": { "id": id } })
}

/// The identifier the store holds an S3 document under.
fn s3_identifier(uri: &str) -> Value {
    json!({ "dataSourceType": "S3", "s3": { "uri": uri } })
}

/// A document's identity as the pair the listing is ordered by: the kind of
/// data source it belongs to, then its own id within that kind.
fn identity(identifier: &Value) -> (String, String) {
    let kind = identifier["dataSourceType"]
        .as_str()
        .unwrap_or_else(|| panic!("an identifier carries no dataSourceType: {identifier:?}"))
        .to_string();
    assert!(
        CONTENT_TYPES.contains(&kind.as_str()),
        "an identifier names a kind the model does not declare: {identifier:?}"
    );
    let id = if kind == "CUSTOM" {
        identifier["custom"]["id"].as_str()
    } else {
        identifier["s3"]["uri"].as_str()
    };
    (
        kind,
        id.unwrap_or_else(|| panic!("an identifier carries no id: {identifier:?}"))
            .to_string(),
    )
}

/// The identities given, in the order this listing reports them: the order is
/// deterministic and stable, so a page resumes exactly after the one before it.
fn in_document_order(identifiers: &[Value]) -> Vec<Value> {
    let mut ordered = identifiers.to_vec();
    ordered.sort_by_key(identity);
    ordered
}

/// Assert `detail` reports a stored document of this knowledge base and data
/// source, with a status drawn from the ingestion progression.
fn assert_detail(case: &str, detail: &Value, knowledge_base_id: &str, data_source_id: &str) {
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
    assert!(
        detail.get("statusReason").is_none(),
        "case {case}: a document that was not IGNORED carries no statusReason, got {detail:?}"
    );
}

/// Every document a data source holds is reported, over a table of data sources
/// that differ in the kind of data source, the kind of document identity (each
/// value of the modelled `ContentDataSourceType`, and both in one data source)
/// and the number of documents held — none, one, more than one request may
/// carry, and ids whose ingestion order is not their document order.
#[tokio::test]
async fn list_knowledge_base_documents_reports_every_document_the_data_source_holds() {
    // Rows: (case, the data source to create, the documents to ingest in that
    // order, the identities they are stored under).
    let twelve: Vec<Value> = (0..12)
        .map(|index| custom_document(&format!("bulk-{index:02}")))
        .collect();
    let twelve_identifiers: Vec<Value> = (0..12)
        .map(|index| custom_identifier(&format!("bulk-{index:02}")))
        .collect();

    let cases: Vec<(&str, Value, Vec<Value>, Vec<Value>)> = vec![
        (
            "a data source holding no document",
            custom_data_source("listing-empty"),
            vec![],
            vec![],
        ),
        (
            "one custom document",
            custom_data_source("listing-one"),
            vec![custom_document("the-only-document")],
            vec![custom_identifier("the-only-document")],
        ),
        (
            "custom documents ingested out of document order",
            custom_data_source("listing-unordered"),
            vec![
                custom_document("zeta"),
                custom_document("alpha"),
                custom_document("mu"),
            ],
            vec![
                custom_identifier("zeta"),
                custom_identifier("alpha"),
                custom_identifier("mu"),
            ],
        ),
        (
            "S3 documents, identified by URI",
            s3_data_source("listing-s3"),
            vec![
                s3_document("s3://listing-docs/reports/q2.txt"),
                s3_document("s3://listing-docs/reports/q1.txt"),
            ],
            vec![
                s3_identifier("s3://listing-docs/reports/q2.txt"),
                s3_identifier("s3://listing-docs/reports/q1.txt"),
            ],
        ),
        (
            "both kinds of identity in one data source",
            managed_connector_data_source("listing-mixed"),
            vec![
                s3_document("s3://listing-docs/mixed/b.txt"),
                custom_document("mixed-custom"),
                s3_document("s3://listing-docs/mixed/a.txt"),
            ],
            vec![
                s3_identifier("s3://listing-docs/mixed/b.txt"),
                custom_identifier("mixed-custom"),
                s3_identifier("s3://listing-docs/mixed/a.txt"),
            ],
        ),
        (
            "twelve documents, more than one ingestion request may carry",
            custom_data_source("listing-bulk"),
            twelve,
            twelve_identifiers,
        ),
    ];

    for (case, data_source, documents, identifiers) in cases {
        let emulator = Emulator::new();
        let knowledge_base_id = emulator
            .knowledge_base(vector_knowledge_base("listing-kb"))
            .await;
        let data_source_id = emulator
            .data_source(&knowledge_base_id, data_source)
            .await;
        emulator
            .ingest_all(&knowledge_base_id, &data_source_id, &documents)
            .await;

        let listed = emulator
            .list(&knowledge_base_id, &data_source_id, None, None)
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected ListKnowledgeBaseDocuments -> 200, got {listed:?}"
        );
        assert_eq!(
            listed.identifiers(),
            in_document_order(&identifiers),
            "case {case}: expected every stored document once, in document order, got {:?}",
            listed.identifiers()
        );
        assert_eq!(
            listed.next_token(),
            None,
            "case {case}: a listing that fits in one page carries no nextToken, got {:?}",
            listed.next_token()
        );
        for detail in listed.details() {
            assert_detail(case, &detail, &knowledge_base_id, &data_source_id);
        }

        // A read changes nothing a later caller sees: the same listing answers
        // the same documents in the same order.
        let again = emulator
            .list(&knowledge_base_id, &data_source_id, None, None)
            .await;
        assert_eq!(
            again.identifiers(),
            listed.identifiers(),
            "case {case}: the listing order must be stable across reads, got {:?}",
            again.identifiers()
        );
    }
}

/// `maxResults` truncates a page and the `nextToken` it hands out resumes
/// exactly after it, so walking the pages yields the whole listing once, over a
/// table of `maxResults` values spanning the published bounds of the modelled
/// range (1..1000) and the member omitted.
#[tokio::test]
async fn list_knowledge_base_documents_pages_by_max_results_and_resumes_after_each_page() {
    // A data source holding documents of both modelled identity kinds, so a
    // page boundary can fall inside either kind.
    let documents: Vec<Value> = (0..7)
        .map(|index| custom_document(&format!("paged-{index}")))
        .chain((0..6).map(|index| s3_document(&format!("s3://listing-docs/paged/{index}.txt"))))
        .collect();
    let identifiers: Vec<Value> = (0..7)
        .map(|index| custom_identifier(&format!("paged-{index}")))
        .chain((0..6).map(|index| s3_identifier(&format!("s3://listing-docs/paged/{index}.txt"))))
        .collect();
    let all = in_document_order(&identifiers);

    for (case, max_results) in [
        ("maxResults omitted", None),
        ("maxResults at the range's lower bound", Some(1)),
        ("maxResults below the document count", Some(5)),
        ("maxResults one below the document count", Some(12)),
        ("maxResults at the document count", Some(13)),
        ("maxResults above the document count", Some(50)),
        ("maxResults at the range's upper bound", Some(1000)),
    ] {
        let emulator = Emulator::new();
        let knowledge_base_id = emulator
            .knowledge_base(vector_knowledge_base("kb-for-paging"))
            .await;
        let data_source_id = emulator
            .data_source(
                &knowledge_base_id,
                managed_connector_data_source("listing-paged"),
            )
            .await;
        emulator
            .ingest_all(&knowledge_base_id, &data_source_id, &documents)
            .await;

        let page_cap = max_results
            .map_or(MAX_DOCUMENTS_PER_PAGE, |requested: i64| {
                usize::try_from(requested).unwrap_or(MAX_DOCUMENTS_PER_PAGE)
            })
            .min(MAX_DOCUMENTS_PER_PAGE);

        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = emulator
                .list(
                    &knowledge_base_id,
                    &data_source_id,
                    max_results,
                    token.as_deref(),
                )
                .await;
            assert_eq!(
                page.status, 200,
                "case {case}: expected ListKnowledgeBaseDocuments -> 200, got {page:?}"
            );
            let reported = page.identifiers();
            assert!(
                reported.len() <= page_cap,
                "case {case}: a page carries at most {page_cap} documents, got {reported:?}"
            );
            walked.extend(reported.clone());
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case}: paging must terminate, walked {walked:?}"
            );
            match page.next_token() {
                None => break,
                Some(next) => {
                    assert_eq!(
                        reported.len(),
                        page_cap,
                        "case {case}: a page that carries a nextToken is a full page, \
                         got {reported:?}"
                    );
                    token = Some(next);
                }
            }
        }

        assert_eq!(
            walked, all,
            "case {case}: walking the pages must yield every document once, in document \
             order, got {walked:?}"
        );
        assert_eq!(
            pages,
            all.len().div_ceil(page_cap),
            "case {case}: expected {} page(s) of {page_cap}, walked {pages}",
            all.len().div_ceil(page_cap)
        );
    }
}

/// One data source reports its own documents and no other's — not another data
/// source of the same knowledge base, and not a data source of another
/// knowledge base — and each detail carries the status the clock has made due
/// rather than the status the ingestion minted.
#[tokio::test]
async fn list_knowledge_base_documents_isolates_each_data_source_and_reports_the_due_status() {
    let emulator = Emulator::new();

    let first_kb = emulator
        .knowledge_base(vector_knowledge_base("listing-first-kb"))
        .await;
    let second_kb = emulator
        .knowledge_base(vector_knowledge_base("listing-second-kb"))
        .await;
    let first_ds = emulator
        .data_source(&first_kb, custom_data_source("listing-first-ds"))
        .await;
    let second_ds = emulator
        .data_source(&first_kb, custom_data_source("listing-second-ds"))
        .await;
    let foreign_ds = emulator
        .data_source(&second_kb, custom_data_source("listing-foreign-ds"))
        .await;

    // The same document id is pushed into all three data sources: a document is
    // identified WITHIN its data source, so these are three documents.
    let shared = vec![custom_document("shared-id")];
    emulator.ingest_all(&first_kb, &first_ds, &shared).await;
    emulator
        .ingest_all(&first_kb, &second_ds, &[custom_document("second-only")])
        .await;
    emulator.ingest_all(&second_kb, &foreign_ds, &shared).await;

    // Rows: each data source of each knowledge base, with the documents it
    // alone holds.
    for (case, knowledge_base_id, data_source_id, expected) in [
        (
            "the first data source",
            &first_kb,
            &first_ds,
            vec![custom_identifier("shared-id")],
        ),
        (
            "a second data source of the same knowledge base",
            &first_kb,
            &second_ds,
            vec![custom_identifier("second-only")],
        ),
        (
            "a data source of another knowledge base",
            &second_kb,
            &foreign_ds,
            vec![custom_identifier("shared-id")],
        ),
    ] {
        let listed = emulator
            .list(knowledge_base_id, data_source_id, None, None)
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected ListKnowledgeBaseDocuments -> 200, got {listed:?}"
        );
        assert_eq!(
            listed.identifiers(),
            expected,
            "case {case}: a data source reports its own documents and no other's, got {:?}",
            listed.identifiers()
        );
        for detail in listed.details() {
            assert_detail(case, &detail, knowledge_base_id, data_source_id);
        }
    }

    // Once the clock has carried the ingestion through, the listing reports the
    // terminal status of the progression — read from the stored record, not from
    // any request.
    tokio::time::sleep(DOCUMENT_INGESTION_SPAN + DOCUMENT_INDEXING_SPAN + MARGIN).await;
    let listed = emulator.list(&first_kb, &first_ds, None, None).await;
    for detail in listed.details() {
        assert_eq!(
            detail["status"].as_str(),
            Some(INDEXED),
            "case after the ingestion spans: expected the terminal status {INDEXED}, \
             got {detail:?}"
        );
    }
}

/// A knowledge base or data source the store does not hold is a missing
/// resource, and a `nextToken` this service never issued or a `maxResults`
/// outside the modelled range is a bad request — none of which changes what the
/// listing reports.
#[tokio::test]
async fn list_knowledge_base_documents_refuses_unknown_resources_and_a_foreign_token() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("listing-refusals-kb"))
        .await;
    let data_source_id = emulator
        .data_source(&knowledge_base_id, custom_data_source("listing-refusals-ds"))
        .await;
    let other_kb = emulator
        .knowledge_base(vector_knowledge_base("listing-refusals-other-kb"))
        .await;
    emulator
        .ingest_all(
            &knowledge_base_id,
            &data_source_id,
            &[custom_document("kept")],
        )
        .await;

    // Rows: a well-formed id of each resource the store does not hold, and the
    // pair that names a real data source under the wrong knowledge base.
    for (case, kb, ds, missing) in [
        (
            "an unknown knowledge base",
            "0123456789",
            data_source_id.as_str(),
            "0123456789",
        ),
        (
            "an unknown data source",
            knowledge_base_id.as_str(),
            "aZ09aZ09aZ",
            "aZ09aZ09aZ",
        ),
        (
            "a data source of another knowledge base",
            other_kb.as_str(),
            data_source_id.as_str(),
            data_source_id.as_str(),
        ),
    ] {
        let listed = emulator.list(kb, ds, None, None).await;
        assert_eq!(
            listed.status, 404,
            "case {case}: expected ListKnowledgeBaseDocuments -> 404, got {listed:?}"
        );
        assert_eq!(
            listed.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {listed:?}"
        );
        assert!(
            listed.message().contains(missing),
            "case {case}: the refusal names the resource that was not found, got {:?}",
            listed.message()
        );
    }

    // Rows: tokens no page of this listing handed out — a position is a plain
    // decimal count of the documents already reported, so nothing else names
    // one.
    for (case, token) in [
        ("a token from somewhere else", "not-a-pagination-token"),
        ("a signed number", "+1"),
        ("a zero-padded position", "01"),
        ("the start of the listing, which needs no token", "0"),
        ("a hexadecimal number", "0x10"),
        ("a decimal fraction", "1.5"),
    ] {
        let listed = emulator
            .list(&knowledge_base_id, &data_source_id, None, Some(token))
            .await;
        assert_eq!(
            listed.status, 400,
            "case {case} ({token}): expected ListKnowledgeBaseDocuments -> 400, got {listed:?}"
        );
        assert_eq!(
            listed.error_type.as_deref(),
            Some("ValidationException"),
            "case {case} ({token}): expected ValidationException, got {listed:?}"
        );
    }

    // `maxResults` is modelled `range 1..1000`: one past either published bound
    // is refused at the boundary, before the handler runs.
    for (case, max_results) in [
        ("one below the range's lower bound", 0),
        ("one above the range's upper bound", 1001),
    ] {
        let listed = emulator
            .list(&knowledge_base_id, &data_source_id, Some(max_results), None)
            .await;
        assert_eq!(
            listed.status, 400,
            "case {case} (maxResults={max_results}): expected \
             ListKnowledgeBaseDocuments -> 400, got {listed:?}"
        );
    }

    // A position past the end of the listing is a position all the same: it
    // reports no document and resumes nowhere.
    let past_the_end = emulator
        .list(&knowledge_base_id, &data_source_id, None, Some("99"))
        .await;
    assert_eq!(
        past_the_end.status, 200,
        "case a position past the end: expected ListKnowledgeBaseDocuments -> 200, \
         got {past_the_end:?}"
    );
    assert!(
        past_the_end.details().is_empty(),
        "case a position past the end: expected no document, got {:?}",
        past_the_end.details()
    );
    assert_eq!(
        past_the_end.next_token(),
        None,
        "case a position past the end: expected no nextToken, got {:?}",
        past_the_end.next_token()
    );

    // Every refusal above left the stored documents exactly as they were.
    let listed = emulator
        .list(&knowledge_base_id, &data_source_id, None, None)
        .await;
    assert_eq!(
        listed.identifiers(),
        vec![custom_identifier("kept")],
        "case after the refusals: the data source's documents are still reported, got {:?}",
        listed.identifiers()
    );
}
