// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's operations, driven end to end over the real wire path (a
//! restJson1 request through the multi-service router into the mounted service), so what
//! is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: `AccessDeniedException` is a service-common error of Amazon
//! Managed Grafana (`smithy_query(query="digest")` lists it under `common_errors`), so
//! EVERY operation evaluates the identity-based IAM policies in force on the principal its
//! credentials name against `grafana:<OperationName>` BEFORE it reads or writes the
//! workspace store — a request no statement allows (IAM's default deny), or one an
//! explicit `Deny` covers, is answered HTTP 403 `AccessDeniedException` and changes
//! nothing, while a request a matching `Allow` covers reaches the operation's own answer.
//! The refusal names the principal, the action and the resource the request addressed: the
//! ARN of the workspace it named, or of the account's workspaces collection for an
//! operation that addresses no single workspace.
//!
//! The role, the user, their policies and the session live in the IAM and STS stores of
//! the SAME shared engine — the stores those services' own handlers write — so the
//! documents under test are the ones `iam:PutRolePolicy` stores and the principal a
//! refusal blames is the one `sts:GetCallerIdentity` answers for the key the request
//! signed with.
//!
//! The tables span what the rule generalises over: the operation (a create, a read, an
//! update, a delete and a listing, which is every shape of call this service serves), the
//! document in force (none at all, another action of this service, another service's
//! actions, an explicit deny beside an allow, an allow of exactly the action, of the
//! action in another case, of the whole service and of everything), the principal (an
//! assumed-role session, an IAM user's long-term access key and the account's own
//! published credential) and the resource an allow is scoped to (every resource, this
//! account's workspaces, one workspace, another workspace).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state as grafana;
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, and so the Region of every ARN a refusal
/// names.
const REGION: &str = "eu-west-1";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDTEST";

/// The access key id of the assumed-role session the role rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The access key id of the IAM user's long-term key the user rows sign with.
const USER_KEY_ID: &str = "AKIAEXAMPLEUSERKEY01";

/// The role the session is assumed from, the session's own name, and the IAM user.
const ROLE: &str = "grafana-operator";
const SESSION: &str = "grafana-session";
const USER_NAME: &str = "grafana-user";

/// The name of the inline policy the role and the user carry.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// A router serving this service beside the IAM and STS services, over ONE shared engine,
/// so the policies and the session the cases seed are the ones those services hold.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
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

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// Call one route of this service signed with `access_key_id` — what names the principal
/// the request speaks for.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "grafana.eu-west-1.amazonaws.com")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/grafana/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
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

/// The IAM user's ARN, as IAM holds it and `sts:GetCallerIdentity` reports it.
fn user_arn() -> String {
    format!(
        "arn:aws:iam::{}:user/{USER_NAME}",
        ::engine::emulator_account_id()
    )
}

/// The ARN of the account's workspaces collection in this Region, spelled out here rather
/// than imported: Amazon Managed Grafana's resource section carries a leading `/`
/// (IAM Service Authorization Reference,
/// `arn:${Partition}:grafana:${Region}:${Account}:/workspaces/${ResourceId}`).
fn workspaces_arn() -> String {
    format!(
        "arn:aws:grafana:{REGION}:{}:/workspaces",
        ::engine::emulator_account_id()
    )
}

/// The ARN of one workspace in this Region.
fn workspace_arn(id: &str) -> String {
    format!("{}/{id}", workspaces_arn())
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
            role_id: "AROAEXAMPLEGRAFANAROLE".to_string(),
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

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key
/// id to it. NO policy is attached.
fn given_user_key(engine: &Arc<engine::InMemoryEngine>) {
    let minted = ::aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER_NAME),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER_NAME.to_string(),
            user_id: "AIDAEXAMPLEGRAFANAUSER".to_string(),
            arn: user_arn(),
            create_date: minted,
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(USER_NAME),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: minted,
        },
    );
}

/// One inline policy holding `document`, as `iam:PutRolePolicy` / `iam:PutUserPolicy`
/// store one.
fn inline(document: &Value) -> BTreeMap<String, iam::InlinePolicy> {
    BTreeMap::from([(
        iam::name_key(POLICY_NAME),
        iam::InlinePolicy {
            policy_name: POLICY_NAME.to_string(),
            policy_document: document.to_string(),
        },
    )])
}

/// Attach `document` to the role as its inline policy, replacing whatever it carried.
/// `None` takes every policy off it, as `iam:DeleteRolePolicy` does.
fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.role_inline_policies.remove(&iam::name_key(ROLE));
    if let Some(document) = document {
        store
            .role_inline_policies
            .insert(iam::name_key(ROLE), inline(document));
    }
}

/// The same for the IAM user, as `iam:PutUserPolicy` / `iam:DeleteUserPolicy` do.
fn given_user_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.user_inline_policies.remove(&iam::name_key(USER_NAME));
    if let Some(document) = document {
        store
            .user_inline_policies
            .insert(iam::name_key(USER_NAME), inline(document));
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

/// The documents a row puts in force on the principal, and which of IAM's refusals the
/// request then meets — `None` for a request a matching `Allow` permits.
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
            Some(allow(json!("s3:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": "grafana:*", "Resource": "*" },
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
            // IAM compares action names case-insensitively, so the spelling of the allow
            // is not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(action.to_lowercase()))),
            None,
        ),
        (
            "an allow listing this action beside another service's",
            Some(allow(json!(["s3:GetObject", action]))),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!("grafana:*"))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// One call under test: the operation's name, how to send it, and the resource clause its
/// refusal has to name.
struct Call {
    operation: &'static str,
    method: &'static str,
    /// The URI, with `{id}` standing for the fixture workspace's id.
    uri: &'static str,
    body: Option<Value>,
    /// Whether the resource the refusal names is the workspace (`true`) or the account's
    /// workspaces collection (`false`).
    about_one_workspace: bool,
    /// A member a served answer carries, so an authorized row is checked to have reached
    /// the operation itself.
    served_member: &'static str,
}

/// Every shape of call this service serves: a create, a read, an update, a delete and a
/// listing.
fn calls() -> Vec<Call> {
    vec![
        Call {
            operation: "CreateWorkspace",
            method: "POST",
            uri: "/workspaces",
            body: Some(json!({
                "accountAccessType": "CURRENT_ACCOUNT",
                "permissionType": "CUSTOMER_MANAGED",
                "authenticationProviders": ["AWS_SSO"],
            })),
            about_one_workspace: false,
            served_member: "workspace",
        },
        Call {
            operation: "ListWorkspaces",
            method: "GET",
            uri: "/workspaces",
            body: None,
            about_one_workspace: false,
            served_member: "workspaces",
        },
        Call {
            operation: "DescribeWorkspace",
            method: "GET",
            uri: "/workspaces/{id}",
            body: None,
            about_one_workspace: true,
            served_member: "workspace",
        },
        Call {
            operation: "UpdateWorkspace",
            method: "PUT",
            uri: "/workspaces/{id}",
            body: Some(json!({"workspaceDescription": "authorized"})),
            about_one_workspace: true,
            served_member: "workspace",
        },
        Call {
            operation: "DeleteWorkspace",
            method: "DELETE",
            uri: "/workspaces/{id}",
            body: None,
            about_one_workspace: true,
            served_member: "workspace",
        },
    ]
}

/// Create the fixture workspace the single-workspace rows address, with the account's own
/// credential — which this gate does not bound, so the fixture exists whatever policy the
/// row then puts in force.
///
/// The record is handed over SETTLED: a change to a workspace whose create is still in
/// flight is refused as a conflict, and these cases are about WHO may call rather than
/// about the transition span, so the status change the clock owes it is materialised at
/// once instead of waited for.
async fn given_workspace(router: &MultiServiceRouter, engine: &Arc<engine::InMemoryEngine>) -> String {
    let created = call_as(
        router,
        ACCOUNT_KEY_ID,
        "POST",
        "/workspaces",
        Some(json!({
            "accountAccessType": "CURRENT_ACCOUNT",
            "permissionType": "CUSTOMER_MANAGED",
            "authenticationProviders": ["AWS_SSO"],
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the account's own credential must be able to create the fixture, got {} with {:?}",
        created.status, created.body
    );
    settle_the_clock(engine);
    created.body["workspace"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWorkspace must answer an id: {:?}", created.body))
        .to_string()
}

/// Materialise every status change the clock owes the workspaces in the store, by settling
/// them at an instant past the transition span. Nothing sleeps and nothing is written that
/// the clock did not already owe.
fn settle_the_clock(engine: &Arc<engine::InMemoryEngine>) {
    let store = grafana::service_state(&**engine).expect("the grafana store");
    store
        .lock()
        .settle_all(SystemTime::now() + grafana::LIFECYCLE_TRANSITION);
}

/// How many workspaces the account holds, read with the account's own credential.
async fn workspace_count(router: &MultiServiceRouter) -> usize {
    let listed = call_as(router, ACCOUNT_KEY_ID, "GET", "/workspaces", None).await;
    listed.body["workspaces"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// Require the refusal `answer` to be IAM's, naming the principal, action and resource.
fn assert_refusal(case: &str, answer: &Answer, expected_message: &str) {
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected HTTP 403, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "AccessDeniedException",
        "case {case:?}: expected an AccessDeniedException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.message(),
        expected_message,
        "case {case:?}: expected IAM's wording, got {:?}",
        answer.message()
    );
}

/// THE RULE: every operation is judged against its OWN `grafana:<OperationName>` action
/// before it touches the store; a refusal is HTTP 403 `AccessDeniedException` naming the
/// principal, the action, the resource the request addressed and which of IAM's rules
/// refused it, an allowed call reaches the operation's own answer, and a refused call
/// changes nothing.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action() {
    for call in calls() {
        let action = format!("grafana:{}", call.operation);
        // Another action of this service, which a row grants instead of the one under test.
        let other = calls()
            .into_iter()
            .map(|candidate| format!("grafana:{}", candidate.operation))
            .find(|candidate| *candidate != action)
            .expect("this service has more than one operation");

        for (row, document, refused) in documents_in_force(&action, &other) {
            let case = format!("{} with {row}", call.operation);
            // A fresh store, role and session per row: no row can pass on another's write.
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(&engine, document.as_ref());
            let id = given_workspace(&router, &engine).await;
            let before = workspace_count(&router).await;

            let resource = if call.about_one_workspace {
                workspace_arn(&id)
            } else {
                workspaces_arn()
            };
            let answer = call_as(
                &router,
                SESSION_KEY_ID,
                call.method,
                &call.uri.replace("{id}", &id),
                call.body.clone(),
            )
            .await;

            match refused {
                Some(refusal) => {
                    assert_refusal(
                        &case,
                        &answer,
                        &format!(
                            "User: {} is not authorized to perform: {action} on resource: \
                             {resource} {}",
                            session_arn(),
                            refusal.reason(&action)
                        ),
                    );
                    // The refusal carries no part of the operation's own answer, and the
                    // store is as it was: a refused call reports nothing and changes
                    // nothing.
                    assert!(
                        answer.body.get(call.served_member).is_none(),
                        "case {case:?}: a refused call must report nothing about the \
                         workspaces, got {:?}",
                        answer.body
                    );
                    assert_eq!(
                        workspace_count(&router).await,
                        before,
                        "case {case:?}: a refused call must leave the store as it was"
                    );
                }
                None => {
                    assert!(
                        answer.status < 300,
                        "case {case:?}: an authorized call must reach the operation, got {} \
                         with {:?}",
                        answer.status,
                        answer.body
                    );
                    assert!(
                        answer.body.get(call.served_member).is_some(),
                        "case {case:?}: the served answer must carry {:?}, got {:?}",
                        call.served_member,
                        answer.body
                    );
                }
            }
        }
    }
}

/// THE RULE for the other two principals a request can speak for: an IAM user's long-term
/// key is judged on the USER's policies, and the account's own published credential — the
/// key neither STS nor IAM holds a record for — is not a principal this emulator holds
/// policies for and is served.
#[tokio::test]
async fn a_user_key_is_judged_on_its_own_policies_and_the_account_key_is_served() {
    let action = "grafana:CreateWorkspace";
    let request = json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": "CUSTOMER_MANAGED",
        "authenticationProviders": ["AWS_SSO"],
    });

    // (case, the user's policy, the refusal it meets)
    let rows: Vec<(&str, Option<Value>, Option<Refusal>)> = vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of another service only",
            Some(allow(json!("s3:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of this service's actions",
            Some(allow(json!("grafana:*"))),
            None,
        ),
    ];

    for (row, document, refused) in rows {
        let case = format!("the IAM user with {row}");
        let (router, engine) = router_with_identity();
        given_user_key(&engine);
        given_user_policy(&engine, document.as_ref());

        let answer = call_as(
            &router,
            USER_KEY_ID,
            "POST",
            "/workspaces",
            Some(request.clone()),
        )
        .await;
        match refused {
            Some(refusal) => {
                assert_refusal(
                    &case,
                    &answer,
                    &format!(
                        "User: {} is not authorized to perform: {action} on resource: {} {}",
                        user_arn(),
                        workspaces_arn(),
                        refusal.reason(action)
                    ),
                );
                assert_eq!(
                    workspace_count(&router).await,
                    0,
                    "case {case:?}: a refused create must mint no workspace"
                );
            }
            None => {
                assert_eq!(
                    answer.status, 202,
                    "case {case:?}: an allowed create must be accepted, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    workspace_count(&router).await,
                    1,
                    "case {case:?}: an allowed create must mint exactly one workspace"
                );
            }
        }
    }

    // The account's own published credential holds no IAM policies at all, and is served.
    let (router, _engine) = router_with_identity();
    let served = call_as(&router, ACCOUNT_KEY_ID, "POST", "/workspaces", Some(request)).await;
    assert_eq!(
        served.status, 202,
        "the account's own credential must be served, got {} with {:?}",
        served.status, served.body
    );
}

/// THE RULE for the resource half of the decision: an allow scoped to a RESOURCE covers
/// only the calls that address that resource, and the ARN it is matched against is the one
/// built from the request.
#[tokio::test]
async fn an_allow_is_scoped_to_the_resource_the_request_addresses() {
    let (router, engine) = router_with_identity();
    given_session(&engine);
    let id = given_workspace(&router, &engine).await;
    let another = "g-ffffffffff";

    // (case, the Resource element of the allow, whether DescribeWorkspace on `id` passes)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("every resource", json!("*"), true),
        (
            "every workspace of this account",
            json!(format!("{}/*", workspaces_arn())),
            true,
        ),
        ("exactly this workspace", json!(workspace_arn(&id)), true),
        ("another workspace", json!(workspace_arn(another)), false),
        (
            "the workspaces collection itself, which is not one workspace",
            json!(workspaces_arn()),
            false,
        ),
    ];

    for (case, resource, allowed) in rows {
        given_role_policy(
            &engine,
            Some(&json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "grafana:DescribeWorkspace",
                    "Resource": resource,
                }],
            })),
        );
        let answer = call_as(
            &router,
            SESSION_KEY_ID,
            "GET",
            &format!("/workspaces/{id}"),
            None,
        )
        .await;
        if allowed {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the allow must cover this call, got {} with {:?}",
                answer.status, answer.body
            );
        } else {
            assert_refusal(
                case,
                &answer,
                &format!(
                    "User: {} is not authorized to perform: grafana:DescribeWorkspace on \
                     resource: {} because no identity-based policy allows the \
                     grafana:DescribeWorkspace action",
                    session_arn(),
                    workspace_arn(&id)
                ),
            );
        }
    }
}
