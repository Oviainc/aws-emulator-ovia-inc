// SPDX-License-Identifier: CC0-1.0

//! The update slice, end to end over HTTP: `UpdateDataSource`, `UpdateFlow` and
//! `UpdateKnowledgeBase` write the members the request carries into the stored
//! resource, and the matching read reports exactly those values back.
//!
//! Every test here drives a TABLE of updates whose rows differ along what the
//! rule generalises over — which optional members are supplied, which are left
//! out (and so dropped), the value an enum member takes, and the size and shape
//! of a nested configuration — and asserts, for each row, that the following
//! read carries the row's own values rather than the creation's.
//!
//! The timestamps are asserted on their WIRE FORM as well as their value:
//! `createdAt` and `updatedAt` target `DateTimestamp`, which carries
//! `@timestampFormat("date-time")`
//! (`smithy_query(query="shape", shape="DataSource,KnowledgeBase,GetFlowResponse")`),
//! so each one goes onto the restJson1 wire as an RFC-3339 string.
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

/// A second role ARN, so a row can change `roleArn` to a different value.
const OTHER_KB_ROLE: &str = "arn:aws:iam::000000000000:role/AmazonBedrockKnowledgeBaseRoleTwo";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// A role ARN matching the modelled `FlowExecutionRoleArn` pattern.
const EXECUTION_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// A second flow execution role, so a row can change `executionRoleArn`.
const OTHER_EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/AmazonBedrockFlowRoleTwo";

/// Long enough that two calls around it fall in different milliseconds, so an
/// `updatedAt` the update stamps is distinguishable from the `createdAt` the
/// create stamped.
const BETWEEN_CALLS: Duration = Duration::from_millis(20);

/// Every member `UpdateDataSource` carries that the modelled `DataSource`
/// reports back, so the value a row supplies for one is what the following read
/// must report — and a member a row leaves out must be absent from the resource.
/// `dataDeletionPolicy` is excluded: it is the one member with a documented
/// default, and it is asserted on its own.
const UPDATED_DATA_SOURCE_MEMBERS: [&str; 5] = [
    "name",
    "description",
    "dataSourceConfiguration",
    "serverSideEncryptionConfiguration",
    "vectorIngestionConfiguration",
];

/// Every member `UpdateKnowledgeBase` carries that the modelled `KnowledgeBase`
/// reports back.
const UPDATED_KNOWLEDGE_BASE_MEMBERS: [&str; 5] = [
    "name",
    "description",
    "roleArn",
    "knowledgeBaseConfiguration",
    "storageConfiguration",
];

/// Every member `UpdateFlow` carries that `UpdateFlowResponse` and
/// `GetFlowResponse` report back. `flowIdentifier` is excluded: it selects the
/// flow rather than describing it.
const UPDATED_FLOW_MEMBERS: [&str; 5] = [
    "name",
    "description",
    "executionRoleArn",
    "customerEncryptionKeyArn",
    "definition",
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
        // The knowledge bases here bind to an S3 Vectors index, which
        // `CreateKnowledgeBase` resolves in the service that owns it — so the
        // index is created first, as a caller creates it first against AWS.
        crate::vector_store_fixture::seed_index(
            &engine,
            "arn:aws:s3vectors:::bucket/update-round-trip-vectors",
            "update-round-trip-index",
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

    /// Create a knowledge base and answer its whole 202 response.
    async fn create_knowledge_base(&self, request: Value) -> Answer {
        self.call("PUT", "/knowledgebases/", Some(request)).await
    }

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

    async fn create_flow(&self, request: Value) -> Answer {
        self.call("POST", "/flows/", Some(request)).await
    }

    async fn update_flow(&self, identifier: &str, request: Value) -> Answer {
        self.call("PUT", &format!("/flows/{identifier}/"), Some(request))
            .await
    }

    async fn get_flow(&self, identifier: &str) -> Answer {
        self.call("GET", &format!("/flows/{identifier}/"), None).await
    }
}

/// A self-managed vector knowledge base over an S3 vector store.
fn vector_knowledge_base_configuration() -> (Value, Value) {
    (
        json!({
            "type": "VECTOR",
            "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
        }),
        json!({
            "type": "S3_VECTORS",
            "s3VectorsConfiguration": {
                "vectorBucketArn": "arn:aws:s3vectors:::bucket/update-round-trip-vectors",
                "indexName": "update-round-trip-index",
            },
        }),
    )
}

fn vector_knowledge_base(name: &str) -> Value {
    let (knowledge_base_configuration, storage_configuration) =
        vector_knowledge_base_configuration();
    json!({
        "name": name,
        "roleArn": KB_ROLE,
        "knowledgeBaseConfiguration": knowledge_base_configuration,
        "storageConfiguration": storage_configuration,
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

/// An S3 connector over `bucket`: the `type` discriminator names the
/// `s3Configuration` member that carries its connection details.
fn s3_configuration(bucket: &str) -> Value {
    json!({
        "type": "S3",
        "s3Configuration": { "bucketArn": format!("arn:aws:s3:::{bucket}") },
    })
}

/// A passthrough flow definition: an `Input` node wired straight to an `Output`
/// node. The node types, the union keys and the IO data types are the modelled
/// `FlowNodeType` / `FlowNodeConfiguration` / `FlowNodeIODataType` values.
fn passthrough_definition() -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "String" }],
            },
            {
                "name": "FlowOutput",
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
            },
        ],
        "connections": [
            {
                "name": "FlowInputFlowOutputDataSource",
                "type": "Data",
                "source": "FlowInput",
                "target": "FlowOutput",
                "configuration": {
                    "data": { "sourceOutput": "document", "targetInput": "document" },
                },
            },
        ],
    })
}

/// A two-branch definition: the same `Input` fanned out to two `Output` nodes,
/// so a row differs from [`passthrough_definition`] in node and connection
/// count as well as in content.
fn fan_out_definition() -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "String" }],
            },
            {
                "name": "FirstOutput",
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
            },
            {
                "name": "SecondOutput",
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
            },
        ],
        "connections": [
            {
                "name": "FlowInputFirstOutputDataSource",
                "type": "Data",
                "source": "FlowInput",
                "target": "FirstOutput",
                "configuration": {
                    "data": { "sourceOutput": "document", "targetInput": "document" },
                },
            },
            {
                "name": "FlowInputSecondOutputDataSource",
                "type": "Data",
                "source": "FlowInput",
                "target": "SecondOutput",
                "configuration": {
                    "data": { "sourceOutput": "document", "targetInput": "document" },
                },
            },
        ],
    })
}

/// Assert `value` is the modelled `date-time` wire form: an RFC-3339 string,
/// never the epoch number the protocol default would write.
fn assert_date_time_wire_form(case: &str, member: &str, value: &Value) {
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case}: expected {member} to be a date-time string, got {value:?}")
    });
    assert!(
        text.contains('T') && text.ends_with('Z'),
        "case {case}: expected {member} in the RFC-3339 form the modelled date-time \
         format asks for, got {text:?}"
    );
}

/// Assert every member of `members` that `request` supplied is reported back by
/// `answer` at `prefix`, and every member it left out is absent there.
fn assert_members_follow_the_request(
    case: &str,
    operation: &str,
    members: &[&str],
    request: &Value,
    prefix: &[&str],
    answer: &Answer,
) {
    for member in members {
        let mut path = prefix.to_vec();
        path.push(member);
        let reported = answer.member(&path);
        match request.get(member) {
            Some(requested) => assert_eq!(
                reported, requested,
                "case {case}: expected {operation} to report the {member} the update \
                 supplied, got {reported:?}"
            ),
            None => assert!(
                reported.is_null(),
                "case {case}: the update supplied no {member}, so {operation} must report \
                 none, got {reported:?}"
            ),
        }
    }
}

/// `UpdateDataSource` writes what the request carries into the stored data
/// source: each row's own values are what the update answers AND what the
/// following `GetDataSource` reports, `createdAt` never moves, and `updatedAt`
/// is the instant of the update.
#[tokio::test]
async fn updated_data_sources_round_trip_through_get_data_source() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base_id(vector_knowledge_base("update-round-trip-kb"))
        .await;

    let created = emulator
        .create_data_source(
            &knowledge_base_id,
            json!({
                "name": "update-round-trip-source",
                "description": "the description the create supplied",
                "dataSourceConfiguration": s3_configuration("update-round-trip-docs"),
                "dataDeletionPolicy": "RETAIN",
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected CreateDataSource -> 200, got {created:?}"
    );
    let data_source_id = created.string(&["dataSource", "dataSourceId"]);
    let created_at = created.string(&["dataSource", "createdAt"]);

    // The rows differ along which optional members the update supplies, the
    // value the `dataDeletionPolicy` enum takes, and the kind and content of the
    // connector configuration. They run in sequence against the same data
    // source, so each row also asserts that the PREVIOUS row's values are gone.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a new description and the DELETE deletion policy",
            json!({
                "name": "update-round-trip-source",
                "description": "the description the first update supplied",
                "dataSourceConfiguration": s3_configuration("update-round-trip-docs"),
                "dataDeletionPolicy": "DELETE",
            }),
        ),
        (
            "back to the RETAIN deletion policy",
            json!({
                "name": "update-round-trip-source",
                "description": "the description the second update supplied",
                "dataSourceConfiguration": s3_configuration("update-round-trip-docs"),
                "dataDeletionPolicy": "RETAIN",
            }),
        ),
        (
            "no description and no deletion policy, which the update drops",
            json!({
                "name": "update-round-trip-source",
                "dataSourceConfiguration": s3_configuration("update-round-trip-docs"),
            }),
        ),
        (
            "a new name, a different bucket and a parsing configuration",
            json!({
                "name": "renamed-round-trip-source",
                "description": "a source with a parsing configuration",
                "dataSourceConfiguration": s3_configuration("update-round-trip-other-docs"),
                // `vectorIngestionConfiguration` varies here through
                // `parsingConfiguration`, NOT through `chunkingConfiguration`:
                // the chunking configuration is the one member this operation
                // documents as unchangeable once the connector exists, and this
                // data source was created without one, so a row that supplied
                // one would be refused (see
                // `immutable_configuration_members.rs`). `SMART_PARSING` is a
                // modelled `ParsingStrategy` value
                // (`smithy_query(query="shape",shape="ParsingStrategy")`).
                "vectorIngestionConfiguration": {
                    "parsingConfiguration": { "parsingStrategy": "SMART_PARSING" },
                },
                "dataDeletionPolicy": "DELETE",
            }),
        ),
        (
            "a CUSTOM connector in place of the S3 one",
            json!({
                "name": "renamed-round-trip-source",
                "description": "a custom connector source",
                "dataSourceConfiguration": { "type": "CUSTOM" },
                "dataDeletionPolicy": "RETAIN",
            }),
        ),
    ];

    for (case, request) in rows {
        tokio::time::sleep(BETWEEN_CALLS).await;
        let updated = emulator
            .update_data_source(&knowledge_base_id, &data_source_id, request.clone())
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateDataSource -> 200, got {updated:?}"
        );
        assert_members_follow_the_request(
            case,
            "UpdateDataSource",
            &UPDATED_DATA_SOURCE_MEMBERS,
            &request,
            &["dataSource"],
            &updated,
        );

        let fetched = emulator
            .get_data_source(&knowledge_base_id, &data_source_id)
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetDataSource -> 200, got {fetched:?}"
        );
        assert_members_follow_the_request(
            case,
            "GetDataSource",
            &UPDATED_DATA_SOURCE_MEMBERS,
            &request,
            &["dataSource"],
            &fetched,
        );

        // `dataDeletionPolicy` follows the request too; this knowledge base is
        // self-managed, so the member has no default and an update that names
        // none leaves the data source without one.
        let policy = fetched.member(&["dataSource", "dataDeletionPolicy"]);
        assert_eq!(
            policy,
            request.get("dataDeletionPolicy").unwrap_or(&Value::Null),
            "case {case}: expected the dataDeletionPolicy the update supplied, got {policy:?}"
        );

        // The identity of the resource and the instant it was created are not
        // the update's to change; the instant it was last updated is.
        assert_eq!(
            fetched.string(&["dataSource", "dataSourceId"]),
            data_source_id,
            "case {case}: an update never re-mints the dataSourceId"
        );
        assert_eq!(
            fetched.string(&["dataSource", "createdAt"]),
            created_at,
            "case {case}: an update never moves createdAt"
        );
        let updated_at = fetched.string(&["dataSource", "updatedAt"]);
        assert_ne!(
            updated_at, created_at,
            "case {case}: the update happened after the create, so updatedAt must have \
             moved past createdAt, got {updated_at:?}"
        );
        assert_eq!(
            updated.member(&["dataSource", "updatedAt"]),
            fetched.member(&["dataSource", "updatedAt"]),
            "case {case}: the update and the read report one stored updatedAt"
        );
        for member in ["createdAt", "updatedAt"] {
            assert_date_time_wire_form(
                case,
                member,
                fetched.member(&["dataSource", member]),
            );
        }
    }
}

/// A data source of a service-managed knowledge base keeps the documented
/// `dataDeletionPolicy` default across an update that names none, by the same
/// rule the create applies it by.
#[tokio::test]
async fn updating_a_managed_data_source_keeps_the_documented_deletion_policy_default() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base_id(managed_knowledge_base("managed-default-kb"))
        .await;

    let created = emulator
        .create_data_source(
            &knowledge_base_id,
            json!({
                "name": "managed-default-source",
                "dataSourceConfiguration": {
                    "type": "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
                    "managedKnowledgeBaseConnectorConfiguration": {
                        "connectorParameters": { "type": "CUSTOM", "version": "1" },
                    },
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected CreateDataSource -> 200, got {created:?}"
    );
    let data_source_id = created.string(&["dataSource", "dataSourceId"]);

    for (case, request, expected_policy) in [
        (
            "an update that names no deletion policy",
            json!({
                "name": "managed-default-source",
                "description": "no policy named",
                "dataSourceConfiguration": {
                    "type": "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
                    "managedKnowledgeBaseConnectorConfiguration": {
                        "connectorParameters": { "type": "CUSTOM", "version": "1" },
                    },
                },
            }),
            "DELETE",
        ),
        (
            "an update that names the policy explicitly",
            json!({
                "name": "managed-default-source",
                "description": "the policy named explicitly",
                "dataSourceConfiguration": {
                    "type": "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
                    "managedKnowledgeBaseConnectorConfiguration": {
                        "connectorParameters": { "type": "CUSTOM", "version": "1" },
                    },
                },
                "dataDeletionPolicy": "DELETE",
            }),
            "DELETE",
        ),
    ] {
        let updated = emulator
            .update_data_source(&knowledge_base_id, &data_source_id, request)
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateDataSource -> 200, got {updated:?}"
        );
        let fetched = emulator
            .get_data_source(&knowledge_base_id, &data_source_id)
            .await;
        let policy = fetched.string(&["dataSource", "dataDeletionPolicy"]);
        assert_eq!(
            policy, expected_policy,
            "case {case}: expected the managed default {expected_policy}, got {policy:?}"
        );
    }
}

/// `UpdateFlow` writes what the request carries into the stored flow, the
/// definition included, and returns the flow to the `NotPrepared` status it
/// documents.
#[tokio::test]
async fn updated_flows_round_trip_through_get_flow() {
    let emulator = Emulator::new();
    let created = emulator
        .create_flow(json!({
            "name": "update-round-trip-flow",
            "description": "the description the create supplied",
            "executionRoleArn": EXECUTION_ROLE,
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "case setup: expected CreateFlow -> 201, got {created:?}"
    );
    let flow_id = created.string(&["id"]);
    let created_at = created.string(&["createdAt"]);

    // The rows differ along which optional members the update supplies and in
    // the size of the definition, which the create supplied none of.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a new description and a two-node definition",
            json!({
                "name": "update-round-trip-flow",
                "description": "the description the first update supplied",
                "executionRoleArn": EXECUTION_ROLE,
                "definition": passthrough_definition(),
            }),
        ),
        (
            "a three-node definition and no description",
            json!({
                "name": "update-round-trip-flow",
                "executionRoleArn": EXECUTION_ROLE,
                "definition": fan_out_definition(),
            }),
        ),
        (
            "no definition at all, which the update drops",
            json!({
                "name": "update-round-trip-flow",
                "description": "a flow with no definition",
                "executionRoleArn": EXECUTION_ROLE,
            }),
        ),
        (
            "a new name and a different execution role",
            json!({
                "name": "renamed-round-trip-flow",
                "description": "a renamed flow",
                "executionRoleArn": OTHER_EXECUTION_ROLE,
                "definition": passthrough_definition(),
            }),
        ),
    ];

    for (case, request) in rows {
        tokio::time::sleep(BETWEEN_CALLS).await;
        let updated = emulator.update_flow(&flow_id, request.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateFlow -> 200, got {updated:?}"
        );
        assert_members_follow_the_request(
            case,
            "UpdateFlow",
            &UPDATED_FLOW_MEMBERS,
            &request,
            &[],
            &updated,
        );

        let fetched = emulator.get_flow(&flow_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetFlow -> 200, got {fetched:?}"
        );
        assert_members_follow_the_request(
            case,
            "GetFlow",
            &UPDATED_FLOW_MEMBERS,
            &request,
            &[],
            &fetched,
        );

        // This operation's own contract: the updated flow is NotPrepared, and
        // the version it updated is the DRAFT one.
        for (operation, answer) in [("UpdateFlow", &updated), ("GetFlow", &fetched)] {
            let status = answer.string(&["status"]);
            assert_eq!(
                status, "NotPrepared",
                "case {case}: expected {operation} to report the NotPrepared status an \
                 update leaves the flow in, got {status:?}"
            );
            let version = answer.string(&["version"]);
            assert_eq!(
                version, "DRAFT",
                "case {case}: expected {operation} to report the DRAFT version, got \
                 {version:?}"
            );
        }

        assert_eq!(
            fetched.string(&["id"]),
            flow_id,
            "case {case}: an update never re-mints the flow id"
        );
        assert_eq!(
            fetched.string(&["arn"]),
            created.string(&["arn"]),
            "case {case}: an update never re-mints the flow ARN"
        );
        assert_eq!(
            fetched.string(&["createdAt"]),
            created_at,
            "case {case}: an update never moves createdAt"
        );
        let updated_at = fetched.string(&["updatedAt"]);
        assert_ne!(
            updated_at, created_at,
            "case {case}: the update happened after the create, so updatedAt must have \
             moved past createdAt, got {updated_at:?}"
        );
        assert_eq!(
            updated.member(&["updatedAt"]),
            fetched.member(&["updatedAt"]),
            "case {case}: the update and the read report one stored updatedAt"
        );
        for member in ["createdAt", "updatedAt"] {
            assert_date_time_wire_form(case, member, fetched.member(&[member]));
        }
    }
}

/// `UpdateKnowledgeBase` writes what the request carries into the stored
/// knowledge base; the status the clock owns reports the change being applied
/// and settles back to `ACTIVE` on its own.
#[tokio::test]
async fn updated_knowledge_bases_round_trip_through_get_knowledge_base() {
    let emulator = Emulator::new();
    let created = emulator
        .create_knowledge_base(vector_knowledge_base("kb-round-trip"))
        .await;
    assert_eq!(
        created.status, 202,
        "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
    );
    let knowledge_base_id = created.string(&["knowledgeBase", "knowledgeBaseId"]);
    let created_at = created.string(&["knowledgeBase", "createdAt"]);

    // Let the clock finish the creation it owes, so the status assertion below
    // is about what the update does to the status rather than about the race
    // between the two.
    tokio::time::sleep(Duration::from_millis(900)).await;

    let (knowledge_base_configuration, storage_configuration) =
        vector_knowledge_base_configuration();
    // The rows differ along which optional member is supplied and which of the
    // required ones changes: `knowledgeBaseConfiguration` and
    // `storageConfiguration` are the ones this operation documents as having to
    // be sent unchanged, so every row carries them.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a new description",
            json!({
                "name": "kb-round-trip",
                "description": "the description the first update supplied",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": knowledge_base_configuration.clone(),
                "storageConfiguration": storage_configuration.clone(),
            }),
        ),
        (
            "no description, which the update drops",
            json!({
                "name": "kb-round-trip",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": knowledge_base_configuration.clone(),
                "storageConfiguration": storage_configuration.clone(),
            }),
        ),
        (
            "a new name and a different role",
            json!({
                "name": "renamed-kb-round-trip",
                "description": "a renamed knowledge base",
                "roleArn": OTHER_KB_ROLE,
                "knowledgeBaseConfiguration": knowledge_base_configuration.clone(),
                "storageConfiguration": storage_configuration.clone(),
            }),
        ),
    ];

    for (case, request) in rows {
        tokio::time::sleep(BETWEEN_CALLS).await;
        let updated = emulator
            .update_knowledge_base(&knowledge_base_id, request.clone())
            .await;
        assert_eq!(
            updated.status, 202,
            "case {case}: expected UpdateKnowledgeBase -> 202, got {updated:?}"
        );
        assert_members_follow_the_request(
            case,
            "UpdateKnowledgeBase",
            &UPDATED_KNOWLEDGE_BASE_MEMBERS,
            &request,
            &["knowledgeBase"],
            &updated,
        );

        let fetched = emulator.get_knowledge_base(&knowledge_base_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetKnowledgeBase -> 200, got {fetched:?}"
        );
        assert_members_follow_the_request(
            case,
            "GetKnowledgeBase",
            &UPDATED_KNOWLEDGE_BASE_MEMBERS,
            &request,
            &["knowledgeBase"],
            &fetched,
        );

        // The update is accepted rather than applied inside the call, so the
        // knowledge base reports the change being applied until the clock has
        // settled it — never CREATING, which belongs to the creation the clock
        // already finished. The transition itself is asserted in
        // `update_reports_the_transitional_status.rs`.
        let status = fetched.string(&["knowledgeBase", "status"]);
        assert_eq!(
            status, "UPDATING",
            "case {case}: expected the knowledge base to report the update being applied, \
             got {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(900)).await;
        let status = emulator
            .get_knowledge_base(&knowledge_base_id)
            .await
            .string(&["knowledgeBase", "status"]);
        assert_eq!(
            status, "ACTIVE",
            "case {case}: expected the knowledge base to settle back at ACTIVE, got {status:?}"
        );
        assert_eq!(
            fetched.string(&["knowledgeBase", "knowledgeBaseId"]),
            knowledge_base_id,
            "case {case}: an update never re-mints the knowledgeBaseId"
        );
        assert_eq!(
            fetched.string(&["knowledgeBase", "knowledgeBaseArn"]),
            created.string(&["knowledgeBase", "knowledgeBaseArn"]),
            "case {case}: an update never re-mints the knowledge base ARN"
        );
        assert_eq!(
            fetched.string(&["knowledgeBase", "createdAt"]),
            created_at,
            "case {case}: an update never moves createdAt"
        );
        let updated_at = fetched.string(&["knowledgeBase", "updatedAt"]);
        assert_ne!(
            updated_at, created_at,
            "case {case}: the update happened after the create, so updatedAt must have \
             moved past createdAt, got {updated_at:?}"
        );
        for member in ["createdAt", "updatedAt"] {
            assert_date_time_wire_form(
                case,
                member,
                fetched.member(&["knowledgeBase", member]),
            );
        }
    }
}

/// An update of a resource the store does not hold changes nothing and is
/// refused with the error that operation's model declares for a missing
/// resource, at that error's modelled status.
#[tokio::test]
async fn update_operations_refuse_a_resource_the_store_does_not_hold() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base_id(vector_knowledge_base("not-found-kb"))
        .await;
    let created = emulator
        .create_data_source(
            &knowledge_base_id,
            json!({
                "name": "not-found-probe-source",
                "dataSourceConfiguration": s3_configuration("not-found-docs"),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected CreateDataSource -> 200, got {created:?}"
    );
    let data_source_id = created.string(&["dataSource", "dataSourceId"]);
    let other_knowledge_base_id = emulator
        .knowledge_base_id(vector_knowledge_base("not-found-other-kb"))
        .await;
    let (knowledge_base_configuration, storage_configuration) =
        vector_knowledge_base_configuration();

    // A well-formed id the store does not hold, and — for the data source — an
    // id that belongs to ANOTHER knowledge base: a data source is identified by
    // the pair, so the second is as absent as the first.
    let data_source_cases: Vec<(&str, String, String)> = vec![
        (
            "a dataSourceId the store does not hold",
            knowledge_base_id.clone(),
            "0123456789".to_string(),
        ),
        (
            "a dataSourceId of another knowledge base",
            other_knowledge_base_id.clone(),
            data_source_id.clone(),
        ),
        (
            "a knowledgeBaseId the store does not hold",
            "0123456789".to_string(),
            data_source_id.clone(),
        ),
    ];
    for (case, requested_knowledge_base, requested_data_source) in data_source_cases {
        let refused = emulator
            .update_data_source(
                &requested_knowledge_base,
                &requested_data_source,
                json!({
                    "name": "not-found-probe-source",
                    "dataSourceConfiguration": s3_configuration("not-found-docs"),
                }),
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case}: expected UpdateDataSource -> 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected the modelled not-found error, got {refused:?}"
        );
    }

    for (case, identifier) in [
        ("a flowIdentifier the store does not hold", "0123456789"),
        (
            "a flow ARN the store does not hold",
            "arn%3Aaws%3Abedrock%3Aeu-west-1%3A000000000000%3Aflow%2F0123456789",
        ),
    ] {
        let refused = emulator
            .update_flow(
                identifier,
                json!({
                    "name": "not-found-probe-flow",
                    "executionRoleArn": EXECUTION_ROLE,
                }),
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case}: expected UpdateFlow -> 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected the modelled not-found error, got {refused:?}"
        );
    }

    let refused = emulator
        .update_knowledge_base(
            "0123456789",
            json!({
                "name": "not-found-probe-kb",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": knowledge_base_configuration.clone(),
                "storageConfiguration": storage_configuration.clone(),
            }),
        )
        .await;
    assert_eq!(
        refused.status, 404,
        "case a knowledgeBaseId the store does not hold: expected \
         UpdateKnowledgeBase -> 404, got {refused:?}"
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "case a knowledgeBaseId the store does not hold: expected the modelled \
         not-found error, got {refused:?}"
    );

    // A refused update leaves the resource exactly as it was: the data source
    // the second row named is still the one its own knowledge base holds.
    let fetched = emulator
        .get_data_source(&knowledge_base_id, &data_source_id)
        .await;
    assert_eq!(
        fetched.status, 200,
        "case after the refusals: expected GetDataSource -> 200, got {fetched:?}"
    );
    assert_eq!(
        fetched.string(&["dataSource", "name"]),
        "not-found-probe-source",
        "case after the refusals: a refused update wrote nothing, got {fetched:?}"
    );
}

/// An update whose configuration does not satisfy its own `type` discriminator
/// is a bad request, refused with the modelled `ValidationException` — the same
/// gate the create applies, so an update cannot store a configuration a create
/// would have refused.
#[tokio::test]
async fn update_operations_refuse_a_configuration_that_fails_its_own_discriminator() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base_id(vector_knowledge_base("discriminator-kb"))
        .await;
    let created = emulator
        .create_data_source(
            &knowledge_base_id,
            json!({
                "name": "discriminator-source",
                "dataSourceConfiguration": s3_configuration("discriminator-docs"),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case setup: expected CreateDataSource -> 200, got {created:?}"
    );
    let data_source_id = created.string(&["dataSource", "dataSourceId"]);

    // The rows differ along which discriminator is unsatisfied: a type whose
    // own member is missing, and a type the model does not declare at all.
    for (case, configuration) in [
        (
            "an S3 data source with no s3Configuration",
            json!({ "type": "S3" }),
        ),
        (
            "a data source type the model does not declare",
            json!({ "type": "NOT_A_DATA_SOURCE_TYPE" }),
        ),
    ] {
        let refused = emulator
            .update_data_source(
                &knowledge_base_id,
                &data_source_id,
                json!({
                    "name": "discriminator-source",
                    "dataSourceConfiguration": configuration,
                }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected UpdateDataSource -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected the modelled ValidationException, got {refused:?}"
        );
    }

    // The refusals wrote nothing: the data source still carries the connector
    // the create stored.
    let fetched = emulator
        .get_data_source(&knowledge_base_id, &data_source_id)
        .await;
    assert_eq!(
        fetched.member(&["dataSource", "dataSourceConfiguration"]),
        &s3_configuration("discriminator-docs"),
        "case after the refusals: a refused update wrote nothing, got {fetched:?}"
    );

    // A `flowIdentifier` of neither modelled form is the same kind of bad
    // request on the flow path.
    let refused = emulator
        .update_flow(
            "not-an-identifier",
            json!({
                "name": "discriminator-flow",
                "executionRoleArn": EXECUTION_ROLE,
            }),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case a flowIdentifier of neither modelled form: expected UpdateFlow -> 400, \
         got {refused:?}"
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ValidationException"),
        "case a flowIdentifier of neither modelled form: expected the modelled \
         ValidationException, got {refused:?}"
    );
}
