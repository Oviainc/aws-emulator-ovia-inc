// SPDX-License-Identifier: CC0-1.0

//! `UpdateFlow` over the DRAFT version, end to end over HTTP.
//!
//! This suite pins the two properties of an update that a caller can observe
//! beyond "the members came back": that the stored definition keeps every
//! numeric inference parameter it carried, at the width the model gives that
//! member, and that concurrent updates of one flow leave ONE whole DRAFT
//! belonging to exactly one of them — a racer that does not win may only lose
//! with the modelled `ConflictException`, never with a torn record.
//!
//! The numeric table spans the class rather than one definition: its rows
//! differ in value (both published bounds of the `Temperature`/`TopP` range,
//! `smithy_query(query="shape", shape="Temperature,TopP,MaximumLength")`), in
//! how the value is representable (exact binary fractions, decimals that are
//! not, six significant digits) and in how many stop sequences come with it.
//! The concurrency table differs in the NUMBER of racers and in which members
//! each racer supplies.
//!
//! Every assertion goes through the router, so it exercises the real wire path
//! (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.

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

/// A role ARN matching the modelled `FlowExecutionRoleArn` pattern.
const EXECUTION_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// A second flow execution role, so a racer can carry a different one.
const OTHER_EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/AmazonBedrockFlowRoleTwo";

/// A model identifier matching the modelled `FlowPromptModelIdentifier` pattern.
const PROMPT_MODEL: &str = "anthropic.claude-3-haiku-20240307-v1:0";

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

    /// Create a flow and answer its minted `flowId`.
    async fn flow_id(&self, name: &str) -> String {
        let created = self
            .create_flow(json!({ "name": name, "executionRoleArn": EXECUTION_ROLE }))
            .await;
        assert_eq!(
            created.status, 201,
            "case setup {name}: expected CreateFlow -> 201, got {created:?}"
        );
        created.string(&["id"])
    }
}

/// A passthrough flow definition: an `Input` node wired straight to an `Output`
/// node.
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
                "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
            },
        ],
    })
}

/// A three-node definition, so a racer's definition differs from
/// [`passthrough_definition`] in node and connection COUNT.
fn fan_out_definition() -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "Object" }],
            },
            {
                "name": "LeftOutput",
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "Object", "expression": "$.data.left" }],
            },
            {
                "name": "RightOutput",
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "Object", "expression": "$.data.right" }],
            },
        ],
        "connections": [
            {
                "name": "InputToLeft",
                "type": "Data",
                "source": "FlowInput",
                "target": "LeftOutput",
                "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
            },
            {
                "name": "InputToRight",
                "type": "Data",
                "source": "FlowInput",
                "target": "RightOutput",
                "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
            },
        ],
    })
}

/// A definition carrying a `Prompt` node whose inline inference configuration
/// holds the numeric parameters under test. The node types, the union keys and
/// the member names are the modelled `FlowNodeType` / `FlowNodeConfiguration` /
/// `PromptFlowNodeInlineConfiguration` / `PromptModelInferenceConfiguration`
/// ones.
fn prompt_definition(temperature: f64, top_p: f64, max_tokens: i64, stops: &[&str]) -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "String" }],
            },
            {
                "name": "Generate",
                "type": "Prompt",
                "configuration": {
                    "prompt": {
                        "sourceConfiguration": {
                            "inline": {
                                "templateType": "TEXT",
                                "templateConfiguration": {
                                    "text": {
                                        "text": "Summarise {{document}}",
                                        "inputVariables": [{ "name": "document" }],
                                    },
                                },
                                "modelId": PROMPT_MODEL,
                                "inferenceConfiguration": {
                                    "text": {
                                        "temperature": temperature,
                                        "topP": top_p,
                                        "maxTokens": max_tokens,
                                        "stopSequences": stops,
                                    },
                                },
                            },
                        },
                    },
                },
                "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
                "outputs": [{ "name": "modelCompletion", "type": "String" }],
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
                "name": "InputToPrompt",
                "type": "Data",
                "source": "FlowInput",
                "target": "Generate",
                "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
            },
            {
                "name": "PromptToOutput",
                "type": "Data",
                "source": "Generate",
                "target": "FlowOutput",
                "configuration": {
                    "data": { "sourceOutput": "modelCompletion", "targetInput": "document" },
                },
            },
        ],
    })
}

/// The path, inside a flow definition, of the inference configuration
/// [`prompt_definition`] builds.
const INFERENCE_PATH: [&str; 8] = [
    "definition",
    "nodes",
    "1",
    "configuration",
    "prompt",
    "sourceConfiguration",
    "inline",
    "inferenceConfiguration",
];

/// The inference configuration an answer reports, as a JSON value.
fn reported_inference<'a>(answer: &'a Answer, case: &str) -> &'a Value {
    let mut at = &answer.body;
    for key in INFERENCE_PATH {
        at = match key.parse::<usize>() {
            Ok(index) => &at[index],
            Err(_) => &at[key],
        };
    }
    let at = &at["text"];
    assert!(
        at.is_object(),
        "case {case}: expected the stored prompt node to report its text inference \
         configuration, got {at:?}"
    );
    at
}

/// `Temperature` and `TopP` are modelled as `float`
/// (`smithy_query(query="shape", shape="Temperature,TopP")`), so a value is
/// preserved exactly when it survives at 32-bit width — that width IS the
/// member's precision, and nothing coarser is acceptable.
fn assert_float_preserved(case: &str, member: &str, expected: f64, reported: &Value) {
    let actual = reported.as_f64().unwrap_or_else(|| {
        panic!("case {case}: expected {member} to be reported as a number, got {reported:?}")
    });
    assert_eq!(
        actual as f32, expected as f32,
        "case {case}: expected {member} to be preserved as {expected}, got {actual}"
    );
}

/// `UpdateFlow` stores the flow definition's numeric inference parameters as the
/// request supplied them, and the `GetFlow` that follows reports that same
/// stored definition — every row of the table, and across a second update that
/// replaces the row's numbers with the next row's.
#[tokio::test]
async fn update_flow_preserves_the_numeric_parameters_of_the_definition() {
    let emulator = Emulator::new();
    let flow_id = emulator.flow_id("numeric-precision-flow").await;

    // Rows differ in value — both published bounds of the modelled
    // `Temperature`/`TopP` range (min 0, max 1) and the `MaximumLength` minimum
    // (0) — in whether the decimal has an exact binary form, in how many
    // significant digits it needs, and in the number of stop sequences
    // (`StopSequences` admits 0..4).
    let rows: [(&str, f64, f64, i64, &[&str]); 6] = [
        ("the minimum of every modelled range", 0.0, 0.0, 0, &[]),
        (
            "the maximum of the float ranges",
            1.0,
            1.0,
            4096,
            &["\n\nHuman:"],
        ),
        ("a typical decimal pair", 0.7, 0.9, 512, &["END"]),
        ("exact binary fractions", 0.25, 0.125, 1, &[]),
        (
            "six significant decimal digits",
            0.123456,
            0.987654,
            200_000,
            &["A", "B", "C", "D"],
        ),
        ("decimals with no exact binary form", 0.3, 0.1, 7, &[]),
    ];

    for (case, temperature, top_p, max_tokens, stops) in rows {
        let definition = prompt_definition(temperature, top_p, max_tokens, stops);
        let updated = emulator
            .update_flow(
                &flow_id,
                json!({
                    "name": "numeric-precision-flow",
                    "executionRoleArn": EXECUTION_ROLE,
                    "definition": definition.clone(),
                }),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateFlow -> 200, got {updated:?}"
        );

        let fetched = emulator.get_flow(&flow_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetFlow -> 200, got {fetched:?}"
        );

        for (operation, answer) in [("UpdateFlow", &updated), ("GetFlow", &fetched)] {
            let reported = reported_inference(answer, case);
            assert_float_preserved(
                case,
                &format!("{operation} temperature"),
                temperature,
                &reported["temperature"],
            );
            assert_float_preserved(
                case,
                &format!("{operation} topP"),
                top_p,
                &reported["topP"],
            );
            let reported_tokens = reported["maxTokens"].as_i64();
            assert_eq!(
                reported_tokens,
                Some(max_tokens),
                "case {case}: expected {operation} to report maxTokens {max_tokens}, got {:?}",
                reported["maxTokens"]
            );
            let reported_stops = reported["stopSequences"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .map(|value| value.as_str().unwrap_or_default().to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            assert_eq!(
                reported_stops,
                stops.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "case {case}: expected {operation} to report the supplied stopSequences, got {:?}",
                reported["stopSequences"]
            );
        }

        // The whole definition, not only its numbers, is the one just written:
        // the read reports exactly what the update answered.
        assert_eq!(
            fetched.member(&["definition"]),
            updated.member(&["definition"]),
            "case {case}: GetFlow must report the definition UpdateFlow stored, got {:?}",
            fetched.member(&["definition"])
        );
        assert_eq!(
            fetched.string(&["status"]),
            "NotPrepared",
            "case {case}: an update leaves the flow NotPrepared"
        );
    }
}

/// Concurrent `UpdateFlow` calls on one flow leave ONE whole DRAFT: every call
/// either wins with 200 or loses with the modelled `ConflictException` (409),
/// at least one wins, and the stored record afterwards carries every member of
/// exactly one racer's request rather than a mix of several.
///
/// The emulator serialises the update under the store's own lock, so in practice
/// every racer wins and the losing arm is not taken. What the arm asserts is
/// that a racer is never refused with anything OTHER than the conflict the model
/// declares — a racer answered a validation error, a not-found or a 500 fails the
/// case — and the force of the test is in what follows it: the stored record is
/// one racer's request WHOLE, and the flow's identity and `createdAt` are where
/// the create left them. A torn record, a re-minted id and an unmodelled refusal
/// each fail here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_update_flow_calls_leave_one_whole_draft() {
    let emulator = Arc::new(Emulator::new());

    /// One racer's request: a name that identifies it, and the members that
    /// must come back together with that name.
    struct Racer {
        name: &'static str,
        description: Option<&'static str>,
        execution_role: &'static str,
        definition: Option<Value>,
    }

    impl Racer {
        fn request(&self) -> Value {
            let mut request = json!({
                "name": self.name,
                "executionRoleArn": self.execution_role,
            });
            if let Some(description) = self.description {
                request["description"] = json!(description);
            }
            if let Some(definition) = &self.definition {
                request["definition"] = definition.clone();
            }
            request
        }
    }

    let racers = || -> Vec<Racer> {
        vec![
            Racer {
                name: "racingUpdateAlpha",
                description: Some("the racer carrying a passthrough definition"),
                execution_role: EXECUTION_ROLE,
                definition: Some(passthrough_definition()),
            },
            Racer {
                name: "racingUpdateBeta",
                description: Some("the racer carrying a three-node definition"),
                execution_role: OTHER_EXECUTION_ROLE,
                definition: Some(fan_out_definition()),
            },
            Racer {
                name: "racingUpdateGamma",
                description: None,
                execution_role: EXECUTION_ROLE,
                definition: None,
            },
            Racer {
                name: "racingUpdateDelta",
                description: Some("the racer carrying a prompt definition"),
                execution_role: OTHER_EXECUTION_ROLE,
                // Exact binary fractions: this racer's definition is compared
                // to the stored one value for value, and a `float` member is
                // only equal on the wire when the decimal has an exact binary
                // form. The numeric table above is where precision itself is
                // asserted, at the member's own 32-bit width.
                definition: Some(prompt_definition(0.5, 0.75, 1024, &["STOP"])),
            },
        ]
    };

    // Rows differ in how many callers race for the one DRAFT.
    for racer_count in [2usize, 3, 4] {
        let case = format!("{racer_count} racing updates");
        let flow_id = emulator.flow_id("racing-update-flow").await;
        let created = emulator.get_flow(&flow_id).await;
        let created_arn = created.string(&["arn"]);
        let created_at = created.string(&["createdAt"]);

        let all = racers();
        let mut running = Vec::new();
        for racer in all.iter().take(racer_count) {
            let emulator = Arc::clone(&emulator);
            let flow_id = flow_id.clone();
            let request = racer.request();
            running.push(tokio::spawn(async move {
                emulator.update_flow(&flow_id, request).await
            }));
        }

        let mut winners = 0usize;
        for (index, handle) in running.into_iter().enumerate() {
            let answer = handle.await.expect("the racing update task ran to completion");
            match answer.status {
                200 => winners += 1,
                409 => assert_eq!(
                    answer.error_type.as_deref(),
                    Some("ConflictException"),
                    "case {case}, racer {index}: a racing update may only lose with the \
                     modelled ConflictException, got {answer:?}"
                ),
                other => panic!(
                    "case {case}, racer {index}: expected 200 or a 409 ConflictException, \
                     got {other} in {answer:?}"
                ),
            }
        }
        assert!(
            winners >= 1,
            "case {case}: at least one racing update must win, {winners} did"
        );

        // The stored DRAFT belongs to exactly ONE racer: the name picks it, and
        // every other member must be that same racer's.
        let fetched = emulator.get_flow(&flow_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetFlow -> 200, got {fetched:?}"
        );
        let stored_name = fetched.string(&["name"]);
        let winner = all
            .iter()
            .take(racer_count)
            .find(|racer| racer.name == stored_name)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the stored flow must carry one racer's name, got \
                     {stored_name:?}"
                )
            });

        assert_eq!(
            fetched.member(&["description"]),
            &winner
                .description
                .map(|text| json!(text))
                .unwrap_or(Value::Null),
            "case {case}: the stored description must be {}'s, got {:?}",
            winner.name,
            fetched.member(&["description"])
        );
        assert_eq!(
            fetched.string(&["executionRoleArn"]),
            winner.execution_role,
            "case {case}: the stored executionRoleArn must be {}'s",
            winner.name
        );
        assert_eq!(
            fetched.member(&["definition"]),
            &winner.definition.clone().unwrap_or(Value::Null),
            "case {case}: the stored definition must be {}'s, got {:?}",
            winner.name,
            fetched.member(&["definition"])
        );

        // The identity of the flow, and what the clock stamped at creation, are
        // not a racer's to move.
        assert_eq!(
            fetched.string(&["id"]),
            flow_id,
            "case {case}: a racing update never re-mints the flow id"
        );
        assert_eq!(
            fetched.string(&["arn"]),
            created_arn,
            "case {case}: a racing update never re-mints the flow ARN"
        );
        assert_eq!(
            fetched.string(&["createdAt"]),
            created_at,
            "case {case}: a racing update never moves createdAt"
        );
        assert_eq!(
            fetched.string(&["status"]),
            "NotPrepared",
            "case {case}: the DRAFT a racing update left behind is NotPrepared"
        );
        assert_eq!(
            fetched.string(&["version"]),
            "DRAFT",
            "case {case}: an update writes the DRAFT version"
        );

        // A read changes nothing a later caller sees.
        let again = emulator.get_flow(&flow_id).await;
        assert_eq!(
            again.body, fetched.body,
            "case {case}: two reads of one settled flow report one record"
        );
    }
}
