// SPDX-License-Identifier: CC0-1.0

//! `CreateFlowVersion`, end to end over HTTP, read back through
//! `ListFlowVersions`: a flow enumerates its DRAFT version plus one summary per
//! numbered version cut from it, each carrying that version's own status.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.
//!
//! The one thing these tests reach into the store for is the INSTANT a pending
//! preparation comes due: `PrepareFlow` stores it rather than sleeping, so a
//! test moves it into the past instead of waiting out
//! [`FLOW_PREPARATION_SPAN`](service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::FLOW_PREPARATION_SPAN).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::service_state;
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on, and the source of the region its ARNs
/// carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `FlowExecutionRoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/(service-role/)?.+$`.
const EXECUTION_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

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

    /// The `version` member of every reported summary, in the order the
    /// response listed them.
    fn listed_versions(&self) -> Vec<String> {
        self.summaries()
            .iter()
            .map(|summary| {
                summary["version"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a summary carries no version: {summary:?}"))
                    .to_string()
            })
            .collect()
    }

    /// `(version, status)` for every reported summary, in the order the response
    /// listed them.
    fn listed_statuses(&self) -> Vec<(String, String)> {
        self.summaries()
            .iter()
            .map(|summary| {
                (
                    summary["version"].as_str().unwrap_or_default().to_string(),
                    summary["status"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    fn summaries(&self) -> Vec<Value> {
        self.member(&["flowVersionSummaries"])
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "flowVersionSummaries is not an array in {:?}",
                    self.body
                )
            })
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
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

    fn escape(identifier: &str) -> String {
        identifier.replace(':', "%3A").replace('/', "%2F")
    }

    /// `CreateFlow`, which mints the flow and the DRAFT version a numbered
    /// version is cut from.
    async fn create_flow(&self, case: &str, request: Value) -> Answer {
        let created = self.call("POST", "/flows/", Some(request)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected CreateFlow -> 201, got {created:?}"
        );
        created
    }

    async fn update_flow(&self, case: &str, identifier: &str, request: Value) -> Answer {
        let escaped = Self::escape(identifier);
        let updated = self
            .call("PUT", &format!("/flows/{escaped}/"), Some(request))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateFlow -> 200, got {updated:?}"
        );
        updated
    }

    async fn prepare_flow(&self, case: &str, identifier: &str) -> Answer {
        let escaped = Self::escape(identifier);
        let prepared = self
            .call("POST", &format!("/flows/{escaped}/"), Some(json!({})))
            .await;
        assert_eq!(
            prepared.status, 202,
            "case {case}: expected PrepareFlow -> 202, got {prepared:?}"
        );
        prepared
    }

    async fn create_flow_version(&self, identifier: &str, request: Value) -> Answer {
        let escaped = Self::escape(identifier);
        self.call("POST", &format!("/flows/{escaped}/versions"), Some(request))
            .await
    }

    async fn list_flow_versions(&self, identifier: &str) -> Answer {
        let escaped = Self::escape(identifier);
        self.call("GET", &format!("/flows/{escaped}/versions"), None)
            .await
    }

    /// Move the instant the pending preparation comes due into the past, so the
    /// next caller observes the outcome without the test sleeping for the span.
    fn make_preparation_due(&self, case: &str, flow_id: &str) {
        let state = service_state(&*self.engine).expect("the bedrock-agent store");
        let mut state = state.lock();
        let due = SystemTime::now() - Duration::from_secs(1);
        let pending = state
            .flows
            .get_mut(flow_id)
            .expect("the stored flow")
            .pending_preparation
            .as_mut()
            .unwrap_or_else(|| panic!("case {case}: expected a preparation in flight"));
        pending.due = due;
        for version in state.flow_versions.values_mut() {
            if let Some((_, next)) = version.pending_status {
                version.pending_status = Some((due, next));
            }
        }
    }
}

/// A passthrough flow definition: an `Input` node wired straight to an `Output`
/// node. The node types, the union keys and the IO data types are the modelled
/// `FlowNodeType` / `FlowNodeConfiguration` / `FlowNodeIODataType` values.
fn passthrough_definition(output_name: &str) -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "String" }],
            },
            {
                "name": output_name,
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
                "target": output_name,
                "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
            },
        ],
    })
}

/// The listing a flow with `count` numbered versions reports: its DRAFT version
/// first, then every numbered version in the order it was cut.
fn expected_listing(count: u32) -> Vec<String> {
    ::std::iter::once("DRAFT".to_string())
        .chain((1..=count).map(|version| version.to_string()))
        .collect()
}

/// Every version a flow carries is enumerated: its DRAFT version plus one
/// summary per version `CreateFlowVersion` cut, numbered incrementally from 1.
///
/// Rows differ in the COUNT of versions cut — including none at all, where the
/// DRAFT is the only version there is.
#[tokio::test]
async fn create_flow_version_cuts_incrementally_numbered_versions_the_listing_enumerates() {
    for count in [0_u32, 1, 2, 3, 5] {
        let case = format!("{count} version(s) cut");
        let emulator = Emulator::new();
        let flow = emulator
            .create_flow(
                &case,
                json!({
                    "name": "version-minting-flow",
                    "description": "the flow being versioned",
                    "executionRoleArn": EXECUTION_ROLE,
                    "definition": passthrough_definition("FlowOutput"),
                }),
            )
            .await;
        let flow_id = flow.string(&["id"]);
        let flow_arn = flow.string(&["arn"]);
        let flow_name = flow.string(&["name"]);
        emulator.prepare_flow(&case, &flow_id).await;
        emulator.make_preparation_due(&case, &flow_id);

        for expected_version in 1..=count {
            let cut = emulator
                .create_flow_version(&flow_id, json!({}))
                .await;
            assert_eq!(
                cut.status, 201,
                "case {case}: expected CreateFlowVersion -> 201, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["version"]),
                expected_version.to_string(),
                "case {case}: versions are numbered incrementally from 1, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["id"]),
                flow_id,
                "case {case}: the version names the flow it was cut from, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["arn"]),
                flow_arn,
                "case {case}: the version carries the ARN of its flow, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["name"]),
                flow_name,
                "case {case}: the version snapshots the flow's name, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["executionRoleArn"]),
                EXECUTION_ROLE,
                "case {case}: the version snapshots the flow's execution role, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["description"]),
                "the flow being versioned",
                "case {case}: a request that gives no description leaves the flow's own, \
                 got {cut:?}"
            );
            assert_eq!(
                cut.string(&["status"]),
                "Prepared",
                "case {case}: the version snapshots the DRAFT's status, got {cut:?}"
            );
            assert_eq!(
                cut.member(&["definition"]),
                flow.member(&["definition"]),
                "case {case}: the version packages the DRAFT's definition, got {cut:?}"
            );
            let created_at = cut.string(&["createdAt"]);
            assert!(
                created_at.contains('T') && created_at.ends_with('Z'),
                "case {case}: expected createdAt in the modelled date-time wire form, \
                 got {created_at:?}"
            );
        }

        // The round trip: the listing enumerates the DRAFT and every version cut.
        let listed = emulator.list_flow_versions(&flow_id).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected ListFlowVersions -> 200, got {listed:?}"
        );
        assert_eq!(
            listed.listed_versions(),
            expected_listing(count),
            "case {case}: expected the DRAFT version then every version cut, got {:?}",
            listed.listed_versions()
        );
        for summary in listed.summaries() {
            assert_eq!(
                summary["id"].as_str(),
                Some(flow_id.as_str()),
                "case {case}: a summary names the flow it belongs to, got {summary:?}"
            );
            assert_eq!(
                summary["arn"].as_str(),
                Some(flow_arn.as_str()),
                "case {case}: a summary carries the ARN of its flow, got {summary:?}"
            );
            // The DRAFT was prepared before any version was cut, so every entry —
            // the DRAFT itself and each snapshot of it — reports that outcome.
            assert_eq!(
                summary["status"].as_str(),
                Some("Prepared"),
                "case {case}: a summary reports that version's own status, got {summary:?}"
            );
        }
    }
}

/// A numbered version is a SNAPSHOT: each summary reports the status of its own
/// version, and a later change to the DRAFT moves the DRAFT alone. A version cut
/// from a DRAFT still being prepared settles with that preparation.
#[tokio::test]
async fn each_summary_reports_its_own_version_status_as_the_draft_moves_on() {
    let case = "the DRAFT moves while its versions stand";
    let emulator = Emulator::new();
    let flow = emulator
        .create_flow(
            case,
            json!({
                "name": "snapshot-flow",
                "executionRoleArn": EXECUTION_ROLE,
                "definition": passthrough_definition("FlowOutput"),
            }),
        )
        .await;
    let flow_id = flow.string(&["id"]);

    // A version cut from a DRAFT that has never been prepared carries the only
    // status that DRAFT has ever had.
    let first = emulator.create_flow_version(&flow_id, json!({})).await;
    assert_eq!(
        first.status, 201,
        "case {case}: expected CreateFlowVersion -> 201, got {first:?}"
    );
    assert_eq!(
        emulator.list_flow_versions(&flow_id).await.listed_statuses(),
        vec![
            ("DRAFT".to_string(), "NotPrepared".to_string()),
            ("1".to_string(), "NotPrepared".to_string()),
        ],
        "case {case}: a version cut from an unprepared DRAFT reports NotPrepared"
    );

    // Preparing the DRAFT moves the DRAFT. The version already cut does not
    // move with it: it packages the definition it was cut from.
    emulator.prepare_flow(case, &flow_id).await;
    assert_eq!(
        emulator.list_flow_versions(&flow_id).await.listed_statuses(),
        vec![
            ("DRAFT".to_string(), "Preparing".to_string()),
            ("1".to_string(), "NotPrepared".to_string()),
        ],
        "case {case}: preparing the DRAFT leaves an already-cut version where it was"
    );

    // A version cut WHILE the DRAFT is being prepared packages the very
    // definition being prepared, so it reports that transient status and then
    // settles with it rather than standing in it for ever.
    let second = emulator.create_flow_version(&flow_id, json!({})).await;
    assert_eq!(
        second.string(&["status"]),
        "Preparing",
        "case {case}: a version cut mid-preparation snapshots the DRAFT's status, \
         got {second:?}"
    );
    emulator.make_preparation_due(case, &flow_id);
    assert_eq!(
        emulator.list_flow_versions(&flow_id).await.listed_statuses(),
        vec![
            ("DRAFT".to_string(), "Prepared".to_string()),
            ("1".to_string(), "NotPrepared".to_string()),
            ("2".to_string(), "Prepared".to_string()),
        ],
        "case {case}: the preparation's outcome is reported for the DRAFT and for the \
         version cut from it"
    );

    // Updating the flow moves the DRAFT back to NotPrepared, and neither
    // numbered version moves with it.
    emulator
        .update_flow(
            case,
            &flow_id,
            json!({
                "name": "snapshot-flow",
                "executionRoleArn": EXECUTION_ROLE,
                "definition": passthrough_definition("RenamedOutput"),
            }),
        )
        .await;
    assert_eq!(
        emulator.list_flow_versions(&flow_id).await.listed_statuses(),
        vec![
            ("DRAFT".to_string(), "NotPrepared".to_string()),
            ("1".to_string(), "NotPrepared".to_string()),
            ("2".to_string(), "Prepared".to_string()),
        ],
        "case {case}: updating the DRAFT moves the DRAFT alone"
    );
}

/// The `@idempotencyToken` makes a retried cut report the version it already
/// cut; the version's description is the request's when it gives one; and a flow
/// the store does not hold, or an identifier of neither modelled form, is
/// refused rather than cut.
#[tokio::test]
async fn create_flow_version_replays_a_retried_cut_and_refuses_an_unknown_flow() {
    let case = "retried and refused cuts";
    let emulator = Emulator::new();
    let flow = emulator
        .create_flow(
            case,
            json!({
                "name": "idempotent-version-flow",
                "executionRoleArn": EXECUTION_ROLE,
                "definition": passthrough_definition("FlowOutput"),
            }),
        )
        .await;
    let flow_id = flow.string(&["id"]);
    let flow_arn = flow.string(&["arn"]);

    // The modelled `ClientToken` is `length 33..256, pattern
    // ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`.
    let token = "idempotency-token-for-flow-version-1";
    let other_token = "idempotency-token-for-flow-version-2";

    let first = emulator
        .create_flow_version(
            &flow_id,
            json!({ "clientToken": token, "description": "the first cut version" }),
        )
        .await;
    assert_eq!(
        first.status, 201,
        "case {case}: expected CreateFlowVersion -> 201, got {first:?}"
    );
    assert_eq!(
        first.string(&["description"]),
        "the first cut version",
        "case {case}: the version carries the description the request gave, got {first:?}"
    );

    let retried = emulator
        .create_flow_version(
            &flow_id,
            json!({ "clientToken": token, "description": "the first cut version" }),
        )
        .await;
    assert_eq!(
        retried.body, first.body,
        "case {case}: a retry of the same token reports the version it already cut, \
         got {retried:?}"
    );

    let second = emulator
        .create_flow_version(&flow_id, json!({ "clientToken": other_token }))
        .await;
    assert_eq!(
        second.string(&["version"]),
        "2",
        "case {case}: a different token cuts the next version, got {second:?}"
    );

    let listed = emulator.list_flow_versions(&flow_id).await;
    assert_eq!(
        listed.listed_versions(),
        expected_listing(2),
        "case {case}: the retry cut no second version, got {:?}",
        listed.listed_versions()
    );

    // Rows: both modelled `FlowIdentifier` forms, over a well-formed id the
    // store does not hold.
    let unknown = "0123456789";
    for (form, identifier) in [
        ("bare flowId", unknown.to_string()),
        ("flow ARN", flow_arn.replace(flow_id.as_str(), unknown)),
    ] {
        let refused = emulator.create_flow_version(&identifier, json!({})).await;
        assert_eq!(
            refused.status, 404,
            "case {case} ({form}): expected CreateFlowVersion -> 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case} ({form}): expected ResourceNotFoundException, got {refused:?}"
        );
        let message = refused.string(&["message"]);
        assert!(
            message.contains("CreateFlowVersion") && message.contains("Flow"),
            "case {case} ({form}): the refusal names the operation and the resource type, \
             got {message:?}"
        );
    }

    // Rows differ in HOW the identifier misses the modelled `FlowIdentifier`
    // forms: a flow name, ids of the wrong length or alphabet, and ARNs wrong in
    // the service, the resource type and the partition.
    for (form, identifier) in [
        ("a flow name, not an identifier", "idempotent-version-flow".to_string()),
        ("nine characters", "012345678".to_string()),
        ("eleven characters", "0123456789a".to_string()),
        ("a character outside the id alphabet", "0123_56789".to_string()),
        ("an ARN naming another service", flow_arn.replace(":bedrock:", ":s3:")),
        ("an ARN naming another resource type", flow_arn.replace(":flow/", ":agent/")),
        ("an ARN in another partition", flow_arn.replace("arn:aws:", "arn:aws-cn:")),
    ] {
        let refused = emulator.create_flow_version(&identifier, json!({})).await;
        assert_eq!(
            refused.status, 400,
            "case {case} ({form}, {identifier}): expected CreateFlowVersion -> 400, \
             got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case} ({form}, {identifier}): expected ValidationException, got {refused:?}"
        );
    }

    // Every refusal above cut nothing.
    let listed = emulator.list_flow_versions(&flow_id).await;
    assert_eq!(
        listed.listed_versions(),
        expected_listing(2),
        "case {case}: a refused cut leaves the flow's versions as they were, got {:?}",
        listed.listed_versions()
    );
}
