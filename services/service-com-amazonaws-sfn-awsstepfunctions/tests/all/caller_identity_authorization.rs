// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IDENTITY-BASED authorization a start of an execution is
//! subject to, driven over each service's own wire protocol through the production
//! `MultiServiceRouter` with Step Functions, IAM and the token service mounted on ONE
//! engine — the shape `core/router/src/lib.rs` mounts them in — so the policies the
//! decision reads are the documents IAM's own handlers wrote and the session a request
//! speaks for is the one the token service minted.
//!
//! The rule under test: starting an execution is the IAM action
//! `<this service's action namespace>:<operation name>` on the state-machine ARN the
//! request carries, authorized against the identity-based policies in force on the
//! CALLER at the instant of the call. A caller no readable document allows is refused
//! with HTTP 400 `AccessDeniedException` carrying IAM's own sentence — the principal,
//! the action, the resource and which of IAM's rules refused it — and NO execution is
//! created; a caller a document allows is served. Nothing is cached, so a grant just
//! written grants on the next call and one just replaced stops granting on the next
//! call.
//!
//! The rows differ along what the rule generalises over: WHAT the caller's documents
//! say about the call (a grant of every resource, a grant scoped to the very ARN the
//! request names, a grant scoped to another state machine, a grant of a prefix of this
//! service's actions, documents that allow only another service's actions, and a grant
//! overturned by an explicit deny), WHICH operation starts the execution (the
//! asynchronous `StartExecution` of a STANDARD workflow and the synchronous
//! `StartSyncExecution` of an EXPRESS one), and WHO the caller is (a session of an IAM
//! role — what a call made by a function's or a service's execution role presents — an
//! IAM user holding a long-term key, and the account's own published credential, which
//! no identity-based policy governs).
//!
//! The grant→revoke→restore cycle of one long-lived session is its own test below,
//! because that is the sequence a least-privilege deployment is verified by: the
//! identical call succeeds, is refused once the grant is withdrawn, and succeeds again
//! once it is restored.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::execution_data_encryption::{form_encode, xml_element, Wire, SFN};
use crate::state_machine_lifecycle::{error_code, string, ACCOUNT, HELLO_DEFINITION, REGION};

/// The code AWS's authorization layer refuses a call with, and the status it carries.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 400;

/// The access-key id standing for the account's own published credential: neither the
/// token service nor IAM holds a record of it, so it resolves to the account's ambient
/// identity, which no identity-based policy governs.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The role a state machine of this suite runs as. It is only recorded on the state
/// machine — what the rows decide is what the CALLER may do, not what the execution's
/// own role may.
const MACHINE_ROLE: &str = "arn:aws:iam::000000000000:role/StepFunctionsRole";

/// IAM's own API version, as its awsQuery requests carry it.
const IAM_VERSION: &str = "2010-05-08";

/// The token service's API version, from its own model.
const STS_VERSION: &str = "2011-06-15";

/// Step Functions, IAM and the token service over ONE engine, so a request's principal,
/// its policies and the state machine it names are all records these services' own
/// handlers wrote.
struct Deployment {
    router: MultiServiceRouter,
}

impl Deployment {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// Call one awsJson operation of one service, signed with `access_key_id` in the
    /// SigV4 credential scope — which is what routes the request and says which
    /// principal it speaks for — and return `(status, body)`.
    async fn call_as(
        &self,
        access_key_id: &str,
        wire: &Wire,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{}/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef",
            wire.signing_name
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{}.{REGION}.amazonaws.com", wire.signing_name))
            .header(
                "content-type",
                format!("application/x-amz-json-{}", wire.json_version),
            )
            .header("x-amz-target", format!("{}.{operation}", wire.target_prefix))
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

    /// One Step Functions call signed with the account's own credential, requiring 200.
    async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call_as(ACCOUNT_KEY, &SFN, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected 200 from Step Functions, got {status} with {parsed:?}"
        );
        parsed
    }

    /// One awsQuery call to the service `signing_name`, signed with the account's own
    /// credential, as the response body's text.
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
                    "AWS4-HMAC-SHA256 \
                     Credential={ACCOUNT_KEY}/20260101/{REGION}/{signing_name}/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {action}: expected 200 from {signing_name}, got {status} with {text}"
        );
        text
    }

    /// One IAM call.
    async fn iam(&self, action: &str, params: &[(&str, String)]) -> String {
        self.query("iam", IAM_VERSION, action, params).await
    }

    /// An IAM role trusting this account to assume it, as its ARN.
    async fn role(&self, role_name: &str) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
                "Action": "sts:AssumeRole",
            }]
        });
        let created = self
            .iam(
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust.to_string()),
                ],
            )
            .await;
        xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateRole answers the role Arn, got {created}"))
    }

    /// An IAM user, as the access key id a request speaks for it with and its ARN.
    async fn user(&self, user_name: &str) -> (String, String) {
        let created = self
            .iam("CreateUser", &[("UserName", user_name.to_string())])
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser answers the user Arn, got {created}"));
        let minted = self
            .iam("CreateAccessKey", &[("UserName", user_name.to_string())])
            .await;
        let key = xml_element(&minted, "AccessKeyId")
            .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}"));
        (key, arn)
    }

    /// Replace the inline policy of an IAM identity with `document` — which is what a
    /// deployment granting, revoking and restoring a permission does.
    async fn set_policy(&self, identity: &Identity, document: &Value) {
        let (action, named) = match identity {
            Identity::Session { role_name, .. } => ("PutRolePolicy", ("RoleName", role_name)),
            Identity::User { user_name, .. } => ("PutUserPolicy", ("UserName", user_name)),
            // The account's own published credential carries no identity policy at all.
            Identity::Account => return,
        };
        self.iam(
            action,
            &[
                (named.0, named.1.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", document.to_string()),
            ],
        )
        .await;
    }

    /// A session of the role `role_arn`, as the access key id the session signs with
    /// and the assumed-role ARN `sts:GetCallerIdentity` reports for it.
    async fn session(&self, role_arn: &str, session_name: &str) -> (String, String) {
        let assumed = self
            .query(
                "sts",
                STS_VERSION,
                "AssumeRole",
                &[
                    ("RoleArn", role_arn.to_string()),
                    ("RoleSessionName", session_name.to_string()),
                ],
            )
            .await;
        let key = xml_element(&assumed, "AccessKeyId")
            .unwrap_or_else(|| panic!("AssumeRole answers an AccessKeyId, got {assumed}"));
        let arn = xml_element(&assumed, "Arn")
            .unwrap_or_else(|| panic!("AssumeRole answers the assumed-role Arn, got {assumed}"));
        (key, arn)
    }

    /// How many executions the state machine `machine` holds, which is what a refused
    /// start must leave unchanged.
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

/// The caller a row is taken as: what signs its requests, what a refusal names it, and
/// where its inline policy is written.
enum Identity {
    /// A session of an IAM role, which is what a call made by an execution role
    /// presents.
    Session {
        role_name: &'static str,
        key: String,
        arn: String,
    },
    /// An IAM user holding a long-term access key.
    User {
        user_name: &'static str,
        key: String,
        arn: String,
    },
    /// The account's own published credential, which no identity-based policy governs.
    Account,
}

impl Identity {
    /// The access key id this caller's requests are signed with.
    fn key(&self) -> &str {
        match self {
            Self::Session { key, .. } | Self::User { key, .. } => key,
            Self::Account => ACCOUNT_KEY,
        }
    }

    /// The principal a refusal names, as `sts:GetCallerIdentity` reports it.
    fn principal(&self) -> String {
        match self {
            Self::Session { arn, .. } | Self::User { arn, .. } => arn.clone(),
            Self::Account => String::new(),
        }
    }

    /// Whether any identity-based policy bounds this caller at all.
    fn is_bounded(&self) -> bool {
        !matches!(self, Self::Account)
    }

    fn case(&self) -> &'static str {
        match self {
            Self::Session { .. } => "a session of an IAM role",
            Self::User { .. } => "an IAM user holding a long-term key",
            Self::Account => "the account's own published credential",
        }
    }
}

/// One of the two operations that START an execution: the action it performs and the
/// request it takes.
struct Start {
    case: &'static str,
    /// The operation name, which is also the second half of the IAM action it performs.
    operation: &'static str,
    /// Whether it runs an EXPRESS workflow (`StartSyncExecution`) or a STANDARD one.
    express: bool,
}

/// Both starts, each named by the workflow type it serves.
fn starts() -> [Start; 2] {
    [
        Start {
            case: "the asynchronous start of a STANDARD workflow",
            operation: "StartExecution",
            express: false,
        },
        Start {
            case: "the synchronous start of an EXPRESS workflow",
            operation: "StartSyncExecution",
            express: true,
        },
    ]
}

/// The IAM action an operation of this service performs: its own name under the
/// service's action namespace, which is the model's `awsService.arnNamespace`
/// (`smithy_query(query="service_info")`: `arnNamespace: states`).
fn action_of(operation: &str) -> String {
    format!("states:{operation}")
}

/// Which of IAM's rules refused a call, as the refusal's own clause.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Refused {
    /// Nothing readable allowed the action.
    NothingAllows,
    /// A document that CAN be read denies it explicitly.
    ExplicitDeny,
    /// The call is served.
    Served,
}

impl Refused {
    /// The clause IAM closes the sentence with for this outcome.
    fn clause(self, action: &str) -> String {
        match self {
            Self::NothingAllows => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
            Self::Served => String::new(),
        }
    }
}

/// One document a bounded caller may carry, and what it makes of a start.
struct Row {
    case: &'static str,
    /// The inline policy, built from the ARNs of the two state machines the rows run
    /// against.
    document: fn(&str, &str) -> Value,
    outcome: Refused,
}

/// An identity policy with the statements given, under the version IAM's documents
/// declare.
fn policy(statements: Vec<Value>) -> Value {
    json!({ "Version": "2012-10-17", "Statement": statements })
}

/// The rows: what a caller's documents say about the call.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a grant of both start actions on every resource",
            document: |_, _| {
                policy(vec![json!({
                    "Effect": "Allow",
                    "Action": ["states:StartExecution", "states:StartSyncExecution"],
                    "Resource": "*",
                })])
            },
            outcome: Refused::Served,
        },
        Row {
            case: "a grant scoped to the state machines the requests name",
            document: |standard, express| {
                policy(vec![json!({
                    "Effect": "Allow",
                    "Action": ["states:StartExecution", "states:StartSyncExecution"],
                    "Resource": [standard, express],
                })])
            },
            outcome: Refused::Served,
        },
        Row {
            case: "a grant of every action of this service",
            document: |_, _| {
                policy(vec![json!({
                    "Effect": "Allow",
                    "Action": "states:*",
                    "Resource": "*",
                })])
            },
            outcome: Refused::Served,
        },
        Row {
            case: "a grant of the prefix both start actions share",
            document: |_, _| {
                policy(vec![json!({
                    "Effect": "Allow",
                    "Action": "states:Start*",
                    "Resource": "*",
                })])
            },
            outcome: Refused::Served,
        },
        Row {
            case: "a grant scoped to ANOTHER state machine",
            document: |_, _| {
                policy(vec![json!({
                    "Effect": "Allow",
                    "Action": ["states:StartExecution", "states:StartSyncExecution"],
                    "Resource":
                        format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:somewhere-else"),
                })])
            },
            outcome: Refused::NothingAllows,
        },
        Row {
            // The document the issue this suite was written for revokes a function's
            // grant down to: it allows another service's actions and nothing of this
            // one.
            case: "documents that allow only another service's actions",
            document: |_, _| {
                policy(vec![json!({
                    "Effect": "Allow",
                    "Action": [
                        "logs:CreateLogGroup",
                        "logs:CreateLogStream",
                        "logs:PutLogEvents",
                    ],
                    "Resource": "*",
                })])
            },
            outcome: Refused::NothingAllows,
        },
        Row {
            case: "a grant of every action overturned by an explicit deny",
            document: |_, _| {
                policy(vec![
                    json!({ "Effect": "Allow", "Action": "states:*", "Resource": "*" }),
                    json!({
                        "Effect": "Deny",
                        "Action": ["states:StartExecution", "states:StartSyncExecution"],
                        "Resource": "*",
                    }),
                ])
            },
            outcome: Refused::ExplicitDeny,
        },
    ]
}

/// The request one start takes against the state machine `machine`, under `name`.
fn start_request(machine: &str, name: &str) -> Value {
    json!({ "stateMachineArn": machine, "name": name, "input": r#"{"n":1}"# })
}

/// The `message` an awsJson error body carries, under either casing.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every start is authorized against the identity-based policies in force on the
/// caller: one no readable document allows is refused with HTTP 400
/// `AccessDeniedException` carrying IAM's own sentence and creates no execution, and
/// one a document allows is served.
#[tokio::test]
async fn a_start_is_authorized_against_the_callers_identity_policies() {
    let deployment = Deployment::new();
    let standard = string(
        &deployment
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": "authorized-standard",
                    "definition": HELLO_DEFINITION,
                    "roleArn": MACHINE_ROLE,
                }),
            )
            .await,
        "stateMachineArn",
    );
    let express = string(
        &deployment
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": "authorized-express",
                    "definition": HELLO_DEFINITION,
                    "roleArn": MACHINE_ROLE,
                    "type": "EXPRESS",
                }),
            )
            .await,
        "stateMachineArn",
    );

    // The three callers, each made once: the rows replace the inline policy of the
    // bounded two, which is what a deployment granting and revoking a permission does.
    let role_name = "identity-authorized-runner";
    let role_arn = deployment.role(role_name).await;
    let (session_key, session_arn) = deployment.session(&role_arn, "one-session").await;
    let user_name = "identity-authorized-user";
    let (user_key, user_arn) = deployment.user(user_name).await;
    let callers = [
        Identity::Session {
            role_name,
            key: session_key,
            arn: session_arn,
        },
        Identity::User {
            user_name,
            key: user_key,
            arn: user_arn,
        },
        Identity::Account,
    ];

    for (position, caller) in callers.iter().enumerate() {
        for (index, row) in rows().into_iter().enumerate() {
            deployment
                .set_policy(caller, &(row.document)(&standard, &express))
                .await;
            for start in starts() {
                let machine = if start.express { &express } else { &standard };
                let case = format!("{} / {} / {}", caller.case(), row.case, start.case);
                let action = action_of(start.operation);
                // The account's own credential carries no identity policy, so the
                // rows' documents decide nothing for it: every start is served.
                let outcome = if caller.is_bounded() {
                    row.outcome
                } else {
                    Refused::Served
                };
                // "This API action is not supported by EXPRESS state machines"
                // (`ListExecutions`), so the executions of one are counted only where
                // that listing serves — the asynchronous start's own workflow type.
                let before = if start.express {
                    None
                } else {
                    Some(deployment.execution_count(machine).await)
                };
                let (status, answer) = deployment
                    .call_as(
                        caller.key(),
                        &SFN,
                        start.operation,
                        &start_request(
                            machine,
                            // A name of its own per row: "if you call StartExecution
                            // with the same name and input as a running execution, the
                            // call succeeds and return the same response as the
                            // original request", so a reused name would answer the
                            // earlier start rather than make this one.
                            &format!("caller-{position}-row-{index}-{}", start.operation),
                        ),
                    )
                    .await;
                let after = if start.express {
                    None
                } else {
                    Some(deployment.execution_count(machine).await)
                };

                if outcome == Refused::Served {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: a start the caller's policies allow is served, got \
                         {status} with {answer:?}"
                    );
                    assert!(
                        !string(&answer, "executionArn").is_empty(),
                        "case {case:?}: expected the started execution's ARN, got {answer:?}"
                    );
                    assert_eq!(
                        after,
                        before.map(|held| held + 1),
                        "case {case:?}: a served start creates one execution, got {before:?} then \
                         {after:?}"
                    );
                    continue;
                }

                assert_eq!(
                    (status, error_code(&answer).as_str()),
                    (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                    "case {case:?}: a start no document allows is refused, got {status} with \
                     {answer:?}"
                );
                let expected = format!(
                    "User: {} is not authorized to perform: {action} on resource: {machine} {}",
                    caller.principal(),
                    outcome.clause(&action),
                );
                assert_eq!(
                    message(&answer),
                    expected,
                    "case {case:?}: expected IAM's refusal {expected:?}, got {answer:?}"
                );
                // The refusal is taken BEFORE anything is created: the state machine
                // holds exactly the executions it held before the call.
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused start creates no execution, got {before:?} then \
                     {after:?}"
                );
            }
        }
    }
}

/// The grant→revoke→restore cycle of ONE long-lived session: the identical call is
/// served while the grant is in force, refused as soon as it is withdrawn — creating no
/// execution — and served again once it is restored. Nothing of the decision is cached
/// on the session, the state machine or the engine.
#[tokio::test]
async fn a_withdrawn_grant_stops_a_start_and_a_restored_one_serves_it_again() {
    let deployment = Deployment::new();
    let machine = string(
        &deployment
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": "grant-cycle",
                    "definition": HELLO_DEFINITION,
                    "roleArn": MACHINE_ROLE,
                }),
            )
            .await,
        "stateMachineArn",
    );
    let role_name = "grant-cycle-runner";
    let role_arn = deployment.role(role_name).await;
    let (key, session_arn) = deployment.session(&role_arn, "held-session").await;
    let caller = Identity::Session {
        role_name,
        key,
        arn: session_arn.clone(),
    };

    let granting = |machine: &str| {
        policy(vec![json!({
            "Effect": "Allow",
            "Action": "states:StartExecution",
            "Resource": machine,
        })])
    };
    let revoked = policy(vec![json!({
        "Effect": "Allow",
        "Action": ["logs:CreateLogGroup", "logs:CreateLogStream", "logs:PutLogEvents"],
        "Resource": "*",
    })]);

    // (case, the document in force, whether the start is served)
    let phases: Vec<(&str, Value, bool)> = vec![
        ("granted", granting(&machine), true),
        ("revoked", revoked, false),
        ("restored", granting(&machine), true),
    ];

    let action = action_of("StartExecution");
    for (phase, document, served) in phases {
        deployment.set_policy(&caller, &document).await;
        let before = deployment.execution_count(&machine).await;
        let (status, answer) = deployment
            .call_as(
                caller.key(),
                &SFN,
                "StartExecution",
                &start_request(&machine, &format!("{phase}-run")),
            )
            .await;
        let after = deployment.execution_count(&machine).await;
        if served {
            assert_eq!(
                status, 200,
                "case {phase:?}: the start is served while the grant is in force, got {status} \
                 with {answer:?}"
            );
            assert_eq!(
                after,
                before + 1,
                "case {phase:?}: the served start creates one execution, got {before} then {after}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (ACCESS_DENIED_STATUS, ACCESS_DENIED),
            "case {phase:?}: the start is refused once the grant is withdrawn, got {status} with \
             {answer:?}"
        );
        let expected = format!(
            "User: {session_arn} is not authorized to perform: {action} on resource: {machine} \
             because no identity-based policy allows the {action} action"
        );
        assert_eq!(
            message(&answer),
            expected,
            "case {phase:?}: expected IAM's refusal {expected:?}, got {answer:?}"
        );
        assert_eq!(
            after, before,
            "case {phase:?}: a refused start creates no execution, got {before} then {after}"
        );
    }
}
