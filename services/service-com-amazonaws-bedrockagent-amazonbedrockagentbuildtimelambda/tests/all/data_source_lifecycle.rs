// SPDX-License-Identifier: CC0-1.0

//! The data-source slice, end to end over HTTP: `CreateDataSource` attaches a
//! data source to a stored knowledge base and `GetDataSource` reports that
//! stored record back.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather
//! than calling a handler directly.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
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

/// Every member `CreateDataSource` accepts and the modelled `DataSource`
/// reports back, so a request's value for each must survive the round trip
/// verbatim. `dataDeletionPolicy` is excluded: it is the one member with a
/// documented default, and it is asserted on its own.
const ECHOED_MEMBERS: [&str; 5] = [
    "name",
    "description",
    "dataSourceConfiguration",
    "serverSideEncryptionConfiguration",
    "vectorIngestionConfiguration",
];

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
    /// A member of the response's `dataSource`.
    fn ds(&self, member: &str) -> &Value {
        &self.body["dataSource"][member]
    }

    fn ds_string(&self, member: &str) -> String {
        self.ds(member)
            .as_str()
            .unwrap_or_else(|| panic!("dataSource.{member} is not a string in {:?}", self.body))
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

    async fn create_data_source(&self, knowledge_base_id: &str, request: Value) -> Answer {
        self.call(
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
            Some(request),
        )
        .await
    }

    async fn get_data_source(&self, knowledge_base_id: &str, data_source_id: &str) -> Answer {
        self.call(
            "GET",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}"),
            None,
        )
        .await
    }
}

/// A self-managed vector knowledge base over an S3 vector store: the kind the
/// S3-ingestion scenario attaches a data source to.
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
                "vectorBucketArn": "arn:aws:s3vectors:::bucket/ingestion-demo-vectors",
                "indexName": "ingestion-demo-index",
            },
        },
    })
}

/// A service-managed knowledge base: the one kind whose data sources have a
/// documented `dataDeletionPolicy` default.
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

/// Every error `CreateDataSource` declares, paired with the HTTP status its
/// model binds to that error.
///
/// Source: this service's Smithy model
/// (`smithy_query(query="operation",operation="CreateDataSource")`).
const CREATE_DATA_SOURCE_MODELLED_ERRORS: [(&str, u16); 7] = [
    ("AccessDeniedException", 403),
    ("ConflictException", 409),
    ("InternalServerException", 500),
    ("ResourceNotFoundException", 404),
    ("ServiceQuotaExceededException", 402),
    ("ThrottlingException", 429),
    ("ValidationException", 400),
];

/// The same for `GetDataSource`, whose union is narrower: it mints nothing, so
/// it declares neither a conflict nor a quota refusal.
///
/// Source: this service's Smithy model
/// (`smithy_query(query="operation",operation="GetDataSource")`).
const GET_DATA_SOURCE_MODELLED_ERRORS: [(&str, u16); 5] = [
    ("AccessDeniedException", 403),
    ("InternalServerException", 500),
    ("ResourceNotFoundException", 404),
    ("ThrottlingException", 429),
    ("ValidationException", 400),
];

/// The code an operation nobody has implemented answers with. It is in no
/// operation's modelled union, so a client's generated error taxonomy has no
/// branch for it and it surfaces as a bare client error.
const UNIMPLEMENTED_CODE: &str = "NotImplemented";

/// Assert `answer` refuses `operation` with one of the errors that operation's
/// model declares, carrying the status the model binds to it.
///
/// The exact-code check subsumes the two before it; those two are kept because
/// they are what make a failure read as "this code is outside the operation's
/// union" rather than as a plain string mismatch.
fn assert_modelled_refusal(
    case: &str,
    operation: &str,
    union: &[(&str, u16)],
    expected_code: &str,
    answer: &Answer,
) {
    let code = answer.error_type.clone().unwrap_or_else(|| {
        panic!("case {case}: expected {operation} to answer an error code, got {answer:?}")
    });
    assert_ne!(
        code, UNIMPLEMENTED_CODE,
        "case {case}: {operation} is implemented, so it must never answer the \
         unimplemented-operation code, got {answer:?}"
    );
    let Some((_, modelled_status)) = union.iter().find(|(name, _)| *name == code) else {
        panic!(
            "case {case}: {operation} answered {code:?}, which is not among the errors its \
             model declares ({union:?}), got {answer:?}"
        );
    };
    assert_eq!(
        code, expected_code,
        "case {case}: expected {operation} to answer {expected_code}, got {answer:?}"
    );
    assert_eq!(
        answer.status, *modelled_status,
        "case {case}: expected {code} to carry HTTP {modelled_status}, got {answer:?}"
    );
}

/// `CreateDataSource` and `GetDataSource` are implemented, so each one serves a
/// well-formed request and refuses every other with an error its OWN model
/// declares, at that error's modelled status — never with the code an
/// unimplemented operation answers, which no client's error taxonomy carries a
/// branch for.
#[tokio::test]
async fn data_source_operations_refuse_only_with_their_modelled_errors() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("modelled-errors-kb"))
        .await;

    // A well-formed request is SERVED: the operations answer their modelled
    // success, so the refusals below are refusals of the request rather than of
    // the operation.
    let created = emulator
        .create_data_source(
            &knowledge_base_id,
            json!({
                "name": "modelled-errors-source",
                "dataSourceConfiguration": {
                    "type": "S3",
                    "s3Configuration": { "bucketArn": "arn:aws:s3:::ingestion-demo-docs" },
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case a well-formed create: expected CreateDataSource -> 200, got {created:?}"
    );
    let data_source_id = created.ds_string("dataSourceId");
    let fetched = emulator
        .get_data_source(&knowledge_base_id, &data_source_id)
        .await;
    assert_eq!(
        fetched.status, 200,
        "case a well-formed read: expected GetDataSource -> 200, got {fetched:?}"
    );
    assert_eq!(
        fetched.ds_string("name"),
        "modelled-errors-source",
        "case a well-formed read: expected the name the create supplied, got {:?}",
        fetched.ds("name")
    );

    // Rows differ along the kind of bad request: a resource the store does not
    // hold, a required member left out, and a value outside a member's modelled
    // constraint (its pattern or its length). The refusals that come from the
    // `dataSourceConfiguration` discriminator have their own test below, so they
    // are not restated here; one row per operation names a resource the store
    // does not hold, so the union is exercised on a code other than
    // `ValidationException` too.
    //
    // The constraints are this service's model
    // (`smithy_query(query="operation",operation="CreateDataSource")`):
    // `knowledgeBaseId` and `name` carry patterns, `description` is length
    // 1..200 and `clientToken` is length 33..256.
    let s3_configuration = json!({
        "type": "S3",
        "s3Configuration": { "bucketArn": "arn:aws:s3:::ingestion-demo-docs" },
    });
    let over_long_description = "d".repeat(201);
    let create_cases: Vec<(&str, String, Value, &str)> = vec![
        (
            "a well-formed knowledgeBaseId the store does not hold",
            "0123456789".to_string(),
            json!({
                "name": "orphan-source",
                "dataSourceConfiguration": s3_configuration.clone(),
            }),
            "ResourceNotFoundException",
        ),
        (
            "a knowledgeBaseId shorter than the modelled Id pattern admits",
            "short".to_string(),
            json!({
                "name": "bad-id-source",
                "dataSourceConfiguration": s3_configuration.clone(),
            }),
            "ValidationException",
        ),
        (
            "a knowledgeBaseId longer than the modelled Id pattern admits",
            "0123456789a".to_string(),
            json!({
                "name": "bad-id-source",
                "dataSourceConfiguration": s3_configuration.clone(),
            }),
            "ValidationException",
        ),
        (
            "no name, which the model requires",
            knowledge_base_id.clone(),
            json!({ "dataSourceConfiguration": s3_configuration.clone() }),
            "ValidationException",
        ),
        (
            "no dataSourceConfiguration, which the model requires",
            knowledge_base_id.clone(),
            json!({ "name": "nameless-configuration" }),
            "ValidationException",
        ),
        (
            "a name outside the modelled Name pattern",
            knowledge_base_id.clone(),
            json!({
                "name": "a name with spaces!",
                "dataSourceConfiguration": s3_configuration.clone(),
            }),
            "ValidationException",
        ),
        (
            "a description shorter than the modelled minimum length",
            knowledge_base_id.clone(),
            json!({
                "name": "empty-description",
                "description": "",
                "dataSourceConfiguration": s3_configuration.clone(),
            }),
            "ValidationException",
        ),
        (
            "a description longer than the modelled maximum length",
            knowledge_base_id.clone(),
            json!({
                "name": "long-description",
                "description": over_long_description,
                "dataSourceConfiguration": s3_configuration.clone(),
            }),
            "ValidationException",
        ),
        (
            "a clientToken shorter than the modelled minimum length",
            knowledge_base_id.clone(),
            json!({
                "name": "short-token",
                "clientToken": "too-short",
                "dataSourceConfiguration": s3_configuration,
            }),
            "ValidationException",
        ),
    ];

    for (case, requested_knowledge_base, request, expected_code) in create_cases {
        let refused = emulator
            .create_data_source(&requested_knowledge_base, request)
            .await;
        assert_modelled_refusal(
            case,
            "CreateDataSource",
            &CREATE_DATA_SOURCE_MODELLED_ERRORS,
            expected_code,
            &refused,
        );
    }

    // The reader's own union is narrower, and the same rule holds over it.
    let get_cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a data-source id nothing holds",
            knowledge_base_id.as_str(),
            "0123456789",
            "ResourceNotFoundException",
        ),
        (
            "a dataSourceId outside the modelled Id pattern",
            knowledge_base_id.as_str(),
            "short",
            "ValidationException",
        ),
        (
            "a knowledgeBaseId outside the modelled Id pattern",
            "short",
            data_source_id.as_str(),
            "ValidationException",
        ),
    ];

    for (case, requested_knowledge_base, requested_data_source, expected_code) in get_cases {
        let refused = emulator
            .get_data_source(requested_knowledge_base, requested_data_source)
            .await;
        assert_modelled_refusal(
            case,
            "GetDataSource",
            &GET_DATA_SOURCE_MODELLED_ERRORS,
            expected_code,
            &refused,
        );
    }
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

/// `CreateDataSource` persists every member the request supplied and
/// `GetDataSource` reports that record back — over a table of requests that
/// differ in the `dataSourceConfiguration.type` discriminator, the kind of the
/// owning knowledge base, which optional members they carry and the form of
/// the name.
#[tokio::test]
async fn created_data_sources_round_trip_through_get_data_source() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: the
    // `dataSourceConfiguration.type` discriminator (kinds that carry a
    // connection member and kinds the model pairs with none), which optional
    // members are present, and the form of the modelled `Name`.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "an S3 data source with only its required members",
            vector_knowledge_base("s3-minimal-kb"),
            json!({
                "name": "s3-documents",
                "dataSourceConfiguration": {
                    "type": "S3",
                    "s3Configuration": { "bucketArn": "arn:aws:s3:::ingestion-demo-docs" },
                },
            }),
        ),
        (
            "an S3 data source carrying every optional member",
            vector_knowledge_base("s3-full-kb"),
            json!({
                "name": "S3_documents1",
                "description": "the bucket the retrieve-and-generate scenario reads from",
                "dataSourceConfiguration": {
                    "type": "S3",
                    "s3Configuration": {
                        "bucketArn": "arn:aws:s3:::ingestion-demo-docs",
                        "inclusionPrefixes": ["documents/"],
                        "bucketOwnerAccountId": "000000000000",
                    },
                },
                "dataDeletionPolicy": "RETAIN",
                "serverSideEncryptionConfiguration": {
                    "kmsKeyArn": "arn:aws:kms:eu-west-1:000000000000:key/\
                                  12345678-1234-1234-1234-123456789012",
                },
                "vectorIngestionConfiguration": {
                    "chunkingConfiguration": {
                        "chunkingStrategy": "FIXED_SIZE",
                        "fixedSizeChunkingConfiguration": {
                            "maxTokens": 300,
                            "overlapPercentage": 20,
                        },
                    },
                },
            }),
        ),
        (
            "a WEB data source",
            vector_knowledge_base("web-kb"),
            json!({
                "name": "web-crawl",
                "dataSourceConfiguration": {
                    "type": "WEB",
                    "webConfiguration": {
                        "sourceConfiguration": {
                            "urlConfiguration": {
                                "seedUrls": [{ "url": "https://example.com/docs" }],
                            },
                        },
                    },
                },
            }),
        ),
        (
            // CUSTOM is one of the kinds the model pairs with no sibling
            // configuration member, so it needs none.
            "a CUSTOM data source, whose type carries no connection member",
            vector_knowledge_base("custom-kb"),
            json!({
                "name": "custom_documents",
                "description": "documents pushed in through the ingestion API",
                "dataSourceConfiguration": { "type": "CUSTOM" },
            }),
        ),
        (
            "a REDSHIFT_METADATA data source, whose type carries no connection member",
            vector_knowledge_base("redshift-kb"),
            json!({
                "name": "redshift-metadata-0",
                "dataSourceConfiguration": { "type": "REDSHIFT_METADATA" },
            }),
        ),
        (
            // A managed knowledge base: the documented default for
            // `dataDeletionPolicy` applies here and nowhere else.
            //
            // This row supplies the whole EFFECTIVE connector configuration —
            // the media-extraction configuration and the connector parameters a
            // request may instead leave to the service — so it asserts that
            // what the request DID write is stored exactly as written. What the
            // service completes for a request that leaves them unset, and the
            // status a connector with a source to reach takes, are asserted in
            // `managed_connector_defaults.rs`.
            "a managed-connector data source of a MANAGED knowledge base",
            managed_knowledge_base("managed-kb"),
            json!({
                "name": "managed-connector",
                "dataSourceConfiguration": {
                    "type": "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
                    "managedKnowledgeBaseConnectorConfiguration": {
                        // The connector's own parameter document, as the
                        // service reports it: a JSON text whose members are in
                        // sorted order.
                        "connectorParameters": "{\"type\":\"CUSTOM\",\
                             \"filterConfiguration\":{\"maxFileSizeInMegaBytes\":\"250\"},\
                             \"aclEnabled\":false,\"version\":\"1\"}",
                        "mediaExtractionConfiguration": {
                            "imageExtractionConfiguration": {
                                "imageExtractionStatus": "DISABLED",
                            },
                        },
                    },
                },
            }),
        ),
    ];

    for (case, knowledge_base_request, request) in cases {
        let managed = knowledge_base_request["knowledgeBaseConfiguration"]["type"] == "MANAGED";
        let knowledge_base_id = emulator.knowledge_base(knowledge_base_request).await;

        let created = emulator
            .create_data_source(&knowledge_base_id, request.clone())
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected CreateDataSource -> 200, got {created:?}"
        );

        let data_source_id = created.ds_string("dataSourceId");
        assert_well_formed_id(case, "dataSourceId", &data_source_id);
        assert_eq!(
            created.ds_string("knowledgeBaseId"),
            knowledge_base_id,
            "case {case}: the data source belongs to the knowledge base the URI named, got {:?}",
            created.ds("knowledgeBaseId")
        );
        assert_eq!(
            created.ds_string("status"),
            "AVAILABLE",
            "case {case}: a data source this call created is ready for ingestion, got {created:?}"
        );
        // `createdAt`/`updatedAt` target `DateTimestamp`, which carries
        // `@timestampFormat("date-time")`
        // (`smithy_query(query="shape", shape="DataSource")`), so each one is populated
        // AND carried as an RFC-3339 string rather than an epoch number.
        for member in ["createdAt", "updatedAt"] {
            let at = created.ds(member);
            let text = at.as_str().unwrap_or_else(|| {
                panic!("case {case}: expected {member} to be a date-time string, got {at:?}")
            });
            assert!(
                text.contains('T') && text.ends_with('Z'),
                "case {case}: expected {member} in the modelled date-time wire form, \
                 got {text:?}"
            );
        }
        assert!(
            created.ds("failureReasons").is_null(),
            "case {case}: nothing has failed on a data source just created, got {:?}",
            created.ds("failureReasons")
        );

        // The documented default: a managed knowledge base's data source takes
        // DELETE when the request names no policy; for any other kind the
        // member stays absent rather than being invented.
        let expected_policy = request
            .get("dataDeletionPolicy")
            .cloned()
            .unwrap_or_else(|| if managed { json!("DELETE") } else { Value::Null });
        assert_eq!(
            created.ds("dataDeletionPolicy"),
            &expected_policy,
            "case {case}: expected dataDeletionPolicy {expected_policy:?}, got {:?}",
            created.ds("dataDeletionPolicy")
        );

        // The round trip: the stored record, read back through GetDataSource.
        let fetched = emulator
            .get_data_source(&knowledge_base_id, &data_source_id)
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetDataSource -> 200, got {fetched:?}"
        );
        assert_eq!(
            fetched.body["dataSource"], created.body["dataSource"],
            "case {case}: GetDataSource must report the data source exactly as \
             CreateDataSource persisted it, got {:?}",
            fetched.body
        );

        // Every member the request supplied survives the round trip verbatim,
        // and nothing is invented for one it omitted.
        for member in ECHOED_MEMBERS {
            if let Some(supplied) = request.get(member) {
                assert_eq!(
                    fetched.ds(member),
                    supplied,
                    "case {case}: expected {member} to round-trip as {supplied:?}, got {:?}",
                    fetched.ds(member)
                );
            } else {
                assert!(
                    fetched.ds(member).is_null(),
                    "case {case}: {member} was not supplied, so nothing may be invented for it, \
                     got {:?}",
                    fetched.ds(member)
                );
            }
        }

        // A read changes nothing a later caller sees.
        let again = emulator
            .get_data_source(&knowledge_base_id, &data_source_id)
            .await;
        assert_eq!(
            again.body, fetched.body,
            "case {case}: a read must not change what a later caller sees"
        );
    }
}

/// `dataSourceConfiguration.type` is a discriminator: the sibling member it
/// names must be supplied. A request that does not satisfy its own
/// discriminator is refused with `ValidationException` (400) — not accepted,
/// and not reported as a missing resource.
#[tokio::test]
async fn create_data_source_refuses_a_configuration_its_type_does_not_carry() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("refusals-kb"))
        .await;

    // One row per modelled `DataSourceType` the model pairs with a
    // configuration member, each with that member missing, plus a type the
    // model does not declare at all.
    //
    // Source: the `DataSourceType` enum and the `DataSourceConfiguration`
    // structure of this service's Smithy model.
    let cases: Vec<(&str, Value)> = vec![
        ("S3 without s3Configuration", json!({ "type": "S3" })),
        ("WEB without webConfiguration", json!({ "type": "WEB" })),
        (
            "CONFLUENCE without confluenceConfiguration",
            json!({ "type": "CONFLUENCE" }),
        ),
        (
            "SALESFORCE without salesforceConfiguration",
            json!({ "type": "SALESFORCE" }),
        ),
        (
            "SHAREPOINT without sharePointConfiguration",
            json!({ "type": "SHAREPOINT" }),
        ),
        (
            "MANAGED_KNOWLEDGE_BASE_CONNECTOR without its own configuration member",
            json!({ "type": "MANAGED_KNOWLEDGE_BASE_CONNECTOR" }),
        ),
        (
            "S3 carrying another kind's configuration member",
            json!({
                "type": "S3",
                "webConfiguration": {
                    "sourceConfiguration": {
                        "urlConfiguration": { "seedUrls": [{ "url": "https://example.com" }] },
                    },
                },
            }),
        ),
        (
            "a type the model does not declare",
            json!({
                "type": "FTP",
                "s3Configuration": { "bucketArn": "arn:aws:s3:::ingestion-demo-docs" },
            }),
        ),
    ];

    for (case, configuration) in cases {
        let refused = emulator
            .create_data_source(
                &knowledge_base_id,
                json!({ "name": "refused-source", "dataSourceConfiguration": configuration }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected CreateDataSource -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {refused:?}"
        );
    }

    // A refused create persisted nothing: a well-formed request still works.
    let created = emulator
        .create_data_source(
            &knowledge_base_id,
            json!({
                "name": "after-refusals",
                "dataSourceConfiguration": { "type": "CUSTOM" },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case after the refusals: a well-formed create is still served, got {created:?}"
    );
}

/// A data source is identified by the knowledge base it belongs to as well as
/// its own id: `CreateDataSource` answers `ResourceNotFoundException` (404) for
/// a knowledge base the store does not hold, and `GetDataSource` does the same
/// for a data-source id nothing holds or one that belongs to another knowledge
/// base.
#[tokio::test]
async fn data_sources_are_scoped_to_the_knowledge_base_that_holds_them() {
    let emulator = Emulator::new();
    let first_kb = emulator.knowledge_base(vector_knowledge_base("scope-kb-1")).await;
    let second_kb = emulator.knowledge_base(vector_knowledge_base("scope-kb-2")).await;

    // Rows: well-formed knowledgeBaseIds the store does not hold.
    for unknown in ["0123456789", "aZ09aZ09aZ"] {
        let refused = emulator
            .create_data_source(
                unknown,
                json!({
                    "name": "orphan-source",
                    "dataSourceConfiguration": { "type": "CUSTOM" },
                }),
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case knowledge base {unknown}: expected CreateDataSource -> 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case knowledge base {unknown}: expected ResourceNotFoundException, got {refused:?}"
        );
    }

    let created = emulator
        .create_data_source(
            &first_kb,
            json!({
                "name": "scoped-source",
                "dataSourceConfiguration": {
                    "type": "S3",
                    "s3Configuration": { "bucketArn": "arn:aws:s3:::ingestion-demo-docs" },
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected CreateDataSource -> 200, got {created:?}"
    );
    let data_source_id = created.ds_string("dataSourceId");

    // Rows: a data-source id nothing holds, one differing from the stored id in
    // its last character only, and the stored id read through the OTHER
    // knowledge base.
    let mut nearly = data_source_id.clone();
    nearly.pop();
    nearly.push(if data_source_id.ends_with('X') { 'Y' } else { 'X' });
    let cases: Vec<(&str, &str, &str)> = vec![
        ("an id nothing holds", first_kb.as_str(), "0123456789"),
        (
            "an id differing in its last character",
            first_kb.as_str(),
            nearly.as_str(),
        ),
        (
            "the stored id read through another knowledge base",
            second_kb.as_str(),
            data_source_id.as_str(),
        ),
    ];
    for (case, knowledge_base_id, requested) in cases {
        if requested == data_source_id && knowledge_base_id == first_kb {
            continue;
        }
        let fetched = emulator.get_data_source(knowledge_base_id, requested).await;
        assert_eq!(
            fetched.status, 404,
            "case {case}: expected GetDataSource -> 404, got {fetched:?}"
        );
        assert_eq!(
            fetched.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {fetched:?}"
        );
    }

    // The data source that does exist is still readable through its own
    // knowledge base.
    let fetched = emulator.get_data_source(&first_kb, &data_source_id).await;
    assert_eq!(
        fetched.status, 200,
        "case the stored data source: expected GetDataSource -> 200, got {fetched:?}"
    );
}

/// A repeated `@idempotencyToken` returns the data source the first call
/// created, and two data sources that share a name are still two distinct data
/// sources — the model constrains `Name`'s shape, not its uniqueness.
#[tokio::test]
async fn create_data_source_honours_its_idempotency_token_and_mints_one_per_call() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(vector_knowledge_base("idempotency-kb"))
        .await;

    // Two tokens (each at least the modelled 33 characters), each replayed.
    for token in [
        "idempotency-token-for-the-first-ds",
        "IdempotencyTokenForTheSecondDs00000",
    ] {
        let request = json!({
            "name": "idempotent-source",
            "dataSourceConfiguration": {
                "type": "S3",
                "s3Configuration": { "bucketArn": "arn:aws:s3:::ingestion-demo-docs" },
            },
            "clientToken": token,
        });
        let once = emulator
            .create_data_source(&knowledge_base_id, request.clone())
            .await;
        let again = emulator
            .create_data_source(&knowledge_base_id, request)
            .await;
        assert_eq!(
            once.status, 200,
            "case token {token}: expected 200 on the first call, got {once:?}"
        );
        assert_eq!(
            again.status, 200,
            "case token {token}: expected 200 on the replay, got {again:?}"
        );
        assert_eq!(
            again.body["dataSource"], once.body["dataSource"],
            "case token {token}: a replayed idempotency token must return the data source the \
             first call created, got {:?}",
            again.body
        );
    }

    // Same name, no token: distinct data sources, each readable back.
    let request = json!({
        "name": "shared-name-source",
        "dataSourceConfiguration": { "type": "CUSTOM" },
    });
    let first = emulator
        .create_data_source(&knowledge_base_id, request.clone())
        .await;
    let second = emulator
        .create_data_source(&knowledge_base_id, request)
        .await;
    assert_eq!(
        second.status, 200,
        "case duplicate name: the model constrains Name's pattern, not its uniqueness, \
         got {second:?}"
    );
    assert_ne!(
        first.ds_string("dataSourceId"),
        second.ds_string("dataSourceId"),
        "case duplicate name: each create mints its own dataSourceId"
    );
    for (case, answer) in [("first", &first), ("second", &second)] {
        let id = answer.ds_string("dataSourceId");
        let fetched = emulator.get_data_source(&knowledge_base_id, &id).await;
        assert_eq!(
            fetched.body["dataSource"], answer.body["dataSource"],
            "case duplicate name {case}: GetDataSource reports the data source create minted"
        );
    }
}
