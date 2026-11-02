// SPDX-License-Identifier: CC0-1.0

//! The delete slice, end to end over HTTP: a delete this service accepts takes
//! the resource it names out of the store, so every later read of it answers
//! the modelled `ResourceNotFoundException` — 404, the error type in
//! `x-amzn-errortype`, and the message AWS words for that resource family —
//! instead of the resource.
//!
//! The tests drive a TABLE whose rows differ along what the rule generalises
//! over: the resource family (`DeleteKnowledgeBase`, `DeleteDataSource`,
//! `DeleteFlow`), how many child resources the deleted resource contains, the
//! `dataDeletionPolicy` the data source carries, whether an ingestion job has
//! run for it, the kind of knowledge base it belongs to, and the form the
//! request names the resource by (a bare id or an ARN).
//!
//! A knowledge base and a data source carry the transient `DELETING` status
//! their modelled shapes document while the removal the clock owes is
//! outstanding, so the read after the delete is polled to its 404; a flow,
//! whose modelled `FlowStatus` has no such value, is gone by the time the
//! delete answers.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather
//! than calling a handler directly.

use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// The region of [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// A role ARN matching the modelled `FlowExecutionRoleArn` pattern.
const EXECUTION_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// The error every one of these reads answers once the resource is gone, and
/// the status its model binds to it.
///
/// Source: this service's Smithy model
/// (`smithy_query(query="operation",operation="GetKnowledgeBase,GetDataSource,GetFlow")`),
/// where `ResourceNotFoundException` is bound with `httpError` 404.
const NOT_FOUND: (&str, u16) = ("ResourceNotFoundException", 404);

/// How long a poll waits for the removal an accepted delete put on the clock:
/// several times the span itself, so a loaded machine still converges, and
/// bounded so a hung poll fails this test rather than hanging the suite.
const REMOVAL_POLL_CAP: Duration = Duration::from_secs(30);

/// How long one poll attempt waits before reading again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

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
}

/// The resource a row deletes, and so the read that reports it.
#[derive(Debug, Clone)]
enum Target {
    KnowledgeBase(String),
    /// A data source, identified by the knowledge base it belongs to as well as
    /// its own id.
    DataSource(String, String),
    /// A flow, named by whichever of its two modelled identifier forms the row
    /// uses.
    Flow(String),
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

    async fn create_knowledge_base(&self, request: Value) -> String {
        let created = self.call("PUT", "/knowledgebases/", Some(request)).await;
        assert_eq!(
            created.status, 202,
            "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.string(&["knowledgeBase", "knowledgeBaseId"])
    }

    async fn get_knowledge_base(&self, knowledge_base_id: &str) -> Answer {
        self.call("GET", &format!("/knowledgebases/{knowledge_base_id}"), None)
            .await
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

    async fn create_data_source(&self, knowledge_base_id: &str, request: Value) -> Answer {
        self.call(
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
            Some(request),
        )
        .await
    }

    async fn data_source_id(&self, knowledge_base_id: &str, request: Value) -> String {
        let created = self.create_data_source(knowledge_base_id, request).await;
        assert_eq!(
            created.status, 200,
            "case setup: expected CreateDataSource -> 200, got {created:?}"
        );
        created.string(&["dataSource", "dataSourceId"])
    }

    async fn get_data_source(&self, knowledge_base_id: &str, data_source_id: &str) -> Answer {
        self.call(
            "GET",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}"),
            None,
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

    async fn delete_data_source(&self, knowledge_base_id: &str, data_source_id: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}"),
            None,
        )
        .await
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

    async fn create_flow(&self, request: Value) -> String {
        let created = self.call("POST", "/flows/", Some(request)).await;
        assert_eq!(
            created.status, 201,
            "case setup: expected CreateFlow -> 201, got {created:?}"
        );
        created.string(&["id"])
    }

    async fn get_flow(&self, flow_identifier: &str) -> Answer {
        self.call("GET", &format!("/flows/{}/", escaped(flow_identifier)), None)
            .await
    }

    async fn update_flow(&self, flow_identifier: &str, request: Value) -> Answer {
        self.call(
            "PUT",
            &format!("/flows/{}/", escaped(flow_identifier)),
            Some(request),
        )
        .await
    }

    async fn delete_flow(&self, flow_identifier: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/flows/{}/", escaped(flow_identifier)),
            None,
        )
        .await
    }

    /// The read that reports `target`, whatever family it belongs to.
    async fn read(&self, target: &Target) -> Answer {
        match target {
            Target::KnowledgeBase(knowledge_base_id) => {
                self.get_knowledge_base(knowledge_base_id).await
            }
            Target::DataSource(knowledge_base_id, data_source_id) => {
                self.get_data_source(knowledge_base_id, data_source_id).await
            }
            Target::Flow(flow_identifier) => self.get_flow(flow_identifier).await,
        }
    }

    /// Read `target` until it answers the modelled 404, or [`REMOVAL_POLL_CAP`]
    /// passes — the removal is owed by the CLOCK, so a client converges on it by
    /// polling exactly this way.
    async fn poll_until_not_found(&self, target: &Target) -> Answer {
        let deadline = Instant::now() + REMOVAL_POLL_CAP;
        loop {
            let answer = self.read(target).await;
            if answer.status == NOT_FOUND.1 || Instant::now() >= deadline {
                return answer;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// A `flowIdentifier` as it goes into the request path: the `:` and `/` of an
/// ARN are percent-encoded, which is how a client sends a label carrying them.
fn escaped(flow_identifier: &str) -> String {
    flow_identifier.replace(':', "%3A").replace('/', "%2F")
}

/// A self-managed vector knowledge base over an S3 vector store.
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
                "vectorBucketArn": "arn:aws:s3vectors:::bucket/delete-slice-vectors",
                "indexName": "delete-slice-index",
            },
        },
    })
}

/// A service-managed knowledge base: the one kind whose data sources have a
/// documented `dataDeletionPolicy` default (DELETE).
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

/// An S3 data source over `bucket`, with whatever `dataDeletionPolicy` the row
/// supplies (none when the row leaves it to the knowledge base's default).
fn s3_data_source(name: &str, bucket: &str, data_deletion_policy: Option<&str>) -> Value {
    let mut request = json!({
        "name": name,
        "dataSourceConfiguration": {
            "type": "S3",
            "s3Configuration": { "bucketArn": format!("arn:aws:s3:::{bucket}") },
        },
    });
    if let Some(policy) = data_deletion_policy {
        request["dataDeletionPolicy"] = json!(policy);
    }
    request
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
                "name": "InputToOutput",
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

/// The `ResourceNotFoundException` message AWS words for a knowledge base the
/// request names and the service does not hold: fixed text around the id the
/// request carried.
fn knowledge_base_not_found(knowledge_base_id: &str) -> String {
    format!("KnowledgeBase with id {knowledge_base_id} is not found.")
}

/// The same for a data source: the enclosing knowledge base is not part of the
/// sentence.
fn data_source_not_found(data_source_id: &str) -> String {
    format!("DataSource with id {data_source_id} is not found.")
}

/// The same for a flow: the whole sentence is fixed text for the error class
/// except the operation the caller invoked and the resource type, and the
/// requested identifier is not part of it.
fn flow_not_found(operation: &str) -> String {
    format!(
        "Couldn't perform {operation} operation. Failed to retrieve resource type Flow \
         because it doesn't exist. Check the resource ID and retry your request."
    )
}

/// Assert `answer` is the modelled not-found refusal, carrying the error type in
/// the `x-amzn-errortype` header and `message` the family's AWS sentence.
fn assert_not_found(case: &str, operation: &str, expected_message: &str, answer: &Answer) {
    let (code, status) = NOT_FOUND;
    assert_eq!(
        answer.status, status,
        "case {case}: expected {operation} to answer HTTP {status}, got {answer:?}"
    );
    assert_eq!(
        answer.error_type_header.as_deref(),
        Some(code),
        "case {case}: expected {operation} to name {code} in x-amzn-errortype, got {answer:?}"
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some(code),
        "case {case}: expected {operation} to answer {code}, got {answer:?}"
    );
    assert_eq!(
        answer.string(&["message"]),
        expected_message,
        "case {case}: expected {operation} to word its refusal {expected_message:?}, \
         got {answer:?}"
    );
}

/// A delete this service accepts removes the resource, so the read that reported
/// it converges on the modelled 404 `ResourceNotFoundException` with the
/// family's AWS wording.
///
/// The rows differ along the family, the children the resource contains, the
/// `dataDeletionPolicy` its data source carries, the kind of knowledge base it
/// belongs to, whether an ingestion job has run for it, and the identifier form
/// the request names it by.
#[tokio::test]
async fn an_accepted_delete_makes_every_later_read_answer_the_modelled_not_found() {
    let emulator = Emulator::new();

    // Row: an empty knowledge base.
    let empty_kb = emulator
        .create_knowledge_base(vector_knowledge_base("delete-empty-kb"))
        .await;

    // Row: a knowledge base holding two data sources, so the delete covers a
    // resource with children as well as one without.
    let populated_kb = emulator
        .create_knowledge_base(vector_knowledge_base("delete-populated-kb"))
        .await;
    for index in 0..2 {
        emulator
            .data_source_id(
                &populated_kb,
                s3_data_source(
                    &format!("delete-populated-source-{index}"),
                    &format!("delete-slice-docs-{index}"),
                    None,
                ),
            )
            .await;
    }

    // Row: a data source whose `dataDeletionPolicy` is RETAIN, and one of a
    // managed knowledge base, which takes the documented DELETE default — the
    // policy governs the ingested data, so neither value keeps the data source
    // itself in the store.
    let retain_kb = emulator
        .create_knowledge_base(vector_knowledge_base("delete-retain-kb"))
        .await;
    let retain_source = emulator
        .data_source_id(
            &retain_kb,
            s3_data_source("delete-retain-source", "delete-slice-retain", Some("RETAIN")),
        )
        .await;
    let managed_kb = emulator
        .create_knowledge_base(managed_knowledge_base("delete-managed-kb"))
        .await;
    let managed_source = emulator
        .data_source_id(
            &managed_kb,
            s3_data_source("delete-managed-source", "delete-slice-managed", None),
        )
        .await;

    // Row: a data source an ingestion job has already run for.
    let ingested_kb = emulator
        .create_knowledge_base(vector_knowledge_base("delete-ingested-kb"))
        .await;
    let ingested_source = emulator
        .data_source_id(
            &ingested_kb,
            s3_data_source("delete-ingested-source", "delete-slice-ingested", None),
        )
        .await;
    let started = emulator
        .start_ingestion_job(&ingested_kb, &ingested_source, json!({}))
        .await;
    assert_eq!(
        started.status, 202,
        "case setup: expected StartIngestionJob -> 202, got {started:?}"
    );

    // Rows: a flow named by its bare id, and one named by its ARN — the two
    // forms the modelled `FlowIdentifier` admits.
    let by_id_flow = emulator
        .create_flow(json!({
            "name": "delete-by-id-flow",
            "executionRoleArn": EXECUTION_ROLE,
            "definition": passthrough_definition(),
        }))
        .await;
    let by_arn_flow = emulator
        .create_flow(json!({
            "name": "delete-by-arn-flow",
            "executionRoleArn": EXECUTION_ROLE,
        }))
        .await;
    let by_arn_identifier = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:flow/{by_arn_flow}");

    struct Row {
        case: &'static str,
        target: Target,
        /// The identifier the DELETE request names, which for a flow may be
        /// either of its two modelled forms.
        delete_identifier: String,
        /// The status the delete's own response carries, and the member it
        /// reports it in — `None` for `DeleteFlow`, whose response carries only
        /// the flow's id.
        delete_status: u16,
        deleting_status: Option<&'static str>,
        /// The read that reports this resource, for the failure text.
        read_operation: &'static str,
        expected_message: String,
    }

    let rows = vec![
        Row {
            case: "a knowledge base holding no data source",
            target: Target::KnowledgeBase(empty_kb.clone()),
            delete_identifier: empty_kb.clone(),
            delete_status: 202,
            deleting_status: Some("DELETING"),
            read_operation: "GetKnowledgeBase",
            expected_message: knowledge_base_not_found(&empty_kb),
        },
        Row {
            case: "a knowledge base holding two data sources",
            target: Target::KnowledgeBase(populated_kb.clone()),
            delete_identifier: populated_kb.clone(),
            delete_status: 202,
            deleting_status: Some("DELETING"),
            read_operation: "GetKnowledgeBase",
            expected_message: knowledge_base_not_found(&populated_kb),
        },
        Row {
            case: "a data source whose dataDeletionPolicy is RETAIN",
            target: Target::DataSource(retain_kb.clone(), retain_source.clone()),
            delete_identifier: retain_source.clone(),
            delete_status: 202,
            deleting_status: Some("DELETING"),
            read_operation: "GetDataSource",
            expected_message: data_source_not_found(&retain_source),
        },
        Row {
            case: "a data source of a managed knowledge base, on the DELETE default",
            target: Target::DataSource(managed_kb.clone(), managed_source.clone()),
            delete_identifier: managed_source.clone(),
            delete_status: 202,
            deleting_status: Some("DELETING"),
            read_operation: "GetDataSource",
            expected_message: data_source_not_found(&managed_source),
        },
        Row {
            case: "a data source an ingestion job has run for",
            target: Target::DataSource(ingested_kb.clone(), ingested_source.clone()),
            delete_identifier: ingested_source.clone(),
            delete_status: 202,
            deleting_status: Some("DELETING"),
            read_operation: "GetDataSource",
            expected_message: data_source_not_found(&ingested_source),
        },
        Row {
            case: "a flow named by its flowId",
            target: Target::Flow(by_id_flow.clone()),
            delete_identifier: by_id_flow.clone(),
            delete_status: 200,
            deleting_status: None,
            read_operation: "GetFlow",
            expected_message: flow_not_found("GetFlow"),
        },
        Row {
            case: "a flow named by its ARN",
            target: Target::Flow(by_arn_identifier.clone()),
            delete_identifier: by_arn_identifier.clone(),
            delete_status: 200,
            deleting_status: None,
            read_operation: "GetFlow",
            expected_message: flow_not_found("GetFlow"),
        },
    ];

    for row in rows {
        // The resource is there before the delete: the 404 below is the delete's
        // doing rather than a resource that never existed.
        let before = emulator.read(&row.target).await;
        assert_eq!(
            before.status, 200,
            "case {}: expected {} to report the resource before the delete, got {before:?}",
            row.case, row.read_operation
        );

        let deleted = match &row.target {
            Target::KnowledgeBase(_) => {
                emulator.delete_knowledge_base(&row.delete_identifier).await
            }
            Target::DataSource(knowledge_base_id, _) => {
                emulator
                    .delete_data_source(knowledge_base_id, &row.delete_identifier)
                    .await
            }
            Target::Flow(_) => emulator.delete_flow(&row.delete_identifier).await,
        };
        assert_eq!(
            deleted.status, row.delete_status,
            "case {}: expected the delete to answer HTTP {}, got {deleted:?}",
            row.case, row.delete_status
        );
        match (&row.target, row.deleting_status) {
            (Target::KnowledgeBase(knowledge_base_id), Some(deleting)) => {
                assert_eq!(
                    deleted.string(&["knowledgeBaseId"]),
                    *knowledge_base_id,
                    "case {}: expected the delete to report the knowledge base it deleted, \
                     got {deleted:?}",
                    row.case
                );
                assert_eq!(
                    deleted.string(&["status"]),
                    deleting,
                    "case {}: expected the delete to report {deleting}, got {deleted:?}",
                    row.case
                );
            }
            (Target::DataSource(knowledge_base_id, data_source_id), Some(deleting)) => {
                assert_eq!(
                    deleted.string(&["knowledgeBaseId"]),
                    *knowledge_base_id,
                    "case {}: expected the delete to report the knowledge base the data \
                     source belonged to, got {deleted:?}",
                    row.case
                );
                assert_eq!(
                    deleted.string(&["dataSourceId"]),
                    *data_source_id,
                    "case {}: expected the delete to report the data source it deleted, \
                     got {deleted:?}",
                    row.case
                );
                assert_eq!(
                    deleted.string(&["status"]),
                    deleting,
                    "case {}: expected the delete to report {deleting}, got {deleted:?}",
                    row.case
                );
            }
            (Target::Flow(_), _) => {
                assert_eq!(
                    deleted.string(&["id"]),
                    by_flow_id(&row.delete_identifier),
                    "case {}: expected the delete to report the flowId it deleted, \
                     got {deleted:?}",
                    row.case
                );
            }
            (target, deleting) => panic!(
                "case {}: {target:?} and {deleting:?} are not a pair this table builds",
                row.case
            ),
        }

        // No read after the delete ever reports the resource as it stood: a
        // family whose model carries a deleting status reports THAT until the
        // removal the clock owes happens, and nothing else.
        let after = emulator.read(&row.target).await;
        match row.deleting_status {
            Some(deleting) if after.status == 200 => {
                let reported = match &row.target {
                    Target::KnowledgeBase(_) => after.string(&["knowledgeBase", "status"]),
                    _ => after.string(&["dataSource", "status"]),
                };
                assert_eq!(
                    reported, deleting,
                    "case {}: expected the read after the delete to report {deleting}, \
                     got {after:?}",
                    row.case
                );
            }
            Some(_) => assert_eq!(
                after.status,
                NOT_FOUND.1,
                "case {}: a read after the delete answers the resource as DELETING or the \
                 modelled 404, got {after:?}",
                row.case
            ),
            None => assert_eq!(
                after.status,
                NOT_FOUND.1,
                "case {}: a flow has no deleting status, so the read after the delete \
                 answers the modelled 404 at once, got {after:?}",
                row.case
            ),
        }

        let gone = emulator.poll_until_not_found(&row.target).await;
        assert_not_found(row.case, row.read_operation, &row.expected_message, &gone);
    }
}

/// The `flowId` inside whichever identifier form a row used: the last path
/// segment of an ARN, or the bare id itself.
fn by_flow_id(flow_identifier: &str) -> String {
    flow_identifier
        .rsplit('/')
        .next()
        .unwrap_or(flow_identifier)
        .to_string()
}

/// Deleting a knowledge base deletes what it contains: the data sources inside
/// it, and the ingestion jobs that ran for them, go with it.
#[tokio::test]
async fn deleting_a_knowledge_base_removes_the_data_sources_inside_it() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .create_knowledge_base(vector_knowledge_base("cascade-kb"))
        .await;

    // Rows differ along the number of children: two data sources, one of which
    // has run an ingestion job.
    let mut data_source_ids = Vec::new();
    for index in 0..2 {
        data_source_ids.push(
            emulator
                .data_source_id(
                    &knowledge_base_id,
                    s3_data_source(
                        &format!("cascade-source-{index}"),
                        &format!("cascade-docs-{index}"),
                        None,
                    ),
                )
                .await,
        );
    }
    let started = emulator
        .start_ingestion_job(&knowledge_base_id, &data_source_ids[0], json!({}))
        .await;
    assert_eq!(
        started.status, 202,
        "case setup: expected StartIngestionJob -> 202, got {started:?}"
    );
    let ingestion_job_id = started.string(&["ingestionJob", "ingestionJobId"]);

    let deleted = emulator.delete_knowledge_base(&knowledge_base_id).await;
    assert_eq!(
        deleted.status, 202,
        "case the knowledge base: expected DeleteKnowledgeBase -> 202, got {deleted:?}"
    );
    let gone = emulator
        .poll_until_not_found(&Target::KnowledgeBase(knowledge_base_id.clone()))
        .await;
    assert_not_found(
        "the knowledge base",
        "GetKnowledgeBase",
        &knowledge_base_not_found(&knowledge_base_id),
        &gone,
    );

    for data_source_id in &data_source_ids {
        let read = emulator
            .get_data_source(&knowledge_base_id, data_source_id)
            .await;
        assert_not_found(
            "a data source inside the deleted knowledge base",
            "GetDataSource",
            &data_source_not_found(data_source_id),
            &read,
        );
    }

    let job = emulator
        .get_ingestion_job(&knowledge_base_id, &data_source_ids[0], &ingestion_job_id)
        .await;
    assert_eq!(
        job.status,
        NOT_FOUND.1,
        "case an ingestion job of the deleted knowledge base: expected GetIngestionJob -> \
         {}, got {job:?}",
        NOT_FOUND.1
    );
    assert_eq!(
        job.error_type.as_deref(),
        Some(NOT_FOUND.0),
        "case an ingestion job of the deleted knowledge base: expected {}, got {job:?}",
        NOT_FOUND.0
    );
}

/// A resource an accepted delete is working through serves no operation that
/// acts ON it: each one answers the modelled not-found it will answer once the
/// removal has happened.
#[tokio::test]
async fn a_deleted_resource_serves_no_operation_that_acts_on_it() {
    let emulator = Emulator::new();

    let knowledge_base_id = emulator
        .create_knowledge_base(vector_knowledge_base("in-use-kb"))
        .await;
    let data_source_id = emulator
        .data_source_id(
            &knowledge_base_id,
            s3_data_source("in-use-source", "in-use-docs", None),
        )
        .await;
    let other_knowledge_base_id = emulator
        .create_knowledge_base(vector_knowledge_base("in-use-other-kb"))
        .await;
    let other_data_source_id = emulator
        .data_source_id(
            &other_knowledge_base_id,
            s3_data_source("in-use-other-source", "in-use-other-docs", None),
        )
        .await;
    let flow_id = emulator
        .create_flow(json!({
            "name": "in-use-flow",
            "executionRoleArn": EXECUTION_ROLE,
        }))
        .await;

    // The data source goes first, so the operations that act on a data source
    // are checked while its own knowledge base is still live.
    let deleted_source = emulator
        .delete_data_source(&other_knowledge_base_id, &other_data_source_id)
        .await;
    assert_eq!(
        deleted_source.status, 202,
        "case setup: expected DeleteDataSource -> 202, got {deleted_source:?}"
    );
    let deleted_knowledge_base = emulator.delete_knowledge_base(&knowledge_base_id).await;
    assert_eq!(
        deleted_knowledge_base.status, 202,
        "case setup: expected DeleteKnowledgeBase -> 202, got {deleted_knowledge_base:?}"
    );
    let deleted_flow = emulator.delete_flow(&flow_id).await;
    assert_eq!(
        deleted_flow.status, 200,
        "case setup: expected DeleteFlow -> 200, got {deleted_flow:?}"
    );

    // Rows: one per operation that resolves a deleted resource to act on it,
    // spanning all three families and both create-, update- and
    // start-shaped operations.
    let create_in_deleted_kb = emulator
        .create_data_source(
            &knowledge_base_id,
            s3_data_source("orphan-source", "orphan-docs", None),
        )
        .await;
    assert_not_found(
        "CreateDataSource under a deleted knowledge base",
        "CreateDataSource",
        &knowledge_base_not_found(&knowledge_base_id),
        &create_in_deleted_kb,
    );

    let update_deleted_kb = emulator
        .update_knowledge_base(
            &knowledge_base_id,
            json!({
                "name": "in-use-kb-renamed",
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": {
                    "type": "VECTOR",
                    "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
                },
                "storageConfiguration": {
                    "type": "S3_VECTORS",
                    "s3VectorsConfiguration": {
                        "vectorBucketArn": "arn:aws:s3vectors:::bucket/delete-slice-vectors",
                        "indexName": "delete-slice-index",
                    },
                },
            }),
        )
        .await;
    assert_not_found(
        "UpdateKnowledgeBase on a deleted knowledge base",
        "UpdateKnowledgeBase",
        &knowledge_base_not_found(&knowledge_base_id),
        &update_deleted_kb,
    );

    let ingest_deleted_kb = emulator
        .start_ingestion_job(&knowledge_base_id, &data_source_id, json!({}))
        .await;
    assert_not_found(
        "StartIngestionJob under a deleted knowledge base",
        "StartIngestionJob",
        &knowledge_base_not_found(&knowledge_base_id),
        &ingest_deleted_kb,
    );

    let update_deleted_source = emulator
        .update_data_source(
            &other_knowledge_base_id,
            &other_data_source_id,
            s3_data_source("in-use-other-source-renamed", "in-use-other-docs", None),
        )
        .await;
    assert_not_found(
        "UpdateDataSource on a deleted data source",
        "UpdateDataSource",
        &data_source_not_found(&other_data_source_id),
        &update_deleted_source,
    );

    let ingest_deleted_source = emulator
        .start_ingestion_job(&other_knowledge_base_id, &other_data_source_id, json!({}))
        .await;
    assert_not_found(
        "StartIngestionJob on a deleted data source",
        "StartIngestionJob",
        &data_source_not_found(&other_data_source_id),
        &ingest_deleted_source,
    );

    let update_deleted_flow = emulator
        .update_flow(
            &flow_id,
            json!({ "name": "in-use-flow-renamed", "executionRoleArn": EXECUTION_ROLE }),
        )
        .await;
    assert_not_found(
        "UpdateFlow on a deleted flow",
        "UpdateFlow",
        &flow_not_found("UpdateFlow"),
        &update_deleted_flow,
    );
}

/// A delete of a resource nothing holds is the modelled not-found, and a repeat
/// of an accepted delete reports the deletion already under way rather than
/// starting a second one.
#[tokio::test]
async fn deleting_what_nothing_holds_is_the_modelled_not_found() {
    let emulator = Emulator::new();

    // Rows: a well-formed identifier of each family that the store does not
    // hold. Every one matches its member's modelled pattern
    // (`^[0-9a-zA-Z]{10}$`), so the refusal comes from the store rather than
    // from the boundary validator.
    let unknown = "0123456789";
    let unknown_knowledge_base = emulator.delete_knowledge_base(unknown).await;
    assert_not_found(
        "DeleteKnowledgeBase on an id nothing holds",
        "DeleteKnowledgeBase",
        &knowledge_base_not_found(unknown),
        &unknown_knowledge_base,
    );

    let knowledge_base_id = emulator
        .create_knowledge_base(vector_knowledge_base("repeat-delete-kb"))
        .await;
    let unknown_data_source = emulator
        .delete_data_source(&knowledge_base_id, unknown)
        .await;
    assert_not_found(
        "DeleteDataSource on an id nothing holds",
        "DeleteDataSource",
        &data_source_not_found(unknown),
        &unknown_data_source,
    );

    let unknown_flow = emulator.delete_flow(unknown).await;
    assert_not_found(
        "DeleteFlow on an id nothing holds",
        "DeleteFlow",
        &flow_not_found("DeleteFlow"),
        &unknown_flow,
    );

    // A repeat while the deletion is still under way reports the same resource
    // in the same status: the delete is modelled `@idempotent`.
    let data_source_id = emulator
        .data_source_id(
            &knowledge_base_id,
            s3_data_source("repeat-delete-source", "repeat-delete-docs", None),
        )
        .await;
    let once = emulator
        .delete_data_source(&knowledge_base_id, &data_source_id)
        .await;
    let again = emulator
        .delete_data_source(&knowledge_base_id, &data_source_id)
        .await;
    assert_eq!(
        (once.status, once.string(&["status"])),
        (again.status, again.string(&["status"])),
        "case a repeated DeleteDataSource: expected the same answer as {once:?}, got {again:?}"
    );

    let kb_once = emulator.delete_knowledge_base(&knowledge_base_id).await;
    let kb_again = emulator.delete_knowledge_base(&knowledge_base_id).await;
    assert_eq!(
        (kb_once.status, kb_once.string(&["status"])),
        (kb_again.status, kb_again.string(&["status"])),
        "case a repeated DeleteKnowledgeBase: expected the same answer as {kb_once:?}, \
         got {kb_again:?}"
    );

    // Once the clock has made the removal due, the delete itself answers the
    // modelled not-found: there is nothing left to delete.
    tokio::time::sleep(DELETION_SPAN + POLL_INTERVAL).await;
    let after_removal = emulator.delete_knowledge_base(&knowledge_base_id).await;
    assert_not_found(
        "DeleteKnowledgeBase once the removal has happened",
        "DeleteKnowledgeBase",
        &knowledge_base_not_found(&knowledge_base_id),
        &after_removal,
    );
}
