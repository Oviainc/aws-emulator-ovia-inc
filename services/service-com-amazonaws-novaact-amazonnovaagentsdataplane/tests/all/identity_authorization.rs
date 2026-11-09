// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's operations, driven end to end over the real wire path (a
//! restJson1 request through the multi-service router into the mounted service), so what is
//! asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: `AccessDeniedException` is a service-common error of this service, so
//! EVERY operation evaluates the identity-based IAM policies in force on the principal its
//! credentials name — against `nova-act:<OperationName>` on this service's ARN for the
//! resource the request addresses — BEFORE it reads or writes anything. A request no
//! statement allows (IAM's default deny), or one an explicit `Deny` covers, is answered HTTP
//! 403 `AccessDeniedException` naming the principal, the action, that resource and which of
//! IAM's rules refused it; it performs nothing and leaves no resource behind, while a request
//! a matching `Allow` covers reaches the operation's own answer.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME shared
//! engine — the stores those services' own handlers write — so the documents under test are
//! the ones `iam:PutRolePolicy` stores and the principal a refusal blames is the one
//! `sts:GetCallerIdentity` answers for the key the request signed with.
//!
//! The tables span what the rule generalises over: the operation (all sixteen of them: four
//! creates, three reads, two deletes, two updates, four listings and one step invocation),
//! the resource each one addresses (a definition the body names, a definition the path names,
//! a collection of children, a run, a session, an act, and an operation that addresses no
//! resource of this service at all), the document in force (none at all, another action of
//! this service, another service's actions, an explicit deny beside an allow, an allow of
//! exactly the action, of the action in another case, of the whole service and of
//! everything), the resource an allow is scoped to (every resource, one definition, any
//! definition, the children of one definition, a definition in another Region) and the
//! principal (an assumed-role session and the account's own published credential).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::EmulatedService;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, and so the Region of every ARN they are judged
/// against.
const REGION: &str = "us-east-1";

/// This service's IAM action prefix, which is its `arnNamespace` and its `sigv4Name`
/// (`smithy_query(query="service_info")`).
const ACTION_PREFIX: &str = "nova-act";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The access key id of the assumed-role session every row signs with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The role that session is assumed from, and the session's own name.
const ROLE: &str = "nova-act-caller";
const SESSION: &str = "act-session";

/// The name of the inline policy the role carries.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// Identifiers of this service's resource chain that no call ever minted, so a row that gets
/// past authorization meets the operation's own not-found answer rather than a second
/// authorization decision.
const DEFINITION: &str = "authorized-planner";
const RUN_ID: &str = "0f4c8b26-7b1c-4f0a-9d3e-9f6b2c1a5e77";
const SESSION_ID: &str = "1a2b3c4d-5e6f-4a8b-9c0d-1e2f3a4b5c6d";
const ACT_ID: &str = "2b3c4d5e-6f7a-4b8c-9d0e-1f2a3b4c5d6e";

/// A router serving this service beside the IAM and STS services, over ONE shared engine, so
/// the policies and the session the cases seed are the ones those services hold.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these cases are about; the credential scope is still read for
    // the routing key, the ARN Region and the access key id the principal is resolved from.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// What one call answered: its HTTP status, the wire error code and the parsed body.
struct Answer {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Answer {
    /// The answer's `message`, or the empty string when it carries none.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Whether this answer is the modelled authorization refusal.
    fn refused(&self) -> bool {
        self.error_code.as_deref() == Some("AccessDeniedException")
    }
}

/// Call one operation of this service signed with `access_key_id` — what names the principal
/// the request speaks for.
async fn call_as(router: &MultiServiceRouter, access_key_id: &str, operation: &Operation) -> Answer {
    let payload = operation
        .body
        .as_ref()
        .map_or_else(Vec::new, |value| value.to_string().into_bytes());
    let request = http::Request::builder()
        .method(operation.method)
        .uri(format!("{}{}", operation.path, operation.query))
        .header("host", format!("{ACTION_PREFIX}.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20261029/{REGION}/{ACTION_PREFIX}/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_code,
        body,
    }
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}",
        ::engine::emulator_account_id()
    )
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/{ROLE}", ::engine::emulator_account_id())
}

/// This service's ARN for `resource_path`, in the Region every call is signed for.
fn resource_of(resource_path: &str) -> String {
    format!(
        "arn:aws:{ACTION_PREFIX}:{REGION}:{}:{resource_path}",
        ::engine::emulator_account_id()
    )
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them.
fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let now = SystemTime::now();
    let account = ::engine::emulator_account_id();
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLENOVAACT00001".to_string(),
            arn: role_arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: account.to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, now))
        .expect("the session is recorded");
}

/// Attach `document` to the role as its inline policy, replacing whatever it carried — what
/// `iam:PutRolePolicy` does. `None` takes every policy off it, as `iam:DeleteRolePolicy` does.
fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    match document {
        None => {
            store.role_inline_policies.remove(&iam::name_key(ROLE));
        }
        Some(document) => {
            store.role_inline_policies.insert(
                iam::name_key(ROLE),
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
    }
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// Which of IAM's two refusals a row expects.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// The default deny of a request no statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Refusal {
    /// The clause IAM closes the refusal with.
    fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// One operation of this service, as a client addresses it: the request line the rule is
/// judged on, the body it carries, and the resource the request ADDRESSES.
struct Operation {
    name: &'static str,
    method: &'static str,
    /// The URI PATH, which is what the resource the request is judged against is built from
    /// — so it is held apart from the query string.
    path: String,
    /// The query string, carrying the parameters the model REQUIRES of this operation: the
    /// REST router resolves no route for a request that leaves one out, so a request without
    /// them never reaches the operation at all.
    query: &'static str,
    body: Option<Value>,
    /// The resource field of the ARN this request is judged against, or `None` for an
    /// operation that addresses no resource of this service.
    resource_path: Option<String>,
}

impl Operation {
    /// The ARN the refusal must name, and the clause it is stated in — empty for a request
    /// that addresses no resource of this service.
    fn resource_clause(&self) -> String {
        match &self.resource_path {
            None => String::new(),
            Some(path) => format!(" on resource: {}", resource_of(path)),
        }
    }
}

/// Every operation this service serves, each with a request of its own. The rule covers all
/// of them, so the table is the whole surface rather than a sample of it.
fn operations() -> Vec<Operation> {
    let definition = format!("workflow-definition/{DEFINITION}");
    let run = format!("{definition}/workflow-run/{RUN_ID}");
    let session = format!("{run}/session/{SESSION_ID}");
    vec![
        Operation {
            name: "CreateWorkflowDefinition",
            method: "PUT",
            path: "/workflow-definitions".to_string(),
            query: "",
            body: Some(json!({ "name": DEFINITION, "description": "authorized" })),
            // A create addresses the resource it would create, which it names in its body.
            resource_path: Some(definition.clone()),
        },
        Operation {
            name: "ListWorkflowDefinitions",
            method: "POST",
            path: "/workflow-definitions".to_string(),
            query: "",
            body: Some(json!({})),
            resource_path: Some("workflow-definition/*".to_string()),
        },
        Operation {
            name: "GetWorkflowDefinition",
            method: "GET",
            path: format!("/workflow-definitions/{DEFINITION}"),
            query: "",
            body: None,
            resource_path: Some(definition.clone()),
        },
        Operation {
            name: "DeleteWorkflowDefinition",
            method: "DELETE",
            path: format!("/workflow-definitions/{DEFINITION}"),
            query: "",
            body: None,
            resource_path: Some(definition.clone()),
        },
        Operation {
            name: "CreateWorkflowRun",
            method: "PUT",
            path: format!("/workflow-definitions/{DEFINITION}/workflow-runs"),
            query: "",
            body: Some(json!({})),
            resource_path: Some(format!("{definition}/workflow-run/*")),
        },
        Operation {
            name: "ListWorkflowRuns",
            method: "POST",
            path: format!("/workflow-definitions/{DEFINITION}/workflow-runs"),
            query: "",
            body: Some(json!({})),
            resource_path: Some(format!("{definition}/workflow-run/*")),
        },
        Operation {
            name: "GetWorkflowRun",
            method: "GET",
            path: format!("/workflow-definitions/{DEFINITION}/workflow-runs/{RUN_ID}"),
            query: "",
            body: None,
            resource_path: Some(run.clone()),
        },
        Operation {
            name: "UpdateWorkflowRun",
            method: "PUT",
            path: format!("/workflow-definitions/{DEFINITION}/workflow-runs/{RUN_ID}"),
            query: "",
            body: Some(json!({ "status": "COMPLETED" })),
            resource_path: Some(run.clone()),
        },
        Operation {
            name: "DeleteWorkflowRun",
            method: "DELETE",
            path: format!("/workflow-definitions/{DEFINITION}/workflow-runs/{RUN_ID}"),
            query: "",
            body: None,
            resource_path: Some(run.clone()),
        },
        Operation {
            name: "ListSessions",
            method: "POST",
            path: format!("/workflow-definitions/{DEFINITION}/workflow-runs/{RUN_ID}"),
            query: "",
            body: Some(json!({})),
            resource_path: Some(run.clone()),
        },
        Operation {
            name: "CreateSession",
            method: "PUT",
            path: format!("/workflow-definitions/{DEFINITION}/workflow-runs/{RUN_ID}/sessions"),
            query: "",
            body: Some(json!({})),
            resource_path: Some(format!("{run}/session/*")),
        },
        Operation {
            name: "CreateAct",
            method: "PUT",
            path: format!(
                "/workflow-definitions/{DEFINITION}/workflow-runs/{RUN_ID}/sessions/\
                 {SESSION_ID}/acts"
            ),
            query: "",
            body: Some(json!({ "task": "do the thing" })),
            resource_path: Some(format!("{session}/act/*")),
        },
        Operation {
            name: "UpdateAct",
            method: "PUT",
            path: format!(
                "/workflow-definitions/{DEFINITION}/workflow-runs/{RUN_ID}/sessions/\
                 {SESSION_ID}/acts/{ACT_ID}"
            ),
            query: "",
            body: Some(json!({ "status": "COMPLETED" })),
            resource_path: Some(format!("{session}/act/{ACT_ID}")),
        },
        Operation {
            name: "InvokeActStep",
            method: "PUT",
            path: format!(
                "/workflow-definitions/{DEFINITION}/workflow-runs/{RUN_ID}/sessions/\
                 {SESSION_ID}/acts/{ACT_ID}/invoke-step/"
            ),
            query: "",
            body: Some(json!({ "callResults": [] })),
            resource_path: Some(format!("{session}/act/{ACT_ID}")),
        },
        Operation {
            name: "ListActs",
            method: "POST",
            path: format!("/workflow-definitions/{DEFINITION}/acts"),
            query: "",
            body: Some(json!({})),
            // This path leaves the model's resource chain at the definition, so the
            // definition is the deepest resource it names.
            resource_path: Some(definition),
        },
        Operation {
            name: "ListModels",
            method: "POST",
            path: "/models".to_string(),
            // `clientCompatibilityVersion` is a REQUIRED `@httpQuery` member of this
            // operation (`smithy_query(query="operation",operation="ListModels")`).
            query: "?clientCompatibilityVersion=1",
            body: Some(json!({})),
            // The model catalog is service-managed: this operation addresses no resource of
            // this account, which is what IAM's any-resource wildcard stands for.
            resource_path: None,
        },
    ]
}

/// The documents a row puts in force on the role, and which of IAM's refusals the request then
/// meets — `None` for a request a matching `Allow` permits.
fn documents_in_force(
    action: &str,
    other: &str,
) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of another action of this service, but not this one",
            Some(allow(json!(other))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of every action of another service",
            Some(allow(json!("bedrock:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": format!("{ACTION_PREFIX}:*"), "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of exactly this action",
            Some(allow(json!(action))),
            None,
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow is
            // not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(action.to_lowercase()))),
            None,
        ),
        (
            "an allow listing this action beside another service's",
            Some(allow(json!(["bedrock:InvokeModel", action]))),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!(format!("{ACTION_PREFIX}:*")))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// THE RULE: every operation is judged against its OWN `nova-act:<OperationName>` action on
/// the resource the request addresses, before it performs anything; a refusal is HTTP 403
/// `AccessDeniedException` naming the principal, the action, that resource and which of IAM's
/// rules refused it, and an allowed call reaches the operation's own answer.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action_and_resource() {
    for operation in operations() {
        let action = format!("{ACTION_PREFIX}:{}", operation.name);
        // Another action of this service, which a row grants instead of the one under test.
        let other = operations()
            .into_iter()
            .map(|row| format!("{ACTION_PREFIX}:{}", row.name))
            .find(|candidate| *candidate != action)
            .expect("this service has more than one operation");

        for (row, document, refused) in documents_in_force(&action, &other) {
            let case = format!("{} with {row}", operation.name);
            // A fresh store, role and session per row: no row can pass on another's write.
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(&engine, document.as_ref());

            let answer = call_as(&router, SESSION_KEY_ID, &operation).await;
            match refused {
                Some(refusal) => {
                    assert_eq!(
                        answer.status, 403,
                        "case {case:?}: expected HTTP 403, got {} ({:?})",
                        answer.status, answer.body
                    );
                    assert!(
                        answer.refused(),
                        "case {case:?}: expected an AccessDeniedException, got {:?} ({:?})",
                        answer.error_code,
                        answer.body
                    );
                    assert_eq!(
                        answer.message(),
                        format!(
                            "User: {} is not authorized to perform: {action}{} {}",
                            session_arn(),
                            operation.resource_clause(),
                            refusal.reason(&action)
                        ),
                        "case {case:?}: expected IAM's wording, got {:?}",
                        answer.body
                    );
                }
                None => {
                    // Authorization answers BEFORE anything is read, so an allowed call falls
                    // through to the operation's own verdict — here a refusal about the
                    // resources nothing created, or a success for an operation that needs
                    // none — and never to another authorization decision.
                    assert!(
                        !answer.refused() && answer.status != 403,
                        "case {case:?}: an allowed request must not be refused, got {} ({:?})",
                        answer.status,
                        answer.body
                    );
                }
            }
        }
    }
}

/// RULE: the decision is taken before the handler runs, so a refused call PERFORMS NOTHING:
/// a `CreateWorkflowDefinition` an explicit `Deny` covers creates no definition, which a read
/// with the account's own credential — which the gate does not bound — then answers
/// `ResourceNotFoundException` for.
#[tokio::test]
async fn a_denied_create_leaves_no_workflow_definition_behind() {
    let (router, engine) = router_with_identity();
    given_session(&engine);
    given_role_policy(
        &engine,
        Some(&json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Deny",
                "Action": format!("{ACTION_PREFIX}:*"),
                "Resource": "*",
            }],
        })),
    );

    let create = operations()
        .into_iter()
        .find(|row| row.name == "CreateWorkflowDefinition")
        .expect("the create operation");
    let refused = call_as(&router, SESSION_KEY_ID, &create).await;
    assert_eq!(
        refused.status, 403,
        "the denied create must answer HTTP 403, got {} ({:?})",
        refused.status, refused.body
    );
    assert!(
        refused.refused(),
        "the denied create must answer AccessDeniedException, got {:?} ({:?})",
        refused.error_code,
        refused.body
    );
    assert_eq!(
        refused.message(),
        format!(
            "User: {} is not authorized to perform: {ACTION_PREFIX}:CreateWorkflowDefinition on \
             resource: {} with an explicit deny in an identity-based policy",
            session_arn(),
            resource_of(&format!("workflow-definition/{DEFINITION}"))
        ),
        "the denied create must state IAM's explicit-deny sentence, got {:?}",
        refused.body
    );
    assert!(
        refused.body.get("status").is_none(),
        "a denied create must report no definition status, got {:?}",
        refused.body
    );

    let read = operations()
        .into_iter()
        .find(|row| row.name == "GetWorkflowDefinition")
        .expect("the read operation");
    let answer = call_as(&router, ACCOUNT_KEY_ID, &read).await;
    assert_eq!(
        answer.status, 404,
        "a denied create must have created nothing, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "a denied create must have created nothing, got {:?} ({:?})",
        answer.error_code,
        answer.body
    );
    assert_eq!(
        answer.message(),
        "Workflow definition not found",
        "the read must state which kind of resource is missing, got {:?}",
        answer.body
    );
}

/// RULE: the account's own published credential — a key neither STS nor IAM holds a record
/// for — is not a principal the IAM store holds policies for, so it is served whatever the
/// role's policies say.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    for operation in operations() {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        // The role carries nothing at all, which is what refuses the SESSION above.
        given_role_policy(&engine, None);

        let answer = call_as(&router, ACCOUNT_KEY_ID, &operation).await;
        assert!(
            !answer.refused() && answer.status != 403,
            "case {:?}: the account's own credential must not be refused, got {} ({:?})",
            operation.name,
            answer.status,
            answer.body
        );
    }
}

/// RULE: a request is judged against the ARN of the resource it ADDRESSES, so an allow scoped
/// to one resource covers the operations that address it and no other — including the same
/// resource in another Region, which is a different ARN.
#[tokio::test]
async fn an_action_is_judged_against_the_resource_the_request_addresses() {
    let definition = resource_of(&format!("workflow-definition/{DEFINITION}"));
    let any_definition = resource_of("workflow-definition/*");
    let under_the_definition = resource_of(&format!("workflow-definition/{DEFINITION}/*"));
    let another_region = format!(
        "arn:aws:{ACTION_PREFIX}:eu-west-1:{}:workflow-definition/{DEFINITION}",
        ::engine::emulator_account_id()
    );
    // The operations this table varies the allow's scope over: one addressing the definition
    // the body names, one addressing the definition collection, one addressing the definition
    // the path names, one addressing a run under it, and one addressing no resource at all.
    let judged = [
        "CreateWorkflowDefinition",
        "ListWorkflowDefinitions",
        "GetWorkflowDefinition",
        "GetWorkflowRun",
        "ListModels",
    ];

    // (case, the Resource the allow is scoped to, which of the operations above it covers)
    let rows: Vec<(&str, String, Vec<&str>)> = vec![
        ("every resource", "*".to_string(), judged.to_vec()),
        (
            "exactly one definition",
            definition,
            vec!["CreateWorkflowDefinition", "GetWorkflowDefinition"],
        ),
        (
            // IAM's `*` matches any run of characters, including the `/` an ARN nests a
            // child resource under, so this scope reaches the children too.
            "any definition",
            any_definition,
            vec![
                "CreateWorkflowDefinition",
                "ListWorkflowDefinitions",
                "GetWorkflowDefinition",
                "GetWorkflowRun",
            ],
        ),
        (
            "everything under one definition",
            under_the_definition,
            vec!["GetWorkflowRun"],
        ),
        (
            "the same definition in another Region",
            another_region,
            Vec::new(),
        ),
    ];

    for (case, resource, covered) in rows {
        for operation in operations() {
            if !judged.contains(&operation.name) {
                continue;
            }
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(
                &engine,
                Some(&json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": format!("{ACTION_PREFIX}:*"),
                        "Resource": resource.clone(),
                    }],
                })),
            );

            let answer = call_as(&router, SESSION_KEY_ID, &operation).await;
            let allowed = covered.contains(&operation.name);
            assert_eq!(
                answer.refused(),
                !allowed,
                "case {case:?}/{}: an allow scoped to {resource:?} must {}cover this operation, \
                 got {} ({:?})",
                operation.name,
                if allowed { "" } else { "not " },
                answer.status,
                answer.body
            );
        }
    }
}
