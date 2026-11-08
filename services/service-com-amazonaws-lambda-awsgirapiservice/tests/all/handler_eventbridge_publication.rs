// SPDX-License-Identifier: CC0-1.0

//! WHO a running handler's own `PutEvents` is made as, and what EventBridge therefore does
//! with it: Lambda, Amazon EventBridge, Amazon SQS, IAM and the token service on ONE engine,
//! the credential an invocation of a function is signed with minted through the very seam a
//! run's SDK gets (`state::execution::execution_role_session`), and the publication it makes
//! read back where a client reads it — off the queue the matching rule targets.
//!
//! The RULE pinned here: a call a handler makes with the SDK its runtime supplies is signed as
//! a SESSION OF THE FUNCTION'S EXECUTION ROLE, and a session of a role is resolvable to that
//! role by every service that authorizes a request — so the service serving the call judges it
//! against the policies in force on the role at the instant of the call, serves a publication
//! those policies grant, and refuses one they do not with its own access-denied refusal naming
//! the session as the principal. Before this held, such a call reached a service as a
//! credential no store could place and was served as the account's own, so an execution role
//! granted nothing could still publish.
//!
//! The rows span the class rather than the one recorded case: they differ along WHAT the
//! role's policies say about the publication each makes — this action granted on the bus the
//! entry names, every action of the service granted on it, no statement about it at all, and a
//! grant over it together with an explicit deny — which is also to say along WHICH BUS each
//! publication names, because a statement's `Resource` is what scopes a grant to one bus. Each
//! row asserts both halves — what the publication answered, and what the BUS did with the
//! event, read through the rule's own SQS target — so a call that answered without publishing,
//! or published without answering, fails here.
//!
//! A last publication runs with the role's own policy DELETED, leaving the AWS-managed policy
//! a Lambda execution role is created with: the publication that was served is then refused, so
//! the grant is read as it stands when the call is made rather than as it stood when the
//! credential was minted.
//!
//! The RUNNING handler's half of this rule — a `python3.x` function whose code does
//! `import boto3` and calls `PutEvents`, whose payload is what the SDK gave the guest — is the
//! EventBridge crate's `handler_execution_role_publication.rs`, which mounts this service
//! beside the one that decides the publication. What this suite adds is the TABLE of policy
//! shapes, driven over the wire with the credential Lambda mints for the function, so the
//! decision is read on every shape without a run per row.

use std::sync::Arc;
use std::time::SystemTime;

use emulator_server::CredentialStore;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::execution_role_session::execution_role_identity;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential: neither the token
/// service nor IAM holds a record of it, so it resolves to the account's ambient identity,
/// which is what every request this suite makes on its own behalf presents.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The execution role the function runs as, and the function's own name — the session a
/// handler's calls are made under is named for both.
const ROLE_NAME: &str = "lambda-event-publisher";
const FUNCTION_NAME: &str = "publishes-events";

/// The name of the inline identity policy this suite writes on the execution role.
const INLINE_POLICY: &str = "inline";

/// The AWS-managed policy a Lambda execution role is created with: it grants the log writes a
/// function makes and nothing else, so a role carrying only it has no grant on any other
/// service.
const BASIC_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// The bus the rule below watches — the account's default one — and the three other buses the
/// account holds, one per row of the table: the one a grant over every action of the service
/// names, the one no statement names, and the one a statement explicitly denies. No rule
/// watches any of the three.
const BUS: &str = "default";
const WILDCARD_GRANT_BUS: &str = "service-wildcard-bus";
const UNGRANTED_BUS: &str = "ungranted-bus";
const DENIED_BUS: &str = "denied-bus";

/// The rule that carries this suite's control queue, its target id, and the queue's name.
const RULE: &str = "receives-handler-publications";
const TARGET_ID: &str = "t1";
const QUEUE: &str = "handler-publications";

/// The source and detail type the publications carry, which is what the rule matches on.
const SOURCE: &str = "com.demo.handler-publications";
const DETAIL_TYPE: &str = "Handler Published";

/// The action a publication onto a bus performs, as a statement names it and as a refusal
/// reports it.
const PUT_EVENTS_ACTION: &str = "events:PutEvents";

/// The code and status EventBridge refuses an unauthorized publication with.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 400;

/// The service principal EventBridge delivers to a queue as, which the queue's own policy has
/// to grant: "for Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge
/// relies on resource-based policies" (`PutTargets`).
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// EventBridge, Amazon SQS, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them, with THIS service's execution-role seam reaching the
/// same engine: the session it mints is the one the services below resolve.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// A SigV4 `Authorization` whose credential scope names `service` and whose access key id
    /// is `access_key_id`, which is what selects the principal the request speaks for. The
    /// signature is never checked here.
    fn authorization(service: &str, access_key_id: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{service}/\
             aws4_request, SignedHeaders=host, Signature=not-verified"
        )
    }

    /// One `awsJson` RPC call against the service whose signing name is `signing_name` — the
    /// credential scope is what routes it — presenting `access_key_id`.
    async fn rpc_as(
        &self,
        access_key_id: &str,
        signing_name: &str,
        target_prefix: &str,
        json_version: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header(
                "content-type",
                format!("application/x-amz-json-{json_version}"),
            )
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                Self::authorization(signing_name, access_key_id),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed: Value = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&payload).to_string()))
        };
        (status, parsed)
    }

    /// One EventBridge call that must be served, as the account's own credential.
    async fn events(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answered) = self
            .rpc_as(ACCOUNT_KEY, "events", "AWSEvents", "1.1", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {answered}"
        );
        answered
    }

    /// One Amazon SQS call that must be served, as the account's own credential.
    async fn sqs(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answered) = self
            .rpc_as(ACCOUNT_KEY, "sqs", "AmazonSQS", "1.0", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {answered}"
        );
        answered
    }

    /// One `PutEvents` presenting `access_key_id`: one entry onto `bus`, carrying `marker` as
    /// its detail. Answers the status and body, because a row of the table below is refused.
    async fn put_events_as(
        &self,
        access_key_id: &str,
        bus: &str,
        marker: &str,
    ) -> (u16, Value) {
        self.rpc_as(
            access_key_id,
            "events",
            "AWSEvents",
            "1.1",
            "PutEvents",
            &json!({
                "Entries": [{
                    "EventBusName": bus,
                    "Source": SOURCE,
                    "DetailType": DETAIL_TYPE,
                    "Detail": json!({"marker": marker}).to_string(),
                }],
            }),
        )
        .await
    }

    /// One awsQuery call to IAM, as the account's own credential, which must be served.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://iam.amazonaws.com/")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", Self::authorization("iam", ACCOUNT_KEY))
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: IAM {action} must succeed, got {status} {answered}"
        );
    }

    /// Create the execution role, trusted by Lambda and carrying the AWS-managed policy a
    /// Lambda execution role is created with — written with IAM's own operations, so the role
    /// a session is minted of is the record `iam:CreateRole` wrote.
    async fn execution_role(&self, case: &str) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": "lambda.amazonaws.com"},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.iam(
            case,
            "CreateRole",
            &[
                ("RoleName", ROLE_NAME.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        self.iam(
            case,
            "AttachRolePolicy",
            &[
                ("RoleName", ROLE_NAME.to_string()),
                ("PolicyArn", BASIC_EXECUTION_POLICY.to_string()),
            ],
        )
        .await;
        role_arn()
    }

    /// The inline identity policy in force on the execution role from now on: `document`, or
    /// none at all.
    async fn role_policy(&self, case: &str, document: Option<&str>) {
        match document {
            Some(document) => {
                self.iam(
                    case,
                    "PutRolePolicy",
                    &[
                        ("RoleName", ROLE_NAME.to_string()),
                        ("PolicyName", INLINE_POLICY.to_string()),
                        ("PolicyDocument", document.to_string()),
                    ],
                )
                .await;
            }
            None => {
                self.iam(
                    case,
                    "DeleteRolePolicy",
                    &[
                        ("RoleName", ROLE_NAME.to_string()),
                        ("PolicyName", INLINE_POLICY.to_string()),
                    ],
                )
                .await;
            }
        }
    }

    /// The other buses the account holds, which the rows' statements name and no rule watches.
    async fn other_buses(&self, case: &str) {
        for name in [WILDCARD_GRANT_BUS, UNGRANTED_BUS, DENIED_BUS] {
            self.events(case, "CreateEventBus", &json!({ "Name": name }))
                .await;
        }
    }

    /// The queue the rule targets, carrying the policy EventBridge's delivery needs, and the
    /// rule that watches the default bus for the suite's own event; answers the queue's url.
    async fn control_queue(&self, case: &str) -> String {
        let created = self
            .sqs(case, "CreateQueue", &json!({ "QueueName": QUEUE }))
            .await;
        let url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
            .to_string();
        let attributes = self
            .sqs(
                case,
                "GetQueueAttributes",
                &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
            )
            .await;
        let arn = attributes["Attributes"]["QueueArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {attributes}"))
            .to_string();
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": "AllowEventBridge",
                "Effect": "Allow",
                "Principal": {"Service": EVENTS_PRINCIPAL},
                "Action": "sqs:SendMessage",
                "Resource": arn,
            }],
        })
        .to_string();
        self.sqs(
            case,
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
        )
        .await;
        // The rule that carries the queue: it matches exactly the events this suite
        // publishes, so a message on the queue is one of them and nothing else.
        let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
        self.events(
            case,
            "PutRule",
            &json!({ "Name": RULE, "EventPattern": pattern, "State": "ENABLED" }),
        )
        .await;
        let targets = self
            .events(
                case,
                "PutTargets",
                &json!({
                    "Rule": RULE,
                    "Targets": [{ "Id": TARGET_ID, "Arn": arn }],
                }),
            )
            .await;
        assert_eq!(
            targets["FailedEntryCount"],
            json!(0),
            "case {case:?}: PutTargets reported a failed entry: {targets}"
        );
        url
    }

    /// The `detail.marker` of every message waiting on the queue at `url`, taken off it and
    /// sorted.
    async fn drained_markers(&self, case: &str, url: &str) -> Vec<String> {
        let received = self
            .sqs(
                case,
                "ReceiveMessage",
                &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
            )
            .await;
        let messages = received["Messages"].as_array().cloned().unwrap_or_default();
        let mut markers: Vec<String> = messages
            .iter()
            .map(|message| {
                let text = message["Body"].as_str().unwrap_or_default();
                let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
                assert_eq!(
                    body["source"].as_str().unwrap_or_default(),
                    SOURCE,
                    "case {case:?}: expected this suite's own source on the queue, got {body}"
                );
                body["detail"]["marker"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        markers.sort();
        markers
    }

    /// The access key id an invocation of [`FUNCTION_NAME`] running as the execution role makes
    /// its own SDK calls with, minted through the seam a run's SDK is built from
    /// (`state::execution::execution_role_session`, which `function_sdk_access` hands the
    /// run) — so what the rows below present is the credential a handler's call really carries.
    fn invocation_credential(&self, role_arn: &str) -> String {
        let identity = execution_role_identity(
            &*self.engine,
            &CredentialStore::default(),
            role_arn,
            FUNCTION_NAME,
            SystemTime::now(),
        )
        .expect("a session is minted for a role IAM holds");
        identity.access_key_id.clone()
    }
}

/// The ARN of the execution role, as IAM holds it.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{ROLE_NAME}",
        engine::emulator_account_id()
    )
}

/// The assumed-role session ARN a call made by an invocation of the function speaks for: the
/// session a function's calls are made under is named for the FUNCTION.
fn session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE_NAME}/{FUNCTION_NAME}",
        engine::emulator_account_id()
    )
}

/// The ARN of the event bus `name` in the Region every call here is signed for.
fn bus_arn(name: &str) -> String {
    format!(
        "arn:aws:events:{REGION}:{}:event-bus/{name}",
        engine::emulator_account_id()
    )
}

/// One identity policy document carrying `statements`.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// `application/x-www-form-urlencoded` for one awsQuery parameter value.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// One row of the table: the bus the publication names, and the refusal that must come of
/// publishing onto it under the one policy below.
///
/// Whether the event must reach the rule's queue follows from those two: a served publication
/// onto the bus the rule watches reaches it, and nothing else does.
struct Row {
    case: &'static str,
    /// The bus the entry names.
    bus: &'static str,
    /// The reason clause the refusal must end in, or `None` when the publication is served.
    refusal: Option<String>,
}

/// The sentence clause EventBridge ends a refusal with when nothing in force grants the
/// publication.
fn implicit_deny() -> Option<String> {
    Some(format!(
        "because no identity-based policy allows the {PUT_EVENTS_ACTION} action"
    ))
}

/// The clause it ends one with when a document in force explicitly denies it.
fn explicit_deny() -> Option<String> {
    Some("with an explicit deny in an identity-based policy".to_string())
}

/// The rows, differing along what the role's policies say about the publication: the action
/// granted on the bus the entry names, every action of the service granted on it, no statement
/// about it at all, and a grant over it together with an explicit deny.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the action granted on the bus the entry names",
            bus: BUS,
            refusal: None,
        },
        Row {
            case: "every action of the service granted on the bus the entry names",
            bus: WILDCARD_GRANT_BUS,
            refusal: None,
        },
        Row {
            case: "no statement about the bus the entry names",
            bus: UNGRANTED_BUS,
            refusal: implicit_deny(),
        },
        Row {
            case: "the bus the entry names granted and explicitly denied",
            bus: DENIED_BUS,
            refusal: explicit_deny(),
        },
    ]
}

/// The ONE inline identity policy the rows are decided against: a statement per row, scoped by
/// `Resource` to the bus that row's publication names.
///
/// The grant of ANOTHER action of the service over every resource is here too, so the row whose
/// bus no `events:PutEvents` statement names is refused while the role does hold a grant on the
/// service — an implicit deny rather than a role that holds nothing at all, which is the state
/// the last publication of this suite is made under.
fn grant_document() -> String {
    document(json!([
        {"Effect": "Allow", "Action": PUT_EVENTS_ACTION, "Resource": bus_arn(BUS)},
        {"Effect": "Allow", "Action": "events:*", "Resource": bus_arn(WILDCARD_GRANT_BUS)},
        {"Effect": "Allow", "Action": "events:PutRule", "Resource": "*"},
        {"Effect": "Allow", "Action": PUT_EVENTS_ACTION, "Resource": bus_arn(DENIED_BUS)},
        {"Effect": "Deny", "Action": PUT_EVENTS_ACTION, "Resource": bus_arn(DENIED_BUS)},
    ]))
}

/// The error code a refusal body names, under either spelling an AWS JSON answer carries.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .or_else(|| body.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` a refusal body carries.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert what one publication came to, against its row.
fn assert_answer(case: &str, bus: &str, refusal: Option<&str>, status: u16, body: &Value) {
    match refusal {
        Some(reason) => {
            assert_eq!(
                (status, error_code(body).as_str()),
                (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                "case {case:?} onto {bus}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED}, got \
                 {status} with {body}"
            );
            // The refusal names the principal the call spoke for — the session of the
            // function's execution role, named for the function — and the bus the entry named.
            let expected = format!(
                "User: {} is not authorized to perform: {PUT_EVENTS_ACTION} on resource: {} \
                 {reason}",
                session_arn(),
                bus_arn(bus)
            );
            assert_eq!(
                message(body),
                expected,
                "case {case:?} onto {bus}: expected the refusal {expected:?}, got {body}"
            );
        }
        None => {
            assert_eq!(
                status, 200,
                "case {case:?} onto {bus}: expected the publication to be served, got {status} \
                 with {body}"
            );
            assert_eq!(
                body["FailedEntryCount"],
                json!(0),
                "case {case:?} onto {bus}: expected no failed entry, got {body}"
            );
            assert!(
                body["Entries"][0]["EventId"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty()),
                "case {case:?} onto {bus}: expected an EventId on the accepted entry, got {body}"
            );
        }
    }
}

/// RULE: the call a handler makes with the SDK its runtime supplies is signed as a session of
/// the function's execution role, and EventBridge resolves that session to its role — so the
/// publication is judged against the policies in force on the role, served where they grant
/// the action on the bus the entry names and refused with `AccessDeniedException` naming the
/// session where they do not.
#[tokio::test]
async fn a_publication_made_with_an_invocations_credential_is_judged_against_its_role() {
    let emulator = Emulator::new();
    let role = emulator.execution_role("setup").await;
    emulator.other_buses("setup").await;
    let queue = emulator.control_queue("setup").await;
    emulator.role_policy("setup", Some(&grant_document())).await;
    // The credential a handler's own call carries, minted once per execution environment
    // exactly as an invocation's seam mints it.
    let credential = emulator.invocation_credential(&role);

    let rows = rows();
    let mut expected: Vec<String> = Vec::new();
    for (at, row) in rows.iter().enumerate() {
        let case = row.case;
        let marker = format!("marker-{at}");
        let (status, body) = emulator
            .put_events_as(&credential, row.bus, &marker)
            .await;
        assert_answer(case, row.bus, row.refusal.as_deref(), status, &body);
        if row.refusal.is_none() && row.bus == BUS {
            expected.push(marker);
        }
    }
    // What the publications actually did, read where a client reads them: a served publication
    // onto the bus the rule watches puts the event on that rule's queue, and a refused one — or
    // one onto a bus no rule watches — puts nothing.
    expected.sort();
    let delivered = emulator.drained_markers("the granted role", &queue).await;
    assert_eq!(
        delivered, expected,
        "the granted role: expected the markers {expected:?} on the rule's queue, got \
         {delivered:?}"
    );

    // The grant is read as it stands NOW: with the role's own policy deleted — leaving the
    // AWS-managed policy it was created with, which grants no action of this service — the
    // publication that was served is refused, with the SAME credential, and nothing reaches the
    // queue.
    let case = "the role carrying only the AWS-managed policy it was created with";
    emulator.role_policy(case, None).await;
    let (status, body) = emulator
        .put_events_as(&credential, BUS, "marker-withdrawn")
        .await;
    assert_answer(case, BUS, implicit_deny().as_deref(), status, &body);
    let delivered = emulator.drained_markers(case, &queue).await;
    assert!(
        delivered.is_empty(),
        "case {case:?}: expected nothing on the rule's queue, got {delivered:?}"
    );
}
