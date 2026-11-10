// SPDX-License-Identifier: CC0-1.0

//! The AUDIT leg of this front door: every Step Functions call it serves is a management
//! event of `states.amazonaws.com`.
//!
//! RULE: a service that serves a control-plane API call records that call in the account's
//! management-event log — the operation's own name as `eventName`, this service's
//! `eventSource`, the `Management` category, `readOnly` from the operation's own semantics,
//! the Region and the credential it was served for, the parameters the request carried as
//! `requestParameters` (a member the model declares `@sensitive` redacted), what a MUTATING
//! call answered as `responseElements` (a reporting call and a failed call have none), the
//! id the CALLER was answered under as `requestID`, and — for a call that failed — the
//! FAULT SHAPE's name as `errorCode` with the message the caller was answered with. Each
//! served call gets its OWN record. A request refused before any handler ran is not a
//! record of a served call and is recorded as nothing.
//!
//! The table below spans what the rule generalises over rather than the one call a
//! recording made: reporting and mutating operations across state machines, executions,
//! tags and activity callbacks; both outcomes (served and refused) over three fault shapes;
//! a request whose sensitive members carry values that appear nowhere else; and the two
//! answers that are not served calls at all (a constraint refusal and a request naming no
//! modelled operation).
//!
//! Every call goes over the wire through the same router the production emulator uses, as
//! an `awsJson1_0` `POST /` with `X-Amz-Target`; the records are read back out of the
//! SHARED log in the engine the service was built from — the one store CloudTrail's
//! `LookupEvents` reads. The end-to-end leg through that operation, and the attribution of
//! a call to the role session that made it, is
//! `core/router/tests/all/sfn_management_events.rs`, which needs CloudTrail, IAM and STS
//! mounted beside this service.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::service::management_events::{
    management_event_log, EVENT_SOURCE, SENSITIVE_MEMBERS,
};
use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;
use service_common::management_events::{
    ManagementEvent, ManagementEventLog, HIDDEN_SENSITIVE_VALUE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix of this service (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "AWSStepFunctions";

/// The sigv4 signing name every request below is signed with.
const SIGNING_NAME: &str = "states";

/// The access key id every call below is signed with, which is what the recorded caller of
/// each one is.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The Region every call below is signed for, and so the Region its records live in.
const REGION: &str = "us-west-2";

/// The account every ARN of this emulator carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `eventCategory` CloudTrail stamps on a control-plane call's record.
const MANAGEMENT: &str = "Management";

/// The `eventType` of a record made for an AWS API call.
const AWS_API_CALL: &str = "AwsApiCall";

/// The header AWS reports a response's request id in, lowercased as HTTP compares it.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// A role ARN inside the modelled `Arn` length (1..256).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/StepFunctionsRole";

/// A definition that succeeds immediately with a static result.
const HELLO_DEFINITION: &str =
    r#"{"StartAt":"Hello","States":{"Hello":{"Type":"Pass","Result":{"msg":"hi"},"End":true}}}"#;

/// The execution input, the task output and the stop cause the calls below carry: values
/// that appear nowhere else, so finding one inside a record would be unambiguous.
const SECRET_INPUT: &str = r#"{"card":"n0t-in-the-trail-input"}"#;
const SECRET_OUTPUT: &str = r#"{"receipt":"n0t-in-the-trail-output"}"#;
const SECRET_CAUSE: &str = "n0t-in-the-trail-cause";

/// One emulator: a router that claims `states` requests, and the shared engine whose
/// management-event log the records are read out of.
struct Audited {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Audited {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call one operation and answer `(status, body, request id)` — the three things a
    /// caller observes, since the record names the request by the id the caller was
    /// answered under.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value, Option<String>) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/{SIGNING_NAME}/\
             aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "case {operation}: expected a JSON body, got {error} for {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed, request_id)
    }

    /// Call one operation that has to succeed, and answer its body.
    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, answered, _) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "setup: {operation} must answer 200, got {status} with {answered}"
        );
        answered
    }

    /// Every record in the shared log, oldest first, as the CloudTrail event document a
    /// lookup reports.
    fn records(&self) -> Vec<Value> {
        let log = management_event_log(&*self.engine)
            .expect("this engine carries the shared management-event log");
        let held = log.lock();
        held.iter()
            .map(ManagementEvent::to_cloudtrail_event_json)
            .collect()
    }

    /// The records of the calls to `event_name`, oldest first.
    fn records_of(&self, event_name: &str) -> Vec<Value> {
        self.records()
            .into_iter()
            .filter(|event| event.get("eventName") == Some(&json!(event_name)))
            .collect()
    }
}

/// A string member of a record, or `None` when the record does not carry it.
fn at(event: &Value, pointer: &str) -> Option<String> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The SHAPE name of an `awsJson1_0` error answer, whose `__type` may carry the model's
/// namespace.
fn shape_name(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error body, got {body}"))
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` an error answer states.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a refusal with a message, got {body}"))
        .to_string()
}

/// One row of the table: the call it makes and what the record of it must carry.
struct Served {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The HTTP status the caller sees.
    status: u16,
    /// The operation's own semantics, as the record's `readOnly`.
    read_only: bool,
    /// Whether this answer is a SERVED call at all: a request refused before any handler
    /// ran is recorded as nothing.
    recorded: bool,
}

/// Assert the facts every record of a served call carries, whatever the call was.
fn assert_recorded_call(case: &str, event: &Value, operation: &str, read_only: bool) {
    for (pointer, expected) in [
        ("/eventName", operation),
        ("/eventSource", EVENT_SOURCE),
        ("/eventCategory", MANAGEMENT),
        ("/eventType", AWS_API_CALL),
        ("/awsRegion", REGION),
        ("/recipientAccountId", ACCOUNT),
        ("/userIdentity/accessKeyId", ACCESS_KEY_ID),
    ] {
        assert_eq!(
            at(event, pointer).as_deref(),
            Some(expected),
            "case {case:?}: expected {pointer} {expected:?}, got {event}"
        );
    }
    assert_eq!(
        event.get("readOnly"),
        Some(&json!(read_only)),
        "case {case:?}: expected readOnly {read_only}, got {event}"
    );
    assert!(
        at(event, "/eventID").is_some_and(|id| !id.is_empty()),
        "case {case:?}: a record carries its own eventID, got {event}"
    );
}

/// RULE, over the whole surface: each call this service SERVES is one record of the
/// account's event history, carrying the operation, this service's source, the Region and
/// caller, the operation's own `readOnly`, the parameters it was dispatched with and the
/// outcome it answered — and an answer that was never served is no record.
#[tokio::test]
async fn every_served_call_is_recorded_as_a_management_event() {
    let emulator = Audited::new();
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "audited-machine",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    let machine = created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create reports the ARN it minted, got {created}"))
        .to_string();
    let missing = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");
    emulator
        .ok("CreateActivity", &json!({ "name": "audited-activity" }))
        .await;
    let activity = format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:audited-activity");

    // The rows differ along what the rule generalises over: the KIND of call (create,
    // read, list, start, tag, settle a task), the RESOURCE it addresses, both OUTCOMES,
    // and the two answers that are not served calls.
    let rows: Vec<Served> = vec![
        Served {
            case: "a read of a state machine",
            operation: "DescribeStateMachine",
            body: json!({ "stateMachineArn": machine }),
            status: 200,
            read_only: true,
            recorded: true,
        },
        Served {
            case: "a read of a state machine that was never created",
            operation: "DescribeStateMachine",
            body: json!({ "stateMachineArn": missing }),
            status: 400,
            read_only: true,
            recorded: true,
        },
        Served {
            case: "a listing that names no resource",
            operation: "ListStateMachines",
            body: json!({}),
            status: 200,
            read_only: true,
            recorded: true,
        },
        Served {
            case: "a start of an execution",
            operation: "StartExecution",
            body: json!({
                "stateMachineArn": machine,
                "name": "audited-run",
                "input": SECRET_INPUT,
            }),
            status: 200,
            read_only: false,
            recorded: true,
        },
        Served {
            case: "a read of an activity",
            operation: "DescribeActivity",
            body: json!({ "activityArn": activity }),
            status: 200,
            read_only: true,
            recorded: true,
        },
        Served {
            case: "a poll that hands a worker a task",
            operation: "GetActivityTask",
            body: json!({ "activityArn": activity, "workerName": "audited-worker" }),
            status: 200,
            read_only: false,
            recorded: true,
        },
        Served {
            case: "a tag write",
            operation: "TagResource",
            body: json!({
                "resourceArn": machine,
                "tags": [{ "key": "audited", "value": "yes" }],
            }),
            status: 200,
            read_only: false,
            recorded: true,
        },
        Served {
            case: "a read of the tags that write left",
            operation: "ListTagsForResource",
            body: json!({ "resourceArn": machine }),
            status: 200,
            read_only: true,
            recorded: true,
        },
        Served {
            case: "a task settled with a token no task was ever handed out under",
            operation: "SendTaskSuccess",
            body: json!({ "taskToken": "n0t-a-token", "output": SECRET_OUTPUT }),
            status: 400,
            read_only: false,
            recorded: true,
        },
        Served {
            case: "a stop of an execution that was never started",
            operation: "StopExecution",
            body: json!({
                "executionArn":
                    format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:audited-machine:nope"),
                "error": "Halted",
                "cause": SECRET_CAUSE,
            }),
            status: 400,
            read_only: false,
            recorded: true,
        },
        Served {
            case: "a definition validated without a state machine",
            operation: "ValidateStateMachineDefinition",
            body: json!({ "definition": HELLO_DEFINITION }),
            status: 200,
            read_only: true,
            recorded: true,
        },
        // Refused BEFORE any handler ran: the required `definition` is absent, so the
        // framework's constraint walk answers and no call was served.
        Served {
            case: "a create refused by the constraint walk",
            operation: "CreateStateMachine",
            body: json!({ "name": "no-definition", "roleArn": ROLE_ARN }),
            status: 400,
            read_only: false,
            recorded: false,
        },
        // Nothing of this service's surface at all.
        Served {
            case: "a request naming no modelled operation",
            operation: "NoSuchOperationInThisModel",
            body: json!({}),
            status: 0,
            read_only: false,
            recorded: false,
        },
    ];

    let mut seen_event_ids: Vec<String> = Vec::new();
    for row in rows {
        let before = emulator.records_of(row.operation).len();
        let (status, answered, request_id) = emulator.call(row.operation, &row.body).await;
        if row.status != 0 {
            assert_eq!(
                status, row.status,
                "case {:?}: expected HTTP {}, got {status} with {answered}",
                row.case, row.status
            );
        }
        let after = emulator.records_of(row.operation);
        if !row.recorded {
            assert_eq!(
                after.len(),
                before,
                "case {:?}: an answer that served no call is recorded as nothing, got {after:?}",
                row.case
            );
            continue;
        }
        assert_eq!(
            after.len(),
            before + 1,
            "case {:?}: expected one more record of {}, got {after:?}",
            row.case,
            row.operation
        );
        let event = after
            .last()
            .unwrap_or_else(|| panic!("case {:?}: the record just made", row.case));
        assert_recorded_call(row.case, event, row.operation, row.read_only);
        // The record names the request by the id the CALLER was answered under, so a
        // client holding a response can find the record of that very call.
        assert_eq!(
            at(event, "/requestID"),
            request_id,
            "case {:?}: expected requestID {request_id:?}, got {event}",
            row.case
        );
        // A failure records the fault SHAPE's name and the message the caller saw; a
        // success records neither.
        let expected_code = (status != 200).then(|| shape_name(&answered));
        assert_eq!(
            at(event, "/errorCode"),
            expected_code,
            "case {:?}: expected errorCode {expected_code:?}, got {event}",
            row.case
        );
        if status != 200 {
            assert_eq!(
                at(event, "/errorMessage"),
                Some(message_of(&answered)),
                "case {:?}: the record repeats the message the caller saw, got {event}",
                row.case
            );
        }
        // "responseElements" is what the call ANSWERED with: a reporting call and a failed
        // call have none.
        let answers = status == 200 && !row.read_only;
        assert_eq!(
            event
                .get("responseElements")
                .is_some_and(|elements| !elements.is_null()),
            answers,
            "case {:?}: expected responseElements present={answers}, got {event}",
            row.case
        );
        let event_id = at(event, "/eventID").unwrap_or_default();
        assert!(
            !seen_event_ids.contains(&event_id),
            "case {:?}: each served call is its own event, got the already recorded \
             {event_id:?}",
            row.case
        );
        seen_event_ids.push(event_id);
    }
}

/// RULE: the parameters a record carries are the ones the request was DISPATCHED with,
/// member for member — and a member this model declares `@sensitive` carries CloudTrail's
/// redaction literal instead of the value, so a workflow's own data is retrievable from
/// event history nowhere.
#[tokio::test]
async fn a_record_carries_the_parameters_the_call_was_dispatched_with() {
    let emulator = Audited::new();
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "parameterised",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    let machine = created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create reports the ARN it minted, got {created}"))
        .to_string();

    // The create's own record: the members it carried, with the Amazon States Language
    // definition redacted and the ARN it minted reported as what it answered.
    let create = emulator
        .records_of("CreateStateMachine")
        .pop()
        .unwrap_or_else(|| panic!("the create is recorded"));
    for (pointer, expected) in [
        ("/requestParameters/name", "parameterised"),
        ("/requestParameters/roleArn", ROLE_ARN),
        ("/requestParameters/definition", HIDDEN_SENSITIVE_VALUE),
        ("/responseElements/stateMachineArn", machine.as_str()),
    ] {
        assert_eq!(
            at(&create, pointer).as_deref(),
            Some(expected),
            "case \"a create\": expected {pointer} {expected:?}, got {create}"
        );
    }

    let started = emulator
        .ok(
            "StartExecution",
            &json!({
                "stateMachineArn": machine,
                "name": "parameterised-run",
                "input": SECRET_INPUT,
            }),
        )
        .await;
    let execution = started["executionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the start reports the execution it minted, got {started}"))
        .to_string();
    let start = emulator
        .records_of("StartExecution")
        .pop()
        .unwrap_or_else(|| panic!("the start is recorded"));
    for (pointer, expected) in [
        ("/requestParameters/stateMachineArn", machine.as_str()),
        ("/requestParameters/name", "parameterised-run"),
        ("/requestParameters/input", HIDDEN_SENSITIVE_VALUE),
        ("/responseElements/executionArn", execution.as_str()),
    ] {
        assert_eq!(
            at(&start, pointer).as_deref(),
            Some(expected),
            "case \"a start\": expected {pointer} {expected:?}, got {start}"
        );
    }

    // A read's record carries exactly the member the request named, and nothing the
    // request did not carry: an absent member is left out rather than recorded as null.
    emulator
        .ok("DescribeStateMachine", &json!({ "stateMachineArn": machine }))
        .await;
    let read = emulator
        .records_of("DescribeStateMachine")
        .pop()
        .unwrap_or_else(|| panic!("the read is recorded"));
    assert_eq!(
        read.get("requestParameters"),
        Some(&json!({ "stateMachineArn": machine })),
        "case \"a read\": the record carries exactly the member the request named, got {read}"
    );

    // Nothing a sensitive member carried reaches the log, through any call.
    emulator
        .call(
            "SendTaskSuccess",
            &json!({ "taskToken": "n0t-a-token", "output": SECRET_OUTPUT }),
        )
        .await;
    emulator
        .call(
            "StopExecution",
            &json!({ "executionArn": execution, "error": "Halted", "cause": SECRET_CAUSE }),
        )
        .await;
    emulator
        .call(
            "TestState",
            &json!({
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "input": SECRET_INPUT,
            }),
        )
        .await;
    let whole_log = Value::Array(emulator.records()).to_string();
    for secret in [SECRET_INPUT, SECRET_OUTPUT, SECRET_CAUSE, HELLO_DEFINITION] {
        assert!(
            !whole_log.contains(secret),
            "expected no sensitive value in event history, found {secret:?} in {whole_log}"
        );
    }
    // The redaction is by MEMBER name, and every member the model declares sensitive is in
    // that set, so the two cases above are instances rather than the whole rule.
    for member in ["input", "output", "cause", "definition", "taskToken"] {
        assert!(
            SENSITIVE_MEMBERS.contains(&member),
            "expected {member:?} among the members this service withholds, got \
             {SENSITIVE_MEMBERS:?}"
        );
    }
}

/// RULE: the log is the ACCOUNT's, shared by every service that records a call — so the
/// records this service writes land in the very store CloudTrail's `LookupEvents` reads,
/// and not in a copy of its own.
#[tokio::test]
async fn the_records_land_in_the_accounts_shared_log() {
    let emulator = Audited::new();
    emulator
        .ok("CreateActivity", &json!({ "name": "shared-log" }))
        .await;
    // Reached as any other reader of the shared engine reaches it, by the SHARED type
    // rather than through this service's own state.
    let shared = emulator.engine.state::<ManagementEventLog>();
    let recorded: Vec<String> = shared
        .lock()
        .iter()
        .map(|event| event.call.event_name.clone())
        .collect();
    assert!(
        recorded.iter().any(|name| name == "CreateActivity"),
        "expected the call in the account's shared log, got {recorded:?}"
    );
}

/// This crate's Smithy model — the source the set of withheld members is derived from, read
/// back here so the two cannot drift.
const MODEL_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-sfn-awsstepfunctions.json"
);

/// The Smithy trait that marks a member, or the shape a member targets, as carrying data
/// the serving API withholds.
const SENSITIVE_TRAIT: &str = "smithy.api#sensitive";

/// Whether `shapes` declares `member` sensitive — on the member itself, or on the shape it
/// targets, which is where this model puts it (`SensitiveData`, `SensitiveError`, …).
fn declares_sensitive(shapes: &serde_json::Map<String, Value>, member: &Value) -> bool {
    let stated = |carrier: Option<&Value>| {
        carrier
            .and_then(|shape| shape.get("traits"))
            .and_then(Value::as_object)
            .is_some_and(|traits| traits.contains_key(SENSITIVE_TRAIT))
    };
    if stated(Some(member)) {
        return true;
    }
    let target = member.get("target").and_then(Value::as_str);
    stated(target.and_then(|target| shapes.get(target)))
}

/// RULE: the members whose values a record withholds are the model's own — the redaction is
/// by member NAME, so the set is exactly the names this model declares `@sensitive`
/// wherever they appear: a name the model marks nowhere is not withheld (such a record would
/// hide a value AWS reports), and a name the model marks on one member and leaves plain on
/// another is not withheld either (hiding it would redact the plain one too).
///
/// Derived from the model rather than transcribed, so a model that marks a new member
/// sensitive, or unmarks one, fails here rather than leaving a payload in event history or
/// a reported value hidden from it.
#[test]
fn the_withheld_members_are_the_models_own_sensitive_members() {
    let source = std::fs::read_to_string(MODEL_PATH).unwrap_or_else(|error| {
        panic!("the Smithy model at {MODEL_PATH} must be readable: {error}")
    });
    let model: Value = serde_json::from_str(&source).expect("the Smithy model parses as JSON");
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the Smithy model carries a shapes map");

    // Every member name of this model, split by whether the model declares that name
    // sensitive where it appears: a name may appear on several shapes.
    let mut sensitive: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut plain: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for shape in shapes.values() {
        let Some(members) = shape.get("members").and_then(Value::as_object) else {
            continue;
        };
        for (name, member) in members {
            if declares_sensitive(shapes, member) {
                sensitive.insert(name.as_str());
            } else {
                plain.insert(name.as_str());
            }
        }
    }
    let withheld: std::collections::BTreeSet<&str> = sensitive
        .iter()
        .copied()
        .filter(|name| !plain.contains(name))
        .collect();
    let stated: std::collections::BTreeSet<&str> = SENSITIVE_MEMBERS.iter().copied().collect();
    assert_eq!(
        stated, withheld,
        "expected the withheld members to be the model's own sensitive member names, got \
         {stated:?} for {withheld:?}"
    );
}
