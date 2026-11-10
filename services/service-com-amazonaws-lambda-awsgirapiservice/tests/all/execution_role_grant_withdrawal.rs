// SPDX-License-Identifier: CC0-1.0

//! WITHDRAWING a grant from a function's execution role, and whether the next call that
//! function's own SDK makes still gets it: Lambda's execution-role seam, IAM, the token
//! service, Amazon EventBridge and Amazon SQS on ONE engine.
//!
//! The RULE pinned here: a call made with the credential of a function's execution-role
//! session is decided against the policy set in force on the role AT THE INSTANT OF THE CALL,
//! so no decision survives a change to that set — however the change was made. Whichever IAM
//! write takes the grant away, the next call presenting the SAME, unexpired session credential
//! is refused `AccessDeniedException`; whichever IAM write puts it back, the call after that is
//! served again. Nothing is cached in either direction: not an allow (revoking a permission is
//! how an operator locks a function out of a service), and not a deny (restoring one has to
//! take effect just as quickly).
//!
//! The rows span the class rather than one withdrawal: they differ along HOW the grant is
//! taken off the role — the inline policy carrying it deleted, that policy rewritten without
//! it, the managed policy carrying it detached, and the operative version of that managed
//! policy moved to one that does not state it. Each is a different IAM record, and each is a
//! withdrawal AWS honours on the next call. Every row asserts all three phases with the same
//! credential — served while the grant stands, refused once it is withdrawn, served again once
//! it is restored — and both halves of each phase: what the call answered, and what the BUS did
//! with the event, read where a client reads it, off the queue the matching rule targets.
//!
//! What the neighbouring suites pin, and this one does not repeat: `handler_eventbridge_publication.rs`
//! drives the table of policy SHAPES a single document can take, and the EventBridge crate's
//! `handler_execution_role_publication.rs` runs a `python3.x` handler through this same seam, so
//! the running-handler half of the rule is already covered and no invocation is made here.

use std::sync::Arc;
use std::time::SystemTime;

use emulator_server::CredentialStore;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::execution_role_session::execution_role_identity;
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoding one awsQuery parameter value travels under, and the reader every
// suite in this target takes a value back out of an IAM answer with, both owned by the suite
// that first needed them rather than written out a second time.
use crate::event_source_mapping_role_access::{element, form_encode};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential: neither the token
/// service nor IAM holds a record of it, so it resolves to the account's ambient identity,
/// which is what every request this suite makes on its own behalf presents.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The execution role the function runs as, and the function's own name — the session a
/// handler's calls are made under is named for both.
const ROLE_NAME: &str = "lambda-grant-withdrawal";
const FUNCTION_NAME: &str = "publishes-until-revoked";

/// The name of the inline identity policy this suite writes on the execution role.
const INLINE_POLICY: &str = "inline";

/// The name of the CUSTOMER-managed policy this suite creates, whose attachment and whose
/// operative version are the other two carriers of the grant.
const MANAGED_POLICY: &str = "lambda-grant-withdrawal-publish";

/// The AWS-managed policy a Lambda execution role is created with: it grants the log writes a
/// function makes and nothing else, so a role carrying only it has no grant on EventBridge.
const BASIC_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// The bus every publication below names — the account's default one, which is the bus the
/// rule that carries this suite's control queue watches.
const BUS: &str = "default";

/// The rule that carries this suite's control queue, its target id, and the queue's name.
const RULE: &str = "receives-publications-until-revoked";
const TARGET_ID: &str = "t1";
const QUEUE: &str = "publications-until-revoked";

/// The source and detail type the publications carry, which is what the rule matches on.
const SOURCE: &str = "com.demo.grant-withdrawal";
const DETAIL_TYPE: &str = "Handler Published";

/// The action a publication onto a bus performs, as a statement names it and as a refusal
/// reports it.
const PUT_EVENTS_ACTION: &str = "events:PutEvents";

/// ANOTHER action of the same service, which the documents that do not carry the grant state
/// instead: a role short of the grant is then still a role holding a grant on the service, so a
/// refusal here is an implicit deny about this action and not an empty policy set.
const OTHER_EVENTS_ACTION: &str = "events:PutRule";

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

    /// One `PutEvents` presenting `access_key_id`: one entry onto the bus the rule watches,
    /// carrying `marker` as its detail. Answers the status and body, because a phase of every
    /// row below is refused.
    async fn put_events_as(&self, access_key_id: &str, marker: &str) -> (u16, Value) {
        self.rpc_as(
            access_key_id,
            "events",
            "AWSEvents",
            "1.1",
            "PutEvents",
            &json!({
                "Entries": [{
                    "EventBusName": BUS,
                    "Source": SOURCE,
                    "DetailType": DETAIL_TYPE,
                    "Detail": json!({"marker": marker}).to_string(),
                }],
            }),
        )
        .await
    }

    /// One awsQuery call to IAM, as the account's own credential; answers its status and body.
    async fn iam_call(&self, action: &str, params: &[(&str, String)]) -> (u16, String) {
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
        (status, String::from_utf8_lossy(response.body()).to_string())
    }

    /// One IAM call that must be served; answers its body.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) -> String {
        let (status, answered) = self.iam_call(action, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: IAM {action} must succeed, got {status} {answered}"
        );
        answered
    }

    /// One IAM call made to bring the role back to a known state, whose outcome is not the
    /// subject of any assertion: removing a policy the role does not carry is an error AWS
    /// answers and this suite does not care about.
    async fn iam_lenient(&self, action: &str, params: &[(&str, String)]) {
        let _ = self.iam_call(action, params).await;
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

    /// The customer-managed policy whose attachment and whose operative version are two of the
    /// carriers under test: its FIRST version states the grant, and a second version states
    /// another action of the service instead. Answers the ARN IAM minted for it and the id of
    /// each version, read off the answers rather than spelled out here.
    async fn managed_policy(&self, case: &str) -> ManagedPolicy {
        let created = self
            .iam(
                case,
                "CreatePolicy",
                &[
                    ("PolicyName", MANAGED_POLICY.to_string()),
                    ("PolicyDocument", document(json!([grant_statement()]))),
                ],
            )
            .await;
        let added = self
            .iam(
                case,
                "CreatePolicyVersion",
                &[
                    ("PolicyArn", element(&created, "Arn")),
                    ("PolicyDocument", document(json!([other_statement()]))),
                ],
            )
            .await;
        ManagedPolicy {
            arn: element(&created, "Arn"),
            granting_version: element(&created, "DefaultVersionId"),
            other_version: element(&added, "VersionId"),
        }
    }

    /// Make the one change `change` names to what the role's policies say, with IAM's own
    /// operations.
    async fn apply(&self, case: &str, change: Change, policy: &ManagedPolicy) {
        let role = ("RoleName", ROLE_NAME.to_string());
        match change {
            Change::InlineGrants | Change::InlineWithoutGrant => {
                let statements = if change == Change::InlineGrants {
                    json!([grant_statement()])
                } else {
                    json!([other_statement()])
                };
                self.iam(
                    case,
                    "PutRolePolicy",
                    &[
                        role,
                        ("PolicyName", INLINE_POLICY.to_string()),
                        ("PolicyDocument", document(statements)),
                    ],
                )
                .await;
            }
            Change::InlineDeleted => {
                self.iam(
                    case,
                    "DeleteRolePolicy",
                    &[role, ("PolicyName", INLINE_POLICY.to_string())],
                )
                .await;
            }
            Change::Attached => {
                self.iam(
                    case,
                    "AttachRolePolicy",
                    &[role, ("PolicyArn", policy.arn.clone())],
                )
                .await;
            }
            Change::Detached => {
                self.iam(
                    case,
                    "DetachRolePolicy",
                    &[role, ("PolicyArn", policy.arn.clone())],
                )
                .await;
            }
            Change::OperativeGrants | Change::OperativeWithoutGrant => {
                let version = if change == Change::OperativeGrants {
                    policy.granting_version.clone()
                } else {
                    policy.other_version.clone()
                };
                self.iam(
                    case,
                    "SetDefaultPolicyVersion",
                    &[
                        ("PolicyArn", policy.arn.clone()),
                        ("VersionId", version),
                    ],
                )
                .await;
            }
        }
    }

    /// Bring the role back to carrying nothing that grants the publication: no inline policy,
    /// the managed policy detached, and its operative version the one that states the grant —
    /// so each row starts from the same state and its own carrier is the only one in play.
    async fn ungranted(&self, case: &str, policy: &ManagedPolicy) {
        self.iam_lenient(
            "DeleteRolePolicy",
            &[
                ("RoleName", ROLE_NAME.to_string()),
                ("PolicyName", INLINE_POLICY.to_string()),
            ],
        )
        .await;
        self.iam_lenient(
            "DetachRolePolicy",
            &[
                ("RoleName", ROLE_NAME.to_string()),
                ("PolicyArn", policy.arn.clone()),
            ],
        )
        .await;
        self.apply(case, Change::OperativeGrants, policy).await;
    }

    /// The queue the rule targets, carrying the policy EventBridge's delivery needs, and the
    /// rule that watches the default bus for this suite's own events; answers the queue's url.
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
        // The rule that carries the queue: it matches exactly the events this suite publishes,
        // so a message on the queue is one of them and nothing else.
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
    ///
    /// Each message read is DELETED, so what a later phase reads is what that phase's own
    /// publication put there and never a redelivery of an earlier one.
    async fn drained_markers(&self, case: &str, url: &str) -> Vec<String> {
        let received = self
            .sqs(
                case,
                "ReceiveMessage",
                &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
            )
            .await;
        let messages = received["Messages"].as_array().cloned().unwrap_or_default();
        for message in &messages {
            let receipt = message["ReceiptHandle"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: no ReceiptHandle on the message {message}")
            });
            self.sqs(
                case,
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": receipt }),
            )
            .await;
        }
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
    /// (`state::execution::execution_role_session`, which `function_sdk_access` hands the run) —
    /// so what the phases below present is the credential a handler's call really carries.
    fn invocation_credential(&self, role_arn: &str) -> String {
        let identity = execution_role_identity(
            &*self.engine,
            &CredentialStore::default(),
            role_arn,
            FUNCTION_NAME,
            SystemTime::now(),
        )
        .expect("a session is minted for a role IAM holds");
        identity.access_key_id
    }
}

/// The customer-managed policy two of the rows carry the grant with.
struct ManagedPolicy {
    /// The ARN IAM minted for it, which an attachment names it by.
    arn: String,
    /// The version whose document states the grant — the one it was created with.
    granting_version: String,
    /// The version whose document states another action of the service instead.
    other_version: String,
}

/// One change to what the execution role's policies say, each made with the IAM operation its
/// name states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    /// `PutRolePolicy`: the role's inline policy states the grant.
    InlineGrants,
    /// `PutRolePolicy`: the same inline policy, rewritten to state another action instead.
    InlineWithoutGrant,
    /// `DeleteRolePolicy`: the role carries no inline policy at all.
    InlineDeleted,
    /// `AttachRolePolicy`: the managed policy whose operative version states the grant.
    Attached,
    /// `DetachRolePolicy`: that policy, no longer attached.
    Detached,
    /// `SetDefaultPolicyVersion`: the operative version of that policy is the one that states
    /// the grant.
    OperativeGrants,
    /// `SetDefaultPolicyVersion`: the operative version is the one that does not.
    OperativeWithoutGrant,
}

/// One row: a carrier of the grant, the ONE call that withdraws it, and the one that puts it
/// back.
struct Row {
    case: &'static str,
    /// What puts the grant on the role, applied from the ungranted state.
    granting: Change,
    /// The withdrawal under test.
    withdrawal: Change,
    /// The restoration that has to take effect just as immediately.
    restoration: Change,
}

/// The rows, differing along HOW the grant is withdrawn: the inline policy carrying it deleted,
/// that policy rewritten without it, the managed policy carrying it detached, and the operative
/// version of that managed policy moved to one that does not state it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the inline policy carrying the grant deleted",
            granting: Change::InlineGrants,
            withdrawal: Change::InlineDeleted,
            restoration: Change::InlineGrants,
        },
        Row {
            case: "the inline policy rewritten without the grant",
            granting: Change::InlineGrants,
            withdrawal: Change::InlineWithoutGrant,
            restoration: Change::InlineGrants,
        },
        Row {
            case: "the managed policy carrying the grant detached",
            granting: Change::Attached,
            withdrawal: Change::Detached,
            restoration: Change::Attached,
        },
        Row {
            case: "the operative version of the managed policy moved off the grant",
            granting: Change::Attached,
            withdrawal: Change::OperativeWithoutGrant,
            restoration: Change::OperativeGrants,
        },
    ]
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

/// The ARN of the event bus every publication here names, in the Region every call is signed
/// for.
fn bus_arn() -> String {
    format!(
        "arn:aws:events:{REGION}:{}:event-bus/{BUS}",
        engine::emulator_account_id()
    )
}

/// The statement that grants the publication, scoped to the bus the entries name.
fn grant_statement() -> Value {
    json!({"Effect": "Allow", "Action": PUT_EVENTS_ACTION, "Resource": bus_arn()})
}

/// The statement a document that does NOT carry the grant states instead.
fn other_statement() -> Value {
    json!({"Effect": "Allow", "Action": OTHER_EVENTS_ACTION, "Resource": "*"})
}

/// One identity policy document carrying `statements`.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
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

/// Assert that one publication was SERVED, and that the event reached the rule's queue.
fn assert_served(phase: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {phase:?}: expected the publication to be served, got {status} with {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {phase:?}: expected no failed entry, got {body}"
    );
    assert!(
        body["Entries"][0]["EventId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "case {phase:?}: expected an EventId on the accepted entry, got {body}"
    );
}

/// Assert that one publication was REFUSED, with the refusal AWS words for a call no
/// identity-based policy allows: the status, the code, and the sentence naming the principal the
/// call spoke for, the action and the bus the entry named.
fn assert_refused(phase: &str, status: u16, body: &Value) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (ACCESS_DENIED_STATUS, ACCESS_DENIED),
        "case {phase:?}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED}, got {status} with \
         {body}"
    );
    let expected = format!(
        "User: {} is not authorized to perform: {PUT_EVENTS_ACTION} on resource: {} because no \
         identity-based policy allows the {PUT_EVENTS_ACTION} action",
        session_arn(),
        bus_arn()
    );
    assert_eq!(
        message(body),
        expected,
        "case {phase:?}: expected the refusal {expected:?}, got {body}"
    );
}

/// RULE: a call made with the credential of a function's execution-role session is decided
/// against the policy set in force on the role at the instant of the call, so a grant withdrawn
/// by ANY of the IAM writes that withdraw one stops being honoured on the very next call —
/// refused `AccessDeniedException` naming the session — and a grant restored is honoured again
/// at once, with the same credential throughout.
#[tokio::test]
async fn a_withdrawn_grant_is_refused_on_the_next_call_the_execution_role_makes() {
    let emulator = Emulator::new();
    let role = emulator.execution_role("setup").await;
    let policy = emulator.managed_policy("setup").await;
    let queue = emulator.control_queue("setup").await;
    // The credential a handler's own call carries, minted ONCE — as an execution environment's
    // is — and presented by every publication below, so what changes between them is only what
    // the role's policies say.
    let credential = emulator.invocation_credential(&role);

    for (at, row) in rows().iter().enumerate() {
        emulator.ungranted(row.case, &policy).await;
        // (the phase, the change made before it, whether the publication must be served)
        let phases = [
            ("while the grant stands", row.granting, true),
            ("once the grant is withdrawn", row.withdrawal, false),
            ("once the grant is restored", row.restoration, true),
        ];
        for (phase, change, served) in phases {
            let case = format!("{}: {phase}", row.case);
            emulator.apply(&case, change, &policy).await;
            let marker = format!("marker-{at}-{}", phase.replace(' ', "-"));
            let (status, body) = emulator.put_events_as(&credential, &marker).await;
            // What the publication ANSWERED, and what the bus did with the event: a call that
            // answered without publishing, or published without answering, fails here.
            let expected: Vec<String> = if served {
                assert_served(&case, status, &body);
                vec![marker]
            } else {
                assert_refused(&case, status, &body);
                Vec::new()
            };
            let delivered = emulator.drained_markers(&case, &queue).await;
            assert_eq!(
                delivered, expected,
                "case {case:?}: expected the markers {expected:?} on the rule's queue, got \
                 {delivered:?}"
            );
        }
    }
}
