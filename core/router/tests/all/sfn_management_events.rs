// SPDX-License-Identifier: CC0-1.0

//! A Step Functions session in CloudTrail event history — across four services on one
//! engine.
//!
//! RULE: a service that serves a control-plane API call records it as a management event of
//! that service, so `cloudtrail:LookupEvents` reports the call under its own `eventName`
//! with the serving service's `eventSource`, the `Management` category, `readOnly` from the
//! operation's own semantics, the Region and account it was served for, the parameters the
//! request carried, the id the CALLER was answered under as `requestID`, and — for a call
//! made with a role session's temporary credentials — `userIdentity.type` `AssumedRole`,
//! the SESSION's own ARN as `userIdentity.arn` and the issuing ROLE's ARN as
//! `sessionContext.sessionIssuer.arn`. A reporting call is recorded exactly as a mutating
//! one is, a REFUSED call included: it records the fault shape's own name as `errorCode`
//! with the message the caller saw and no `responseElements`, while a served mutation
//! records what it answered and no error at all.
//!
//! Only a cross-service suite can pin this: the roles are written by `iam:CreateRole` and
//! `iam:PutRolePolicy`, the caller's session is minted by `sts:AssumeRole`, the calls are
//! served by Step Functions, and every record is read back through
//! `cloudtrail:LookupEvents` — four services over ONE engine, as `core/router/src/lib.rs`
//! mounts them. The per-record shape of this service's own recording is
//! `services/service-com-amazonaws-sfn-awsstepfunctions/tests/all/management_event_recording.rs`.
//!
//! Every call goes over the wire, on a router with request authentication off as every
//! suite in this crate runs it: what a credential scope carries is the key id a handler
//! reads the caller from. Assembling the whole router costs stack (see `boot_serve.rs`), so
//! it is built — and the requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, element, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Step Functions' `X-Amz-Target` prefix, from that service's own model (service shape
/// `AWSStepFunctions`).
const TARGET_PREFIX: &str = "AWSStepFunctions";

/// The Region every call of this suite is signed for, and so the Region its records live in.
const REGION: &str = "us-east-1";

/// The long-term access key id the SETUP calls carry — the roles and the state machine —
/// so the caller's own calls are the only ones made with a session.
const SETUP_KEY_ID: &str = "AKIDTEST";

/// The `eventSource` CloudTrail stamps on a Step Functions management event
/// (`smithy_query(query="service_info")` ->
/// `awsService.cloudTrailEventSource: states.amazonaws.com`).
const SFN_EVENT_SOURCE: &str = "states.amazonaws.com";

/// The `eventCategory` of a control-plane call's record.
const MANAGEMENT: &str = "Management";

/// The header AWS reports a response's request id in, lowercased as HTTP compares it.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The state machine the session starts an execution of, and the execution's name.
const MACHINE: &str = "sfn-audited-session";
const EXECUTION: &str = "sfn-audited-run";

/// The caller's role session name, which the envelope reports as the event's `Username`.
const SESSION_NAME: &str = "sfn-caller-session";

/// A one-`Pass`-state STANDARD state machine.
const DEFINITION: &str =
    r#"{"StartAt":"Only","States":{"Only":{"Type":"Pass","Result":{"ok":true},"End":true}}}"#;

/// The trust policy of the EXECUTION role: the role the state machine itself runs under,
/// which must appear in no record of the caller's calls.
const EXECUTION_TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"states.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// What the caller's role may do: the three Step Functions actions the session makes.
const CALLER_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Action":["states:StartExecution","states:DescribeStateMachine","states:DescribeExecution"],
"Resource":"*"}]}"#;

/// The emulator this suite drives: the whole router over one engine, and the runtime its
/// requests are blocked on.
struct Audited {
    router: MultiServiceRouter,
    runtime: tokio::runtime::Runtime,
}

/// One answer as its caller reads it: the status, the body, and the id the response named
/// the request by — which is what the record of that call must carry.
struct Answered {
    status: u16,
    body: Value,
    request_id: String,
}

impl Audited {
    fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = build_router(engine);
        router.set_request_authentication(false);
        Self {
            router,
            // A MULTI-thread runtime: a started execution is interpreted while the request
            // that started it is answered.
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("a multi-thread runtime"),
        }
    }

    /// Drive one request signed (in the routing sense) with `access_key_id`, answering the
    /// status, the body and the request id the response named.
    fn send(
        &self,
        access_key_id: &str,
        signing_name: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, Vec<u8>, String) {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            );
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.runtime.block_on(self.router.dispatch(request));
        let status = response.status().as_u16();
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        (status, response.into_body().to_vec(), request_id)
    }

    /// One awsQuery call against `signing_name`, signed with `access_key_id`.
    fn query(
        &self,
        access_key_id: &str,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, &str)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let (status, payload, _) = self.send(
            access_key_id,
            signing_name,
            &[("content-type", "application/x-www-form-urlencoded")],
            body.into_bytes(),
        );
        (status, String::from_utf8_lossy(&payload).into_owned())
    }

    /// One Step Functions operation (`awsJson1_0`), signed with `access_key_id`.
    fn sfn(&self, access_key_id: &str, operation: &str, body: &Value) -> Answered {
        let (status, payload, request_id) = self.send(
            access_key_id,
            "states",
            &[
                ("content-type", "application/x-amz-json-1.0"),
                ("x-amz-target", &format!("{TARGET_PREFIX}.{operation}")),
            ],
            serde_json::to_vec(body).expect("the request body serializes"),
        );
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or_else(|_| {
                json!({ "unparsed": String::from_utf8_lossy(&payload).into_owned() })
            })
        };
        Answered {
            status,
            body: parsed,
            request_id,
        }
    }

    /// One Step Functions operation that has to succeed.
    fn sfn_ok(&self, access_key_id: &str, operation: &str, body: &Value) -> Answered {
        let answered = self.sfn(access_key_id, operation, body);
        assert_eq!(
            answered.status, 200,
            "{operation} must answer 200, got {} with {}",
            answered.status, answered.body
        );
        answered
    }

    /// Create a role with `policy` as its trust policy; answer its ARN.
    fn role(&self, name: &str, policy: &str) -> String {
        let (status, body) = self.query(
            SETUP_KEY_ID,
            "iam",
            "2010-05-08",
            "CreateRole",
            &[("RoleName", name), ("AssumeRolePolicyDocument", policy)],
        );
        assert_eq!(
            status, 200,
            "setup: CreateRole({name:?}) must answer 200, got {status} with {body}"
        );
        element(&body, "Arn")
    }

    /// Put an inline policy on `role`.
    fn put_role_policy(&self, role: &str, name: &str, document: &str) {
        let (status, body) = self.query(
            SETUP_KEY_ID,
            "iam",
            "2010-05-08",
            "PutRolePolicy",
            &[
                ("RoleName", role),
                ("PolicyName", name),
                ("PolicyDocument", document),
            ],
        );
        assert_eq!(
            status, 200,
            "setup: PutRolePolicy({name:?}) must answer 200, got {status} with {body}"
        );
    }

    /// `sts:AssumeRole` of `role_arn`, answering the access key id of the session minted
    /// and the SESSION's own ARN as the assume reported it (`AssumedRoleUser.Arn`), so the
    /// assertions below compare the record against what the emulator actually minted
    /// rather than against a second statement of the ARN form.
    fn assume(&self, role_arn: &str, session_name: &str) -> (String, String) {
        let (status, body) = self.query(
            SETUP_KEY_ID,
            "sts",
            "2011-06-15",
            "AssumeRole",
            &[("RoleArn", role_arn), ("RoleSessionName", session_name)],
        );
        assert_eq!(
            status, 200,
            "setup: AssumeRole({session_name:?}) must answer 200, got {status} with {body}"
        );
        let assumed = block_of(&body, "AssumedRoleUser").unwrap_or_else(|| {
            panic!("setup: the assume reports the session it minted, got {body}")
        });
        (element(&body, "AccessKeyId"), element(assumed, "Arn"))
    }

    /// `cloudtrail:LookupEvents` filtered on one attribute over a window around now,
    /// answering each event's envelope paired with its parsed `CloudTrailEvent` document,
    /// most recent first and paged to exhaustion.
    fn lookup(&self, attribute_key: &str, attribute_value: &str) -> Vec<(Value, Value)> {
        let mut found: Vec<(Value, Value)> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({
                "MaxResults": 50,
                "LookupAttributes": [{
                    "AttributeKey": attribute_key,
                    "AttributeValue": attribute_value,
                }],
            });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let (status, payload, _) = self.send(
                SETUP_KEY_ID,
                "cloudtrail",
                &[
                    ("content-type", "application/x-amz-json-1.1"),
                    ("x-amz-target", "CloudTrail_20131101.LookupEvents"),
                ],
                request.to_string().into_bytes(),
            );
            let text = String::from_utf8_lossy(&payload).into_owned();
            assert_eq!(
                status, 200,
                "LookupEvents({attribute_key}={attribute_value}) must succeed, got {status} \
                 with {text}"
            );
            let answered: Value = serde_json::from_str(&text)
                .unwrap_or_else(|error| panic!("the lookup answers JSON ({error}): {text}"));
            for event in answered["Events"]
                .as_array()
                .unwrap_or_else(|| panic!("LookupEvents reports an Events list, got {text}"))
            {
                let document = event["CloudTrailEvent"]
                    .as_str()
                    .unwrap_or_else(|| panic!("an event carries its CloudTrailEvent: {event}"));
                let parsed: Value = serde_json::from_str(document).unwrap_or_else(|error| {
                    panic!("CloudTrailEvent is JSON ({error}): {document}")
                });
                found.push((event.clone(), parsed));
            }
            next_token = answered["NextToken"].as_str().map(ToString::to_string);
            if next_token.is_none() {
                return found;
            }
        }
    }

    /// The records of the calls to `event_name` that the session `session_arn` made, as the
    /// check that filed this issue finds them: a lookup by event name, kept to the records
    /// whose `userIdentity.arn` is that session's.
    fn session_records(&self, event_name: &str, session_arn: &str) -> Vec<(Value, Value)> {
        self.lookup("EventName", event_name)
            .into_iter()
            .filter(|(_, document)| {
                at(document, "/userIdentity/arn").as_deref() == Some(session_arn)
            })
            .collect()
    }
}

/// A string member of a record or an envelope, or `None` when it does not carry it.
fn at(event: &Value, pointer: &str) -> Option<String> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The SHAPE name of an `awsJson1_0` error answer, whose `__type` may carry the namespace.
fn shape_name(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error body, got {body}"))
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert the facts every record of a Step Functions call carries, and the envelope the
/// lookup reports it in.
fn assert_sfn_record(
    case: &str,
    envelope: &Value,
    document: &Value,
    operation: &str,
    read_only: bool,
    account: &str,
) {
    for (pointer, expected) in [
        ("/eventName", operation),
        ("/eventSource", SFN_EVENT_SOURCE),
        ("/eventCategory", MANAGEMENT),
        ("/awsRegion", REGION),
        ("/recipientAccountId", account),
    ] {
        assert_eq!(
            at(document, pointer).as_deref(),
            Some(expected),
            "case {case:?}: expected {pointer} {expected:?}, got {document}"
        );
    }
    assert_eq!(
        document.get("readOnly"),
        Some(&json!(read_only)),
        "case {case:?}: expected readOnly {read_only}, got {document}"
    );
    // The envelope the lookup reports the record in states the same facts, as the fields a
    // client reads without parsing the document: the source, the read classification as a
    // string, and the record's own id.
    assert_eq!(
        at(envelope, "/EventSource").as_deref(),
        Some(SFN_EVENT_SOURCE),
        "case {case:?}: the envelope names the serving service, got {envelope}"
    );
    assert_eq!(
        at(envelope, "/ReadOnly").as_deref(),
        Some(read_only.to_string().as_str()),
        "case {case:?}: expected the envelope's ReadOnly {read_only}, got {envelope}"
    );
    assert_eq!(
        at(envelope, "/EventId"),
        at(document, "/eventID"),
        "case {case:?}: the envelope's EventId is the record's eventID, got {envelope}"
    );
}

/// Assert that `document` attributes the call to the caller's SESSION and to the role that
/// issued it — and to neither the state machine's execution role nor the long-term setup
/// key.
fn assert_session_attribution(
    case: &str,
    envelope: &Value,
    document: &Value,
    session_key: &str,
    session_arn: &str,
    caller_role_arn: &str,
    execution_role_arn: &str,
) {
    // The name the session speaks under, which the envelope reports as the event's
    // `Username` — the field a lookup by that attribute filters on.
    assert_eq!(
        at(envelope, "/Username").as_deref(),
        Some(SESSION_NAME),
        "case {case:?}: the envelope names the session the call was made by, got {envelope}"
    );
    for (pointer, expected) in [
        ("/userIdentity/type", "AssumedRole"),
        ("/userIdentity/accessKeyId", session_key),
        ("/userIdentity/arn", session_arn),
        (
            "/userIdentity/sessionContext/sessionIssuer/arn",
            caller_role_arn,
        ),
    ] {
        assert_eq!(
            at(document, pointer).as_deref(),
            Some(expected),
            "case {case:?}: expected {pointer} {expected:?}, got {document}"
        );
    }
    let identity = document
        .get("userIdentity")
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        !identity.contains(execution_role_arn),
        "case {case:?}: the state machine's execution role must appear nowhere in \
         userIdentity, got {identity}"
    );
    assert!(
        !identity.contains(SETUP_KEY_ID),
        "case {case:?}: the session's call must not be attributed to the setup credential, \
         got {identity}"
    );
}

#[test]
fn a_sessions_start_and_its_refused_read_are_audited_in_event_history() {
    std::thread::Builder::new()
        .name("sfn-management-events".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let emulator = Audited::new();
            let account = engine::emulator_account_id().to_string();

            // The state machine's own execution role, and the SECOND, distinct role the
            // caller assumes to make the calls below.
            let execution_role = emulator.role("sfn-audited-execution", EXECUTION_TRUST_POLICY);
            let caller_trust = format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
                 \"Principal\":{{\"AWS\":\"arn:aws:iam::{account}:root\"}},\
                 \"Action\":\"sts:AssumeRole\"}}]}}"
            );
            let caller_role = emulator.role("sfn-audited-caller", &caller_trust);
            emulator.put_role_policy("sfn-audited-caller", "sfn-calls", CALLER_POLICY);
            // The session's own ARN comes from the assume's answer, so the attribution
            // assertions compare the record against the session STS actually minted.
            let (session_key, session_arn) = emulator.assume(&caller_role, SESSION_NAME);
            assert!(
                session_arn.ends_with(&format!("assumed-role/sfn-audited-caller/{SESSION_NAME}")),
                "setup: the assume must report this caller's session, got {session_arn}"
            );

            let created = emulator.sfn_ok(
                SETUP_KEY_ID,
                "CreateStateMachine",
                &json!({
                    "name": MACHINE,
                    "definition": DEFINITION,
                    "roleArn": execution_role,
                    "type": "STANDARD",
                }),
            );
            let machine = created.body["stateMachineArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("setup: the create reports its ARN, got {}", created.body)
                })
                .to_string();
            let missing = format!("arn:aws:states:{REGION}:{account}:stateMachine:never-created");

            // The three calls the session makes: a start, a read of a state machine that
            // was never created, and a read of the real one.
            let started = emulator.sfn_ok(
                &session_key,
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": EXECUTION,
                    "input": json!({ "probe": EXECUTION }).to_string(),
                }),
            );
            let execution_arn = started.body["executionArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("the start reports its execution, got {}", started.body)
                })
                .to_string();

            let refused = emulator.sfn(
                &session_key,
                "DescribeStateMachine",
                &json!({ "stateMachineArn": missing }),
            );
            assert_eq!(
                refused.status, 400,
                "a read of a state machine that was never created is refused, got {} with {}",
                refused.status, refused.body
            );
            let refused_code = shape_name(&refused.body);
            assert_eq!(
                refused_code, "StateMachineDoesNotExist",
                "the model declares this refusal of DescribeStateMachine, got {}",
                refused.body
            );
            let refused_message = refused.body["message"]
                .as_str()
                .or_else(|| refused.body["Message"].as_str())
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                refused_message,
                format!("State Machine Does Not Exist: '{missing}'"),
                "the refusal states the ARN the request carried, got {}",
                refused.body
            );

            let read = emulator.sfn_ok(
                &session_key,
                "DescribeStateMachine",
                &json!({ "stateMachineArn": machine }),
            );
            assert_eq!(
                read.body["status"],
                json!("ACTIVE"),
                "the read reports the machine the create stored, got {}",
                read.body
            );

            // The start, found the way the check that filed this issue finds it.
            let starts = emulator.session_records("StartExecution", &session_arn);
            assert_eq!(
                starts.len(),
                1,
                "the session's start is one record, got {starts:?}"
            );
            let (start_envelope, start_document) = &starts[0];
            assert_sfn_record(
                "the start",
                start_envelope,
                start_document,
                "StartExecution",
                false,
                &account,
            );
            assert_session_attribution(
                "the start",
                start_envelope,
                start_document,
                &session_key,
                &session_arn,
                &caller_role,
                &execution_role,
            );
            assert_eq!(
                at(start_document, "/requestID").as_deref(),
                Some(started.request_id.as_str()),
                "the record names the request the caller was answered under, got \
                 {start_document}"
            );
            for (pointer, expected) in [
                ("/requestParameters/stateMachineArn", machine.as_str()),
                ("/requestParameters/name", EXECUTION),
                ("/responseElements/executionArn", execution_arn.as_str()),
            ] {
                assert_eq!(
                    at(start_document, pointer).as_deref(),
                    Some(expected),
                    "the start's record carries {pointer} as the call did, got {start_document}"
                );
            }
            for absent in ["errorCode", "errorMessage"] {
                assert_eq!(
                    start_document.get(absent),
                    None,
                    "a served call records no {absent}, got {start_document}"
                );
            }

            // Both reads, the refused one and the served one: a reporting call is recorded
            // exactly as the mutation is.
            let reads = emulator.session_records("DescribeStateMachine", &session_arn);
            assert_eq!(
                reads.len(),
                2,
                "the session's two reads are two records, got {reads:?}"
            );
            // The lookup reports the most recent event first, so the served read comes
            // before the refused one.
            let (served_envelope, served_document) = &reads[0];
            let (refused_envelope, refused_document) = &reads[1];
            assert_ne!(
                at(served_document, "/eventID"),
                at(refused_document, "/eventID"),
                "each served call is its own event, got {reads:?}"
            );

            // (case, the envelope, the record, the ARN the request named, the id the caller
            //  was answered under, the errorCode and errorMessage it must carry)
            for (case, envelope, document, named, request_id, error) in [
                (
                    "the served read",
                    served_envelope,
                    served_document,
                    machine.as_str(),
                    read.request_id.as_str(),
                    None,
                ),
                (
                    "the refused read",
                    refused_envelope,
                    refused_document,
                    missing.as_str(),
                    refused.request_id.as_str(),
                    Some((refused_code.as_str(), refused_message.as_str())),
                ),
            ] {
                assert_sfn_record(
                    case,
                    envelope,
                    document,
                    "DescribeStateMachine",
                    true,
                    &account,
                );
                assert_session_attribution(
                    case,
                    envelope,
                    document,
                    &session_key,
                    &session_arn,
                    &caller_role,
                    &execution_role,
                );
                assert_eq!(
                    at(document, "/requestID").as_deref(),
                    Some(request_id),
                    "case {case:?}: the record names the request the caller was answered \
                     under, got {document}"
                );
                assert_eq!(
                    document.get("requestParameters"),
                    Some(&json!({ "stateMachineArn": named })),
                    "case {case:?}: the record carries exactly the member the request \
                     named, got {document}"
                );
                assert_eq!(
                    document.get("responseElements"),
                    Some(&Value::Null),
                    "case {case:?}: a reporting call records no responseElements, got \
                     {document}"
                );
                assert_eq!(
                    at(document, "/errorCode").as_deref(),
                    error.map(|(code, _)| code),
                    "case {case:?}: expected errorCode {:?}, got {document}",
                    error.map(|(code, _)| code)
                );
                assert_eq!(
                    at(document, "/errorMessage").as_deref(),
                    error.map(|(_, message)| message),
                    "case {case:?}: the record repeats the message the caller saw, got \
                     {document}"
                );
            }

            // The setup's own create is in the same history, attributed to the long-term
            // credential rather than to the session: the recording is a property of serving
            // a call, not of the caller that made this one.
            let creates = emulator.lookup("EventName", "CreateStateMachine");
            let create = creates
                .first()
                .unwrap_or_else(|| panic!("the setup's create is recorded, got {creates:?}"));
            assert_sfn_record(
                "the setup's create",
                &create.0,
                &create.1,
                "CreateStateMachine",
                false,
                &account,
            );
        })
        .expect("spawn the Step Functions management-event thread")
        .join()
        .expect("a session's Step Functions calls must be audited in event history");
}
