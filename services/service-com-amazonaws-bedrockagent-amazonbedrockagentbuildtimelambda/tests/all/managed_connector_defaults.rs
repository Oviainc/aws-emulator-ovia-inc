// SPDX-License-Identifier: CC0-1.0

//! What the service STORES for a managed knowledge base connector data source,
//! end to end over HTTP: the connector configuration the request supplied
//! completed with the defaults the service applies, and the status a data
//! source takes from whether its connector has a source outside the service to
//! reach.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather
//! than calling a handler directly.

use std::sync::Arc;
use std::time::Duration;

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

/// Every value the modelled `DataSourceStatus` enum declares, so a status this
/// emulator reports is asserted to be one of them.
///
/// Source: this service's Smithy model
/// (`smithy_query(query="shape",shape="DataSourceStatus")`).
const DATA_SOURCE_STATUSES: [&str; 6] = [
    "AVAILABLE",
    "DELETING",
    "DELETE_UNSUCCESSFUL",
    "CREATING",
    "UPDATING",
    "FAILED",
];

/// Longer than the span the emulator takes to bring a connector's connection
/// up, so a read after it observes the outcome the clock owed rather than the
/// transient status.
const PAST_THE_SPAN: Duration = Duration::from_millis(900);

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
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

    /// The stored `managedKnowledgeBaseConnectorConfiguration`.
    fn connector(&self) -> &Value {
        &self.body["dataSource"]["dataSourceConfiguration"]
            ["managedKnowledgeBaseConnectorConfiguration"]
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
            "arn:aws:s3vectors:::bucket/connector-defaults-vectors",
            "connector-defaults-index",
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
        Answer { status, body }
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

    async fn update_data_source(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        request: Value,
    ) -> Answer {
        self.call(
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}"),
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

/// A service-managed knowledge base: the kind a managed connector data source
/// is attached to.
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

/// A self-managed vector knowledge base, for the rows whose data source is not
/// a managed connector at all.
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
                "vectorBucketArn": "arn:aws:s3vectors:::bucket/connector-defaults-vectors",
                "indexName": "connector-defaults-index",
            },
        },
    })
}

/// A `MANAGED_KNOWLEDGE_BASE_CONNECTOR` data source request carrying
/// `connector` as its connector configuration.
fn managed_connector_data_source(name: &str, connector: Value) -> Value {
    json!({
        "name": name,
        "dataSourceConfiguration": {
            "type": "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
            "managedKnowledgeBaseConnectorConfiguration": connector,
        },
    })
}

/// Assert `status` is one of the values the model declares for the member.
fn assert_modelled_status(case: &str, status: &str) {
    assert!(
        DATA_SOURCE_STATUSES.contains(&status),
        "case {case}: expected a modelled DataSourceStatus ({DATA_SOURCE_STATUSES:?}), \
         got {status:?}"
    );
}

/// The service runs a managed knowledge base connector, so what it stores for
/// one is the EFFECTIVE connector configuration: the parameters the request
/// supplied, completed with the service's defaults for the ones it left unset
/// and carried as the connector's own parameter document (a JSON text), plus
/// the media-extraction configuration the connector runs with. Every read
/// reports that same stored configuration.
///
/// The text's members are in sorted order, so one stored parameter document has
/// one text and a caller can diff two reads.
#[tokio::test]
async fn managed_connector_configurations_are_stored_completed_with_the_service_defaults() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: which of the
    // defaulted members the request leaves unset, which it supplies (so the
    // supplied value must survive untouched), the connector named by
    // `connectorParameters.type`, and the shape of the parameters themselves
    // (flat members, a nested connection object, a list).
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a CUSTOM connector leaving every defaulted member unset",
            json!({
                "connectorParameters": { "type": "CUSTOM", "version": "1" },
            }),
            json!({
                "connectorParameters": "{\"type\":\"CUSTOM\",\
                     \"filterConfiguration\":{\"maxFileSizeInMegaBytes\":\"500\"},\
                     \"aclEnabled\":false,\"version\":\"1\"}",
                "mediaExtractionConfiguration": {
                    "imageExtractionConfiguration": { "imageExtractionStatus": "ENABLED" },
                },
            }),
        ),
        (
            "an S3 connector whose filter already names a prefix list",
            json!({
                "connectorParameters": {
                    "type": "S3",
                    "version": "1",
                    "connectionConfiguration": { "bucketName": "connector-defaults-docs" },
                    "filterConfiguration": { "inclusionPrefixes": ["documents/"] },
                },
            }),
            json!({
                "connectorParameters": "{\"type\":\"S3\",\
                     \"filterConfiguration\":{\"inclusionPrefixes\":[\"documents/\"],\
                     \"maxFileSizeInMegaBytes\":\"500\"},\
                     \"connectionConfiguration\":{\"bucketName\":\"connector-defaults-docs\"},\
                     \"aclEnabled\":false,\"version\":\"1\"}",
                "mediaExtractionConfiguration": {
                    "imageExtractionConfiguration": { "imageExtractionStatus": "ENABLED" },
                },
            }),
        ),
        (
            "a SHAREPOINT connector that sets both defaulted parameters itself",
            json!({
                "connectorParameters": {
                    "type": "SHAREPOINT",
                    "version": "2",
                    "aclEnabled": true,
                    "filterConfiguration": { "maxFileSizeInMegaBytes": "25" },
                },
            }),
            json!({
                "connectorParameters": "{\"type\":\"SHAREPOINT\",\
                     \"filterConfiguration\":{\"maxFileSizeInMegaBytes\":\"25\"},\
                     \"aclEnabled\":true,\"version\":\"2\"}",
                "mediaExtractionConfiguration": {
                    "imageExtractionConfiguration": { "imageExtractionStatus": "ENABLED" },
                },
            }),
        ),
        (
            "a connector that configures media extraction itself",
            json!({
                "connectorParameters": { "type": "CUSTOM", "version": "1", "aclEnabled": false },
                "mediaExtractionConfiguration": {
                    "imageExtractionConfiguration": { "imageExtractionStatus": "DISABLED" },
                },
            }),
            json!({
                "connectorParameters": "{\"type\":\"CUSTOM\",\"aclEnabled\":false,\
                     \"filterConfiguration\":{\"maxFileSizeInMegaBytes\":\"500\"},\
                     \"version\":\"1\"}",
                "mediaExtractionConfiguration": {
                    "imageExtractionConfiguration": { "imageExtractionStatus": "DISABLED" },
                },
            }),
        ),
        (
            // `mediaExtractionConfiguration` carries three members
            // (`smithy_query(query="shape",shape="MediaExtractionConfiguration")`);
            // the ones the request set stay, and only the image one is
            // completed.
            "a connector configuring audio extraction but not image extraction",
            json!({
                "connectorParameters": { "type": "CUSTOM", "version": "1", "aclEnabled": true },
                "mediaExtractionConfiguration": {
                    "audioExtractionConfiguration": { "audioExtractionStatus": "DISABLED" },
                },
            }),
            json!({
                "connectorParameters": "{\"type\":\"CUSTOM\",\"aclEnabled\":true,\
                     \"filterConfiguration\":{\"maxFileSizeInMegaBytes\":\"500\"},\
                     \"version\":\"1\"}",
                "mediaExtractionConfiguration": {
                    "audioExtractionConfiguration": { "audioExtractionStatus": "DISABLED" },
                    "imageExtractionConfiguration": { "imageExtractionStatus": "ENABLED" },
                },
            }),
        ),
        (
            // The service reports the parameters as a JSON text, so a caller
            // may send back exactly what it read: the text is completed and
            // reported by the same rule as an inlined object.
            "a CUSTOM connector whose parameters arrive as a JSON text",
            json!({
                "connectorParameters": "{\"type\":\"CUSTOM\",\"version\":\"3\"}",
            }),
            json!({
                "connectorParameters": "{\"type\":\"CUSTOM\",\
                     \"filterConfiguration\":{\"maxFileSizeInMegaBytes\":\"500\"},\
                     \"aclEnabled\":false,\"version\":\"3\"}",
                "mediaExtractionConfiguration": {
                    "imageExtractionConfiguration": { "imageExtractionStatus": "ENABLED" },
                },
            }),
        ),
        (
            // No `connectorParameters` at all: there is no connector to
            // complete parameters for, so none are invented — only the
            // media-extraction configuration the connector runs with is stored.
            "a connector configuration carrying no connectorParameters",
            json!({}),
            json!({
                "mediaExtractionConfiguration": {
                    "imageExtractionConfiguration": { "imageExtractionStatus": "ENABLED" },
                },
            }),
        ),
    ];

    for (index, (case, connector, expected)) in cases.into_iter().enumerate() {
        let knowledge_base_id = emulator
            .knowledge_base(managed_knowledge_base(&format!("connector-defaults-kb-{index}")))
            .await;
        let created = emulator
            .create_data_source(
                &knowledge_base_id,
                managed_connector_data_source(&format!("connector-source-{index}"), connector),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected CreateDataSource -> 200, got {created:?}"
        );
        assert_eq!(
            created.connector(),
            &expected,
            "case {case}: expected the create to report the effective connector configuration \
             {expected:?}, got {:?}",
            created.connector()
        );

        // The gap is not a create-time echo: the completed configuration is
        // what the resource HOLDS, so the read reports the same one.
        let data_source_id = created.ds_string("dataSourceId");
        let fetched = emulator
            .get_data_source(&knowledge_base_id, &data_source_id)
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetDataSource -> 200, got {fetched:?}"
        );
        assert_eq!(
            fetched.connector(),
            &expected,
            "case {case}: expected GetDataSource to report the effective connector \
             configuration {expected:?}, got {:?}",
            fetched.connector()
        );
    }
}

/// An update is stored by the same rule: the connector configuration it carries
/// is completed before it is written, so a read after an update reports the
/// effective configuration too.
#[tokio::test]
async fn updated_managed_connector_configurations_are_stored_completed_too() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(managed_knowledge_base("connector-update-kb"))
        .await;
    let created = emulator
        .create_data_source(
            &knowledge_base_id,
            managed_connector_data_source(
                "connector-update-source",
                json!({
                    "connectorParameters": {
                        "type": "CUSTOM",
                        "version": "1",
                        "aclEnabled": false,
                        "filterConfiguration": { "maxFileSizeInMegaBytes": "500" },
                    },
                    "mediaExtractionConfiguration": {
                        "imageExtractionConfiguration": { "imageExtractionStatus": "ENABLED" },
                    },
                }),
            ),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected CreateDataSource -> 200, got {created:?}"
    );
    let data_source_id = created.ds_string("dataSourceId");

    // The rows differ along which defaulted member the update leaves unset and
    // which it names explicitly.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "an update that leaves both defaulted parameters unset",
            json!({ "connectorParameters": { "type": "CUSTOM", "version": "1" } }),
            json!({
                "connectorParameters": "{\"type\":\"CUSTOM\",\
                     \"filterConfiguration\":{\"maxFileSizeInMegaBytes\":\"500\"},\
                     \"aclEnabled\":false,\"version\":\"1\"}",
                "mediaExtractionConfiguration": {
                    "imageExtractionConfiguration": { "imageExtractionStatus": "ENABLED" },
                },
            }),
        ),
        (
            "an update that names a file-size bound of its own",
            json!({
                "connectorParameters": {
                    "type": "CUSTOM",
                    "version": "1",
                    "filterConfiguration": { "maxFileSizeInMegaBytes": "10" },
                },
                "mediaExtractionConfiguration": {
                    "imageExtractionConfiguration": { "imageExtractionStatus": "DISABLED" },
                },
            }),
            json!({
                "connectorParameters": "{\"type\":\"CUSTOM\",\
                     \"filterConfiguration\":{\"maxFileSizeInMegaBytes\":\"10\"},\
                     \"aclEnabled\":false,\"version\":\"1\"}",
                "mediaExtractionConfiguration": {
                    "imageExtractionConfiguration": { "imageExtractionStatus": "DISABLED" },
                },
            }),
        ),
        (
            // The parameters as the previous row's read reported them, put
            // back unchanged: what a read returns is what the next write
            // accepts, and the stored configuration does not drift.
            "an update that sends the parameters back as the JSON text it read",
            json!({
                "connectorParameters": "{\"type\":\"CUSTOM\",\
                     \"filterConfiguration\":{\"maxFileSizeInMegaBytes\":\"10\"},\
                     \"aclEnabled\":false,\"version\":\"1\"}",
                "mediaExtractionConfiguration": {
                    "imageExtractionConfiguration": { "imageExtractionStatus": "DISABLED" },
                },
            }),
            json!({
                "connectorParameters": "{\"type\":\"CUSTOM\",\
                     \"filterConfiguration\":{\"maxFileSizeInMegaBytes\":\"10\"},\
                     \"aclEnabled\":false,\"version\":\"1\"}",
                "mediaExtractionConfiguration": {
                    "imageExtractionConfiguration": { "imageExtractionStatus": "DISABLED" },
                },
            }),
        ),
    ];

    for (case, connector, expected) in rows {
        let updated = emulator
            .update_data_source(
                &knowledge_base_id,
                &data_source_id,
                managed_connector_data_source("connector-update-source", connector),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateDataSource -> 200, got {updated:?}"
        );
        assert_eq!(
            updated.connector(),
            &expected,
            "case {case}: expected the update to report the effective connector configuration \
             {expected:?}, got {:?}",
            updated.connector()
        );
        let fetched = emulator
            .get_data_source(&knowledge_base_id, &data_source_id)
            .await;
        assert_eq!(
            fetched.connector(),
            &expected,
            "case {case}: expected GetDataSource to report the effective connector \
             configuration {expected:?}, got {:?}",
            fetched.connector()
        );
    }
}

/// A data source whose connector has a source outside the service to reach is
/// not ready when the create answers: it reports the modelled transient
/// `CREATING`, and the CLOCK — not a read — carries it to the outcome of
/// bringing that connection up. A data source the service has nothing to
/// connect out for is stored ready, `AVAILABLE`.
#[tokio::test]
async fn data_source_status_follows_whether_its_connector_has_a_source_to_reach() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: whether the data
    // source is a managed connector at all, whether its parameters name a
    // source to connect to (a `connectionConfiguration`, or a connector other
    // than the push-only `CUSTOM` one), and the kind of the owning knowledge
    // base.
    let cases: Vec<(&str, Value, Value, &str, &str)> = vec![
        (
            "a managed S3 connector naming a bucket to read",
            managed_knowledge_base("status-s3-kb"),
            managed_connector_data_source(
                "status-s3-source",
                json!({
                    "connectorParameters": {
                        "type": "S3",
                        "version": "1",
                        "connectionConfiguration": { "bucketName": "status-docs" },
                    },
                }),
            ),
            "CREATING",
            "FAILED",
        ),
        (
            "a managed CONFLUENCE connector, which reads from a site of its own",
            managed_knowledge_base("status-confluence-kb"),
            managed_connector_data_source(
                "status-confluence-source",
                json!({
                    "connectorParameters": { "type": "CONFLUENCE", "version": "1" },
                }),
            ),
            "CREATING",
            "FAILED",
        ),
        (
            "a managed CUSTOM connector, whose documents are pushed in",
            managed_knowledge_base("status-custom-kb"),
            managed_connector_data_source(
                "status-custom-source",
                json!({
                    "connectorParameters": { "type": "CUSTOM", "version": "1" },
                }),
            ),
            "AVAILABLE",
            "AVAILABLE",
        ),
        (
            "a managed connector with no parameters, so no source is named",
            managed_knowledge_base("status-bare-kb"),
            managed_connector_data_source("status-bare-source", json!({})),
            "AVAILABLE",
            "AVAILABLE",
        ),
        (
            // A self-managed configuration is read by an ingestion job, not
            // connected to when the data source is created.
            "a self-managed S3 data source of a vector knowledge base",
            vector_knowledge_base("status-self-managed-kb"),
            json!({
                "name": "status-self-managed-source",
                "dataSourceConfiguration": {
                    "type": "S3",
                    "s3Configuration": { "bucketArn": "arn:aws:s3:::status-docs" },
                },
            }),
            "AVAILABLE",
            "AVAILABLE",
        ),
        (
            "a CUSTOM data source, a type the model pairs with no connection member",
            vector_knowledge_base("status-custom-type-kb"),
            json!({
                "name": "status-custom-type-source",
                "dataSourceConfiguration": { "type": "CUSTOM" },
            }),
            "AVAILABLE",
            "AVAILABLE",
        ),
    ];

    for (case, knowledge_base_request, request, at_create, once_due) in cases {
        let knowledge_base_id = emulator.knowledge_base(knowledge_base_request).await;
        let created = emulator.create_data_source(&knowledge_base_id, request).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected CreateDataSource -> 200, got {created:?}"
        );
        let data_source_id = created.ds_string("dataSourceId");
        let reported = created.ds_string("status");
        assert_modelled_status(case, &reported);
        assert_eq!(
            reported, at_create,
            "case {case}: expected the create to report {at_create}, got {reported:?}"
        );

        // The status the create reported is the stored one, and a read does not
        // move it: both reads before the span report what the create did.
        for attempt in ["the first read", "the second read"] {
            let fetched = emulator
                .get_data_source(&knowledge_base_id, &data_source_id)
                .await;
            let reported = fetched.ds_string("status");
            assert_eq!(
                reported, at_create,
                "case {case}, {attempt}: a read must not move the status, expected \
                 {at_create}, got {reported:?}"
            );
        }

        // Once the clock has made the outcome due, every reader observes it —
        // and it is terminal, so a further read reports the same.
        tokio::time::sleep(PAST_THE_SPAN).await;
        for attempt in ["once due", "and after that"] {
            let fetched = emulator
                .get_data_source(&knowledge_base_id, &data_source_id)
                .await;
            let reported = fetched.ds_string("status");
            assert_modelled_status(case, &reported);
            assert_eq!(
                reported, once_due,
                "case {case}, {attempt}: expected {once_due} once the span has elapsed, \
                 got {reported:?}"
            );
        }
    }
}
