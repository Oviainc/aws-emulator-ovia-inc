// SPDX-License-Identifier: CC0-1.0

//! IDEMPOTENCY against a store that moves under the caller.
//!
//! The rule asserted here, over a set of changes rather than one recorded retry: a
//! retry carrying a `clientToken` the service already committed is answered from the
//! idempotency index BEFORE any refusal that reads state a call after the first one
//! can have changed — what the referenced command admits (a store the IoT control
//! plane writes at any time) and what the target device's concurrency quota allows —
//! so it completes with the `executionId` the first call minted, creates no second
//! execution and consumes no slot, whatever has happened to that state since. Only
//! the request's OWN values are judged ahead of the index, which is why the same
//! token with DIFFERENT values is still the token's modelled conflict.
//!
//! The refusals themselves are unchanged for a request that is NOT such a replay:
//! every row drives the identical request under a FRESH token and asserts the status,
//! error code and message the changed state earns it.
//!
//! Every request goes over the wire (restJson1 through the multi-service router), so
//! what is asserted is the HTTP behaviour a fleet-control client sees.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    command_registry, iot_commands::CommandFacts, MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING,
};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name, which is how the router picks the service for a request.
const SIGV4: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/iot-jobs-data/aws4_request, SignedHeaders=host, Signature=0";

/// The command every row starts an execution of: a control-plane resource, so the
/// shared index is seeded with it exactly as `iot:CreateCommand` would.
const COMMAND_ARN: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot";

/// The device's published command-execution concurrency limit, under a short name.
const LIMIT: usize = MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING;

/// One emulator holding one shared engine, driven over HTTP.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    /// An emulator whose shared IoT command index holds the one command these rows
    /// start, created with a payload template (so a start may carry `parameters`).
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        command_registry(&*engine)
            .expect("the shared IoT command index")
            .lock()
            .register(COMMAND_ARN, CommandFacts::default())
            .expect("register the command these rows start");
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map_or_else(Bytes::new, |body| Bytes::from(body.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://data.jobs.iot.us-east-1.amazonaws.com{path}"))
            .header("authorization", SIGV4)
            .header("content-type", "application/json")
            .body(payload)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One `StartCommandExecution` as a row asks for it, answered as it came off the
    /// wire so a refusal is as assertable as an accept.
    async fn start(&self, thing: &str, request: &Request<'_>) -> (u16, Value) {
        let mut body = json!({
            "targetArn": format!("arn:aws:iot:us-east-1:111122223333:thing/{thing}"),
            "commandArn": COMMAND_ARN,
        });
        if let Some(token) = request.client_token {
            body["clientToken"] = Value::from(token);
        }
        if let Some(parameters) = request.parameters.as_ref() {
            body["parameters"] = parameters.clone();
        }
        if let Some(seconds) = request.execution_timeout_seconds {
            body["executionTimeoutSeconds"] = Value::from(seconds);
        }
        self.call("POST", "/command-executions", Some(body)).await
    }

    /// How many executions the device's own queue reports — the record of whether a
    /// call created anything.
    async fn queued(&self, thing: &str) -> usize {
        let (status, list) = self.call("GET", &format!("/things/{thing}/jobs"), None).await;
        assert_eq!(
            status, 200,
            "the queue read of {thing} must answer 200, got {status} {list}"
        );
        list["queuedJobs"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a list of queued summaries, got {list}"))
            .len()
    }

    /// What `iot:UpdateCommand(deprecated=true)` writes to the shared index.
    fn deprecate_the_command(&self) {
        assert!(
            command_registry(&*self.engine)
                .expect("the shared IoT command index")
                .lock()
                .set_deprecated(COMMAND_ARN, true),
            "deprecating the command must find it registered"
        );
    }

    /// What `iot:DeleteCommand` writes to the shared index.
    fn delete_the_command(&self) {
        assert!(
            command_registry(&*self.engine)
                .expect("the shared IoT command index")
                .lock()
                .remove(COMMAND_ARN),
            "deleting the command must find it registered"
        );
    }

    /// A command of the same ARN created again, this time with a literal payload:
    /// `CreateCommand` decides the payload kind, so this is a delete and a create.
    fn recreate_the_command_with_a_static_payload(&self) {
        self.delete_the_command();
        command_registry(&*self.engine)
            .expect("the shared IoT command index")
            .lock()
            .register(
                COMMAND_ARN,
                CommandFacts {
                    static_payload: true,
                    deprecated: false,
                },
            )
            .expect("register the recreated command");
    }
}

/// The parts of a `StartCommandExecution` a row varies.
struct Request<'a> {
    client_token: Option<&'a str>,
    parameters: Option<Value>,
    execution_timeout_seconds: Option<i64>,
}

/// What happens to the state around the caller between its first call and its retry.
#[derive(Debug, Clone, Copy)]
enum Change {
    /// Nothing at all: the control row, where the replay is the only thing that can
    /// make the retry answer the first call's id.
    Nothing,
    /// `iot:UpdateCommand` deprecated the command.
    TheCommandIsDeprecated,
    /// `iot:DeleteCommand` removed the command.
    TheCommandIsDeleted,
    /// The command was removed and created again with a literal, static payload.
    TheCommandNowHasAStaticPayload,
    /// The device filled its command-execution concurrency limit with other starts.
    TheDeviceIsAtItsConcurrencyLimit,
}

/// What a start that is NOT a replay answers once a row's change has happened:
/// (HTTP status, error code, message) or `None` where the change refuses nothing.
type FreshRequestAnswer = Option<(u16, &'static str, String)>;

/// One row: the change, the request form it is driven with, and what the change does
/// to a request that is not a replay.
struct Row {
    case: &'static str,
    thing: &'static str,
    change: Change,
    client_token: &'static str,
    parameters: Option<Value>,
    execution_timeout_seconds: Option<i64>,
}

/// The messages this operation refuses with, built as the handler builds them from
/// the request and the index. Source: the operation's modelled `ValidationException`
/// (`smithy_query(query="operation", operation="StartCommandExecution")`, httpError
/// 400) and the refusals `tests/all/command_execution_validation.rs` pins.
fn refusal_for(change: Change) -> FreshRequestAnswer {
    let command_id = COMMAND_ARN.rsplit('/').next().unwrap_or_default();
    match change {
        Change::Nothing => None,
        Change::TheCommandIsDeprecated => Some((
            400,
            "ValidationException",
            format!("The command {command_id} has been deprecated."),
        )),
        Change::TheCommandIsDeleted => Some((
            400,
            "ValidationException",
            format!("The command {command_id} was not found."),
        )),
        Change::TheCommandNowHasAStaticPayload => Some((
            400,
            "ValidationException",
            "The parameters option is unsupported for commands with static payloads.".to_string(),
        )),
        Change::TheDeviceIsAtItsConcurrencyLimit => Some((
            402,
            "ServiceQuotaExceededException",
            format!(
                "the device already has {LIMIT} command executions that have not reached a \
                 terminal status, the maximum of {LIMIT} concurrent command executions per device"
            ),
        )),
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "nothing changes",
            thing: "laser-1",
            change: Change::Nothing,
            client_token: "t1",
            parameters: None,
            execution_timeout_seconds: None,
        },
        Row {
            case: "the command is deprecated between the two calls",
            thing: "laser-2",
            change: Change::TheCommandIsDeprecated,
            client_token: "retry.token-2",
            parameters: Some(json!({"waitSeconds": {"S": "5"}})),
            execution_timeout_seconds: Some(60),
        },
        Row {
            case: "the command is deleted between the two calls",
            thing: "laser-3",
            change: Change::TheCommandIsDeleted,
            client_token: "~!@#$%^&*()_+",
            parameters: None,
            execution_timeout_seconds: Some(1),
        },
        Row {
            case: "the command is recreated with a static payload",
            thing: "laser-4",
            change: Change::TheCommandNowHasAStaticPayload,
            // `ClientRequestTokenV2` is modelled len 1..64: a token at that maximum.
            client_token: "tttttttttttttttttttttttttttttttttttttttttttttttttttttttttttttttt",
            parameters: Some(json!({"intensity": {"I": 7}, "mode.full": {"S": "deep"}})),
            execution_timeout_seconds: None,
        },
        Row {
            case: "the device reaches its concurrency limit between the two calls",
            thing: "laser-5",
            change: Change::TheDeviceIsAtItsConcurrencyLimit,
            client_token: "a",
            parameters: Some(json!({"waitSeconds": {"L": 9}})),
            execution_timeout_seconds: Some(30),
        },
    ]
}

/// The wire error code of a modeled failure (restJson1 carries it in `__type`).
fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("")
}

/// The `message` of a modeled failure.
fn error_message(body: &Value) -> &str {
    body["message"].as_str().unwrap_or("")
}

fn execution_id(body: &Value) -> &str {
    body["executionId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an executionId, got {body}"))
}

/// Fill the device's remaining concurrency slots with starts of their own, so the
/// next request that is not a replay meets the limit.
async fn fill_the_remaining_slots(emulator: &Emulator, case: &str, thing: &str, held: usize) {
    for slot in held..LIMIT {
        let (status, body) = emulator
            .start(
                thing,
                &Request {
                    client_token: None,
                    parameters: None,
                    execution_timeout_seconds: None,
                },
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: filling slot {slot} must be admitted, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_client_token_replay_outlives_every_change_to_the_state_around_it() {
    for row in rows() {
        let Row {
            case,
            thing,
            change,
            client_token,
            ref parameters,
            execution_timeout_seconds,
        } = row;
        let emulator = Emulator::new();
        let replayed_request = |token: Option<&'static str>| Request {
            client_token: token,
            parameters: parameters.clone(),
            execution_timeout_seconds,
        };

        // The first call, which commits the token.
        let (status, first) = emulator
            .start(thing, &replayed_request(Some(client_token)))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first start must be served, got {status} {first}"
        );
        let committed = execution_id(&first).to_string();

        // The state around the caller moves.
        match change {
            Change::Nothing => {}
            Change::TheCommandIsDeprecated => emulator.deprecate_the_command(),
            Change::TheCommandIsDeleted => emulator.delete_the_command(),
            Change::TheCommandNowHasAStaticPayload => {
                emulator.recreate_the_command_with_a_static_payload();
            }
            Change::TheDeviceIsAtItsConcurrencyLimit => {
                fill_the_remaining_slots(&emulator, case, thing, 1).await;
            }
        }
        let queued_before_the_retry = emulator.queued(thing).await;

        // The identical retry completes with the execution the first call minted.
        let (status, replay) = emulator
            .start(thing, &replayed_request(Some(client_token)))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the identical retry must complete, got {status} {replay}"
        );
        assert_eq!(
            execution_id(&replay),
            committed,
            "case {case:?}: the retry must answer the first call's executionId, got {replay}"
        );
        let queued_after_the_retry = emulator.queued(thing).await;
        assert_eq!(
            queued_after_the_retry, queued_before_the_retry,
            "case {case:?}: the retry must create no second execution, expected \
             {queued_before_the_retry} queued, got {queued_after_the_retry}"
        );

        // The same request under a FRESH token is a NEW request, so the changed state
        // judges it exactly as it judges a first start.
        let (status, fresh) = emulator
            .start(thing, &replayed_request(Some("a-token-never-used-before")))
            .await;
        match refusal_for(change) {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: an unchanged state admits a fresh token, got {status} {fresh}"
                );
                assert_ne!(
                    execution_id(&fresh),
                    committed,
                    "case {case:?}: a fresh token must mint its own execution, got {fresh}"
                );
                assert_eq!(
                    emulator.queued(thing).await,
                    queued_after_the_retry + 1,
                    "case {case:?}: an admitted start queues exactly one execution, got {fresh}"
                );
            }
            Some((expected_status, expected_code, expected_message)) => {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected {expected_status} for a fresh token, got {status} \
                     {fresh}"
                );
                assert_eq!(
                    error_code(&fresh),
                    expected_code,
                    "case {case:?}: expected {expected_code}, got {fresh}"
                );
                assert_eq!(
                    error_message(&fresh),
                    expected_message,
                    "case {case:?}: expected the message {expected_message:?}, got {fresh}"
                );
                assert_eq!(
                    emulator.queued(thing).await,
                    queued_after_the_retry,
                    "case {case:?}: a refused start must queue nothing, got {fresh}"
                );
            }
        }
    }
}

/// RULE, the other half: what is judged ahead of the index is the request's OWN
/// values, so a token reused for a request that differs in any of them is the token
/// member's documented 409 conflict — including after the state around the caller
/// changed, where the conflict is still about the token and not about that state.
#[tokio::test]
async fn a_reused_token_with_different_values_is_the_tokens_conflict() {
    // (case, the retry's parameters, the retry's executionTimeoutSeconds) — one row
    // per request member the token covers.
    let rows: [(&str, Option<Value>, Option<i64>); 3] = [
        (
            "different parameters",
            Some(json!({"waitSeconds": {"S": "9"}})),
            Some(60),
        ),
        ("no parameters at all", None, Some(60)),
        (
            "a different executionTimeoutSeconds",
            Some(json!({"waitSeconds": {"S": "5"}})),
            Some(61),
        ),
    ];
    let first_parameters = json!({"waitSeconds": {"S": "5"}});

    for (case, parameters, execution_timeout_seconds) in rows {
        let emulator = Emulator::new();
        let thing = "laser-conflict";
        let token = "one-token";
        let (status, first) = emulator
            .start(
                thing,
                &Request {
                    client_token: Some(token),
                    parameters: Some(first_parameters.clone()),
                    execution_timeout_seconds: Some(60),
                },
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first start must be served, got {status} {first}"
        );
        let committed = execution_id(&first).to_string();

        emulator.deprecate_the_command();

        let (status, body) = emulator
            .start(
                thing,
                &Request {
                    client_token: Some(token),
                    parameters,
                    execution_timeout_seconds,
                },
            )
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 for a reused token with different values, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str().unwrap_or_default(),
            committed,
            "case {case:?}: the conflict must name the execution the token holds, got {body}"
        );
        assert_eq!(
            emulator.queued(thing).await,
            1,
            "case {case:?}: the conflicting retry must create nothing, got {body}"
        );
    }
}
