// SPDX-License-Identifier: CC0-1.0

//! The refusals of the knowledge-base slice, end to end over HTTP, and the
//! sentence each one carries.
//!
//! Three rules are driven here, each over a TABLE whose rows differ along what
//! the rule generalises over:
//!
//! * a document may be ingested only into a data source whose own type is the
//!   type the document's content names, for EVERY modelled `DataSourceType` and
//!   both modelled `ContentDataSourceType` values;
//! * a knowledge base whose configuration names the vector type needs a storage
//!   configuration, on the create and on the update alike, and no other kind of
//!   knowledge base does;
//! * an operation addressed to a data source by the pair of path labels reports
//!   that DATA SOURCE as the missing resource whenever the pair resolves to
//!   nothing, whichever half of the pair the store does not hold.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather
//! than calling a handler directly.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::DELETION_SPAN;
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

/// A second embedding model ARN, so an update can ask for a different one.
const OTHER_EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v1";

/// A secret ARN matching the modelled `SecretArn` pattern, for the connector
/// kinds whose source configuration requires credentials.
const CREDENTIALS_SECRET: &str =
    "arn:aws:secretsmanager:eu-west-1:000000000000:secret:connector-credentials";

/// The bad-request error of this service, and the status its model binds to it.
///
/// Source: this service's Smithy model
/// (`smithy_query(query="operation",operation="IngestKnowledgeBaseDocuments,UpdateKnowledgeBase")`),
/// where `ValidationException` is bound with `httpError` 400.
const VALIDATION: (&str, u16) = ("ValidationException", 400);

/// The missing-resource error of this service, and the status its model binds to
/// it (`smithy_query(query="operation",operation="DeleteDataSource")`).
const NOT_FOUND: (&str, u16) = ("ResourceNotFoundException", 404);

/// The `DocumentStatus` the model gives an identifier the data source holds no
/// document for, which is what a refused ingestion leaves behind.
const DOCUMENT_NOT_FOUND: &str = "NOT_FOUND";

/// A margin over the span the clock owes a removal, so a test that waits for it
/// is not racing the instant it comes due.
const MARGIN: Duration = Duration::from_millis(200);

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
    error_type: Option<String>,
    /// Whether the `x-amzn-errortype` HEADER itself carried the error name: a
    /// client's error taxonomy reads that header, so a body-only error name is
    /// not the same answer.
    error_type_header: Option<String>,
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

    /// The refusal's `message` member, or the empty string on an answer that
    /// carries none.
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
        // The knowledge bases here bind to an S3 Vectors index, which
        // `CreateKnowledgeBase` resolves in the service that owns it — so every
        // index a row names is created first, as a caller creates it first
        // against AWS. Each row's knowledge base names a bucket and an index
        // after itself (`vector_knowledge_base`).
        for name in [
            "fidelity-ingest-kb",
            "fidelity-storage-kb",
            "fidelity-type-kb",
            "fidelity-delete-kb",
            "fidelity-delete-other-kb",
            "fidelity-delete-doomed-kb",
        ] {
            crate::vector_store_fixture::seed_index(
                &engine,
                &format!("arn:aws:s3vectors:::bucket/{name}"),
                &format!("{name}-index"),
                crate::vector_store_fixture::EMBEDDING_DIMENSION,
            );
        }
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
        let error_type_header = response
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
        let error_type = error_type_header.clone().or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        Answer {
            status,
            error_type,
            error_type_header,
            body,
        }
    }

    async fn create_knowledge_base(&self, request: Value) -> Answer {
        self.call("PUT", "/knowledgebases/", Some(request)).await
    }

    /// Create a knowledge base and answer its minted `knowledgeBaseId`.
    async fn knowledge_base(&self, request: Value) -> String {
        let created = self.create_knowledge_base(request).await;
        assert_eq!(
            created.status, 202,
            "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.string(&["knowledgeBase", "knowledgeBaseId"])
    }

    async fn update_knowledge_base(&self, knowledge_base_id: &str, request: Value) -> Answer {
        self.call(
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}"),
            Some(request),
        )
        .await
    }

    async fn delete_knowledge_base(&self, knowledge_base_id: &str) -> Answer {
        self.call("DELETE", &format!("/knowledgebases/{knowledge_base_id}"), None)
            .await
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
        created.string(&["dataSource", "dataSourceId"])
    }

    async fn delete_data_source(&self, knowledge_base_id: &str, data_source_id: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}"),
            None,
        )
        .await
    }

    async fn ingest(&self, knowledge_base_id: &str, data_source_id: &str, documents: Vec<Value>) -> Answer {
        self.call(
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/documents"),
            Some(json!({ "documents": documents })),
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

/// A self-managed vector knowledge base, with the vector store the kind needs.
fn vector_knowledge_base(name: &str) -> Value {
    let mut request = vector_knowledge_base_without_storage(name, EMBEDDING_MODEL);
    request["storageConfiguration"] = json!({
        "type": "S3_VECTORS",
        "s3VectorsConfiguration": {
            "vectorBucketArn": format!("arn:aws:s3vectors:::bucket/{name}"),
            "indexName": format!("{name}-index"),
        },
    });
    request
}

/// The same request with no `storageConfiguration` member at all, over whichever
/// embedding model the row names.
fn vector_knowledge_base_without_storage(name: &str, embedding_model: &str) -> Value {
    json!({
        "name": name,
        "roleArn": KB_ROLE,
        "knowledgeBaseConfiguration": {
            "type": "VECTOR",
            "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": embedding_model },
        },
    })
}

/// A service-managed knowledge base: a kind that has no vector store of its own,
/// so it needs no `storageConfiguration`.
fn managed_knowledge_base(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": KB_ROLE,
        "knowledgeBaseConfiguration": {
            "type": "MANAGED",
            "managedKnowledgeBaseConfiguration": { "embeddingModelType": "MANAGED" },
        },
    })
}

/// A data source request of each modelled `DataSourceType`, carrying that
/// kind's own configuration member where the model pairs one with it.
///
/// Source: the `DataSourceType` enum and the `DataSourceConfiguration`,
/// `WebDataSourceConfiguration` and `ConfluenceDataSourceConfiguration`
/// structures of this service's Smithy model
/// (`smithy_query(query="shape",shape="DataSourceType,DataSourceConfiguration")`).
fn data_source_of_type(name: &str, data_source_type: &str) -> Value {
    let configuration = match data_source_type {
        "S3" => json!({
            "type": "S3",
            "s3Configuration": { "bucketArn": "arn:aws:s3:::fidelity-docs" },
        }),
        "WEB" => json!({
            "type": "WEB",
            "webConfiguration": {
                "sourceConfiguration": {
                    "urlConfiguration": { "seedUrls": [{ "url": "https://example.com/docs" }] },
                },
            },
        }),
        "CONFLUENCE" => json!({
            "type": "CONFLUENCE",
            "confluenceConfiguration": {
                "sourceConfiguration": {
                    "hostUrl": "https://example.atlassian.net",
                    "hostType": "SAAS",
                    "authType": "BASIC",
                    "credentialsSecretArn": CREDENTIALS_SECRET,
                },
            },
        }),
        "MANAGED_KNOWLEDGE_BASE_CONNECTOR" => json!({
            "type": "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
            "managedKnowledgeBaseConnectorConfiguration": {},
        }),
        // CUSTOM and REDSHIFT_METADATA are the kinds the model pairs with no
        // sibling configuration member, so they need none.
        other => json!({ "type": other }),
    };
    json!({ "name": name, "dataSourceConfiguration": configuration })
}

/// A document whose content names `content_type` as the kind of data source it
/// belongs to, carrying that kind's own details.
fn document_of_type(content_type: &str, id: &str) -> Value {
    match content_type {
        "CUSTOM" => json!({
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
        }),
        "S3" => json!({
            "content": {
                "dataSourceType": "S3",
                "s3": { "s3Location": { "uri": format!("s3://fidelity-docs/{id}.txt") } },
            },
        }),
        other => panic!("{other} is not a modelled ContentDataSourceType"),
    }
}

/// The identifier the store holds a document of `content_type` under.
fn identifier_of_type(content_type: &str, id: &str) -> Value {
    match content_type {
        "CUSTOM" => json!({ "dataSourceType": "CUSTOM", "custom": { "id": id } }),
        "S3" => json!({
            "dataSourceType": "S3",
            "s3": { "uri": format!("s3://fidelity-docs/{id}.txt") },
        }),
        other => panic!("{other} is not a modelled ContentDataSourceType"),
    }
}

/// The sentence AWS words when a document's `dataSourceType` is not the type of
/// the data source the request addresses: the kind the document named is the one
/// variable part.
fn type_mismatch_message(document_type: &str) -> String {
    format!(
        "The dataSourceType that you specified in {document_type} doesn't match the type \
         of the data source specified in the request header. Check that both types match \
         and retry your request."
    )
}

/// The sentence AWS words when a vector knowledge base is configured with no
/// vector store: the whole sentence is fixed text.
fn missing_storage_configuration_message() -> String {
    "You must provide a storage configuration if you create a VECTOR knowledge base.".to_string()
}

/// The sentence `DeleteDataSource` words for the data source it cannot resolve:
/// both identifiers of the pair that addresses it, taken from the request path.
fn data_source_not_found_message(data_source_id: &str, knowledge_base_id: &str) -> String {
    format!(
        "DataSource with id {data_source_id} in knowledgeBase with id {knowledge_base_id} \
         is not found."
    )
}

/// Assert `answer` is the modelled refusal `expected` names, carrying the error
/// type in the `x-amzn-errortype` header and `message` the sentence AWS words.
fn assert_refusal(case: &str, expected: (&str, u16), expected_message: &str, answer: &Answer) {
    let (code, status) = expected;
    assert_eq!(
        answer.status, status,
        "case {case}: expected HTTP {status}, got {answer:?}"
    );
    assert_eq!(
        answer.error_type_header.as_deref(),
        Some(code),
        "case {case}: expected {code} in x-amzn-errortype, got {answer:?}"
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some(code),
        "case {case}: expected {code}, got {answer:?}"
    );
    assert_eq!(
        answer.message(),
        expected_message,
        "case {case}: expected the refusal to be worded {expected_message:?}, got {:?}",
        answer.message()
    );
}

/// `IngestKnowledgeBaseDocuments` takes a document only into a data source whose
/// own type is the type the document's content names; a document naming any
/// other type is refused with the modelled `ValidationException` and nothing of
/// the request is written.
///
/// The rows span every modelled `DataSourceType` crossed with both modelled
/// `ContentDataSourceType` values, so they differ along the kind of the data
/// source (including the kinds no content can name, which therefore take no
/// directly ingested document at all), the kind the content names, and whether
/// the two agree.
#[tokio::test]
async fn ingestion_refuses_a_document_whose_type_is_not_the_data_sources_own_type() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("fidelity-ingest-kb"))
        .await;

    // Every value of the modelled `DataSourceType`
    // (`smithy_query(query="shape",shape="DataSourceType")`), each paired here
    // with a data source of that kind.
    let data_source_types = [
        "CUSTOM",
        "S3",
        "WEB",
        "CONFLUENCE",
        "REDSHIFT_METADATA",
        "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
    ];
    // Every value of the modelled `ContentDataSourceType`
    // (`smithy_query(query="shape",shape="ContentDataSourceType")`).
    let content_types = ["CUSTOM", "S3"];

    for data_source_type in data_source_types {
        let data_source_id = emulator
            .data_source(
                &knowledge_base_id,
                data_source_of_type(
                    &format!("fidelity-{}", data_source_type.to_lowercase()),
                    data_source_type,
                ),
            )
            .await;

        for content_type in content_types {
            let case = format!("{content_type} content into a {data_source_type} data source");
            let id = format!("{}-{}", data_source_type.to_lowercase(), content_type.to_lowercase());
            let answer = emulator
                .ingest(
                    &knowledge_base_id,
                    &data_source_id,
                    vec![document_of_type(content_type, &id)],
                )
                .await;

            if content_type == data_source_type {
                assert_eq!(
                    answer.status, 202,
                    "case {case}: the kinds agree, so the ingestion is accepted, got {answer:?}"
                );
                continue;
            }

            assert_refusal(
                &case,
                VALIDATION,
                &type_mismatch_message(content_type),
                &answer,
            );

            // Nothing of a refused request is written: the identifier the
            // document would have been stored under reports the status the
            // model gives a document that was not found.
            let fetched = emulator
                .get_documents(
                    &knowledge_base_id,
                    &data_source_id,
                    vec![identifier_of_type(content_type, &id)],
                )
                .await;
            assert_eq!(
                fetched.status, 200,
                "case {case}: expected GetKnowledgeBaseDocuments -> 200, got {fetched:?}"
            );
            assert_eq!(
                fetched.member(&["documentDetails"])[0]["status"].as_str(),
                Some(DOCUMENT_NOT_FOUND),
                "case {case}: a refused ingestion persists no document, got {fetched:?}"
            );
        }
    }

    // A request carrying one document the data source takes and one it does not
    // is refused as a whole, and neither document is written: the refusal is of
    // the REQUEST, not of a document inside it.
    let custom_source = emulator
        .data_source(
            &knowledge_base_id,
            data_source_of_type("fidelity-partial", "CUSTOM"),
        )
        .await;
    let mixed = emulator
        .ingest(
            &knowledge_base_id,
            &custom_source,
            vec![
                document_of_type("CUSTOM", "would-be-taken"),
                document_of_type("S3", "the-mismatched-one"),
            ],
        )
        .await;
    assert_refusal(
        "a mismatched document alongside one the data source takes",
        VALIDATION,
        &type_mismatch_message("S3"),
        &mixed,
    );
    let fetched = emulator
        .get_documents(
            &knowledge_base_id,
            &custom_source,
            vec![identifier_of_type("CUSTOM", "would-be-taken")],
        )
        .await;
    assert_eq!(
        fetched.member(&["documentDetails"])[0]["status"].as_str(),
        Some(DOCUMENT_NOT_FOUND),
        "case a mismatched document alongside one the data source takes: nothing of the \
         request is written, got {fetched:?}"
    );
}

/// A knowledge base whose configuration names the vector type needs a vector
/// store: a request that leaves `storageConfiguration` out is refused with the
/// modelled `ValidationException` and the sentence AWS words for it — on the
/// create and on the update alike — while a kind that has no vector store of its
/// own is served without one.
#[tokio::test]
async fn a_vector_knowledge_base_without_a_storage_configuration_is_refused() {
    let emulator = Emulator::new();

    // A knowledge base to update: it was created WITH a vector store, so the
    // refusals below are the request's doing.
    let vector_kb = emulator
        .knowledge_base(vector_knowledge_base("fidelity-storage-kb"))
        .await;
    let managed_kb = emulator
        .knowledge_base(managed_knowledge_base("fidelity-managed-kb"))
        .await;

    // Rows: the create and the update, each with no `storageConfiguration`, over
    // the embedding model the knowledge base already has and over a different
    // one — a request that asks for both changes at once is still refused for
    // the storage configuration it does not carry.
    let refused: Vec<(&str, Answer)> = vec![
        (
            "CreateKnowledgeBase with no storageConfiguration",
            emulator
                .create_knowledge_base(vector_knowledge_base_without_storage(
                    "fidelity-storageless-kb",
                    EMBEDDING_MODEL,
                ))
                .await,
        ),
        (
            "UpdateKnowledgeBase with no storageConfiguration",
            emulator
                .update_knowledge_base(
                    &vector_kb,
                    vector_knowledge_base_without_storage(
                        "fidelity-storage-kb",
                        EMBEDDING_MODEL,
                    ),
                )
                .await,
        ),
        (
            "UpdateKnowledgeBase changing the embedding model with no storageConfiguration",
            emulator
                .update_knowledge_base(
                    &vector_kb,
                    vector_knowledge_base_without_storage(
                        "fidelity-storage-kb",
                        OTHER_EMBEDDING_MODEL,
                    ),
                )
                .await,
        ),
        (
            "UpdateKnowledgeBase renaming it with no storageConfiguration",
            emulator
                .update_knowledge_base(
                    &vector_kb,
                    vector_knowledge_base_without_storage(
                        "fidelity-storage-kb-renamed",
                        EMBEDDING_MODEL,
                    ),
                )
                .await,
        ),
    ];
    for (case, answer) in refused {
        assert_refusal(
            case,
            VALIDATION,
            &missing_storage_configuration_message(),
            &answer,
        );
    }

    // Rows: the kinds that carry no vector store of their own, which the same
    // gate must not refuse.
    let served = emulator
        .create_knowledge_base(managed_knowledge_base("fidelity-managed-second-kb"))
        .await;
    assert_eq!(
        served.status, 202,
        "case CreateKnowledgeBase of a MANAGED knowledge base: a kind with no vector store \
         needs no storageConfiguration, got {served:?}"
    );
    let updated = emulator
        .update_knowledge_base(&managed_kb, managed_knowledge_base("fidelity-managed-kb"))
        .await;
    assert_eq!(
        updated.status, 202,
        "case UpdateKnowledgeBase of a MANAGED knowledge base: a kind with no vector store \
         needs no storageConfiguration, got {updated:?}"
    );
}

/// `UpdateKnowledgeBase` cannot change the knowledge base's type: the `type`
/// member is the discriminator of the whole configuration, so a request naming
/// another one is refused with the modelled `ValidationException` in those terms
/// rather than as a member whose value differs, whichever type it names.
#[tokio::test]
async fn updating_the_type_of_a_knowledge_base_is_refused_in_its_own_terms() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("fidelity-type-kb"))
        .await;
    let stored_storage = vector_knowledge_base("fidelity-type-kb")["storageConfiguration"].clone();

    // Rows: each other value of the modelled `KnowledgeBaseType`
    // (`smithy_query(query="shape",shape="KnowledgeBaseType")`) that carries its
    // own configuration member, so the request is complete and the refusal is
    // about the type it names rather than about a member it left out. The stored
    // vector store is re-supplied, so the storage-configuration gate is not what
    // fires either.
    for (case, configuration) in [
        (
            "MANAGED",
            json!({
                "type": "MANAGED",
                "managedKnowledgeBaseConfiguration": { "embeddingModelType": "MANAGED" },
            }),
        ),
        (
            "KENDRA",
            json!({
                "type": "KENDRA",
                "kendraKnowledgeBaseConfiguration": {
                    "kendraIndexArn": "arn:aws:kendra:eu-west-1:000000000000:index/\
                                       a0000000-0000-0000-0000-000000000000",
                },
            }),
        ),
    ] {
        let refused = emulator
            .update_knowledge_base(
                &knowledge_base_id,
                json!({
                    "name": "fidelity-type-kb",
                    "roleArn": KB_ROLE,
                    "knowledgeBaseConfiguration": configuration,
                    "storageConfiguration": stored_storage.clone(),
                }),
            )
            .await;
        assert_refusal(
            &format!("an update naming the {case} type"),
            VALIDATION,
            "You can't modify the type of the knowledge base after creation.",
            &refused,
        );
    }

    // The stored type, re-supplied with the stored vector store: served, so the
    // refusals above are the changed type's doing.
    let served = emulator
        .update_knowledge_base(&knowledge_base_id, vector_knowledge_base("fidelity-type-kb"))
        .await;
    assert_eq!(
        served.status, 202,
        "case the stored type, re-supplied: expected UpdateKnowledgeBase -> 202, got {served:?}"
    );
}

/// `DeleteDataSource` addresses the data source by the pair of path labels, so
/// the resource it reports missing is that DATA SOURCE — named with both
/// identifiers of the pair — whenever the pair resolves to nothing, whichever
/// half of it the store does not hold.
///
/// The rows differ along which half is absent: neither id stored, the knowledge
/// base stored and the data source not, a stored data source addressed under
/// another knowledge base, a knowledge base itself on its way out of the store,
/// and a data source whose own removal the clock has already made due.
#[tokio::test]
async fn delete_data_source_reports_the_data_source_the_pair_does_not_resolve_to() {
    let emulator = Emulator::new();

    // Well-formed identifiers the store holds nothing for: each matches the
    // modelled `^[0-9a-zA-Z]{10}$` pattern of the path labels, so the refusal
    // comes from the store rather than from the boundary validator.
    let unknown_knowledge_base = "0123456789";
    let unknown_data_source = "aZ09aZ09aZ";

    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("fidelity-delete-kb"))
        .await;
    let other_knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("fidelity-delete-other-kb"))
        .await;
    let data_source_id = emulator
        .data_source(
            &knowledge_base_id,
            data_source_of_type("fidelity-delete-ds", "CUSTOM"),
        )
        .await;
    let doomed_knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("fidelity-delete-doomed-kb"))
        .await;
    let doomed_data_source_id = emulator
        .data_source(
            &doomed_knowledge_base_id,
            data_source_of_type("fidelity-delete-doomed-ds", "CUSTOM"),
        )
        .await;
    let deleted = emulator
        .delete_knowledge_base(&doomed_knowledge_base_id)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case setup: expected DeleteKnowledgeBase -> 202, got {deleted:?}"
    );

    for (case, knowledge_base, data_source) in [
        (
            "neither id is stored",
            unknown_knowledge_base,
            unknown_data_source,
        ),
        (
            "the knowledge base is stored and the data source is not",
            knowledge_base_id.as_str(),
            unknown_data_source,
        ),
        (
            "a stored data source addressed under another knowledge base",
            other_knowledge_base_id.as_str(),
            data_source_id.as_str(),
        ),
        (
            "the data source is stored and its knowledge base is not",
            unknown_knowledge_base,
            data_source_id.as_str(),
        ),
        (
            "the knowledge base is itself on its way out of the store",
            doomed_knowledge_base_id.as_str(),
            doomed_data_source_id.as_str(),
        ),
    ] {
        let answer = emulator.delete_data_source(knowledge_base, data_source).await;
        assert_refusal(
            case,
            NOT_FOUND,
            &data_source_not_found_message(data_source, knowledge_base),
            &answer,
        );
    }

    // A data source whose own removal the clock has already made due is gone,
    // and a delete addressed to it reports the same sentence.
    let accepted = emulator
        .delete_data_source(&knowledge_base_id, &data_source_id)
        .await;
    assert_eq!(
        accepted.status, 202,
        "case setup: expected DeleteDataSource -> 202, got {accepted:?}"
    );
    tokio::time::sleep(DELETION_SPAN + MARGIN).await;
    let after_removal = emulator
        .delete_data_source(&knowledge_base_id, &data_source_id)
        .await;
    assert_refusal(
        "a data source whose removal the clock has already made due",
        NOT_FOUND,
        &data_source_not_found_message(&data_source_id, &knowledge_base_id),
        &after_removal,
    );
}
