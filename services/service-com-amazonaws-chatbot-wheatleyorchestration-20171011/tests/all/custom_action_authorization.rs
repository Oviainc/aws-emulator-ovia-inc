// SPDX-License-Identifier: CC0-1.0

//! WHO may reach the operations that act on a CUSTOM ACTION, driven end to end over the
//! real wire path (a restJson1 request through the multi-service router into the mounted
//! service), so what is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: every operation that acts on a custom action evaluates the
//! identity-based IAM policies in force on the principal its credentials name against
//! `chatbot:<OperationName>` on the custom action the request NAMES, BEFORE it reads or
//! writes the chatbot store — a request no statement allows (IAM's default deny), or one
//! an explicit `Deny` covers, is answered HTTP 403 `AccessDeniedException` and changes
//! nothing, while a request a matching `Allow` covers reaches the operation's own answer.
//! The refusal names the principal, the action and the resource ARN the request addressed.
//!
//! The role, the user, their policies and the session live in the IAM and STS stores of
//! the SAME shared engine — the stores those services' own handlers write — so the
//! documents under test are the ones `iam:PutRolePolicy` stores and the principal a
//! refusal blames is the one `sts:GetCallerIdentity` answers for the key the request
//! signed with.
//!
//! The tables span what the rule generalises over: the operation (the create, the read,
//! the update, the delete and the two association calls, which is every call this service
//! serves that names a custom action), the document in force (none at all, another action
//! of this service, another service's actions, an explicit deny beside an allow, an allow
//! of exactly the action, of the whole service and of everything), the principal (an
//! assumed-role session, an IAM user's long-term access key and the account's own
//! published credential) and the resource an allow is scoped to (every resource, one
//! action, another action).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{post_as, Answer, REGION};

/// The canonical emulator account id, spelled out so the test pins the ARN a client sees.
const ACCOUNT: &str = "000000000000";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = crate::wire::DEFAULT_ACCESS_KEY_ID;

/// The access key id of the assumed-role session the role rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The access key id of the IAM user's long-term key the user rows sign with.
const USER_KEY_ID: &str = "AKIAEXAMPLEUSERKEY01";

/// The role the session is assumed from, the session's own name, and the IAM user.
const ROLE: &str = "chatbot-operator";
const SESSION: &str = "chatbot-session";
const USER_NAME: &str = "chatbot-user";

/// The name of the inline policy the role and the user carry.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// The custom action every row below addresses, and another one an allow can be scoped to
/// instead.
const ACTION_NAME: &str = "run-command";
const OTHER_ACTION_NAME: &str = "other-command";

/// The error code an authorization refusal names, and the status it answers with.
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// A router serving this service beside the IAM and STS services, over ONE shared engine,
/// so the policies and the session the cases seed are the ones those services hold.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(chatbot::EmulatedService::new(Arc::clone(&engine)));
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

/// POST `body` to `path` signed with `access_key_id` — what names the principal the
/// request speaks for.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    path: &str,
    body: &Value,
) -> Answer {
    post_as(router, access_key_id, REGION, path, body).await
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE}/{SESSION}")
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}")
}

/// The IAM user's ARN, as IAM holds it and `sts:GetCallerIdentity` reports it.
fn user_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/{USER_NAME}")
}

/// A custom action is account-scoped, so its ARN carries an EMPTY region segment.
fn custom_action_arn(action_name: &str) -> String {
    format!("arn:aws:chatbot::{ACCOUNT}:custom-action/{action_name}")
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them. NO policy is attached.
fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLECHATBOTROLE".to_string(),
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
                account: ACCOUNT.to_string(),
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
/// `iam:CreateAccessKey` record one. NO policy is attached.
fn given_user_key(engine: &Arc<engine::InMemoryEngine>) {
    let minted = ::aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER_NAME),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER_NAME.to_string(),
            user_id: "AIDAEXAMPLECHATBOTUSER".to_string(),
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

/// A policy allowing `actions` on `resource`.
fn allow_on(actions: Value, resource: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resource }]
    })
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    allow_on(actions, json!("*"))
}

/// The six calls of this service that name a custom action: the operation (which is also
/// the IAM action's suffix), its path, and the request body addressing
/// [`ACTION_NAME`]'s action.
fn custom_action_calls() -> Vec<(&'static str, &'static str, Value)> {
    let arn = custom_action_arn(ACTION_NAME);
    let chat_configuration =
        format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/any");
    vec![
        (
            "CreateCustomAction",
            "/create-custom-action",
            json!({
                "ActionName": ACTION_NAME,
                "Definition": {"CommandText": "ssm list-documents"},
            }),
        ),
        (
            "GetCustomAction",
            "/get-custom-action",
            json!({"CustomActionArn": arn}),
        ),
        (
            "UpdateCustomAction",
            "/update-custom-action",
            json!({"CustomActionArn": arn, "Definition": {"CommandText": "ssm list-commands"}}),
        ),
        (
            "DeleteCustomAction",
            "/delete-custom-action",
            json!({"CustomActionArn": arn}),
        ),
        (
            "AssociateToConfiguration",
            "/associate-to-configuration",
            json!({"Resource": arn, "ChatConfiguration": chat_configuration}),
        ),
        (
            "DisassociateFromConfiguration",
            "/disassociate-from-configuration",
            json!({"Resource": arn, "ChatConfiguration": chat_configuration}),
        ),
    ]
}

/// The refusal IAM words for a request no identity-based policy allows.
fn default_deny(principal: &str, operation: &str, resource: &str) -> String {
    let action = format!("chatbot:{operation}");
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

#[tokio::test]
async fn a_principal_no_identity_policy_allows_is_denied_and_changes_nothing() {
    let (router, engine) = router_with_identity();
    given_session(&engine);

    // Each row is a document in force on the role, none of which allows the calls below:
    // no policy at all, another action of this service, another service's actions, an
    // allow of exactly the action but scoped to ANOTHER custom action, and an explicit
    // deny beside an allow of everything.
    let rows: Vec<(&str, Option<Value>, bool)> = vec![
        ("no policy at all", None, false),
        (
            "another action of this service",
            Some(allow(json!("chatbot:DescribeSlackChannelConfigurations"))),
            false,
        ),
        (
            "another service's actions",
            Some(allow(json!("s3:*"))),
            false,
        ),
        (
            "the action, scoped to another custom action",
            Some(allow_on(
                json!("chatbot:*"),
                json!(custom_action_arn(OTHER_ACTION_NAME)),
            )),
            false,
        ),
        (
            "an explicit deny beside an allow of everything",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": "*", "Resource": "*" },
                    { "Effect": "Deny", "Action": "chatbot:*", "Resource": "*" },
                ]
            })),
            true,
        ),
    ];

    for (case, document, explicit) in &rows {
        given_role_policy(&engine, document.as_ref());
        for (operation, path, body) in custom_action_calls() {
            let answer = call_as(&router, SESSION_KEY_ID, path, &body).await;
            assert_eq!(
                answer.status, 403,
                "case {case:?} on {operation}: expected HTTP 403, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                ACCESS_DENIED_EXCEPTION,
                "case {case:?} on {operation}: expected {ACCESS_DENIED_EXCEPTION}, got {}",
                answer.body
            );
            let expected = if *explicit {
                format!(
                    "User: {} is not authorized to perform: chatbot:{operation} on resource: \
                     {} with an explicit deny in an identity-based policy",
                    session_arn(),
                    custom_action_arn(ACTION_NAME)
                )
            } else {
                default_deny(
                    &session_arn(),
                    operation,
                    &custom_action_arn(ACTION_NAME),
                )
            };
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?} on {operation}: expected IAM's wording, got {}",
                answer.body
            );
        }
    }

    // Nothing the denied creates asked for exists: the account's own credential, which
    // this gate does not bound, reads the store back empty.
    let read = call_as(
        &router,
        ACCOUNT_KEY_ID,
        "/get-custom-action",
        &json!({"CustomActionArn": custom_action_arn(ACTION_NAME)}),
    )
    .await;
    assert_eq!(
        read.status, 404,
        "a denied create must create nothing, got {} with {}",
        read.status, read.body
    );
    let listed = call_as(&router, ACCOUNT_KEY_ID, "/list-custom-actions", &json!({})).await;
    assert_eq!(
        listed.status, 200,
        "the listing answers 200, got {} with {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.body.get("CustomActions"),
        Some(&json!([])),
        "a denied create must leave the account holding no custom action, got {}",
        listed.body
    );
}

#[tokio::test]
async fn a_principal_an_identity_policy_allows_reaches_the_operation() {
    let (router, engine) = router_with_identity();
    given_session(&engine);

    // Each row is a document that DOES allow the create below, differing in how widely:
    // exactly the action, the action in another case (IAM compares action names
    // case-insensitively), every action of this service, every action at all, and the
    // action scoped to the very ARN the create mints.
    let rows: Vec<(&str, Value)> = vec![
        (
            "exactly the action",
            allow(json!("chatbot:CreateCustomAction")),
        ),
        (
            "the action in another case",
            allow(json!("CHATBOT:createcustomaction")),
        ),
        ("every action of this service", allow(json!("chatbot:*"))),
        ("every action at all", allow(json!("*"))),
        (
            "the action scoped to the ARNs this create mints",
            // A wildcard over the account's custom-action ARNs: each row below mints its
            // own action name, so this is the scoped form that covers them.
            allow_on(
                json!("chatbot:CreateCustomAction"),
                json!(custom_action_arn(&format!("{ACTION_NAME}-*"))),
            ),
        ),
    ];

    for (index, (case, document)) in rows.iter().enumerate() {
        given_role_policy(&engine, Some(document));
        // A fresh action name per row, so each row's create is a create and not a
        // conflict with the row before it.
        let action_name = format!("{ACTION_NAME}-{index}");
        let answer = call_as(
            &router,
            SESSION_KEY_ID,
            "/create-custom-action",
            &json!({
                "ActionName": action_name,
                "Definition": {"CommandText": "ssm list-documents"},
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: an allowed create is served, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.get("CustomActionArn").and_then(Value::as_str),
            Some(custom_action_arn(&action_name).as_str()),
            "case {case:?}: expected the minted ARN, got {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn every_kind_of_principal_is_judged_on_the_policies_in_force_on_it() {
    let (router, engine) = router_with_identity();
    given_session(&engine);
    given_user_key(&engine);

    // (case, the key the request signs with, the principal a refusal names, whether it is
    // refused with no policy in force). The account's own published credential holds no
    // IAM policies, which is the position root credentials are in, so this gate does not
    // bound it; an IAM user's long-term key and an assumed-role session are both judged
    // on the documents the IAM store holds for them.
    let principals: [(&str, &str, String, bool); 3] = [
        (
            "an assumed-role session",
            SESSION_KEY_ID,
            session_arn(),
            true,
        ),
        ("an IAM user's long-term key", USER_KEY_ID, user_arn(), true),
        (
            "the account's own published credential",
            ACCOUNT_KEY_ID,
            format!("arn:aws:iam::{ACCOUNT}:root"),
            false,
        ),
    ];

    for (index, (case, key_id, principal, refused)) in principals.iter().enumerate() {
        given_role_policy(&engine, None);
        given_user_policy(&engine, None);
        let action_name = format!("principal-{index}");
        let body = json!({
            "ActionName": action_name,
            "Definition": {"CommandText": "ssm list-documents"},
        });
        let answer = call_as(&router, key_id, "/create-custom-action", &body).await;
        if *refused {
            assert_eq!(
                answer.status, 403,
                "case {case:?}: expected HTTP 403, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.message(),
                default_deny(
                    principal,
                    "CreateCustomAction",
                    &custom_action_arn(&action_name)
                ),
                "case {case:?}: expected the refusal to name this principal, got {}",
                answer.body
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be served, got {} with {}",
                answer.status, answer.body
            );
        }

        // Granting the action to the identity in force lifts the refusal, so the decision
        // tracks the stored documents and not the kind of credential. A fresh name, so a
        // call the gate did not bound the first time is still a create here.
        let grant = allow(json!("chatbot:CreateCustomAction"));
        given_role_policy(&engine, Some(&grant));
        given_user_policy(&engine, Some(&grant));
        let granted = call_as(
            &router,
            key_id,
            "/create-custom-action",
            &json!({
                "ActionName": format!("{action_name}-granted"),
                "Definition": {"CommandText": "ssm list-documents"},
            }),
        )
        .await;
        assert_eq!(
            granted.status, 200,
            "case {case:?}: an allowed create is served, got {} with {}",
            granted.status, granted.body
        );
    }
}
