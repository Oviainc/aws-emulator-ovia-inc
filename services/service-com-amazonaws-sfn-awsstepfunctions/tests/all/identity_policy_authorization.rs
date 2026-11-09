// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IAM authorization every call to this service is subject to,
//! driven over each service's own wire protocol through the production
//! `MultiServiceRouter` with Step Functions, IAM and STS mounted on ONE engine — the shape
//! `core/router/src/lib.rs` mounts them in — so the policies a decision reads are the
//! documents IAM's own handlers wrote and the session a call signs as is the one STS's own
//! `AssumeRole` minted.
//!
//! The rule under test: every operation of this API is a signed call, and Step Functions
//! decides it at the IAM layer BEFORE it is dispatched — served only where the
//! identity-based documents in force on the SIGNING principal allow
//! `states:<OperationName>` on the ARN the request names, and otherwise refused with HTTP
//! 400 `AccessDeniedException` carrying the one sentence AWS words an authorization
//! failure with. The decision is taken per request, so a statement just revoked refuses
//! the next call and one put back serves it again, and a refused call reads and writes
//! nothing.
//!
//! The rows differ along what the rule generalises over: the OPERATION (a start, a read of
//! one execution, a read of its history, a list of a machine's executions, a read of the
//! state machine, a listing that addresses no single resource, a read of a resource's
//! tags, and a create judged against the ARN its name mints), the KIND of principal (an
//! assumed-role session — the identity a Lambda function's own calls are made as — an IAM
//! user, and the account's own published credential, which no identity policy governs),
//! what the documents say (every action of this service, nothing of this service, an
//! explicit deny beside a wildcard allow), and — for the resource half of the decision —
//! what the grant's `Resource` element names (one state machine, another state machine,
//! both, every state machine of the Region, every resource).
//!
//! The catalog the gate reads a request's resource through is re-derived from this crate's
//! own Smithy model below, so a model that grows an operation, or renames the member one
//! addresses its resource by, fails here rather than silently deciding that call against
//! every resource.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::service::identity_authorization;
use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::execution_data_encryption::{form_encode, xml_element, SFN};
use crate::state_machine_lifecycle::{
    error_code, string, ACCOUNT, HELLO_DEFINITION, PAST_THE_EXECUTION_DELAY, REGION, ROLE_ARN,
};

/// This crate's Smithy model — the source the catalog in `service::identity_authorization`
/// is a transcription of, read back here so the two cannot drift.
const MODEL_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-sfn-awsstepfunctions.json"
);

/// The code and status AWS refuses an authorization failure with.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 400;

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record of it, so it resolves to the account's ambient identity, which no
/// identity policy governs — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The API versions the IAM and STS setup calls below are made at.
const IAM_VERSION: &str = "2010-05-08";
const STS_VERSION: &str = "2011-06-15";

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// Step Functions, IAM and STS over ONE engine, so the documents a decision reads are the
/// ones IAM's handlers wrote and the session a call signs as is one STS minted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call one Step Functions operation with `access_key_id` in the SigV4 credential
    /// scope, and answer `(status, body)`.
    async fn call_as(&self, access_key_id: &str, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{}/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef",
            SFN.signing_name
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{}.{REGION}.amazonaws.com", SFN.signing_name))
            .header(
                "content-type",
                format!("application/x-amz-json-{}", SFN.json_version),
            )
            .header("x-amz-target", format!("{}.{operation}", SFN.target_prefix))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
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
        (status, parsed)
    }

    /// One Step Functions call as the account's own credential, requiring a 200: the setup,
    /// and the readings a row checks its own effect with.
    async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call_as(ACCOUNT_KEY, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation}: the setup call must succeed, got {status} with {parsed:?}"
        );
        parsed
    }

    /// One awsQuery call to IAM or STS, signed with the account's own credential.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> String {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {action:?}: the setup call must succeed, got {status} with {text}"
        );
        text
    }

    /// An IAM user carrying `policy` as its inline policy, with a long-term access key of
    /// its own.
    async fn user(&self, user_name: &str, policy: &Value) -> Principal {
        let created = self
            .query(
                "iam",
                IAM_VERSION,
                "CreateUser",
                &[("UserName", user_name.to_string())],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser answers the user's Arn, got {created}"));
        self.query(
            "iam",
            IAM_VERSION,
            "PutUserPolicy",
            &[
                ("UserName", user_name.to_string()),
                ("PolicyName", POLICY_NAME.to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
        let minted = self
            .query(
                "iam",
                IAM_VERSION,
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        Principal {
            access_key_id: xml_element(&minted, "AccessKeyId")
                .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}")),
            arn,
        }
    }

    /// An IAM role carrying `policy` as its inline policy, assumed through STS: the
    /// session's access key id and the session ARN — the identity a function's own calls
    /// are made as.
    async fn assumed_role(&self, role_name: &str, policy: &Value) -> Principal {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
                "Action": "sts:AssumeRole",
            }]
        });
        self.query(
            "iam",
            IAM_VERSION,
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust.to_string()),
            ],
        )
        .await;
        self.put_role_policy(role_name, policy).await;
        let assumed = self
            .query(
                "sts",
                STS_VERSION,
                "AssumeRole",
                &[
                    ("RoleArn", format!("arn:aws:iam::{ACCOUNT}:role/{role_name}")),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        Principal {
            access_key_id: xml_element(&assumed, "AccessKeyId")
                .unwrap_or_else(|| panic!("AssumeRole answers an AccessKeyId, got {assumed}")),
            arn: xml_element(&assumed, "Arn")
                .unwrap_or_else(|| panic!("AssumeRole answers the session's Arn, got {assumed}")),
        }
    }

    /// Replace the inline policy of the role `role_name`, which is how a grant is put in
    /// place and withdrawn again.
    async fn put_role_policy(&self, role_name: &str, policy: &Value) {
        self.query(
            "iam",
            IAM_VERSION,
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", POLICY_NAME.to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    }

    /// Delete the role's inline policy altogether, leaving a role IAM holds with no
    /// document at all.
    async fn delete_role_policy(&self, role_name: &str) {
        self.query(
            "iam",
            IAM_VERSION,
            "DeleteRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", POLICY_NAME.to_string()),
            ],
        )
        .await;
    }

    /// A state machine named `name`, as its ARN.
    async fn machine(&self, name: &str) -> String {
        let created = self
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": HELLO_DEFINITION,
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        string(&created, "stateMachineArn")
    }

    /// One execution of `machine`, as its ARN, with the outcome the clock owed it already
    /// due.
    async fn execution(&self, machine: &str, name: &str) -> String {
        let started = self
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": name }),
            )
            .await;
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        string(&started, "executionArn")
    }

    /// How many executions `machine` holds, read as the account.
    async fn execution_count(&self, machine: &str) -> usize {
        let listed = self
            .sfn("ListExecutions", &json!({ "stateMachineArn": machine }))
            .await;
        listed
            .get("executions")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    }
}

/// The inline-policy name every document below is written under.
const POLICY_NAME: &str = "inline";

/// An identity policy allowing `actions` on `resources`.
fn allowing(actions: &[&str], resources: &[&str]) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resources }]
    })
}

/// The sentence AWS states an authorization failure with: who was refused, which action,
/// on which resource, and which of IAM's rules refused it.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// The clause a request no document in force allows is refused with.
fn no_grant(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The message a refusal carries, whichever casing the envelope spells it in.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The ARN of the activity `name` in the Region every call below is signed for.
fn activity_arn(name: &str) -> String {
    format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:{name}")
}

/// One call the rule covers: the operation, the request, and the ARN that request
/// addresses — the resource the decision about it is made on.
struct Row {
    operation: &'static str,
    case: &'static str,
    body: Value,
    resource: String,
}

/// Every call the rows below are taken over, each addressing its resource through a
/// different member of the model: an execution ARN, a state machine ARN, a `resourceArn`,
/// the name a create mints its resource under, and a request that names no resource at
/// all.
fn rows(index: usize, machine: &str, execution: &str) -> Vec<Row> {
    vec![
        Row {
            case: "a start of an execution",
            operation: "StartExecution",
            body: json!({ "stateMachineArn": machine, "name": format!("run-{index}") }),
            resource: machine.to_string(),
        },
        Row {
            case: "a read of one execution",
            operation: "DescribeExecution",
            body: json!({ "executionArn": execution }),
            resource: execution.to_string(),
        },
        Row {
            case: "a read of one execution's history",
            operation: "GetExecutionHistory",
            body: json!({ "executionArn": execution }),
            resource: execution.to_string(),
        },
        Row {
            case: "a list of a state machine's executions",
            operation: "ListExecutions",
            body: json!({ "stateMachineArn": machine }),
            resource: machine.to_string(),
        },
        Row {
            case: "a read of the state machine",
            operation: "DescribeStateMachine",
            body: json!({ "stateMachineArn": machine }),
            resource: machine.to_string(),
        },
        Row {
            case: "a listing that addresses no single resource",
            operation: "ListStateMachines",
            body: json!({}),
            resource: "*".to_string(),
        },
        Row {
            case: "a read of a resource's tags",
            operation: "ListTagsForResource",
            body: json!({ "resourceArn": machine }),
            resource: machine.to_string(),
        },
        Row {
            case: "a create, judged against the ARN its name mints",
            operation: "CreateActivity",
            body: json!({ "name": format!("probe-{index}") }),
            resource: activity_arn(&format!("probe-{index}")),
        },
    ]
}

/// What the documents in force on one caller do to the calls above.
enum Decided {
    /// Every call is served.
    Served,
    /// No readable document allows them, so each is refused for want of a grant.
    NoGrant,
    /// A document denies them explicitly, which the refusal names instead.
    ExplicitDeny,
}

/// How one caller of the rows above is made, and what its documents say.
struct CallerCase {
    case: &'static str,
    /// The inline policy the principal carries, or `None` for the account's own published
    /// credential, which holds no identity policy at all.
    policy: Option<Value>,
    /// Whether the principal is an assumed-role session (as against an IAM user), for a
    /// caller that carries a policy.
    session: bool,
    decided: Decided,
}

/// Every call of this service is served only where the documents in force on the signing
/// principal allow `states:<OperationName>` on the ARN the request names; every other
/// caller is refused with HTTP 400 `AccessDeniedException` naming the principal, the
/// action and that ARN, and the refused call changes nothing.
#[tokio::test]
async fn a_call_is_served_only_where_the_signing_principals_policies_allow_the_action() {
    let callers = vec![
        CallerCase {
            case: "an assumed-role session whose role allows every action of this service",
            policy: Some(allowing(&["states:*"], &["*"])),
            session: true,
            decided: Decided::Served,
        },
        CallerCase {
            case: "an assumed-role session whose role allows nothing of this service",
            policy: Some(allowing(&["kms:Decrypt"], &["*"])),
            session: true,
            decided: Decided::NoGrant,
        },
        CallerCase {
            case: "an assumed-role session whose role is denied this service explicitly",
            policy: Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": "states:*", "Resource": "*" },
                    { "Effect": "Deny", "Action": "states:*", "Resource": "*" },
                ]
            })),
            session: true,
            decided: Decided::ExplicitDeny,
        },
        CallerCase {
            case: "an IAM user whose policy allows every action of this service",
            policy: Some(allowing(&["states:*"], &["*"])),
            session: false,
            decided: Decided::Served,
        },
        CallerCase {
            case: "an IAM user whose documents say nothing about this service",
            policy: Some(allowing(&["sts:GetCallerIdentity"], &["*"])),
            session: false,
            decided: Decided::NoGrant,
        },
        CallerCase {
            case: "the account's own published credential",
            policy: None,
            session: false,
            decided: Decided::Served,
        },
    ];

    for (index, caller) in callers.into_iter().enumerate() {
        let emulator = Emulator::new();
        let machine = emulator.machine(&format!("authz-{index}")).await;
        let execution = emulator.execution(&machine, "first").await;
        let principal = match (&caller.policy, caller.session) {
            (Some(policy), true) => {
                emulator
                    .assumed_role(&format!("sfn-caller-{index}"), policy)
                    .await
            }
            (Some(policy), false) => emulator.user(&format!("sfn-user-{index}"), policy).await,
            (None, _) => Principal {
                access_key_id: ACCOUNT_KEY.to_string(),
                arn: format!("arn:aws:iam::{ACCOUNT}:root"),
            },
        };

        for Row {
            case: row,
            operation,
            body,
            resource,
        } in rows(index, &machine, &execution)
        {
            let case = caller.case;
            let before = emulator.execution_count(&machine).await;
            let (status, answer) = emulator
                .call_as(&principal.access_key_id, operation, &body)
                .await;
            let reason = match caller.decided {
                Decided::Served => {
                    assert_eq!(
                        status, 200,
                        "case {case:?} / {row:?}: a call the documents in force allow is served, \
                         got {status} with {answer:?}"
                    );
                    continue;
                }
                Decided::NoGrant => no_grant(&format!("states:{operation}")),
                Decided::ExplicitDeny => {
                    "with an explicit deny in an identity-based policy".to_string()
                }
            };
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                "case {case:?} / {row:?}: a call the documents in force do not allow is refused, \
                 got {status} with {answer:?}"
            );
            let expected = not_authorized(
                &principal.arn,
                &format!("states:{operation}"),
                &resource,
                &reason,
            );
            let stated = message(&answer);
            assert_eq!(
                stated, expected,
                "case {case:?} / {row:?}: expected {expected:?}, got {stated:?}"
            );
            // A refused call changed nothing: a refused start started no execution.
            let after = emulator.execution_count(&machine).await;
            assert_eq!(
                after, before,
                "case {case:?} / {row:?}: a refused call must change nothing, and the machine \
                 held {before} execution(s) before it"
            );
        }
    }
}

/// The decision is taken per request against the documents as they read NOW: revoking a
/// statement refuses the next call, deleting the document altogether keeps refusing it,
/// and putting the statement back serves it again — and nothing a refused call asked for
/// happened.
#[tokio::test]
async fn revoking_a_statement_refuses_the_next_call_and_restoring_it_serves_again() {
    let emulator = Emulator::new();
    let machine = emulator.machine("revocation").await;
    let execution = emulator.execution(&machine, "first").await;
    let role = "sfn-revocation";
    let granted = allowing(&["states:StartExecution", "states:DescribeExecution"], &["*"]);
    let principal = emulator.assumed_role(role, &granted).await;

    // (case, the document in force — `None` deletes it altogether, whether the two calls
    // below are served)
    let phases: Vec<(&str, Option<Value>, bool)> = vec![
        ("the grant in force", Some(granted.clone()), true),
        (
            "the statement revoked, another left in its place",
            Some(allowing(&["states:ListExecutions"], &["*"])),
            false,
        ),
        ("the document deleted altogether", None, false),
        ("the grant put back", Some(granted.clone()), true),
    ];

    let mut started = emulator.execution_count(&machine).await;
    for (phase_index, (phase, document, served)) in phases.into_iter().enumerate() {
        match &document {
            Some(document) => emulator.put_role_policy(role, document).await,
            None => emulator.delete_role_policy(role).await,
        }
        // Each phase is asked TWICE: a decision taken against the documents in force is
        // the same decision on the next call, so nothing of it latches.
        for attempt in 1..=2 {
            let (status, answer) = emulator
                .call_as(
                    &principal.access_key_id,
                    "DescribeExecution",
                    &json!({ "executionArn": execution }),
                )
                .await;
            if served {
                assert_eq!(
                    status, 200,
                    "case {phase:?} (attempt {attempt}): the read is served, got {status} with \
                     {answer:?}"
                );
                assert_eq!(
                    string(&answer, "executionArn"),
                    execution,
                    "case {phase:?} (attempt {attempt}): the read reports the execution asked \
                     for, got {answer:?}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&answer).as_str()),
                    (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                    "case {phase:?} (attempt {attempt}): the read is refused, got {status} with \
                     {answer:?}"
                );
                let expected = not_authorized(
                    &principal.arn,
                    "states:DescribeExecution",
                    &execution,
                    &no_grant("states:DescribeExecution"),
                );
                assert_eq!(
                    message(&answer),
                    expected,
                    "case {phase:?} (attempt {attempt}): expected {expected:?}, got {answer:?}"
                );
            }

            let name = format!("run-{phase_index}-{attempt}");
            let (status, answer) = emulator
                .call_as(
                    &principal.access_key_id,
                    "StartExecution",
                    &json!({ "stateMachineArn": machine, "name": name }),
                )
                .await;
            if served {
                assert_eq!(
                    status, 200,
                    "case {phase:?} (attempt {attempt}): the start is served, got {status} with \
                     {answer:?}"
                );
                started += 1;
            } else {
                assert_eq!(
                    (status, error_code(&answer).as_str()),
                    (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                    "case {phase:?} (attempt {attempt}): the start is refused, got {status} with \
                     {answer:?}"
                );
                let expected = not_authorized(
                    &principal.arn,
                    "states:StartExecution",
                    &machine,
                    &no_grant("states:StartExecution"),
                );
                assert_eq!(
                    message(&answer),
                    expected,
                    "case {phase:?} (attempt {attempt}): expected {expected:?}, got {answer:?}"
                );
            }
            // A refused start started nothing, and a served one started exactly one.
            let held = emulator.execution_count(&machine).await;
            assert_eq!(
                held, started,
                "case {phase:?} (attempt {attempt}): expected {started} execution(s) to have \
                 been started, got {held}"
            );
        }
    }
}

/// The decision is made about the ARN the request NAMES, so a grant is read for its
/// `Resource` element as well as for its action: a statement scoped to one state machine
/// covers the calls about that machine and is a REFUSAL — not an empty answer — for
/// another.
#[tokio::test]
async fn a_grant_is_read_for_the_resource_the_request_names() {
    let emulator = Emulator::new();
    let first = emulator.machine("scoped-first").await;
    let second = emulator.machine("scoped-second").await;
    let every_machine = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:*");
    let role = "sfn-scoped";
    let principal = emulator
        .assumed_role(role, &allowing(&["states:ListExecutions"], &[&first]))
        .await;

    // (case, what the grant's `Resource` element names, whether the list of the first and
    // of the second machine is served)
    let scopes: Vec<(&str, Vec<String>, [bool; 2])> = vec![
        (
            "a grant scoped to the first machine",
            vec![first.clone()],
            [true, false],
        ),
        (
            "a grant scoped to the second machine",
            vec![second.clone()],
            [false, true],
        ),
        (
            "a grant naming both machines",
            vec![first.clone(), second.clone()],
            [true, true],
        ),
        (
            "a grant scoped to every state machine of the Region",
            vec![every_machine.clone()],
            [true, true],
        ),
        (
            "a grant scoped to every resource",
            vec!["*".to_string()],
            [true, true],
        ),
    ];

    for (case, resources, served) in scopes {
        let named: Vec<&str> = resources.iter().map(String::as_str).collect();
        emulator
            .put_role_policy(role, &allowing(&["states:ListExecutions"], &named))
            .await;
        for (machine, served) in [&first, &second].into_iter().zip(served) {
            let (status, answer) = emulator
                .call_as(
                    &principal.access_key_id,
                    "ListExecutions",
                    &json!({ "stateMachineArn": machine }),
                )
                .await;
            if served {
                assert_eq!(
                    status, 200,
                    "case {case:?} / {machine}: a list the grant covers is served, got {status} \
                     with {answer:?}"
                );
                assert!(
                    answer.get("executions").and_then(Value::as_array).is_some(),
                    "case {case:?} / {machine}: a served list reports its executions, got \
                     {answer:?}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&answer).as_str()),
                    (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                    "case {case:?} / {machine}: a list outside the grant is REFUSED rather than \
                     answered empty, got {status} with {answer:?}"
                );
                let expected = not_authorized(
                    &principal.arn,
                    "states:ListExecutions",
                    machine,
                    &no_grant("states:ListExecutions"),
                );
                assert_eq!(
                    message(&answer),
                    expected,
                    "case {case:?} / {machine}: expected {expected:?}, got {answer:?}"
                );
            }
        }
    }

    // The action half of the same decision: the grant names one action, so another
    // operation about the very machine it names is refused too.
    emulator
        .put_role_policy(role, &allowing(&["states:ListExecutions"], &[&first]))
        .await;
    let (status, answer) = emulator
        .call_as(
            &principal.access_key_id,
            "DescribeStateMachine",
            &json!({ "stateMachineArn": first }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (ACCESS_DENIED_STATUS, ACCESS_DENIED),
        "case \"an action the grant does not name\": expected a refusal, got {status} with \
         {answer:?}"
    );
    let expected = not_authorized(
        &principal.arn,
        "states:DescribeStateMachine",
        &first,
        &no_grant("states:DescribeStateMachine"),
    );
    assert_eq!(
        message(&answer),
        expected,
        "case \"an action the grant does not name\": expected {expected:?}, got {answer:?}"
    );
}

/// The gate reads a request's resource through a catalog keyed by operation
/// (`service::identity_authorization::ADDRESSED_BY`), which is a transcription of this
/// crate's Smithy model: every operation the model binds carries a row, every member a row
/// names is a member of THAT operation's own input, and every member of an input that
/// names an ARN of this service's own resources is listed for it.
///
/// So a model that grows an operation, renames a member, or moves a resource ARN onto
/// another operation fails here rather than leaving that call judged against every
/// resource (or judged by a member its own operation does not model).
#[test]
fn the_catalog_of_addressed_members_is_the_model_s_own() {
    let source = std::fs::read_to_string(MODEL_PATH).unwrap_or_else(|error| {
        panic!("the Smithy model at {MODEL_PATH} must be readable: {error}")
    });
    let model: Value = serde_json::from_str(&source).expect("the Smithy model parses as JSON");
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the Smithy model carries a shapes map");

    // The members of an input that name an ARN of a resource of THIS service: every member
    // the catalog may list, read off the model's own member names rather than guessed at.
    // A `roleArn` names an IAM role and a `taskToken` names no resource, so neither is one.
    let names_a_resource = |member: &str| {
        matches!(
            member,
            "executionArn"
                | "stateMachineArn"
                | "stateMachineAliasArn"
                | "stateMachineVersionArn"
                | "mapRunArn"
                | "activityArn"
                | "resourceArn"
        )
    };

    let mut bound = 0usize;
    for (shape_id, shape) in shapes {
        if shape.get("type").and_then(Value::as_str) != Some("operation") {
            continue;
        }
        let operation = shape_id.rsplit('#').next().unwrap_or(shape_id);
        bound += 1;
        let listed = identity_authorization::addressed_by(operation);
        assert!(
            identity_authorization::ADDRESSED_BY
                .iter()
                .any(|(named, _)| *named == operation),
            "case {operation:?}: the model binds this operation, which the catalog does not \
             carry a row for"
        );
        // The input shape's own members, which is what a row may name.
        let input = shape
            .get("input")
            .and_then(|input| input.get("target"))
            .and_then(Value::as_str)
            .expect("every operation of this model declares an input shape");
        let members = shapes
            .get(input)
            .and_then(|shape| shape.get("members"))
            .and_then(Value::as_object);
        let member_names: Vec<&str> = members
            .map(|members| members.keys().map(String::as_str).collect())
            .unwrap_or_default();
        for named in listed {
            assert!(
                member_names.contains(named),
                "case {operation:?}: the catalog names {named:?}, which {input} does not declare; \
                 its members are {member_names:?}"
            );
        }
        for member in &member_names {
            if !names_a_resource(member) {
                continue;
            }
            assert!(
                listed.contains(member),
                "case {operation:?}: {input} declares {member:?}, a resource ARN of this \
                 service, which the catalog does not list for it; it lists {listed:?}"
            );
        }
    }
    assert_eq!(
        bound,
        identity_authorization::ADDRESSED_BY.len(),
        "expected one catalog row per operation the model binds, got {} row(s) for {bound} \
         operation(s)",
        identity_authorization::ADDRESSED_BY.len()
    );
}
