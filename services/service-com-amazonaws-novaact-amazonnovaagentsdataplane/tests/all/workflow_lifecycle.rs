// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the workflow lifecycle this service's first stateful
//! slice implements: workflow definition -> workflow run -> session -> act, plus the
//! delete that takes the whole chain away again.
//!
//! Every case drives the real `restJson1` surface through the production
//! `MultiServiceRouter`, so a response here is exactly what an SDK client would see.
//! The tables below vary what each rule generalises over — name shape, which optional
//! members are present, list length, signing region, which parent is missing — because
//! the rule is "the response is computed from the stored record", not "this one request
//! answers these bytes".

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::state::{
    service_state, ComAmazonawsNovaactAmazonnovaagentsdataplaneState, DELETION_COMPLETION_DELAY,
    MAX_ACTS_PER_SESSION, MAX_IDEMPOTENCY_ENTRIES, MAX_RUNS_PER_DEFINITION,
    MAX_SESSIONS_PER_RUN, MAX_WORKFLOW_DEFINITIONS, MAX_WORKFLOW_RUNS,
};
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One emulator, reached the way the router reaches it: one shared engine, one service.
///
/// `pub(crate)` so the other suites in this test target drive the same harness rather
/// than each building its own router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    region: String,
    /// The engine the service was built on, so a case can reach this service's store for a
    /// state no implemented operation reaches yet.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these cases are about; the credential scope below is
        // still read for the routing key and the ARN region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            router,
            region: region.to_string(),
            engine,
        }
    }

    /// The store this service's handlers read and write.
    pub(crate) fn store(
        &self,
    ) -> engine::ServiceState<ComAmazonawsNovaactAmazonnovaagentsdataplaneState> {
        service_state(&*self.engine).expect("the nova-act store is reachable from the engine")
    }

    /// Dispatch one request and return its status and parsed body (`Value::Null` when the
    /// response has no body).
    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = match &body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serializable body")),
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20240101/{}/nova-act/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef",
                    self.region
                ),
            )
            .header("content-type", "application/json")
            .body(payload)
            .expect("valid request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `CreateWorkflowDefinition` for `name`, with no optional members.
    pub(crate) async fn create_definition(&self, name: &str) -> (u16, Value) {
        self.call("PUT", "/workflow-definitions", Some(json!({ "name": name })))
            .await
    }

    /// `CreateWorkflowRun` under `name`.
    pub(crate) async fn create_run(&self, name: &str) -> (u16, Value) {
        self.call(
            "PUT",
            &format!("/workflow-definitions/{name}/workflow-runs"),
            Some(json!({
                "modelId": "nova-act-v1",
                "clientInfo": { "compatibilityVersion": 1 },
            })),
        )
        .await
    }

    /// `CreateSession` under that run.
    pub(crate) async fn create_session(&self, name: &str, run_id: &str) -> (u16, Value) {
        self.call(
            "PUT",
            &format!("/workflow-definitions/{name}/workflow-runs/{run_id}/sessions"),
            Some(json!({})),
        )
        .await
    }

    /// `CreateAct` under that session.
    pub(crate) async fn create_act(
        &self,
        name: &str,
        run_id: &str,
        session_id: &str,
        task: &str,
    ) -> (u16, Value) {
        self.call(
            "PUT",
            &format!(
                "/workflow-definitions/{name}/workflow-runs/{run_id}/sessions/{session_id}/acts"
            ),
            Some(json!({ "task": task })),
        )
        .await
    }
}

/// `UuidString` in the model is `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`;
/// this is that pattern, checked without pulling in a regex engine.
fn is_lowercase_uuid(value: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let parts: Vec<&str> = value.split('-').collect();
    parts.len() == GROUPS.len()
        && parts.iter().zip(GROUPS).all(|(part, len)| {
            part.len() == len && part.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
}

pub(crate) fn text(body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("member {member} must be a string in {body}"))
        .to_string()
}

/// A modeled timestamp member, as epoch seconds.
///
/// Accepts either wire form a Smithy timestamp can take on restJson1: the protocol's
/// default epoch-seconds number, and an RFC 3339 string. Which one is emitted is the
/// schema's business, not this service's — what the cases below assert is the VALUE.
fn timestamp_seconds(body: &Value, member: &str) -> f64 {
    match &body[member] {
        Value::Number(seconds) => seconds.as_f64().expect("a finite epoch-second value"),
        Value::String(rendered) => {
            ::aws_smithy_types::DateTime::from_str(
                rendered,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .unwrap_or_else(|e| panic!("member {member} must be an RFC 3339 date-time: {e}"))
            .as_secs_f64()
        }
        other => panic!("member {member} must be a timestamp, got {other}"),
    }
}

/// Seconds since the Unix epoch, for comparing against a reported timestamp.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs_f64()
}

/// A `clientToken` the model accepts: `len 33..256`, `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`.
///
/// `pub(crate)` so the sibling suite that drives the same `@idempotent` create
/// (`tests/all/create_preconditions.rs`) mints its tokens the same way rather than
/// carrying a copy of this rule.
pub(crate) fn client_token(seed: &str) -> String {
    let mut token = format!("tok{seed}");
    while token.len() < 33 {
        token.push('0');
    }
    token
}

/// One definition shape to exercise the lifecycle with. The rows differ along what the
/// stored-record rule generalises over: the name's form, which optional members the
/// create carries, how many tool specs an act declares, and the signing region the ARNs
/// are built in.
struct Case {
    label: &'static str,
    region: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    export_config: Option<Value>,
    model_id: &'static str,
    log_group_name: Option<&'static str>,
    tool_specs: Vec<Value>,
}

fn tool_spec(name: &str) -> Value {
    json!({
        "name": name,
        "description": format!("what {name} does"),
        "inputSchema": { "json": { "type": "object" } },
    })
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "minimal: only the required members",
            region: "us-east-1",
            name: "demo",
            description: None,
            export_config: None,
            model_id: "nova-act-v1",
            log_group_name: None,
            tool_specs: Vec::new(),
        },
        Case {
            label: "every optional member, one tool spec",
            region: "us-east-1",
            name: "demo-workflow",
            description: Some("bootstrap demo"),
            export_config: Some(json!({ "s3BucketName": "nova-act-exports" })),
            model_id: "nova-act-v1",
            log_group_name: Some("/aws/nova-act/demo"),
            tool_specs: vec![tool_spec("search")],
        },
        Case {
            label: "max-length name, underscores, two tool specs, another region",
            region: "eu-west-2",
            name: "A_0123456789_0123456789_0123456789_0123",
            description: Some("x"),
            export_config: Some(json!({
                "s3BucketName": "nova-act-exports",
                "s3KeyPrefix": "runs/2026",
            })),
            model_id: "m",
            log_group_name: Some("logs.group-1/segment"),
            tool_specs: vec![tool_spec("search"), tool_spec("click")],
        },
    ]
}

/// The whole chain, for every row: each response is read back out of state through the
/// matching Get, so nothing here can pass on a canned body.
#[tokio::test]
async fn the_workflow_chain_round_trips_through_stored_state() {
    for case in cases() {
        let emulator = Emulator::new(case.region);
        let label = case.label;

        let mut create = json!({ "name": case.name });
        if let Some(description) = case.description {
            create["description"] = json!(description);
        }
        if let Some(export_config) = &case.export_config {
            create["exportConfig"] = export_config.clone();
        }
        let (status, body) = emulator
            .call("PUT", "/workflow-definitions", Some(create))
            .await;
        assert_eq!(
            status, 201,
            "case {label:?}: CreateWorkflowDefinition must answer 201, got {status} {body}"
        );
        assert_eq!(
            body["status"], "ACTIVE",
            "case {label:?}: a created definition is ACTIVE, got {body}"
        );

        // READ BACK: every member must come from the stored record.
        let (status, definition) = emulator
            .call(
                "GET",
                &format!("/workflow-definitions/{}", case.name),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetWorkflowDefinition must answer 200, got {status} {definition}"
        );
        assert_eq!(
            definition["name"], case.name,
            "case {label:?}: the stored name is reported, got {definition}"
        );
        let expected_arn = engine::emulator_arn(
            "nova-act",
            case.region,
            &format!("workflow-definition/{}", case.name),
        );
        assert_eq!(
            text(&definition, "arn"),
            expected_arn,
            "case {label:?}: expected arn {expected_arn}, got {definition}"
        );
        assert_eq!(
            definition["status"], "ACTIVE",
            "case {label:?}: the stored status is reported, got {definition}"
        );
        let created_at = timestamp_seconds(&definition, "createdAt");
        assert!(
            (created_at - now_seconds()).abs() < 60.0,
            "case {label:?}: createdAt must be the creation instant, got {created_at} in {definition}"
        );
        assert_eq!(
            definition["description"],
            case.description.map_or(Value::Null, |d| json!(d)),
            "case {label:?}: the stored description is reported, got {definition}"
        );
        assert_eq!(
            definition["exportConfig"],
            case.export_config.clone().unwrap_or(Value::Null),
            "case {label:?}: the stored exportConfig is reported, got {definition}"
        );

        // A read does not change what the next reader sees: the same record comes back.
        let (status, again) = emulator
            .call("GET", &format!("/workflow-definitions/{}", case.name), None)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: a second GetWorkflowDefinition must answer 200, got {status} {again}"
        );
        assert_eq!(
            again, definition,
            "case {label:?}: reading the definition twice must report the same record, got {again}"
        );

        // RUN
        let mut create_run = json!({
            "modelId": case.model_id,
            "clientInfo": { "compatibilityVersion": 1, "sdkVersion": "1.2.3" },
        });
        if let Some(log_group_name) = case.log_group_name {
            create_run["logGroupName"] = json!(log_group_name);
        }
        let (status, body) = emulator
            .call(
                "PUT",
                &format!("/workflow-definitions/{}/workflow-runs", case.name),
                Some(create_run),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {label:?}: CreateWorkflowRun must answer 201, got {status} {body}"
        );
        let run_id = text(&body, "workflowRunId");
        assert!(
            is_lowercase_uuid(&run_id),
            "case {label:?}: workflowRunId must be a lowercase UUID, got {run_id:?}"
        );
        assert_eq!(
            body["status"], "RUNNING",
            "case {label:?}: a created run is RUNNING, got {body}"
        );

        let (status, run) = emulator
            .call(
                "GET",
                &format!(
                    "/workflow-definitions/{}/workflow-runs/{run_id}",
                    case.name
                ),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetWorkflowRun must answer 200, got {status} {run}"
        );
        assert_eq!(
            run["workflowRunId"], run_id,
            "case {label:?}: the stored run id is reported, got {run}"
        );
        let expected_run_arn = engine::emulator_arn(
            "nova-act",
            case.region,
            &format!(
                "workflow-definition/{}/workflow-run/{run_id}",
                case.name
            ),
        );
        assert_eq!(
            text(&run, "workflowRunArn"),
            expected_run_arn,
            "case {label:?}: expected workflowRunArn {expected_run_arn}, got {run}"
        );
        assert_eq!(
            run["modelId"], case.model_id,
            "case {label:?}: the stored modelId is reported, got {run}"
        );
        assert_eq!(
            run["status"], "RUNNING",
            "case {label:?}: the stored run status is reported, got {run}"
        );
        let started_at = timestamp_seconds(&run, "startedAt");
        assert!(
            (started_at - now_seconds()).abs() < 60.0,
            "case {label:?}: startedAt must be the creation instant, got {started_at} in {run}"
        );
        assert_eq!(
            run["endedAt"],
            Value::Null,
            "case {label:?}: a running run has no endedAt, got {run}"
        );
        assert_eq!(
            run["logGroupName"],
            case.log_group_name.map_or(Value::Null, |l| json!(l)),
            "case {label:?}: the stored logGroupName is reported, got {run}"
        );

        // SESSION, and the same clientToken twice is one session.
        let token = client_token("session");
        let sessions_path = format!(
            "/workflow-definitions/{}/workflow-runs/{run_id}/sessions",
            case.name
        );
        let (status, body) = emulator
            .call(
                "PUT",
                &sessions_path,
                Some(json!({ "clientToken": token })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {label:?}: CreateSession must answer 201, got {status} {body}"
        );
        let session_id = text(&body, "sessionId");
        assert!(
            is_lowercase_uuid(&session_id),
            "case {label:?}: sessionId must be a lowercase UUID, got {session_id:?}"
        );
        let (status, replay) = emulator
            .call(
                "PUT",
                &sessions_path,
                Some(json!({ "clientToken": token })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {label:?}: a replayed CreateSession still answers 201, got {status} {replay}"
        );
        assert_eq!(
            text(&replay, "sessionId"),
            session_id,
            "case {label:?}: the same clientToken replays session {session_id}, got {replay}"
        );

        // ACT
        let mut create_act = json!({ "task": "search for a product" });
        if !case.tool_specs.is_empty() {
            create_act["toolSpecs"] = Value::Array(case.tool_specs.clone());
        }
        let (status, body) = emulator
            .call(
                "PUT",
                &format!(
                    "/workflow-definitions/{}/workflow-runs/{run_id}/sessions/{session_id}/acts",
                    case.name
                ),
                Some(create_act),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {label:?}: CreateAct must answer 201, got {status} {body}"
        );
        let act_id = text(&body, "actId");
        assert!(
            is_lowercase_uuid(&act_id),
            "case {label:?}: actId must be a lowercase UUID, got {act_id:?}"
        );
        assert_eq!(
            body["status"], "RUNNING",
            "case {label:?}: a created act's initial status is RUNNING, got {body}"
        );
    }
}

/// Each generated id names its own record: two runs under one definition get two ids,
/// and each one reads back with the parameters it was created with.
#[tokio::test]
async fn each_created_run_is_stored_under_its_own_generated_id() {
    let emulator = Emulator::new("us-east-1");
    let (status, _) = emulator
        .call(
            "PUT",
            "/workflow-definitions",
            Some(json!({ "name": "two-runs" })),
        )
        .await;
    assert_eq!(status, 201, "the definition must be created, got {status}");

    let mut seen: Vec<String> = Vec::new();
    for model_id in ["nova-act-v1", "nova-act-v2"] {
        let (status, body) = emulator
            .call(
                "PUT",
                "/workflow-definitions/two-runs/workflow-runs",
                Some(json!({
                    "modelId": model_id,
                    "clientInfo": { "compatibilityVersion": 2 },
                })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {model_id:?}: CreateWorkflowRun must answer 201, got {status} {body}"
        );
        seen.push(text(&body, "workflowRunId"));
    }
    assert_ne!(
        seen[0], seen[1],
        "two runs must get two ids, got {seen:?}"
    );

    for (model_id, run_id) in ["nova-act-v1", "nova-act-v2"].iter().zip(&seen) {
        let (status, run) = emulator
            .call(
                "GET",
                &format!("/workflow-definitions/two-runs/workflow-runs/{run_id}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {model_id:?}: GetWorkflowRun must answer 200, got {status} {run}"
        );
        assert_eq!(
            run["modelId"], *model_id,
            "case {model_id:?}: each run reports its own modelId, got {run}"
        );
    }
}

/// A child whose parent this account does not hold is a 404 at every level of the chain,
/// and so is a read or a delete of a name that was never created.
#[tokio::test]
async fn an_unresolved_parent_is_a_resource_not_found() {
    let emulator = Emulator::new("us-east-1");
    let (status, _) = emulator
        .call(
            "PUT",
            "/workflow-definitions",
            Some(json!({ "name": "present" })),
        )
        .await;
    assert_eq!(status, 201, "the definition must be created, got {status}");
    let (status, body) = emulator
        .call(
            "PUT",
            "/workflow-definitions/present/workflow-runs",
            Some(json!({
                "modelId": "nova-act-v1",
                "clientInfo": { "compatibilityVersion": 1 },
            })),
        )
        .await;
    assert_eq!(status, 201, "the run must be created, got {status} {body}");
    let run_id = text(&body, "workflowRunId");
    let absent_uuid = "00000000-0000-4000-8000-000000000000";

    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "read an absent definition",
            "GET",
            "/workflow-definitions/absent".to_string(),
            None,
        ),
        (
            "delete an absent definition",
            "DELETE",
            "/workflow-definitions/absent".to_string(),
            None,
        ),
        (
            "run under an absent definition",
            "PUT",
            "/workflow-definitions/absent/workflow-runs".to_string(),
            Some(json!({
                "modelId": "nova-act-v1",
                "clientInfo": { "compatibilityVersion": 1 },
            })),
        ),
        (
            "read an absent run",
            "GET",
            format!("/workflow-definitions/present/workflow-runs/{absent_uuid}"),
            None,
        ),
        (
            "session under an absent definition",
            "PUT",
            format!("/workflow-definitions/absent/workflow-runs/{run_id}/sessions"),
            Some(json!({})),
        ),
        (
            "session under an absent run",
            "PUT",
            format!("/workflow-definitions/present/workflow-runs/{absent_uuid}/sessions"),
            Some(json!({})),
        ),
        (
            "act under an absent session",
            "PUT",
            format!(
                "/workflow-definitions/present/workflow-runs/{run_id}/sessions/{absent_uuid}/acts"
            ),
            Some(json!({ "task": "search for a product" })),
        ),
    ];

    for (label, method, path, body) in cases {
        let (status, answer) = emulator.call(method, &path, body).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"], "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {answer}"
        );
        assert!(
            answer["resourceType"].is_string() && answer["resourceId"].is_string(),
            "case {label:?}: the error names the resource it could not find, got {answer}"
        );
    }
}

/// `@idempotent`: repeating a create with the `clientToken` that made the definition has
/// the effect of calling it once, while every OTHER repeat of a name the account already
/// holds — with no token, or with that token and a different body — conflicts with the
/// current state of that resource.
#[tokio::test]
async fn repeating_a_create_replays_it_and_changing_the_body_conflicts() {
    let emulator = Emulator::new("us-east-1");
    let token = client_token("definition");
    let first = json!({
        "name": "idempotent",
        "description": "the first body",
        "clientToken": token,
    });

    let (status, body) = emulator
        .call("PUT", "/workflow-definitions", Some(first.clone()))
        .await;
    assert_eq!(status, 201, "the first create must answer 201, got {status} {body}");

    let cases: Vec<(&str, Value, u16)> = vec![
        ("the identical request with its token", first.clone(), 201),
        // The name is documented unique within the account and region, so an untokened
        // repeat is not a replay of anything: it asks to create a definition that is
        // already there.
        (
            "the identical request without a token",
            json!({ "name": "idempotent", "description": "the first body" }),
            409,
        ),
        (
            "the same token with a different description",
            json!({
                "name": "idempotent",
                "description": "a different body",
                "clientToken": token,
            }),
            409,
        ),
        (
            "the same name with a different description",
            json!({ "name": "idempotent", "description": "a different body" }),
            409,
        ),
        (
            "the same name with the description removed",
            json!({ "name": "idempotent" }),
            409,
        ),
    ];

    for (label, request, expected) in cases {
        let (status, answer) = emulator
            .call("PUT", "/workflow-definitions", Some(request))
            .await;
        assert_eq!(
            status, expected,
            "case {label:?}: expected HTTP {expected}, got {status} {answer}"
        );
        if expected == 409 {
            assert_eq!(
                answer["__type"], "ConflictException",
                "case {label:?}: expected ConflictException, got {answer}"
            );
        } else {
            assert_eq!(
                answer["status"], "ACTIVE",
                "case {label:?}: the replay reports the stored status, got {answer}"
            );
        }
    }

    // And exactly one definition was created: the description is still the first one.
    let (status, definition) = emulator
        .call("GET", "/workflow-definitions/idempotent", None)
        .await;
    assert_eq!(status, 200, "the definition must still resolve, got {status}");
    assert_eq!(
        definition["description"], "the first body",
        "a replayed create must not overwrite the stored record, got {definition}"
    );
}

/// A definition on its way out answers a repeat of its create the same way whether or not
/// the `clientToken` that created it is sent: the request conflicts with the resource's
/// current state either way.
#[tokio::test]
async fn a_deleting_definition_conflicts_with_a_repeat_either_way() {
    let emulator = Emulator::new("us-east-1");
    let token = client_token("deleting");
    let tokened = json!({
        "name": "fading",
        "description": "the first body",
        "clientToken": token,
    });

    let (status, body) = emulator
        .call("PUT", "/workflow-definitions", Some(tokened.clone()))
        .await;
    assert_eq!(status, 201, "the create must answer 201, got {status} {body}");
    let (status, body) = emulator
        .call("DELETE", "/workflow-definitions/fading", None)
        .await;
    assert_eq!(status, 202, "the delete must answer 202, got {status} {body}");

    for (label, request) in [
        ("with the clientToken that created it", tokened.clone()),
        (
            "without a token",
            json!({ "name": "fading", "description": "the first body" }),
        ),
    ] {
        let (status, answer) = emulator
            .call("PUT", "/workflow-definitions", Some(request))
            .await;
        assert_eq!(
            status, 409,
            "case {label:?}: a repeat while the definition is DELETING must be refused 409, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"], "ConflictException",
            "case {label:?}: expected ConflictException, got {answer}"
        );
    }
}

/// Delete is a transition the clock owes: the definition is `DELETING` straight after the
/// call, its runs with it, and once the span has elapsed it stops resolving at all.
#[tokio::test]
async fn delete_reports_deleting_and_then_the_chain_stops_resolving() {
    let emulator = Emulator::new("us-east-1");
    let (status, _) = emulator
        .call(
            "PUT",
            "/workflow-definitions",
            Some(json!({ "name": "going-away" })),
        )
        .await;
    assert_eq!(status, 201, "the definition must be created, got {status}");
    let (status, body) = emulator
        .call(
            "PUT",
            "/workflow-definitions/going-away/workflow-runs",
            Some(json!({
                "modelId": "nova-act-v1",
                "clientInfo": { "compatibilityVersion": 1 },
            })),
        )
        .await;
    assert_eq!(status, 201, "the run must be created, got {status} {body}");
    let run_id = text(&body, "workflowRunId");

    let (status, body) = emulator
        .call("DELETE", "/workflow-definitions/going-away", None)
        .await;
    assert_eq!(
        status, 202,
        "DeleteWorkflowDefinition must answer 202, got {status} {body}"
    );
    assert_eq!(
        body["status"], "DELETING",
        "the delete reports DELETING, got {body}"
    );

    // Within the span the transient state is observable, on the definition and on its run.
    let (status, definition) = emulator
        .call("GET", "/workflow-definitions/going-away", None)
        .await;
    assert_eq!(
        status, 200,
        "a definition inside its deleting span still resolves, got {status} {definition}"
    );
    assert_eq!(
        definition["status"], "DELETING",
        "a definition inside its deleting span reports DELETING, got {definition}"
    );
    let (status, run) = emulator
        .call(
            "GET",
            &format!("/workflow-definitions/going-away/workflow-runs/{run_id}"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "a run of a deleting definition still resolves, got {status} {run}"
    );
    assert_eq!(
        run["status"], "DELETING",
        "a run of a deleting definition reports DELETING, got {run}"
    );

    // A repeat of the delete is the same call, not a second one.
    let (status, body) = emulator
        .call("DELETE", "/workflow-definitions/going-away", None)
        .await;
    assert_eq!(
        status, 202,
        "repeating the delete answers 202, got {status} {body}"
    );
    assert_eq!(
        body["status"], "DELETING",
        "repeating the delete reports DELETING, got {body}"
    );

    std::thread::sleep(DELETION_COMPLETION_DELAY + Duration::from_millis(250));

    for (label, path) in [
        ("the definition", "/workflow-definitions/going-away".to_string()),
        (
            "its run",
            format!("/workflow-definitions/going-away/workflow-runs/{run_id}"),
        ),
    ] {
        let (status, answer) = emulator.call("GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: after the deleting span it must be gone, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"], "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {answer}"
        );
    }

    // The name is free again, and the new definition is a new record.
    let (status, body) = emulator
        .call(
            "PUT",
            "/workflow-definitions",
            Some(json!({ "name": "going-away", "description": "reused name" })),
        )
        .await;
    assert_eq!(
        status, 201,
        "the name is reusable once the delete completed, got {status} {body}"
    );
    let (status, definition) = emulator
        .call("GET", "/workflow-definitions/going-away", None)
        .await;
    assert_eq!(status, 200, "the new definition must resolve, got {status}");
    assert_eq!(
        definition["status"], "ACTIVE",
        "the new definition is ACTIVE, got {definition}"
    );
    assert_eq!(
        definition["description"], "reused name",
        "the new definition is its own record, got {definition}"
    );
}

/// The constraints the model declares are refused with HTTP 400, whichever binding the
/// value arrives in: a JSON body member, or an `@httpLabel` in the path.
#[tokio::test]
async fn values_outside_the_modeled_constraints_are_refused() {
    let emulator = Emulator::new("us-east-1");
    let (status, _) = emulator
        .call(
            "PUT",
            "/workflow-definitions",
            Some(json!({ "name": "constraints" })),
        )
        .await;
    assert_eq!(status, 201, "the definition must be created, got {status}");

    // Each row names the member and the constraint it breaks (from the model: name
    // len 1..40 / ^[a-zA-Z0-9_-]{1,40}$, task len 1..10000, clientToken len 33..256,
    // workflowRunId ^[0-9a-f]{8}-...$).
    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "name absent, though required",
            "PUT",
            "/workflow-definitions".to_string(),
            Some(json!({ "description": "no name" })),
        ),
        (
            "name one character past its maximum of 40",
            "PUT",
            "/workflow-definitions".to_string(),
            Some(json!({ "name": "a".repeat(41) })),
        ),
        (
            "name shorter than its minimum of 1",
            "PUT",
            "/workflow-definitions".to_string(),
            Some(json!({ "name": "" })),
        ),
        (
            "name outside its pattern",
            "PUT",
            "/workflow-definitions".to_string(),
            Some(json!({ "name": "not a name!" })),
        ),
        (
            "clientToken shorter than its minimum of 33",
            "PUT",
            "/workflow-definitions".to_string(),
            Some(json!({ "name": "short-token", "clientToken": "abc" })),
        ),
        (
            "workflowRunId label outside the UUID pattern",
            "GET",
            "/workflow-definitions/constraints/workflow-runs/not-a-uuid".to_string(),
            None,
        ),
        (
            "task shorter than its minimum of 1",
            "PUT",
            format!(
                "/workflow-definitions/constraints/workflow-runs/{uuid}/sessions/{uuid}/acts",
                uuid = "00000000-0000-4000-8000-000000000000"
            ),
            Some(json!({ "task": "" })),
        ),
    ];

    for (label, method, path, body) in cases {
        let (status, answer) = emulator.call(method, &path, body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"], "ValidationException",
            "case {label:?}: expected ValidationException, got {answer}"
        );
    }
}

/// The definition store is bounded: at [`MAX_WORKFLOW_DEFINITIONS`] a further create is
/// refused with the modelled `ServiceQuotaExceededException`, not grown into.
#[tokio::test]
async fn the_definition_store_stops_accepting_definitions_at_its_bound() {
    let emulator = Emulator::new("us-east-1");
    for index in 0..MAX_WORKFLOW_DEFINITIONS {
        let (status, body) = emulator.create_definition(&format!("d-{index}")).await;
        assert_eq!(
            status, 201,
            "case definition {index} of {MAX_WORKFLOW_DEFINITIONS}: must be accepted, got {status} {body}"
        );
    }

    let (status, refused) = emulator.create_definition("one-too-many").await;
    assert_eq!(
        status, 402,
        "case one definition past the bound: must be refused 402, got {status} {refused}"
    );
    assert_eq!(
        refused["__type"], "ServiceQuotaExceededException",
        "case one definition past the bound: expected ServiceQuotaExceededException, got {refused}"
    );
    assert_eq!(
        refused["quotaCode"], "workflow-definitions-per-account",
        "case one definition past the bound: the refusal names the quota, got {refused}"
    );

    // The refusal did not store it, and the store still serves what it holds.
    let (status, answer) = emulator
        .call("GET", "/workflow-definitions/one-too-many", None)
        .await;
    assert_eq!(
        status, 404,
        "case the refused name: nothing was stored for it, got {status} {answer}"
    );
    let (status, answer) = emulator.call("GET", "/workflow-definitions/d-0", None).await;
    assert_eq!(
        status, 200,
        "case an accepted name: it still resolves, got {status} {answer}"
    );
}

/// Each nested store is bounded under its own parent, with the error the operation's own
/// model declares for a refused create.
#[tokio::test]
async fn each_nested_store_stops_accepting_children_at_its_per_parent_bound() {
    let emulator = Emulator::new("us-east-1");
    let (status, _) = emulator.create_definition("busy").await;
    assert_eq!(status, 201, "the definition must be created, got {status}");

    // RUNS per definition.
    let mut first_run = String::new();
    for index in 0..MAX_RUNS_PER_DEFINITION {
        let (status, body) = emulator.create_run("busy").await;
        assert_eq!(
            status, 201,
            "case run {index} of {MAX_RUNS_PER_DEFINITION}: must be accepted, got {status} {body}"
        );
        if index == 0 {
            first_run = text(&body, "workflowRunId");
        }
    }
    let (status, refused) = emulator.create_run("busy").await;
    assert_eq!(
        status, 409,
        "case one run past the per-definition bound: must be refused 409, got {status} {refused}"
    );
    assert_eq!(
        refused["__type"], "ConflictException",
        "case one run past the per-definition bound: expected ConflictException, got {refused}"
    );
    assert!(
        text(&refused, "message").contains("workflow-runs-per-workflow-definition"),
        "case one run past the per-definition bound: the refusal names the quota, got {refused}"
    );

    // SESSIONS per run.
    let mut first_session = String::new();
    for index in 0..MAX_SESSIONS_PER_RUN {
        let (status, body) = emulator.create_session("busy", &first_run).await;
        assert_eq!(
            status, 201,
            "case session {index} of {MAX_SESSIONS_PER_RUN}: must be accepted, got {status} {body}"
        );
        if index == 0 {
            first_session = text(&body, "sessionId");
        }
    }
    let (status, refused) = emulator.create_session("busy", &first_run).await;
    assert_eq!(
        status, 402,
        "case one session past the per-run bound: must be refused 402, got {status} {refused}"
    );
    assert_eq!(
        refused["quotaCode"], "sessions-per-workflow-run",
        "case one session past the per-run bound: the refusal names the quota, got {refused}"
    );

    // ACTS per session.
    for index in 0..MAX_ACTS_PER_SESSION {
        let (status, body) = emulator
            .create_act("busy", &first_run, &first_session, &format!("task {index}"))
            .await;
        assert_eq!(
            status, 201,
            "case act {index} of {MAX_ACTS_PER_SESSION}: must be accepted, got {status} {body}"
        );
    }
    let (status, refused) = emulator
        .create_act("busy", &first_run, &first_session, "one too many")
        .await;
    assert_eq!(
        status, 402,
        "case one act past the per-session bound: must be refused 402, got {status} {refused}"
    );
    assert_eq!(
        refused["quotaCode"], "acts-per-session",
        "case one act past the per-session bound: the refusal names the quota, got {refused}"
    );
}

/// The per-parent bound is not the only one: the run store is bounded ACROSS definitions
/// too, so spreading creates over fresh parents cannot grow the store without limit.
/// The session and act stores are bounded store-wide through the same code path.
#[tokio::test]
async fn the_run_store_is_bounded_across_definitions() {
    let emulator = Emulator::new("us-east-1");
    let parents = MAX_WORKFLOW_RUNS / MAX_RUNS_PER_DEFINITION;
    assert!(
        parents >= 2 && MAX_WORKFLOW_RUNS.is_multiple_of(MAX_RUNS_PER_DEFINITION),
        "this case assumes the store-wide run bound is a multiple of the per-parent one, got {MAX_WORKFLOW_RUNS} and {MAX_RUNS_PER_DEFINITION}"
    );

    for parent in 0..parents {
        let name = format!("p-{parent}");
        let (status, body) = emulator.create_definition(&name).await;
        assert_eq!(
            status, 201,
            "case parent {parent}: the definition must be created, got {status} {body}"
        );
        for index in 0..MAX_RUNS_PER_DEFINITION {
            let (status, body) = emulator.create_run(&name).await;
            assert_eq!(
                status, 201,
                "case parent {parent} run {index}: must be accepted, got {status} {body}"
            );
        }
    }

    // A fresh parent has room under ITS own cap, and is still refused by the store-wide one.
    let (status, _) = emulator.create_definition("fresh").await;
    assert_eq!(status, 201, "the fresh definition must be created, got {status}");
    let (status, refused) = emulator.create_run("fresh").await;
    assert_eq!(
        status, 409,
        "case one run past the store-wide bound: must be refused 409, got {status} {refused}"
    );
    assert!(
        text(&refused, "message").contains("workflow-runs-per-account"),
        "case one run past the store-wide bound: the refusal names the store-wide quota, got {refused}"
    );
}

/// Deleting a definition releases what it held, so the store-wide bounds are a bound on
/// what is HELD rather than on how many creates were ever made.
#[tokio::test]
async fn completing_a_deletion_releases_the_children_it_held() {
    let emulator = Emulator::new("us-east-1");
    let (status, _) = emulator.create_definition("short-lived").await;
    assert_eq!(status, 201, "the definition must be created, got {status}");
    let (status, body) = emulator.create_run("short-lived").await;
    assert_eq!(status, 201, "the run must be created, got {status} {body}");
    let run_id = text(&body, "workflowRunId");
    let (status, body) = emulator.create_session("short-lived", &run_id).await;
    assert_eq!(status, 201, "the session must be created, got {status} {body}");
    let session_id = text(&body, "sessionId");
    let (status, body) = emulator
        .create_act("short-lived", &run_id, &session_id, "a task")
        .await;
    assert_eq!(status, 201, "the act must be created, got {status} {body}");

    let (status, _) = emulator
        .call("DELETE", "/workflow-definitions/short-lived", None)
        .await;
    assert_eq!(status, 202, "the delete must answer 202, got {status}");
    std::thread::sleep(DELETION_COMPLETION_DELAY + Duration::from_millis(250));

    // The next call completes the due deletion; the whole chain is gone with it and the
    // same names are usable again from scratch.
    let (status, _) = emulator.create_definition("short-lived").await;
    assert_eq!(
        status, 201,
        "the definition must be creatable again, got {status}"
    );
    let (status, body) = emulator.create_run("short-lived").await;
    assert_eq!(
        status, 201,
        "a run must be creatable in the new definition, got {status} {body}"
    );
    let (status, answer) = emulator
        .call(
            "GET",
            &format!("/workflow-definitions/short-lived/workflow-runs/{run_id}"),
            None,
        )
        .await;
    assert_eq!(
        status, 404,
        "the run of the deleted definition must be gone, got {status} {answer}"
    );
}

/// Minted ids are unique and in the form the model constrains `UuidString` to, which is
/// what lets a generated id name exactly one stored record.
#[test]
fn minted_ids_are_well_formed_and_never_repeat() {
    let mut state = ComAmazonawsNovaactAmazonnovaagentsdataplaneState::default();
    let mints = 10_000usize;
    let mut seen = std::collections::BTreeSet::new();
    for index in 0..mints {
        let minted = state.mint_uuid();
        assert!(
            is_lowercase_uuid(&minted),
            "case mint {index}: expected the UuidString form, got {minted:?}"
        );
        // Version 4 and the RFC 4122 variant, as the minting documents.
        assert_eq!(
            minted.as_bytes()[14] as char,
            '4',
            "case mint {index}: expected version 4, got {minted:?}"
        );
        assert!(
            matches!(minted.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
            "case mint {index}: expected the RFC 4122 variant, got {minted:?}"
        );
        assert!(
            seen.insert(minted.clone()),
            "case mint {index}: {minted} was minted twice in {mints} mints"
        );
    }
}

/// The idempotency map is bounded, and the oldest remembered token is the one evicted —
/// so a caller sending a fresh `clientToken` every request cannot grow it without limit,
/// not even with tokens whose resource has since been deleted.
#[test]
fn the_idempotency_map_is_bounded_and_evicts_the_oldest() {
    let mut state = ComAmazonawsNovaactAmazonnovaagentsdataplaneState::default();
    let overflow = 25usize;
    for index in 0..(MAX_IDEMPOTENCY_ENTRIES + overflow) {
        state.record_idempotent_create(
            "CreateWorkflowDefinition",
            &format!("token-{index}"),
            "fingerprint".to_string(),
            format!("definition-{index}"),
        );
    }

    assert_eq!(
        state.remembered_idempotent_creates(),
        MAX_IDEMPOTENCY_ENTRIES,
        "case overflow by {overflow}: the map must stay at its bound of {MAX_IDEMPOTENCY_ENTRIES}, got {}",
        state.remembered_idempotent_creates()
    );
    for index in 0..overflow {
        assert!(
            state
                .idempotent_create("CreateWorkflowDefinition", &format!("token-{index}"))
                .is_none(),
            "case overflow by {overflow}: token-{index} is among the oldest and must have been evicted"
        );
    }
    for index in overflow..(MAX_IDEMPOTENCY_ENTRIES + overflow) {
        assert_eq!(
            state
                .idempotent_create("CreateWorkflowDefinition", &format!("token-{index}"))
                .map(|record| record.resource_id.as_str()),
            Some(format!("definition-{index}").as_str()),
            "case overflow by {overflow}: token-{index} must still replay what it created"
        );
    }

    // Re-recording a token already held is not a new entry, so it cannot push the map
    // past its bound either.
    state.record_idempotent_create(
        "CreateWorkflowDefinition",
        &format!("token-{overflow}"),
        "fingerprint".to_string(),
        "definition-rerecorded".to_string(),
    );
    assert_eq!(
        state.remembered_idempotent_creates(),
        MAX_IDEMPOTENCY_ENTRIES,
        "case re-recording a held token: the map must stay at its bound, got {}",
        state.remembered_idempotent_creates()
    );
}
