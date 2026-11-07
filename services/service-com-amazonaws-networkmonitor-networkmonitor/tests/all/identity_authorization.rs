// SPDX-License-Identifier: CC0-1.0

//! WHO may reach CloudWatch Network Monitor's operations, driven end to end over
//! the real wire path (a restJson1 request through the multi-service router into
//! the mounted service), so what is asserted is what a client holding those
//! credentials sees.
//!
//! THE RULE UNDER TEST: `AccessDeniedException` is a service-common error of this
//! service (`smithy_query(query="service_info")` lists it under `common_errors`),
//! so EVERY operation evaluates the identity-based IAM policies in force on the
//! principal its credentials name against `networkmonitor:<OperationName>` BEFORE
//! it reads or writes anything — a request no statement allows (IAM's default
//! deny), or one an explicit `Deny` covers, is answered HTTP 403
//! `AccessDeniedException`, and nothing is created, changed or reported, while a
//! request a matching `Allow` covers reaches the operation's own answer. The
//! decision is taken against the resource the request ADDRESSES: the monitor ARN
//! for a call naming a monitor, the probe ARN for one naming a probe, the
//! `resourceArn` a tag call carries, and the resource IAM writes for a request
//! that addresses none (`*`) for the one listing operation, which therefore names
//! no resource in its refusal.
//!
//! The role, the user, their policies and the session live in the IAM and STS
//! stores of the SAME shared engine — the stores those services' own handlers
//! write — so the documents under test are the ones `iam:PutRolePolicy` stores
//! and the principal a refusal blames is the one `sts:GetCallerIdentity` answers
//! for the key the request signed with.
//!
//! The tables span what the rule generalises over: the operation (all twelve),
//! the document in force (none at all, another action of this service, another
//! service's actions, an explicit deny beside an allow, an allow of exactly the
//! action, of the action in another case, of the whole service and of
//! everything), the principal (an assumed-role session, an IAM user's long-term
//! access key and the account's own published credential), and the resource an
//! allow is scoped to (every resource, this account's monitors, one monitor,
//! another monitor).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, which is the Region every ARN this
/// service mints for it carries.
const REGION: &str = "us-west-2";

/// The access key id of the account's own published credential: a key neither STS
/// nor IAM holds a record for, which is the position AWS account root credentials
/// are in.
const ACCOUNT_KEY_ID: &str = "AKIDTESTONLY";

/// The access key id of the assumed-role session the role rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The access key id of the IAM user's long-term key the user rows sign with.
const USER_KEY_ID: &str = "AKIAEXAMPLEUSERKEY01";

/// The role the session is assumed from, the session's own name, and the IAM user.
const ROLE: &str = "monitor-writer";
const SESSION: &str = "monitor-session";
const USER_NAME: &str = "monitor-user";

/// The name of the inline policy the role and the user carry.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// A probe id of the modelled `ProbeId` form (`^probe-[a-z0-9A-Z-]{21,64}$`), for
/// the probe-shaped operations: the decision never depends on whether the probe
/// exists, so the rows need no minted id.
const PROBE_ID: &str = "probe-000000000000000000001";

/// A router serving this service beside the IAM and STS services, over ONE shared
/// engine, so the policies and the session the cases seed are the ones those
/// services hold.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_networkmonitor_networkmonitor::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
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

/// Call one route of this service signed with `access_key_id` — what names the
/// principal the request speaks for.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/networkmonitor/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let payload = body.map_or_else(bytes::Bytes::new, |b| {
        bytes::Bytes::copy_from_slice(b.as_bytes())
    });
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of an error response (`__type`), or `"<none>"`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

/// The `message` member of an error response, or `"<none>"`.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

/// An ARN as a `@httpLabel` path segment carries it.
fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ':' => "%3A".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The ARN this service mints for a monitor of the signed-for Region.
fn monitor_arn(name: &str) -> String {
    format!(
        "arn:aws:networkmonitor:{REGION}:{}:monitor/{name}",
        ::engine::emulator_account_id()
    )
}

/// The ARN this service mints for a probe of the signed-for Region.
fn probe_arn(probe_id: &str) -> String {
    format!(
        "arn:aws:networkmonitor:{REGION}:{}:probe/{probe_id}",
        ::engine::emulator_account_id()
    )
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}",
        ::engine::emulator_account_id()
    )
}

/// The ARN the account holds the role under, which is the identity whose policies
/// an assumed-role session's request is judged on.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{ROLE}",
        ::engine::emulator_account_id()
    )
}

/// The IAM user's ARN, as IAM holds it and `sts:GetCallerIdentity` reports it.
fn user_arn() -> String {
    format!(
        "arn:aws:iam::{}:user/{USER_NAME}",
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
            role_id: "AROAEXAMPLEMONITORROLE".to_string(),
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
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the
/// access key id to it. NO policy is attached.
fn given_user_key(engine: &Arc<engine::InMemoryEngine>) {
    let minted = ::aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER_NAME),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER_NAME.to_string(),
            user_id: "AIDAEXAMPLEMONITORUSER".to_string(),
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

/// One inline policy holding `document`, as `iam:PutRolePolicy` /
/// `iam:PutUserPolicy` store one.
fn inline(document: &Value) -> BTreeMap<String, iam::InlinePolicy> {
    BTreeMap::from([(
        iam::name_key(POLICY_NAME),
        iam::InlinePolicy {
            policy_name: POLICY_NAME.to_string(),
            policy_document: document.to_string(),
        },
    )])
}

/// Attach `document` to the role as its inline policy, replacing whatever it
/// carried. `None` takes every policy off it, as `iam:DeleteRolePolicy` does.
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

/// The documents a row puts in force on the principal, and which of IAM's
/// refusals the request then meets — `None` for a request a matching `Allow`
/// permits.
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
            Some(allow(json!("ec2:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": "networkmonitor:*", "Resource": "*" },
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
            // IAM compares action names case-insensitively, so the spelling of the
            // allow is not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(action.to_lowercase()))),
            None,
        ),
        (
            "an allow listing this action beside another service's",
            Some(allow(json!(["ec2:DescribeSubnets", action]))),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!("networkmonitor:*"))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// One operation under the gate: its name, the request that reaches it, and the
/// resource clause its refusal carries.
struct Operation {
    operation: &'static str,
    method: &'static str,
    path: String,
    body: Option<String>,
    clause: String,
}

/// Every operation this service serves, with a request each would otherwise
/// answer and the resource clause its refusal names.
///
/// The request has to be one the operation would otherwise REACH, and the gate
/// runs before anything is read, so a monitor that does not exist is enough: an
/// authorized row then meets the operation's own not-found rather than another
/// authorization refusal.
fn operations(name: &str) -> Vec<Operation> {
    let monitor = monitor_arn(name);
    let probe = probe_arn(PROBE_ID);
    let probe_body = format!(
        r#"{{"probe":{{"sourceArn":"arn:aws:ec2:{REGION}:{}:subnet/subnet-0123456789abcdef0","destination":"10.0.0.1","protocol":"ICMP","packetSize":56}}}}"#,
        ::engine::emulator_account_id()
    );
    vec![
        Operation {
            operation: "CreateMonitor",
            method: "POST",
            path: "/monitors".to_string(),
            body: Some(format!(r#"{{"monitorName":"{name}"}}"#)),
            clause: format!(" on resource: {monitor}"),
        },
        Operation {
            operation: "GetMonitor",
            method: "GET",
            path: format!("/monitors/{name}"),
            body: None,
            clause: format!(" on resource: {monitor}"),
        },
        Operation {
            operation: "UpdateMonitor",
            method: "PATCH",
            path: format!("/monitors/{name}"),
            body: Some(r#"{"aggregationPeriod":60}"#.to_string()),
            clause: format!(" on resource: {monitor}"),
        },
        Operation {
            operation: "DeleteMonitor",
            method: "DELETE",
            path: format!("/monitors/{name}"),
            body: None,
            clause: format!(" on resource: {monitor}"),
        },
        Operation {
            operation: "ListMonitors",
            method: "GET",
            path: "/monitors".to_string(),
            body: None,
            // A listing addresses no resource of its own, so IAM judges it
            // against `*` and the refusal names no resource.
            clause: String::new(),
        },
        Operation {
            operation: "CreateProbe",
            method: "POST",
            path: format!("/monitors/{name}/probes"),
            body: Some(probe_body),
            // A probe this call would mint has no id yet, so the resource under
            // judgement is the monitor it would be attached to.
            clause: format!(" on resource: {monitor}"),
        },
        Operation {
            operation: "GetProbe",
            method: "GET",
            path: format!("/monitors/{name}/probes/{PROBE_ID}"),
            body: None,
            clause: format!(" on resource: {probe}"),
        },
        Operation {
            operation: "UpdateProbe",
            method: "PATCH",
            path: format!("/monitors/{name}/probes/{PROBE_ID}"),
            body: Some(r#"{"state":"INACTIVE"}"#.to_string()),
            clause: format!(" on resource: {probe}"),
        },
        Operation {
            operation: "DeleteProbe",
            method: "DELETE",
            path: format!("/monitors/{name}/probes/{PROBE_ID}"),
            body: None,
            clause: format!(" on resource: {probe}"),
        },
        Operation {
            operation: "ListTagsForResource",
            method: "GET",
            path: format!("/tags/{}", encode_label(&monitor)),
            body: None,
            clause: format!(" on resource: {monitor}"),
        },
        Operation {
            operation: "TagResource",
            method: "POST",
            path: format!("/tags/{}", encode_label(&monitor)),
            body: Some(r#"{"tags":{"team":"net"}}"#.to_string()),
            clause: format!(" on resource: {monitor}"),
        },
        Operation {
            operation: "UntagResource",
            method: "DELETE",
            path: format!("/tags/{}?tagKeys=team", encode_label(&monitor)),
            body: None,
            clause: format!(" on resource: {monitor}"),
        },
    ]
}

/// Require the refusal `answer` to be IAM's, with the wording it states.
fn assert_refusal(case: &str, status: u16, answer: &Value, expected_message: &str) {
    assert_eq!(
        status, 403,
        "case {case:?}: expected HTTP 403, got {status} {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        "AccessDeniedException",
        "case {case:?}: expected an AccessDeniedException, got {answer:?}"
    );
    assert_eq!(
        error_message(answer),
        expected_message,
        "case {case:?}: expected IAM's wording, got {answer:?}"
    );
}

/// THE RULE: every operation is judged against its OWN
/// `networkmonitor:<OperationName>` action before it reads or writes anything; a
/// refusal is HTTP 403 `AccessDeniedException` naming the principal, the action,
/// the resource the request addressed and which of IAM's rules refused it, and an
/// allowed call reaches the operation's own answer rather than an authorization
/// failure.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action() {
    const NAME: &str = "authorized";
    let names: Vec<&'static str> = operations(NAME)
        .iter()
        .map(|operation| operation.operation)
        .collect();

    for operation in operations(NAME) {
        let action = format!("networkmonitor:{}", operation.operation);
        // Another action of this service, which a row grants instead of the one
        // under test.
        let other = names
            .iter()
            .map(|name| format!("networkmonitor:{name}"))
            .find(|candidate| *candidate != action)
            .expect("this service has more than one operation");

        for (row, document, refused) in documents_in_force(&action, &other) {
            let case = format!("{} with {row}", operation.operation);
            // A fresh store, role and session per row: no row can pass on
            // another's write.
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(&engine, document.as_ref());

            let (status, answer) = call_as(
                &router,
                SESSION_KEY_ID,
                operation.method,
                &operation.path,
                operation.body.as_deref(),
            )
            .await;
            match refused {
                Some(refusal) => {
                    assert_refusal(
                        &case,
                        status,
                        &answer,
                        &format!(
                            "User: {} is not authorized to perform: {action}{} {}",
                            session_arn(),
                            operation.clause,
                            refusal.reason(&action)
                        ),
                    );
                    // The refusal happened before any state was touched: the
                    // account's own credential still finds nothing stored.
                    let (status, listed) =
                        call_as(&router, ACCOUNT_KEY_ID, "GET", "/monitors", None).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected ListMonitors 200, got {status} {listed:?}"
                    );
                    assert_eq!(
                        listed.get("monitors").and_then(Value::as_array),
                        Some(&vec![]),
                        "case {case:?}: a refused call must store nothing, got {listed:?}"
                    );
                }
                None => {
                    assert_ne!(
                        status, 403,
                        "case {case:?}: an allowed request must not be refused, got {answer:?}"
                    );
                    assert_ne!(
                        error_code(&answer),
                        "AccessDeniedException",
                        "case {case:?}: an allowed request must not be refused, got {answer:?}"
                    );
                }
            }
        }
    }
}

/// THE RECORDED CASE: with an inline `Allow` the assumed-role caller creates a
/// monitor; once an explicit `Deny` for the same action is attached on top the
/// identical call is refused with 403 and creates NOTHING — a follow-up read by
/// the account's own credential finds no monitor — and removing the `Deny`
/// restores success for the same identity.
#[tokio::test]
async fn an_explicit_deny_refuses_create_monitor_and_creates_nothing() {
    let (router, engine) = router_with_identity();
    given_session(&engine);

    let allowed = allow(json!("networkmonitor:CreateMonitor"));
    let denied = json!({
        "Statement": [
            { "Effect": "Allow", "Action": "networkmonitor:CreateMonitor", "Resource": "*" },
            { "Effect": "Deny", "Action": "networkmonitor:CreateMonitor", "Resource": "*" },
        ],
    });

    // The control: the grant alone creates the monitor, and it is readable.
    given_role_policy(&engine, Some(&allowed));
    let (status, created) = call_as(
        &router,
        SESSION_KEY_ID,
        "POST",
        "/monitors",
        Some(r#"{"monitorName":"allowed-first"}"#),
    )
    .await;
    assert_eq!(
        status, 200,
        "an allowed CreateMonitor must be served, got {status} {created:?}"
    );
    assert_eq!(
        created.get("monitorArn").and_then(Value::as_str),
        Some(monitor_arn("allowed-first").as_str()),
        "the created monitor's ARN names this account and Region, got {created:?}"
    );

    // The deny on top: the identical call is refused and nothing is created.
    given_role_policy(&engine, Some(&denied));
    let (status, refused) = call_as(
        &router,
        SESSION_KEY_ID,
        "POST",
        "/monitors",
        Some(r#"{"monitorName":"denied"}"#),
    )
    .await;
    let action = "networkmonitor:CreateMonitor";
    assert_refusal(
        "an explicit deny of CreateMonitor",
        status,
        &refused,
        &format!(
            "User: {} is not authorized to perform: {action} on resource: {} \
             with an explicit deny in an identity-based policy",
            session_arn(),
            monitor_arn("denied")
        ),
    );
    let (status, gone) = call_as(&router, ACCOUNT_KEY_ID, "GET", "/monitors/denied", None).await;
    assert_eq!(
        status, 404,
        "a refused create stores nothing, so the admin read is 404, got {status} {gone:?}"
    );
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {gone:?}"
    );

    // The deny removed: the same identity succeeds again.
    given_role_policy(&engine, Some(&allowed));
    let (status, again) = call_as(
        &router,
        SESSION_KEY_ID,
        "POST",
        "/monitors",
        Some(r#"{"monitorName":"denied"}"#),
    )
    .await;
    assert_eq!(
        status, 200,
        "removing the deny restores the grant, got {status} {again:?}"
    );
    let (status, read) = call_as(&router, ACCOUNT_KEY_ID, "GET", "/monitors/denied", None).await;
    assert_eq!(
        status, 200,
        "the monitor the restored grant created is readable, got {status} {read:?}"
    );
}

/// RULE: an IAM user's long-term access key speaks for THAT user, so the decision
/// is taken on the user's own policies and the refusal names the user's ARN — the
/// same rule the session rows exercise, under the other principal form.
#[tokio::test]
async fn an_iam_user_key_is_judged_on_the_users_own_policies() {
    const NAME: &str = "user-judged";
    // (case, the document in force, the refusal it meets)
    let rows: Vec<(&str, Option<Value>, Option<Refusal>)> = vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an explicit deny of this service",
            Some(json!({
                "Statement": [
                    { "Effect": "Deny", "Action": "networkmonitor:*", "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!("networkmonitor:*"))),
            None,
        ),
    ];

    for operation in operations(NAME) {
        let action = format!("networkmonitor:{}", operation.operation);
        for (row, document, refused) in &rows {
            let case = format!("{} as an IAM user with {row}", operation.operation);
            let (router, engine) = router_with_identity();
            given_user_key(&engine);
            given_user_policy(&engine, document.as_ref());

            let (status, answer) = call_as(
                &router,
                USER_KEY_ID,
                operation.method,
                &operation.path,
                operation.body.as_deref(),
            )
            .await;
            match refused {
                Some(refusal) => assert_refusal(
                    &case,
                    status,
                    &answer,
                    &format!(
                        "User: {} is not authorized to perform: {action}{} {}",
                        user_arn(),
                        operation.clause,
                        refusal.reason(&action)
                    ),
                ),
                None => assert_ne!(
                    status, 403,
                    "case {case:?}: an allowed request must not be refused, got {answer:?}"
                ),
            }
        }
    }
}

/// RULE: the account's own published credential — a key neither STS nor IAM holds
/// a record for — is not a principal the IAM store holds policies for, so it is
/// served whatever the role's policies say.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    const NAME: &str = "account-served";
    for operation in operations(NAME) {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        // The role carries nothing at all, which is what refuses the SESSION.
        given_role_policy(&engine, None);

        let (status, answer) = call_as(
            &router,
            ACCOUNT_KEY_ID,
            operation.method,
            &operation.path,
            operation.body.as_deref(),
        )
        .await;
        assert_ne!(
            status, 403,
            "case {:?}: the account's own credential must not be refused, got {answer:?}",
            operation.operation
        );
    }
}

/// RULE: a monitor-shaped operation is judged against the ARN OF THE MONITOR the
/// request names, so an allow scoped to one monitor covers that monitor and no
/// other.
#[tokio::test]
async fn a_monitor_operation_is_judged_against_the_monitor_the_request_names() {
    // (case, the Resource the allow is scoped to, whether the call is allowed)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("an allow of every resource", json!("*"), true),
        (
            "an allow of exactly this monitor",
            json!(monitor_arn("scoped")),
            true,
        ),
        (
            "an allow of every monitor of this account",
            json!(format!(
                "arn:aws:networkmonitor:{REGION}:{}:monitor/*",
                ::engine::emulator_account_id()
            )),
            true,
        ),
        (
            "an allow of another monitor only",
            json!(monitor_arn("elsewhere")),
            false,
        ),
        (
            "an allow of this account's probes only",
            json!(format!(
                "arn:aws:networkmonitor:{REGION}:{}:probe/*",
                ::engine::emulator_account_id()
            )),
            false,
        ),
    ];

    for (case, resource, allowed) in rows {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        given_role_policy(
            &engine,
            Some(&json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "networkmonitor:CreateMonitor",
                    "Resource": resource,
                }],
            })),
        );

        let (status, answer) = call_as(
            &router,
            SESSION_KEY_ID,
            "POST",
            "/monitors",
            Some(r#"{"monitorName":"scoped"}"#),
        )
        .await;
        if allowed {
            assert_eq!(
                status, 200,
                "case {case:?}: an allowed create must be served, got {status} {answer:?}"
            );
            assert_eq!(
                answer.get("monitorName").and_then(Value::as_str),
                Some("scoped"),
                "case {case:?}: expected the created monitor, got {answer:?}"
            );
        } else {
            let action = "networkmonitor:CreateMonitor";
            assert_refusal(
                case,
                status,
                &answer,
                &format!(
                    "User: {} is not authorized to perform: {action} on resource: {} \
                     because no identity-based policy allows the {action} action",
                    session_arn(),
                    monitor_arn("scoped")
                ),
            );
            let (status, gone) =
                call_as(&router, ACCOUNT_KEY_ID, "GET", "/monitors/scoped", None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused create stores nothing, got {status} {gone:?}"
            );
        }
    }
}
