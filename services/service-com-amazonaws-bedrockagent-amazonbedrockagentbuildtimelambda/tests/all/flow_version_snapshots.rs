// SPDX-License-Identifier: CC0-1.0

//! `CreateFlowVersion`, end to end over HTTP: a numbered version is a SNAPSHOT
//! of one flow's DRAFT version, numbered in that flow's own sequence and
//! reported from the record the cut stored.
//!
//! Two properties are pinned here, each over a table of inputs:
//!
//! * the sequence and the snapshot belong to ONE flow — several flows in one
//!   store number their versions independently, from 1, whichever
//!   `FlowIdentifier` form addresses them and whichever `clientToken` the cut
//!   carries, and each version reports its own flow's members;
//! * the snapshot is frozen at the cut — a later `UpdateFlow` moves the DRAFT
//!   alone, so the version already cut keeps the name, description and
//!   definition it was cut from while the next cut carries the new ones.
//!
//! `flow_version_minting.rs` covers the incremental numbering of a single flow,
//! the retried cut and the refusals; `flow_version_listing.rs` covers the
//! listing's ordering and paging. Every assertion here goes through the router,
//! so it exercises the real wire path (route matching, model constraint
//! validation, serialization) rather than calling a handler directly.
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

/// Three role ARNs, each a different form the modelled `FlowExecutionRoleArn`
/// pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/(service-role/)?.+$`
/// admits: the service-linked path, a plain role path, and the optional account
/// id left out.
const SERVICE_LINKED_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";
const PLAIN_ROLE: &str = "arn:aws:iam::000000000000:role/BedrockFlowsRole";
const ACCOUNTLESS_ROLE: &str = "arn:aws:iam:::role/BedrockFlowsRole";

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

    fn summaries(&self) -> Vec<Value> {
        self.member(&["flowVersionSummaries"])
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("flowVersionSummaries is not an array in {:?}", self.body)
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

    fn escape(identifier: &str) -> String {
        identifier.replace(':', "%3A").replace('/', "%2F")
    }

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

    /// `PrepareFlow`, then the instant its outcome comes due moved into the
    /// past, so the flow is `Prepared` for the next caller without the test
    /// sleeping for the span. A version is cut from a prepared DRAFT, which is
    /// the state every cut below starts from.
    async fn prepare_and_settle(&self, case: &str, flow_id: &str) {
        let escaped = Self::escape(flow_id);
        let prepared = self
            .call("POST", &format!("/flows/{escaped}/"), Some(json!({})))
            .await;
        assert_eq!(
            prepared.status, 202,
            "case {case}: expected PrepareFlow -> 202, got {prepared:?}"
        );
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
    }
}

/// A passthrough flow definition: an `Input` node wired straight to an `Output`
/// node named `output_name`. The node types, the union keys and the IO data
/// types are the modelled `FlowNodeType` / `FlowNodeConfiguration` /
/// `FlowNodeIODataType` values.
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

/// The `CreateFlow`/`UpdateFlow` request body for a flow row, with
/// `description` left out entirely when the row carries none. Both operations
/// take the same members, and `UpdateFlow` documents that a member the request
/// leaves out is left out of the flow.
fn flow_request(
    name: &str,
    description: Option<&str>,
    execution_role: &str,
    output_node: &str,
) -> Value {
    let mut request = json!({
        "name": name,
        "executionRoleArn": execution_role,
        "definition": passthrough_definition(output_node),
    });
    if let Some(description) = description {
        request["description"] = json!(description);
    }
    request
}

/// The listing a flow with `count` numbered versions reports: its DRAFT version
/// first, then every numbered version in the order it was cut.
fn expected_listing(count: u32) -> Vec<String> {
    ::std::iter::once("DRAFT".to_string())
        .chain((1..=count).map(|version| version.to_string()))
        .collect()
}

/// One flow in a store that holds several.
#[derive(Debug, Clone, Copy)]
struct FlowCase {
    /// The flow's name, which a version cut from it snapshots.
    name: &'static str,
    /// The flow's own description, which a cut that gives none inherits.
    description: Option<&'static str>,
    /// Which modelled `FlowExecutionRoleArn` form this flow carries.
    execution_role: &'static str,
    /// The `Output` node this flow's definition ends at: what tells one row's
    /// definition from another's.
    output_node: &'static str,
    /// How many numbered versions this flow is cut into.
    cuts: u32,
    /// Whether the cuts address the flow by its ARN rather than by its bare id.
    /// The modelled `FlowIdentifier` admits both forms for the same flow.
    by_arn: bool,
}

/// Rows differing in the count of versions cut (including none at all), the
/// name, whether a description is carried, the execution-role form, the
/// definition, and the `FlowIdentifier` form the cuts address the flow by.
const FLOW_CASES: &[FlowCase] = &[
    FlowCase {
        name: "alpha-flow",
        description: Some("the alpha flow"),
        execution_role: SERVICE_LINKED_ROLE,
        output_node: "AlphaOutput",
        cuts: 1,
        by_arn: false,
    },
    FlowCase {
        name: "beta_flow",
        description: None,
        execution_role: PLAIN_ROLE,
        output_node: "BetaOutput",
        cuts: 3,
        by_arn: true,
    },
    FlowCase {
        name: "gamma-flow-3",
        description: Some("the gamma flow"),
        execution_role: ACCOUNTLESS_ROLE,
        output_node: "GammaOutput",
        cuts: 2,
        by_arn: false,
    },
    FlowCase {
        name: "d",
        description: Some("the flow no version is cut from"),
        execution_role: PLAIN_ROLE,
        output_node: "DeltaOutput",
        cuts: 0,
        by_arn: true,
    },
];

/// Every flow numbers its versions in its OWN sequence, from 1, and every
/// version reports the flow it was cut from: the cuts below are interleaved
/// across four flows in one store, so a sequence or a snapshot shared between
/// flows would show up immediately. The same `clientToken` is used for every cut
/// of one round, which cuts a version on each flow: an idempotency token names a
/// request on ONE flow.
#[tokio::test]
async fn each_flow_numbers_and_snapshots_its_own_versions() {
    let emulator = Emulator::new();

    // Every flow is created and driven to Prepared first, so each cut below is
    // taken from a prepared DRAFT.
    let mut flows: Vec<(FlowCase, Answer)> = Vec::new();
    for case in FLOW_CASES {
        let created = emulator
            .create_flow(
                case.name,
                flow_request(
                    case.name,
                    case.description,
                    case.execution_role,
                    case.output_node,
                ),
            )
            .await;
        let flow_id = created.string(&["id"]);
        emulator.prepare_and_settle(case.name, &flow_id).await;
        flows.push((*case, created));
    }

    let rounds = FLOW_CASES
        .iter()
        .map(|case| case.cuts)
        .max()
        .unwrap_or_default();
    for round in 1..=rounds {
        // The modelled `ClientToken` is `length 33..256, pattern
        // ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`.
        let token = format!("round-{round}-shared-idempotency-token-across-flows");
        for (case, flow) in &flows {
            if case.cuts < round {
                continue;
            }
            let flow_id = flow.string(&["id"]);
            let flow_arn = flow.string(&["arn"]);
            let identifier = if case.by_arn { &flow_arn } else { &flow_id };
            let label = format!("{} round {round} by {identifier}", case.name);

            let cut = emulator
                .create_flow_version(identifier, json!({ "clientToken": token }))
                .await;
            assert_eq!(
                cut.status, 201,
                "case {label}: expected CreateFlowVersion -> 201, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["version"]),
                round.to_string(),
                "case {label}: each flow numbers its own versions incrementally from 1, \
                 got {cut:?}"
            );
            assert_eq!(
                cut.string(&["id"]),
                flow_id,
                "case {label}: the version names the flow it was cut from, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["arn"]),
                flow_arn,
                "case {label}: the version carries the ARN of its flow, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["name"]),
                case.name,
                "case {label}: the version snapshots its own flow's name, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["executionRoleArn"]),
                case.execution_role,
                "case {label}: the version snapshots its own flow's execution role, \
                 got {cut:?}"
            );
            assert_eq!(
                cut.member(&["description"]),
                &case.description.map_or(Value::Null, |text| json!(text)),
                "case {label}: a cut that gives no description leaves the flow's own, \
                 got {cut:?}"
            );
            assert_eq!(
                cut.member(&["definition"]),
                flow.member(&["definition"]),
                "case {label}: the version packages its own flow's definition, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["status"]),
                "Prepared",
                "case {label}: the version snapshots the prepared DRAFT's status, got {cut:?}"
            );
        }
    }

    // The round trip: each flow enumerates its DRAFT version and exactly the
    // versions cut from it, under its own numbering.
    for (case, flow) in &flows {
        let flow_id = flow.string(&["id"]);
        let flow_arn = flow.string(&["arn"]);
        let listed = emulator.list_flow_versions(&flow_id).await;
        assert_eq!(
            listed.status, 200,
            "case {}: expected ListFlowVersions -> 200, got {listed:?}",
            case.name
        );
        assert_eq!(
            listed.listed_versions(),
            expected_listing(case.cuts),
            "case {}: expected the DRAFT version then this flow's own versions, got {:?}",
            case.name,
            listed.listed_versions()
        );
        for summary in listed.summaries() {
            assert_eq!(
                summary["id"].as_str(),
                Some(flow_id.as_str()),
                "case {}: a summary names the flow it belongs to, got {summary:?}",
                case.name
            );
            assert_eq!(
                summary["arn"].as_str(),
                Some(flow_arn.as_str()),
                "case {}: a summary carries the ARN of its flow, got {summary:?}",
                case.name
            );
        }
        // Both `FlowIdentifier` forms name the same flow, so both report the
        // same listing.
        assert_eq!(
            emulator.list_flow_versions(&flow_arn).await.listed_versions(),
            listed.listed_versions(),
            "case {}: the ARN and the bare id name the same flow",
            case.name
        );
    }
}

/// One way the DRAFT can move after a version has been cut from it.
#[derive(Debug, Clone, Copy)]
struct DraftMove {
    case: &'static str,
    /// The name the DRAFT carries after the update.
    name: &'static str,
    /// The description the DRAFT carries after the update; `None` leaves the
    /// member out of the update, which drops it from the flow.
    description: Option<&'static str>,
    /// The `Output` node the DRAFT's definition ends at after the update.
    output_node: &'static str,
}

/// The flow every row below starts from.
const SOURCE_NAME: &str = "snapshot-source-flow";
const SOURCE_DESCRIPTION: &str = "the DRAFT as the first version was cut from it";
const SOURCE_OUTPUT_NODE: &str = "FirstOutput";

/// Rows differing in WHICH member of the DRAFT moves after the first version was
/// cut: its name, its description (changed, and dropped), its definition, and
/// all of them at once.
const DRAFT_MOVES: &[DraftMove] = &[
    DraftMove {
        case: "the name moves",
        name: "snapshot-source-flow-renamed",
        description: Some(SOURCE_DESCRIPTION),
        output_node: SOURCE_OUTPUT_NODE,
    },
    DraftMove {
        case: "the description moves",
        name: SOURCE_NAME,
        description: Some("the DRAFT after its description changed"),
        output_node: SOURCE_OUTPUT_NODE,
    },
    DraftMove {
        case: "the description is dropped",
        name: SOURCE_NAME,
        description: None,
        output_node: SOURCE_OUTPUT_NODE,
    },
    DraftMove {
        case: "the definition moves",
        name: SOURCE_NAME,
        description: Some(SOURCE_DESCRIPTION),
        output_node: "SecondOutput",
    },
    DraftMove {
        case: "every member moves at once",
        name: "snapshot-source-flow-rebuilt",
        description: Some("the DRAFT rebuilt from end to end"),
        output_node: "RebuiltOutput",
    },
];

/// A numbered version is frozen at the instant it was cut: a later `UpdateFlow`
/// moves the DRAFT alone, so the version already cut still reports the name,
/// description and definition it snapshotted — both when a retried
/// `clientToken` replays it and in the listing — while the NEXT cut carries the
/// DRAFT's new members.
#[tokio::test]
async fn an_update_moves_the_draft_alone_and_the_next_cut_carries_it() {
    for moved in DRAFT_MOVES {
        let case = moved.case;
        let emulator = Emulator::new();
        let flow = emulator
            .create_flow(
                case,
                flow_request(
                    SOURCE_NAME,
                    Some(SOURCE_DESCRIPTION),
                    SERVICE_LINKED_ROLE,
                    SOURCE_OUTPUT_NODE,
                ),
            )
            .await;
        let flow_id = flow.string(&["id"]);
        emulator.prepare_and_settle(case, &flow_id).await;

        // The modelled `ClientToken` is `length 33..256, pattern
        // ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`.
        let token = "the-idempotency-token-of-the-first-cut";
        let first = emulator
            .create_flow_version(&flow_id, json!({ "clientToken": token }))
            .await;
        assert_eq!(
            first.status, 201,
            "case {case}: expected CreateFlowVersion -> 201, got {first:?}"
        );
        assert_eq!(
            first.string(&["name"]),
            SOURCE_NAME,
            "case {case}: the first version snapshots the DRAFT it was cut from, got {first:?}"
        );

        // The DRAFT moves, and is prepared again so the next cut is taken from
        // a prepared DRAFT exactly as the first one was.
        let update = flow_request(
            moved.name,
            moved.description,
            SERVICE_LINKED_ROLE,
            moved.output_node,
        );
        let updated = emulator.update_flow(case, &flow_id, update).await;
        emulator.prepare_and_settle(case, &flow_id).await;

        // The retried token reports the version it already cut, read back out of
        // the store: member for member the answer the first cut gave, not the
        // DRAFT as it stands now.
        let replayed = emulator
            .create_flow_version(&flow_id, json!({ "clientToken": token }))
            .await;
        assert_eq!(
            replayed.body, first.body,
            "case {case}: a retried cut replays the stored snapshot, got {replayed:?}"
        );

        let second = emulator
            .create_flow_version(
                &flow_id,
                json!({ "clientToken": "the-idempotency-token-of-the-second-cut" }),
            )
            .await;
        assert_eq!(
            second.status, 201,
            "case {case}: expected CreateFlowVersion -> 201, got {second:?}"
        );
        assert_eq!(
            second.string(&["version"]),
            "2",
            "case {case}: the next cut takes the next number, got {second:?}"
        );
        assert_eq!(
            second.string(&["name"]),
            moved.name,
            "case {case}: the next cut snapshots the DRAFT as the update left it, \
             got {second:?}"
        );
        assert_eq!(
            second.member(&["description"]),
            &moved.description.map_or(Value::Null, |text| json!(text)),
            "case {case}: the next cut snapshots the DRAFT's description, got {second:?}"
        );
        assert_eq!(
            second.member(&["definition"]),
            updated.member(&["definition"]),
            "case {case}: the next cut packages the DRAFT's new definition, got {second:?}"
        );

        // Whatever moved, it moved for the DRAFT and the next cut only. The
        // members asserted here come from `replayed` — version 1 as the store
        // reports it AFTER the update — not from the answer captured before it.
        assert_eq!(
            replayed.string(&["name"]),
            SOURCE_NAME,
            "case {case}: version 1 still reports the name it was cut with, got {replayed:?}"
        );
        assert_eq!(
            replayed.member(&["description"]),
            &json!(SOURCE_DESCRIPTION),
            "case {case}: version 1 still reports the description it was cut with, \
             got {replayed:?}"
        );
        assert_eq!(
            replayed.member(&["definition"]),
            flow.member(&["definition"]),
            "case {case}: version 1 still packages the definition it was cut with, \
             got {replayed:?}"
        );
        // And this row really did move something, so none of the three above can
        // hold because the DRAFT never changed: the two versions differ in the
        // member the update changed.
        let snapshot_of = |cut: &Answer| {
            (
                cut.member(&["name"]).clone(),
                cut.member(&["description"]).clone(),
                cut.member(&["definition"]).clone(),
            )
        };
        assert_ne!(
            snapshot_of(&replayed),
            snapshot_of(&second),
            "case {case}: the update moved the DRAFT, so the two versions cannot \
             snapshot the same members; version 1 got {:?}",
            snapshot_of(&replayed)
        );

        // The listing reports both versions, each at its own creation instant,
        // and the first version's entry did not move with the DRAFT.
        let listed = emulator.list_flow_versions(&flow_id).await;
        assert_eq!(
            listed.listed_versions(),
            expected_listing(2),
            "case {case}: expected the DRAFT version then both cuts, got {:?}",
            listed.listed_versions()
        );
        let summaries = listed.summaries();
        for (version, cut) in [("1", &first), ("2", &second)] {
            let summary = summaries
                .iter()
                .find(|summary| summary["version"].as_str() == Some(version))
                .unwrap_or_else(|| {
                    panic!("case {case}: version {version} is not listed: {summaries:?}")
                });
            assert_eq!(
                summary["createdAt"].as_str(),
                Some(cut.string(&["createdAt"]).as_str()),
                "case {case}: version {version} is listed at the instant it was cut, \
                 got {summary:?}"
            );
            assert_eq!(
                summary["status"].as_str(),
                Some(cut.string(&["status"]).as_str()),
                "case {case}: version {version} is listed with the status it was cut with, \
                 got {summary:?}"
            );
        }
    }
}
