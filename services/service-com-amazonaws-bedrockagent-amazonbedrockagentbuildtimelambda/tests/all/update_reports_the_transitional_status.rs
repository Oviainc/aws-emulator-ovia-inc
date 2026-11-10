// SPDX-License-Identifier: CC0-1.0

//! The status a mutation the service ACCEPTS reports, end to end over HTTP.
//!
//! `UpdateDataSource` and `UpdateKnowledgeBase` do not complete the change
//! inside the call: each answers with the resource in the transitional status
//! its own status enum declares for a change in flight (`UPDATING` for both),
//! and the CLOCK — not a read — carries the resource to the status it settles
//! at. A data source settles at the outcome of the configuration the update
//! stored; a knowledge base settles back at `ACTIVE`.
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

/// Another role ARN matching the same pattern, for the rows whose update
/// changes the role.
const OTHER_KB_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/AmazonBedrockOtherRole";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// The vector bucket and index the self-managed knowledge bases here bind to.
const VECTOR_BUCKET: &str = "arn:aws:s3vectors:::bucket/transitional-status-vectors";
const VECTOR_INDEX: &str = "transitional-status-index";

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

/// Every value the modelled `KnowledgeBaseStatus` enum declares.
///
/// Source: this service's Smithy model
/// (`smithy_query(query="shape",shape="KnowledgeBaseStatus")`).
const KNOWLEDGE_BASE_STATUSES: [&str; 7] = [
    "CREATING",
    "ACTIVE",
    "DELETING",
    "UPDATING",
    "FAILED",
    "DELETE_UNSUCCESSFUL",
    "UPDATE_UNSUCCESSFUL",
];

/// Longer than the span the emulator takes to apply a change to a knowledge
/// base or a data source, so a read after it observes the status the clock owed
/// rather than the transitional one.
const PAST_THE_SPAN: Duration = Duration::from_millis(900);

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
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
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // A self-managed knowledge base binds to an S3 Vectors index, which
        // `CreateKnowledgeBase` resolves in the service that owns it — so the
        // index is created first, as a caller creates it first against AWS.
        crate::vector_store_fixture::seed_index(
            &engine,
            VECTOR_BUCKET,
            VECTOR_INDEX,
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

    /// Create a knowledge base and answer the whole response.
    async fn create_knowledge_base(&self, request: Value) -> Answer {
        self.call("PUT", "/knowledgebases/", Some(request)).await
    }

    /// Create a knowledge base and answer its minted `knowledgeBaseId`.
    async fn knowledge_base_id(&self, request: Value) -> String {
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

    async fn get_knowledge_base(&self, knowledge_base_id: &str) -> Answer {
        self.call("GET", &format!("/knowledgebases/{knowledge_base_id}"), None)
            .await
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

/// A self-managed vector knowledge base, bound to the index this suite seeded.
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
                "vectorBucketArn": VECTOR_BUCKET,
                "indexName": VECTOR_INDEX,
            },
        },
    })
}

/// A `MANAGED_KNOWLEDGE_BASE_CONNECTOR` data source request carrying
/// `connector_parameters` as its connector's parameter document.
fn managed_connector_data_source(name: &str, connector_parameters: Value) -> Value {
    json!({
        "name": name,
        "dataSourceConfiguration": {
            "type": "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
            "managedKnowledgeBaseConnectorConfiguration": {
                "connectorParameters": connector_parameters,
            },
        },
    })
}

/// Assert `status` is one of the values the model declares for the member.
fn assert_modelled_status(case: &str, catalog: &[&str], status: &str) {
    assert!(
        catalog.contains(&status),
        "case {case}: expected one of the modelled statuses {catalog:?}, got {status:?}"
    );
}

/// `UpdateDataSource` answers with the data source in the modelled transitional
/// `UPDATING` status, a read before the span reports that same stored status,
/// and once the clock has made the outcome due every read reports the status the
/// stored configuration settles at.
#[tokio::test]
async fn update_data_source_reports_updating_until_the_clock_settles_it() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: the kind of
    // configuration the update stores (a managed connector with no source to
    // reach, one that names a source, a self-managed one), the status the data
    // source held BEFORE the update, and so the status it settles at after it.
    let cases: Vec<(&str, Value, Value, Value, &str, &str)> = vec![
        (
            "a CUSTOM managed connector updated in place",
            managed_knowledge_base("transitional-custom-kb"),
            managed_connector_data_source(
                "transitional-custom-source",
                json!({ "type": "CUSTOM", "version": "1" }),
            ),
            managed_connector_data_source(
                "transitional-custom-source",
                json!({ "type": "CUSTOM", "version": "1" }),
            ),
            "AVAILABLE",
            "AVAILABLE",
        ),
        (
            "a CUSTOM connector updated to one that names a source to reach",
            managed_knowledge_base("transitional-to-s3-kb"),
            managed_connector_data_source(
                "transitional-to-s3-source",
                json!({ "type": "CUSTOM", "version": "1" }),
            ),
            managed_connector_data_source(
                "transitional-to-s3-source",
                json!({
                    "type": "S3",
                    "version": "1",
                    "connectionConfiguration": { "bucketName": "transitional-docs" },
                }),
            ),
            "AVAILABLE",
            "FAILED",
        ),
        (
            "a connector with a source to reach updated to the push-only CUSTOM one",
            managed_knowledge_base("transitional-from-confluence-kb"),
            managed_connector_data_source(
                "transitional-from-confluence-source",
                json!({ "type": "CONFLUENCE", "version": "1" }),
            ),
            managed_connector_data_source(
                "transitional-from-confluence-source",
                json!({ "type": "CUSTOM", "version": "1" }),
            ),
            "FAILED",
            "AVAILABLE",
        ),
        (
            // A self-managed configuration is read by an ingestion job rather
            // than connected to, so it settles ready however it changes.
            "a self-managed S3 data source pointed at another bucket",
            vector_knowledge_base("transitional-self-managed-kb"),
            json!({
                "name": "transitional-self-managed-source",
                "dataSourceConfiguration": {
                    "type": "S3",
                    "s3Configuration": { "bucketArn": "arn:aws:s3:::transitional-docs" },
                },
            }),
            json!({
                "name": "transitional-self-managed-source",
                "description": "pointed at the other bucket",
                "dataSourceConfiguration": {
                    "type": "S3",
                    "s3Configuration": { "bucketArn": "arn:aws:s3:::transitional-other-docs" },
                },
            }),
            "AVAILABLE",
            "AVAILABLE",
        ),
        (
            "a CUSTOM data source, a type the model pairs with no connection member",
            vector_knowledge_base("transitional-custom-type-kb"),
            json!({
                "name": "transitional-custom-type-source",
                "dataSourceConfiguration": { "type": "CUSTOM" },
            }),
            json!({
                "name": "transitional-custom-type-source",
                "description": "renamed nothing, changed the description",
                "dataSourceConfiguration": { "type": "CUSTOM" },
            }),
            "AVAILABLE",
            "AVAILABLE",
        ),
    ];

    for (case, knowledge_base_request, create, update, before_the_update, settles_at) in cases {
        let knowledge_base_id = emulator.knowledge_base_id(knowledge_base_request).await;
        let created = emulator.create_data_source(&knowledge_base_id, create).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected CreateDataSource -> 200, got {created:?}"
        );
        let data_source_id = created.string(&["dataSource", "dataSourceId"]);

        // The data source settles at the outcome of the configuration the
        // create stored before the update is submitted, so what the update
        // reports is this call's own transition and not a leftover one.
        tokio::time::sleep(PAST_THE_SPAN).await;
        let settled = emulator
            .get_data_source(&knowledge_base_id, &data_source_id)
            .await;
        let reported = settled.string(&["dataSource", "status"]);
        assert_eq!(
            reported, before_the_update,
            "case {case}: expected the created data source to settle at \
             {before_the_update}, got {reported:?}"
        );

        let updated = emulator
            .update_data_source(&knowledge_base_id, &data_source_id, update)
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateDataSource -> 200, got {updated:?}"
        );
        let reported = updated.string(&["dataSource", "status"]);
        assert_modelled_status(case, &DATA_SOURCE_STATUSES, &reported);
        assert_eq!(
            reported, "UPDATING",
            "case {case}: an accepted update reports the transitional status, got {reported:?}"
        );

        // The status the update reported is the STORED one, and a read does not
        // move it: both reads before the span report what the update did.
        for attempt in ["the first read", "the second read"] {
            let fetched = emulator
                .get_data_source(&knowledge_base_id, &data_source_id)
                .await;
            let reported = fetched.string(&["dataSource", "status"]);
            assert_eq!(
                reported, "UPDATING",
                "case {case}, {attempt}: a read must not move the status, expected \
                 UPDATING, got {reported:?}"
            );
        }

        // Once the clock has made the outcome due, every reader observes it —
        // and it is terminal, so a further read reports the same.
        tokio::time::sleep(PAST_THE_SPAN).await;
        for attempt in ["once due", "and after that"] {
            let fetched = emulator
                .get_data_source(&knowledge_base_id, &data_source_id)
                .await;
            let reported = fetched.string(&["dataSource", "status"]);
            assert_modelled_status(case, &DATA_SOURCE_STATUSES, &reported);
            assert_eq!(
                reported, settles_at,
                "case {case}, {attempt}: expected {settles_at} once the span has elapsed, \
                 got {reported:?}"
            );
        }
    }
}

/// `UpdateKnowledgeBase` answers 202 with the knowledge base in the modelled
/// transitional `UPDATING` status — the status its own member documents as "the
/// knowledge base is being updated" — a read before the span reports that same
/// stored status, and the clock carries it back to `ACTIVE`.
#[tokio::test]
async fn update_knowledge_base_reports_updating_until_the_clock_settles_it_to_active() {
    let emulator = Emulator::new();

    // The rows differ along the kind of knowledge base and along which mutable
    // members the update changes (none of them, the description, the name and
    // the role) — the transition belongs to the accepted call, not to what it
    // changed.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a self-managed VECTOR knowledge base, a new description",
            vector_knowledge_base("transitional-vector-kb"),
            {
                let mut request = vector_knowledge_base("transitional-vector-kb");
                request["description"] = json!("the description the update supplied");
                request
            },
        ),
        (
            "a self-managed VECTOR knowledge base, a new name and role",
            vector_knowledge_base("transitional-renamed-kb"),
            {
                let mut request = vector_knowledge_base("transitional-renamed-kb");
                request["name"] = json!("transitional-renamed-kb-again");
                request["roleArn"] = json!(OTHER_KB_ROLE);
                request
            },
        ),
        (
            "a MANAGED knowledge base, re-supplying exactly what it holds",
            managed_knowledge_base("transitional-managed-kb"),
            managed_knowledge_base("transitional-managed-kb"),
        ),
    ];

    for (case, create, update) in cases {
        let knowledge_base_id = emulator.knowledge_base_id(create).await;

        // The knowledge base is ACTIVE before the update is submitted, so what
        // the update reports is this call's own transition.
        tokio::time::sleep(PAST_THE_SPAN).await;
        let settled = emulator.get_knowledge_base(&knowledge_base_id).await;
        let reported = settled.string(&["knowledgeBase", "status"]);
        assert_eq!(
            reported, "ACTIVE",
            "case {case}: expected the created knowledge base to settle at ACTIVE, \
             got {reported:?}"
        );

        let updated = emulator
            .update_knowledge_base(&knowledge_base_id, update)
            .await;
        assert_eq!(
            updated.status, 202,
            "case {case}: expected UpdateKnowledgeBase -> 202, got {updated:?}"
        );
        let reported = updated.string(&["knowledgeBase", "status"]);
        assert_modelled_status(case, &KNOWLEDGE_BASE_STATUSES, &reported);
        assert_eq!(
            reported, "UPDATING",
            "case {case}: an accepted update reports the transitional status, got {reported:?}"
        );

        for attempt in ["the first read", "the second read"] {
            let fetched = emulator.get_knowledge_base(&knowledge_base_id).await;
            let reported = fetched.string(&["knowledgeBase", "status"]);
            assert_eq!(
                reported, "UPDATING",
                "case {case}, {attempt}: a read must not move the status, expected \
                 UPDATING, got {reported:?}"
            );
        }

        tokio::time::sleep(PAST_THE_SPAN).await;
        for attempt in ["once due", "and after that"] {
            let fetched = emulator.get_knowledge_base(&knowledge_base_id).await;
            let reported = fetched.string(&["knowledgeBase", "status"]);
            assert_modelled_status(case, &KNOWLEDGE_BASE_STATUSES, &reported);
            assert_eq!(
                reported, "ACTIVE",
                "case {case}, {attempt}: expected ACTIVE once the span has elapsed, \
                 got {reported:?}"
            );
        }
    }
}

/// An update submitted while an earlier one is still being applied is accepted
/// the same way: the resource stays in the transitional status and the clock
/// owes the settled one from the LATEST accepted change, so a caller that polls
/// never sees a settled status for a change that has not been applied.
#[tokio::test]
async fn a_second_update_keeps_the_resource_in_the_transitional_status() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base_id(managed_knowledge_base("transitional-twice-kb"))
        .await;
    let created = emulator
        .create_data_source(
            &knowledge_base_id,
            managed_connector_data_source(
                "transitional-twice-source",
                json!({ "type": "CUSTOM", "version": "1" }),
            ),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected CreateDataSource -> 200, got {created:?}"
    );
    let data_source_id = created.string(&["dataSource", "dataSourceId"]);

    for (case, resource) in [
        ("the knowledge base", "knowledgeBase"),
        ("the data source", "dataSource"),
    ] {
        for attempt in ["the first update", "the second update"] {
            let answered = match resource {
                "knowledgeBase" => {
                    emulator
                        .update_knowledge_base(
                            &knowledge_base_id,
                            managed_knowledge_base("transitional-twice-kb"),
                        )
                        .await
                }
                _ => {
                    emulator
                        .update_data_source(
                            &knowledge_base_id,
                            &data_source_id,
                            managed_connector_data_source(
                                "transitional-twice-source",
                                json!({ "type": "CUSTOM", "version": "1" }),
                            ),
                        )
                        .await
                }
            };
            let reported = answered.string(&[resource, "status"]);
            assert_eq!(
                reported, "UPDATING",
                "case {case}, {attempt}: expected the transitional status, got {reported:?}"
            );
        }

        // The clock settles the resource once, from the latest accepted change.
        tokio::time::sleep(PAST_THE_SPAN).await;
        let fetched = match resource {
            "knowledgeBase" => emulator.get_knowledge_base(&knowledge_base_id).await,
            _ => {
                emulator
                    .get_data_source(&knowledge_base_id, &data_source_id)
                    .await
            }
        };
        let reported = fetched.string(&[resource, "status"]);
        let expected = if resource == "knowledgeBase" {
            "ACTIVE"
        } else {
            "AVAILABLE"
        };
        assert_eq!(
            reported, expected,
            "case {case}: expected {expected} once the span has elapsed, got {reported:?}"
        );
    }
}
