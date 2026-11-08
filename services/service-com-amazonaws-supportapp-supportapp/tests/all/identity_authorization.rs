// SPDX-License-Identifier: CC0-1.0

//! WHO may reach the Support App control plane, driven end to end over the real wire path
//! (a restJson1 request through the multi-service router into the mounted service), so what
//! is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: every operation of this API is a signed call (`sigv4`, with no
//! `@optionalAuth` operation in the model — `smithy_query(query="service_info")`), and AWS
//! decides such a call at the IAM layer, in FRONT of the service: the identity-based policies
//! in force on the principal the request's credentials name are evaluated against
//! `supportapp:<OperationName>` on the resource the operation acts on BEFORE the operation
//! reads or writes anything. A request no statement allows (IAM's default deny), or one an
//! explicit `Deny` covers, is answered HTTP 403 `AccessDeniedException` naming the principal,
//! the action, the resource and which of IAM's rules refused it — and nothing is created,
//! changed or reported — while a request a matching `Allow` covers reaches the operation's own
//! answer.
//!
//! The resource is the operation's own modelled URI in this service's ARN namespace
//! (`arn:aws:supportapp:<region>:<account>:/control/<operation-uri>`), because this API
//! publishes no resource types of its own: every operation is a POST to its own path under
//! `/control` (`smithy_query(query="digest")`) and the model declares no ARN member.
//!
//! The role, the user, their policies and the session live in the IAM and STS stores of the
//! SAME shared engine — the stores those services' own handlers write — so the documents under
//! test are the ones `iam:PutRolePolicy` stores and the principal a refusal blames is the one
//! `sts:GetCallerIdentity` answers for the key the request signed with.
//!
//! The tables span what the rule generalises over: the operation (all ten), the document in
//! force (none at all, another action of this service, another service's actions, an explicit
//! deny beside an allow, an allow of exactly the action, of the action in another case, of the
//! whole service, of everything, and one scoped to this operation's resource or to another
//! operation's), and the principal (an assumed-role session, an IAM user's long-term access
//! key and the account's own published credential).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use service_com_amazonaws_supportapp_supportapp::state::{action_for, operation_resource_arn};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, which is the Region every resource ARN the
/// decision is made about carries.
const REGION: &str = "us-west-2";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDTESTONLY";

/// The access key id of the assumed-role session the role rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The access key id of the IAM user's long-term key the user rows sign with.
const USER_KEY_ID: &str = "AKIAEXAMPLEUSERKEY01";

/// The role the session is assumed from, the session's own name, and the IAM user.
const ROLE: &str = "support-app-reader";
const SESSION: &str = "support-app-session";
const USER_NAME: &str = "support-app-user";

/// The name of the inline policy the role and the user carry.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// The Slack ids and the alias the request bodies below carry. No row depends on anything
/// being stored: the decision is taken before the store is reached.
const TEAM_ID: &str = "T012ABCDEFG";
const CHANNEL_ID: &str = "C01234A5BCD";
const ALIAS: &str = "support-app";

/// A router serving this service beside the IAM and STS services, over ONE shared engine, so
/// the policies and the session the cases seed are the ones those services hold.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still routes by the SigV4
    // credential scope, and the access key id in it is what names the principal.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_supportapp_supportapp::EmulatedService::new(Arc::clone(&engine)),
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

/// Call one operation of this service signed with `access_key_id` — what names the principal
/// the request speaks for.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("http://localhost{uri}"))
        .header("host", format!("supportapp.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/supportapp/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize body"),
        ))
        .expect("a well-formed test request");
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

/// The wire error code of an error response (`__type` on restJson1), or `"<none>"`.
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

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{ROLE}/{SESSION}",
        engine::emulator_account_id()
    )
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/{ROLE}", engine::emulator_account_id())
}

/// The IAM user's ARN, as IAM holds it and `sts:GetCallerIdentity` reports it.
fn user_arn() -> String {
    format!(
        "arn:aws:iam::{}:user/{USER_NAME}",
        engine::emulator_account_id()
    )
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them. The role carries NO identity-based
/// policy — only the trust policy that lets it be assumed, which is not a grant of anything.
fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let account = engine::emulator_account_id();
    let role = role_arn();
    let trust_policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{account}:root") },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLESUPPORTAPP".to_string(),
            arn: role.clone(),
            create_date: aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: Some(trust_policy),
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role),
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
        .insert(vend.into_session(SESSION_KEY_ID, SystemTime::now()))
        .expect("the session is recorded");
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key id
/// to it. NO policy is attached.
fn given_user_key(engine: &Arc<engine::InMemoryEngine>) {
    let minted = aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER_NAME),
        service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER_NAME.to_string(),
            user_id: "AIDAEXAMPLESUPPORTAPP".to_string(),
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

/// One inline policy holding `document`, as `iam:PutRolePolicy` / `iam:PutUserPolicy` store
/// one.
fn inline(document: &Value) -> BTreeMap<String, iam::InlinePolicy> {
    BTreeMap::from([(
        iam::name_key(POLICY_NAME),
        iam::InlinePolicy {
            policy_name: POLICY_NAME.to_string(),
            policy_document: document.to_string(),
        },
    )])
}

/// Attach `document` to the role as its inline policy, replacing whatever it carried. `None`
/// takes every policy off it, as `iam:DeleteRolePolicy` does.
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

/// Every operation this service binds, with the URI it is bound to and a request body the
/// model accepts. The decision is taken before the input is used, so a body needs only to
/// satisfy the modelled constraints.
fn operations() -> Vec<(&'static str, String, Value)> {
    let role = format!(
        "arn:aws:iam::{}:role/SupportAppRole",
        engine::emulator_account_id()
    );
    vec![
        (
            "CreateSlackChannelConfiguration",
            "/control/create-slack-channel-configuration".to_string(),
            json!({
                "teamId": TEAM_ID,
                "channelId": CHANNEL_ID,
                "notifyOnCaseSeverity": "none",
                "channelRoleArn": role,
            }),
        ),
        (
            "DeleteAccountAlias",
            "/control/delete-account-alias".to_string(),
            json!({}),
        ),
        (
            "DeleteSlackChannelConfiguration",
            "/control/delete-slack-channel-configuration".to_string(),
            json!({ "teamId": TEAM_ID, "channelId": CHANNEL_ID }),
        ),
        (
            "DeleteSlackWorkspaceConfiguration",
            "/control/delete-slack-workspace-configuration".to_string(),
            json!({ "teamId": TEAM_ID }),
        ),
        (
            "GetAccountAlias",
            "/control/get-account-alias".to_string(),
            json!({}),
        ),
        (
            "ListSlackChannelConfigurations",
            "/control/list-slack-channel-configurations".to_string(),
            json!({}),
        ),
        (
            "ListSlackWorkspaceConfigurations",
            "/control/list-slack-workspace-configurations".to_string(),
            json!({}),
        ),
        (
            "PutAccountAlias",
            "/control/put-account-alias".to_string(),
            json!({ "accountAlias": ALIAS }),
        ),
        (
            "RegisterSlackWorkspaceForOrganization",
            "/control/register-slack-workspace-for-organization".to_string(),
            json!({ "teamId": TEAM_ID }),
        ),
        (
            "UpdateSlackChannelConfiguration",
            "/control/update-slack-channel-configuration".to_string(),
            json!({ "teamId": TEAM_ID, "channelId": CHANNEL_ID, "channelName": "aws-support" }),
        ),
    ]
}

/// The documents a row puts in force on the principal, and which of IAM's refusals the
/// request then meets — `None` for a request a matching `Allow` permits.
fn documents_in_force(
    action: &str,
    other_action: &str,
    resource: &str,
    other_resource: &str,
) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of another action of this service, but not this one",
            Some(allow(json!(other_action))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of every action of another service",
            Some(allow(json!("support:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of this action scoped to another operation's resource",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": other_resource },
                ],
            })),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": "supportapp:*", "Resource": "*" },
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
            "an allow of this action scoped to this operation's own resource",
            Some(json!({
                "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }],
            })),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!("supportapp:*"))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// Require `(status, body)` to be the IAM refusal naming `principal`, `action`, `resource`
/// and `refusal`'s own clause.
fn assert_refused(
    case: &str,
    answer: &(u16, Value),
    principal: &str,
    action: &str,
    resource: &str,
    refusal: Refusal,
) {
    let (status, body) = answer;
    assert_eq!(
        *status, 403,
        "case {case:?}: expected HTTP 403, got {status} / {body}"
    );
    let code = error_code(body);
    assert_eq!(
        code, "AccessDeniedException",
        "case {case:?}: expected AccessDeniedException, got {code:?} / {body}"
    );
    let message = error_message(body);
    let expected = format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {}",
        refusal.reason(action)
    );
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// Require `(status, body)` NOT to be an authorization refusal: the operation's own answer,
/// whatever it is.
fn assert_not_refused(case: &str, answer: &(u16, Value)) {
    let (status, body) = answer;
    assert_ne!(
        *status, 403,
        "case {case:?}: expected the operation's own answer, got HTTP 403 / {body}"
    );
    let code = error_code(body);
    assert_ne!(
        code, "AccessDeniedException",
        "case {case:?}: expected the operation's own answer, got {code:?} / {body}"
    );
}

#[tokio::test]
async fn every_operation_is_decided_against_the_sessions_role_policies() {
    for (operation, uri, body) in operations() {
        let action = action_for(operation);
        let resource = operation_resource_arn(REGION, operation);
        // Another action of this service, and another operation's resource, for the rows that
        // need a grant this request is NOT covered by.
        let other_operation = if operation == "GetAccountAlias" {
            "PutAccountAlias"
        } else {
            "GetAccountAlias"
        };
        let other_action = action_for(other_operation);
        let other_resource = operation_resource_arn(REGION, other_operation);

        for (document_case, document, refusal) in
            documents_in_force(&action, &other_action, &resource, &other_resource)
        {
            let case = format!("{operation} with {document_case}");
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(&engine, document.as_ref());

            let answer = call_as(&router, SESSION_KEY_ID, &uri, &body).await;
            match refusal {
                Some(refusal) => {
                    assert_refused(&case, &answer, &session_arn(), &action, &resource, refusal);
                }
                None => assert_not_refused(&case, &answer),
            }
        }
    }
}

#[tokio::test]
async fn an_iam_users_long_term_key_is_decided_against_the_users_policies() {
    // The same rule for the other principal a request can speak for: a long-term access key
    // belongs to an IAM user, and the user's own policies decide the call.
    for (operation, uri, body) in operations() {
        let action = action_for(operation);
        let resource = operation_resource_arn(REGION, operation);

        let (router, engine) = router_with_identity();
        given_user_key(&engine);
        given_user_policy(&engine, None);
        let refused = call_as(&router, USER_KEY_ID, &uri, &body).await;
        assert_refused(
            &format!("{operation} as a user with no policy"),
            &refused,
            &user_arn(),
            &action,
            &resource,
            Refusal::DefaultDeny,
        );

        let allowing = allow(json!(action));
        given_user_policy(&engine, Some(&allowing));
        let served = call_as(&router, USER_KEY_ID, &uri, &body).await;
        assert_not_refused(
            &format!("{operation} as a user the policy allows"),
            &served,
        );
    }
}

#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    // A request whose access key id neither STS nor IAM holds a record for speaks for the
    // account itself, which holds no identity-based policies — the position AWS account root
    // credentials are in — so it is served.
    for (operation, uri, body) in operations() {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        given_role_policy(&engine, None);
        let answer = call_as(&router, ACCOUNT_KEY_ID, &uri, &body).await;
        assert_not_refused(
            &format!("{operation} as the account's own credential"),
            &answer,
        );
    }
}

#[tokio::test]
async fn a_refused_call_changes_nothing_and_reports_nothing() {
    // The decision is taken before the store is reached, so a refused write leaves the
    // account exactly as it was and a refused read tells the caller nothing about it.
    let (router, engine) = router_with_identity();
    given_session(&engine);

    // With the alias-writing action allowed, the alias is put and read back.
    given_role_policy(&engine, Some(&allow(json!("supportapp:PutAccountAlias"))));
    let put = call_as(
        &router,
        SESSION_KEY_ID,
        "/control/put-account-alias",
        &json!({ "accountAlias": ALIAS }),
    )
    .await;
    assert_eq!(
        put.0, 200,
        "case \"the allowed put\": expected HTTP 200, got {} / {}",
        put.0, put.1
    );

    // Now the role may do nothing: the read is refused rather than answered, and the write
    // the same.
    given_role_policy(&engine, None);
    let get_action = action_for("GetAccountAlias");
    let refused_read = call_as(
        &router,
        SESSION_KEY_ID,
        "/control/get-account-alias",
        &json!({}),
    )
    .await;
    assert_refused(
        "the refused read",
        &refused_read,
        &session_arn(),
        &get_action,
        &operation_resource_arn(REGION, "GetAccountAlias"),
        Refusal::DefaultDeny,
    );
    assert!(
        refused_read.1.get("accountAlias").is_none(),
        "case \"the refused read\": expected no account alias in the refusal, got {}",
        refused_read.1
    );
    let refused_write = call_as(
        &router,
        SESSION_KEY_ID,
        "/control/delete-account-alias",
        &json!({}),
    )
    .await;
    assert_refused(
        "the refused delete",
        &refused_write,
        &session_arn(),
        &action_for("DeleteAccountAlias"),
        &operation_resource_arn(REGION, "DeleteAccountAlias"),
        Refusal::DefaultDeny,
    );

    // The refused delete deleted nothing: with the read allowed again, the alias the first
    // call stored is still there.
    given_role_policy(&engine, Some(&allow(json!("supportapp:GetAccountAlias"))));
    let served_read = call_as(
        &router,
        SESSION_KEY_ID,
        "/control/get-account-alias",
        &json!({}),
    )
    .await;
    assert_eq!(
        served_read.0, 200,
        "case \"the allowed read\": expected HTTP 200, got {} / {}",
        served_read.0, served_read.1
    );
    assert_eq!(
        served_read.1.get("accountAlias").and_then(Value::as_str),
        Some(ALIAS),
        "case \"the allowed read\": expected the stored alias, got {}",
        served_read.1
    );
}
